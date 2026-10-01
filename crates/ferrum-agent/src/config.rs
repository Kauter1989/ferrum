//! Operator configuration (`ferrum-agent.toml`, `docs/agent-skill.md` §11).
//!
//! The operator, not the agent, decides which data may be read, where
//! workspaces live, whether identifiers may leave FERRUM, and whether a
//! harness may confirm results itself. Commands cannot change it.
//!
//! ```toml
//! [data]
//! read_roots = ["/data/incoming"]       # sources outside are refused
//! workspace_root = "/data/workspaces"   # relative workspaces are placed here
//!
//! [privacy]
//! expose_identifiers = false
//! expose_dates = false
//! pseudonymise_uids = true
//! salt = "site-secret"                  # keys the UID pseudonyms
//!
//! [limits]
//! max_render_px = 1024
//! max_voxels = 600_000_000
//!
//! [review]
//! allow_harness_confirmation = false
//! ```
//!
//! Without a configuration file every path is readable (development use);
//! the envelope then carries a warning.

use std::path::{Path, PathBuf};

use crate::envelope::{AgentError, ErrorCode};

/// Parsed operator configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
    /// Sources must lie below one of these (empty: anything).
    pub read_roots: Vec<PathBuf>,
    /// Relative workspace paths are resolved here; workspaces must lie
    /// below it when set.
    pub workspace_root: Option<PathBuf>,
    /// Patient names, IDs, birth dates and accession numbers may appear in
    /// outputs.
    pub expose_identifiers: bool,
    /// Study dates and times may appear in outputs.
    pub expose_dates: bool,
    /// UIDs are replaced by stable salted hashes.
    pub pseudonymise_uids: bool,
    /// Salt of the UID pseudonyms.
    pub salt: String,
    /// Largest render side in pixels.
    pub max_render_px: u32,
    /// Largest volume that may be loaded.
    pub max_voxels: u64,
    /// `review confirm` / `review reject` are allowed for the harness.
    pub allow_harness_confirmation: bool,
    /// `true` when no configuration file was given.
    pub is_default: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            read_roots: Vec::new(),
            workspace_root: None,
            expose_identifiers: false,
            expose_dates: false,
            pseudonymise_uids: true,
            salt: "ferrum".into(),
            max_render_px: 1024,
            max_voxels: 600_000_000,
            allow_harness_confirmation: false,
            is_default: true,
        }
    }
}

fn invalid(msg: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::BadRequest, format!("operator configuration: {}", msg.into()))
}

impl AgentConfig {
    /// Parses the TOML text of a configuration file. Unknown sections and
    /// keys are rejected, so a typo cannot silently weaken a setting.
    pub fn parse(text: &str) -> Result<Self, AgentError> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| invalid(e.message().to_owned()))?;
        let mut c = Self { is_default: false, ..Self::default() };
        for (section, value) in &table {
            let t = value.as_table().ok_or_else(|| invalid(format!("[{section}] must be a table")))?;
            for (key, v) in t {
                c.set(section, key, v)?;
            }
        }
        Ok(c)
    }

    fn set(&mut self, section: &str, key: &str, v: &toml::Value) -> Result<(), AgentError> {
        let what = format!("{section}.{key}");
        let boolean = || v.as_bool().ok_or_else(|| invalid(format!("{what} must be true or false")));
        let positive =
            || v.as_integer().filter(|n| *n > 0).ok_or_else(|| invalid(format!("{what} must be a positive integer")));
        let path = |s: &str| PathBuf::from(s);
        match (section, key) {
            ("data", "read_roots") => {
                let list = v.as_array().ok_or_else(|| invalid(format!("{what} must be a list of paths")))?;
                self.read_roots = list
                    .iter()
                    .map(|p| p.as_str().map(path).ok_or_else(|| invalid(format!("{what} must be a list of paths"))))
                    .collect::<Result<_, _>>()?;
            }
            ("data", "workspace_root") => {
                self.workspace_root =
                    Some(v.as_str().map(path).ok_or_else(|| invalid(format!("{what} must be a path")))?);
            }
            ("privacy", "expose_identifiers") => self.expose_identifiers = boolean()?,
            ("privacy", "expose_dates") => self.expose_dates = boolean()?,
            ("privacy", "pseudonymise_uids") => self.pseudonymise_uids = boolean()?,
            ("privacy", "salt") => {
                self.salt = v.as_str().ok_or_else(|| invalid(format!("{what} must be text")))?.into()
            }
            ("limits", "max_render_px") => {
                self.max_render_px = u32::try_from(positive()?).map_err(|_| invalid(format!("{what} is too large")))?;
            }
            ("limits", "max_voxels") => self.max_voxels = positive()?.unsigned_abs(),
            ("review", "allow_harness_confirmation") => self.allow_harness_confirmation = boolean()?,
            // engines are configured with Stage 15.7; accepted so one file serves both
            ("network", "engines") | ("limits", "command_timeout_s") => {}
            _ => return Err(invalid(format!("unknown setting {what}"))),
        }
        Ok(())
    }

    /// Reads a configuration file.
    pub fn load(path: &Path) -> Result<Self, AgentError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Checks that a source path may be read and returns it absolute.
    pub fn check_source(&self, path: &Path) -> Result<PathBuf, AgentError> {
        let abs = absolute(path)?;
        if self.read_roots.is_empty() || self.read_roots.iter().any(|r| abs.starts_with(normalise(r))) {
            return Ok(abs);
        }
        Err(AgentError::new(ErrorCode::Forbidden, format!("{} is outside the readable data roots", abs.display()))
            .hint("the operator configuration lists the folders FERRUM may read (data.read_roots)"))
    }

    /// Resolves a workspace path (relative paths below `workspace_root`)
    /// and checks that it lies below `workspace_root` when one is set.
    pub fn resolve_workspace(&self, path: &Path) -> Result<PathBuf, AgentError> {
        let joined = match (&self.workspace_root, path.is_relative()) {
            (Some(root), true) => root.join(path),
            _ => path.to_path_buf(),
        };
        let abs = absolute(&joined)?;
        match &self.workspace_root {
            Some(root) if !abs.starts_with(normalise(root)) => Err(AgentError::new(
                ErrorCode::Forbidden,
                format!("workspace {} is outside the workspace root", abs.display()),
            )
            .hint("use a workspace name relative to the workspace root")),
            _ => Ok(abs),
        }
    }
}

/// Absolute path with `.` and `..` resolved lexically (the path may not
/// exist yet), so `root/../elsewhere` cannot escape a root check.
fn absolute(path: &Path) -> Result<PathBuf, AgentError> {
    let abs = std::path::absolute(path).map_err(|e| AgentError::bad_request(format!("{}: {e}", path.display())))?;
    Ok(normalise(&abs))
}

fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_setting() {
        let c = AgentConfig::parse(
            r#"
            [data]
            read_roots = ["/data/incoming", "/mnt/pacs"]
            workspace_root = "/data/workspaces"
            [privacy]
            expose_identifiers = true
            expose_dates = true
            pseudonymise_uids = false
            salt = "s"
            [network]
            engines = ["http://127.0.0.1:8765"]
            [limits]
            max_render_px = 512
            max_voxels = 1000
            command_timeout_s = 5
            [review]
            allow_harness_confirmation = true
            "#,
        )
        .unwrap();
        assert_eq!(c.read_roots, vec![PathBuf::from("/data/incoming"), PathBuf::from("/mnt/pacs")]);
        assert_eq!(c.workspace_root, Some(PathBuf::from("/data/workspaces")));
        assert!(c.expose_identifiers && c.expose_dates && !c.pseudonymise_uids && c.allow_harness_confirmation);
        assert_eq!((c.salt.as_str(), c.max_render_px, c.max_voxels, c.is_default), ("s", 512, 1000, false));
        assert!(AgentConfig::default().is_default);
    }

    #[test]
    fn rejects_unknown_or_mistyped_settings() {
        for bad in [
            "[data]\nread_root = []",
            "[privacy]\nexpose_identifiers = \"yes\"",
            "[limits]\nmax_render_px = 0",
            "[limits]\nmax_render_px = 99999999999",
            "[data]\nread_roots = \"/x\"",
            "[data]\nread_roots = [1]",
            "[data]\nworkspace_root = 1",
            "[privacy]\nsalt = 1",
            "data = 1",
            "[data",
        ] {
            assert_eq!(AgentConfig::parse(bad).unwrap_err().code, ErrorCode::BadRequest, "{bad}");
        }
        assert!(AgentConfig::load(Path::new("/nonexistent/ferrum-agent.toml")).is_err());
    }

    #[test]
    fn path_rules() {
        let c = AgentConfig {
            read_roots: vec!["/data/in".into()],
            workspace_root: Some("/data/ws".into()),
            ..AgentConfig::default()
        };
        assert_eq!(c.check_source(Path::new("/data/in/ct/1.dcm")).unwrap(), PathBuf::from("/data/in/ct/1.dcm"));
        assert_eq!(c.check_source(Path::new("/data/in/../secret")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.check_source(Path::new("/data/income")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.resolve_workspace(Path::new("ct1")).unwrap(), PathBuf::from("/data/ws/ct1"));
        assert_eq!(c.resolve_workspace(Path::new("../x")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.resolve_workspace(Path::new("/tmp/ws")).unwrap_err().code, ErrorCode::Forbidden);
        let open = AgentConfig::default();
        assert!(open.check_source(Path::new("/anything")).is_ok());
        assert_eq!(open.resolve_workspace(Path::new("/tmp/./ws")).unwrap(), PathBuf::from("/tmp/ws"));
    }
}
