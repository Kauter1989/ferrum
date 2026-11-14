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
//! [network]
//! engines = ["http://127.0.0.1:8765"]   # segmentation engines the agent may use
//! gpu_groups = { card = ["http://127.0.0.1:8765"] }   # engines sharing a GPU
//! gpu_capacity_mib = { card = 11264 }   # leases of a group must fit
//! gpu_peaks_mib = { "http://127.0.0.1:8765" = 6000 }  # GPU memory of one call
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

use ferrum_engines::gpu::GpuSettings;

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
    /// Segmentation engines (`ferrum-engine/1` base URLs) the agent may
    /// contact. Without a configuration file only loopback URLs are
    /// allowed.
    pub engines: Vec<String>,
    /// Longest wait for an automatic segmentation job, in seconds.
    pub engine_job_timeout_s: u64,
    /// Engines that share a GPU: groups, capacities, expected peaks and the
    /// lock directory (`docs/gpu-sharing.md`). Engine calls of a group take
    /// a lease first, across processes.
    pub gpu: GpuSettings,
    /// Most prompts one interactive object may take.
    pub max_prompts_per_object: u64,
    /// Margin around the prompts of the region uploaded to an interactive
    /// engine, in mm.
    pub roi_margin_mm: f64,
    /// Engines whose licence restricts them to research use may be used.
    pub allow_research_only: bool,
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
            engines: Vec::new(),
            engine_job_timeout_s: 900,
            gpu: GpuSettings::default(),
            max_prompts_per_object: 8,
            roi_margin_mm: 48.0,
            allow_research_only: true,
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
        c.gpu.validate().map_err(invalid)?;
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
            ("network", "engines") => self.engines = urls(v, &what)?,
            ("network", k) if self.gpu.set(k, v).map_err(invalid)? => {}
            ("limits", "max_prompts_per_object") => self.max_prompts_per_object = positive()?.unsigned_abs(),
            ("limits", "roi_margin_mm") => {
                self.roi_margin_mm = v
                    .as_float()
                    .or_else(|| v.as_integer().map(|i| i as f64))
                    .filter(|m| m.is_finite() && *m >= 0.0)
                    .ok_or_else(|| invalid(format!("{what} must be a non-negative number")))?;
            }
            ("limits", "allow_research_only") => self.allow_research_only = boolean()?,
            ("limits", "engine_job_timeout_s") => self.engine_job_timeout_s = positive()?.unsigned_abs(),
            ("limits", "command_timeout_s") => {}
            _ => return Err(invalid(format!("unknown setting {what}"))),
        }
        Ok(())
    }

    /// The GPU group of an engine URL, if the operator put it in one.
    pub fn gpu_group(&self, url: &str) -> Option<&str> {
        self.gpu.group(url)
    }

    /// Reads a configuration file.
    pub fn load(path: &Path) -> Result<Self, AgentError> {
        let text =
            std::fs::read_to_string(path).map_err(|e| invalid(format!("cannot read {}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Checks that a source path may be read and returns it absolute.
    ///
    /// The comparison resolves symlinks: the returned path and every read
    /// root are canonicalised first, so a link inside a root cannot point
    /// outside it (#82). The check runs on exactly the spelling that is
    /// returned and that the reader will open — `.` and `..` are resolved
    /// lexically, as [`absolute`] does, and a `..` behind a depth-changing
    /// link must not split the checked path from the opened one (security
    /// review of #82). A root that cannot be resolved grants nothing.
    pub fn check_source(&self, path: &Path) -> Result<PathBuf, AgentError> {
        let abs = absolute(path)?;
        if self.read_roots.is_empty() {
            return Ok(abs);
        }
        let real = real_path(&abs)?;
        if self.read_roots.iter().filter_map(|r| real_path(r).ok()).any(|r| real.starts_with(r)) {
            return Ok(abs);
        }
        Err(AgentError::new(ErrorCode::Forbidden, format!("{} is outside the readable data roots", abs.display()))
            .hint("the operator configuration lists the folders FERRUM may read (data.read_roots)"))
    }

    /// Resolves a workspace path (relative paths below `workspace_root`)
    /// and checks that it lies below `workspace_root` when one is set.
    /// The comparison resolves symlinks on the returned spelling, as
    /// [`Self::check_source`] does; a workspace that does not exist yet
    /// still resolves.
    pub fn resolve_workspace(&self, path: &Path) -> Result<PathBuf, AgentError> {
        let joined = match (&self.workspace_root, path.is_relative()) {
            (Some(root), true) => root.join(path),
            _ => path.to_path_buf(),
        };
        let abs = absolute(&joined)?;
        if let Some(root) = &self.workspace_root {
            let real = real_path(&abs)?;
            let root_real = real_path(root).map_err(|e| {
                AgentError::new(
                    ErrorCode::Forbidden,
                    format!("workspace root {} cannot be resolved: {}", root.display(), e.message),
                )
            })?;
            if !real.starts_with(root_real) {
                return Err(AgentError::new(
                    ErrorCode::Forbidden,
                    format!("workspace {} is outside the workspace root", abs.display()),
                )
                .hint("use a workspace name relative to the workspace root"));
            }
        }
        Ok(abs)
    }
}

fn urls(v: &toml::Value, what: &str) -> Result<Vec<String>, AgentError> {
    let list = v.as_array().ok_or_else(|| invalid(format!("{what} must be a list of URLs")))?;
    list.iter()
        .map(|u| {
            u.as_str()
                .map(|s| s.trim_end_matches('/').to_owned())
                .ok_or_else(|| invalid(format!("{what} must be a list of URLs")))
        })
        .collect()
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

/// The path with symlinks resolved, for root comparisons only.
///
/// The deepest existing ancestor is canonicalised: the OS resolves
/// symlinks and `..` in filesystem order, which lexical normalisation
/// cannot — a `..` behind a link climbs out of the link's target, not
/// out of the link's spelling. The remaining components are then applied
/// lexically; the canonical base holds no symlinks, so a `..` in the
/// tail cannot climb through one. A path that does not exist yet keeps
/// its lexical tail (the reader reports the missing file). An error
/// other than "not found" refuses the path: a root that cannot be
/// resolved must not grant access.
fn real_path(path: &Path) -> Result<PathBuf, AgentError> {
    let abs = absolute_raw(path)?;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut base: &Path = &abs;
    loop {
        match std::fs::canonicalize(base) {
            Ok(canon) => return Ok(apply_tail(canon, &tail)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                match base.components().next_back() {
                    Some(std::path::Component::Normal(name)) => tail.push(name.to_os_string()),
                    Some(std::path::Component::ParentDir) => tail.push(std::ffi::OsString::from("..")),
                    Some(std::path::Component::CurDir) => {}
                    // A root or prefix: nothing on this path exists.
                    _ => return Ok(normalise(&abs)),
                }
                match base.parent() {
                    Some(parent) => base = parent,
                    None => return Ok(normalise(&abs)),
                }
            }
            Err(e) => return Err(AgentError::bad_request(format!("{}: {e}", base.display()))),
        }
    }
}

/// Absolute without normalising `.` and `..`: a `..` behind a symlink
/// must reach the filesystem in component order. `std::path::absolute`
/// normalises lexically on Windows, which would erase the link before
/// the check ever sees it.
fn absolute_raw(path: &Path) -> Result<PathBuf, AgentError> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    #[cfg(windows)]
    if matches!(path.components().next(), Some(std::path::Component::Prefix(_))) {
        // Drive-relative (`C:foo`): only the OS can resolve it, and its
        // resolver normalises `..` lexically. Refuse rather than check a
        // spelling the filesystem will not read.
        return Err(AgentError::bad_request(format!(
            "{}: a drive-relative path cannot be checked against the read roots; use an absolute path",
            path.display()
        )));
    }
    let cwd = std::env::current_dir().map_err(|e| AgentError::bad_request(format!("{}: {e}", path.display())))?;
    Ok(cwd.join(path))
}

/// Applies the not-yet-existing tail of a path to its canonical base,
/// innermost component first. The base holds no symlinks, so `..` here
/// is the real parent.
fn apply_tail(mut base: PathBuf, tail: &[std::ffi::OsString]) -> PathBuf {
    for part in tail.iter().rev() {
        if part == std::ffi::OsStr::new("..") {
            base.pop();
        } else {
            base.push(part);
        }
    }
    base
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
            gpu_groups = { card = ["http://127.0.0.1:8765/", "http://127.0.0.1:8766"] }
            gpu_capacity_mib = { card = 11264 }
            gpu_peaks_mib = { "http://127.0.0.1:8765" = 6000 }
            [limits]
            max_render_px = 512
            max_voxels = 1000
            command_timeout_s = 5
            max_prompts_per_object = 4
            roi_margin_mm = 20
            allow_research_only = false
            [review]
            allow_harness_confirmation = true
            "#,
        )
        .unwrap();
        assert_eq!(c.read_roots, vec![PathBuf::from("/data/incoming"), PathBuf::from("/mnt/pacs")]);
        assert_eq!(c.workspace_root, Some(PathBuf::from("/data/workspaces")));
        assert!(c.expose_identifiers && c.expose_dates && !c.pseudonymise_uids && c.allow_harness_confirmation);
        assert_eq!((c.salt.as_str(), c.max_render_px, c.max_voxels, c.is_default), ("s", 512, 1000, false));
        assert_eq!(c.engines, vec!["http://127.0.0.1:8765".to_owned()]);
        assert_eq!(c.gpu_group("http://127.0.0.1:8766"), Some("card"));
        assert_eq!(c.gpu_group("http://127.0.0.1:8765"), Some("card"), "trailing slashes are trimmed");
        assert_eq!(c.gpu_group("http://127.0.0.1:9"), None);
        assert_eq!((c.gpu.capacity("card"), c.gpu.operator_peak("http://127.0.0.1:8765")), (Some(11264), Some(6000)));
        assert_eq!((c.max_prompts_per_object, c.roi_margin_mm, c.allow_research_only), (4, 20.0, false));
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
            "[network]\ngpu_groups = [1]",
            "[network]\ngpu_groups = { a = [1] }",
            "[network]\ngpu_capacity_mib = { nope = 1 }",
            "[network]\ngpu_lock_dir = \"relative\"",
            "[limits]\nroi_margin_mm = -1",
            "[limits]\nallow_research_only = 1",
            "data = 1",
            "[data",
        ] {
            assert_eq!(AgentConfig::parse(bad).unwrap_err().code, ErrorCode::BadRequest, "{bad}");
        }
        assert!(AgentConfig::load(Path::new("/nonexistent/ferrum-agent.toml")).is_err());
    }

    #[test]
    fn path_rules() {
        // The rules are about paths, not about one operating system: the cases
        // are built from an absolute root so Windows (C:\…) and Unix (/…) both
        // exercise the same checks.
        let base = if cfg!(windows) { "C:\\data" } else { "/data" };
        let p = |rest: &str| Path::new(base).join(rest.replace('/', std::path::MAIN_SEPARATOR_STR));
        let c = AgentConfig { read_roots: vec![p("in")], workspace_root: Some(p("ws")), ..AgentConfig::default() };
        assert_eq!(c.check_source(&p("in/ct/1.dcm")).unwrap(), p("in/ct/1.dcm"));
        assert_eq!(c.check_source(&p("in/../secret")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.check_source(&p("income")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.resolve_workspace(Path::new("ct1")).unwrap(), p("ws/ct1"));
        assert_eq!(c.resolve_workspace(Path::new("../x")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.resolve_workspace(&p("tmp/ws")).unwrap_err().code, ErrorCode::Forbidden);

        // A `..` that climbs out of a readable folder is refused in either
        // spelling. This is the security boundary for what an agent may read,
        // so both forms are checked: `/data/in/../secret` is an absolute Unix
        // path and `C:\data\in\..\secret` a Windows one, and each is a rooted
        // path where the other is not.
        let rooted = AgentConfig {
            read_roots: vec![Path::new("/data/in").into(), Path::new(r"C:\data\in").into()],
            ..AgentConfig::default()
        };
        for escape in ["/data/in/../secret", r"C:\data\in\..\secret", "/data/in/sub/../../secret"] {
            assert_eq!(
                rooted.check_source(Path::new(escape)).unwrap_err().code,
                ErrorCode::Forbidden,
                "an escape was accepted: {escape}"
            );
        }
        // The same folder reached without climbing is allowed, in the spelling
        // the platform resolves to a rooted path: a leading `/` without a drive
        // is relative to the current drive on Windows, so it has to name one.
        let inside = if cfg!(windows) { r"C:\data\in\ct\1.dcm" } else { "/data/in/ct/1.dcm" };
        assert!(rooted.check_source(Path::new(inside)).is_ok(), "{inside} is inside a readable root");

        let open = AgentConfig::default();
        assert!(open.check_source(&p("anything")).is_ok());
        assert_eq!(open.resolve_workspace(&p("tmp/./ws")).unwrap(), p("tmp/ws"));
    }

    /// Creates a symlink; on Windows this needs a privilege the test
    /// runner may not hold, so a failed creation reports and skips the
    /// case. CI runs Linux, where every case runs.
    fn symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let r = if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        match r {
            Ok(()) => true,
            Err(e) => {
                eprintln!("skipping a symlink case: {e}");
                false
            }
        }
    }

    #[test]
    fn symlink_out_of_a_read_root_is_refused() {
        // covers F2.1-a: a symlink inside a read root that points outside
        // it must not pass the check — as a file, as a folder, through a
        // folder, or with a `..` behind the link.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.nii");
        std::fs::write(&secret, b"x").unwrap();
        if !symlink(&secret, &root.join("link-file")) {
            return;
        }
        assert!(symlink(&outside, &root.join("link-dir")));
        let c = AgentConfig { read_roots: vec![root.clone()], ..AgentConfig::default() };
        for escape in [root.join("link-file"), root.join("link-dir"), root.join("link-dir/secret.nii")] {
            assert_eq!(
                c.check_source(&escape).unwrap_err().code,
                ErrorCode::Forbidden,
                "an escape was accepted: {}",
                escape.display()
            );
        }
        // `..` is lexical (the `absolute` contract): link-dir/../secret.nii
        // normalises to root/secret.nii — inside the root, and exactly what
        // the reader opens, so it is allowed.
        assert_eq!(c.check_source(&root.join("link-dir/../secret.nii")).unwrap(), root.join("secret.nii"));
        // A real file inside the root stays readable, and the returned
        // path keeps the caller's spelling (workspaces store it).
        let ok = root.join("ok.nii");
        std::fs::write(&ok, b"x").unwrap();
        assert_eq!(c.check_source(&ok).unwrap(), ok);
    }

    #[test]
    fn a_depth_changing_link_cannot_split_the_check_from_the_read() {
        // covers F2.1-a (security-review regression): the link points
        // DEEPER than its spelling, so a `..` behind it resolves differently
        // in OS order and lexically. The check must judge the lexical
        // spelling — the one the reader opens — not the OS-order target.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        let deep = root.join("x/y");
        std::fs::create_dir_all(&deep).unwrap();
        let other = tmp.path().join("patientB");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("scan.nii"), b"x").unwrap();
        if !symlink(&deep, &root.join("link")) {
            return;
        }
        std::fs::write(root.join("ok.nii"), b"x").unwrap();
        let c = AgentConfig { read_roots: vec![root.clone()], ..AgentConfig::default() };
        // OS order: root/x/y/../../patientB/scan.nii = root/patientB/scan.nii
        // (inside); lexical: root/link/../../patientB/scan.nii =
        // tmp/patientB/scan.nii (outside). The reader opens the lexical one,
        // so it must be refused.
        let escape = root.join("link/../../patientB/scan.nii");
        assert_eq!(c.check_source(&escape).unwrap_err().code, ErrorCode::Forbidden);
        // One `..` less: lexical root/ok.nii is inside — allowed, and the
        // returned spelling is exactly what will be opened.
        assert_eq!(c.check_source(&root.join("link/../ok.nii")).unwrap(), root.join("ok.nii"));
        // The workspace boundary holds the same way.
        let cw = AgentConfig { workspace_root: Some(root.clone()), ..AgentConfig::default() };
        assert_eq!(
            cw.resolve_workspace(Path::new("link/../../patientB/scan.nii")).unwrap_err().code,
            ErrorCode::Forbidden
        );
    }

    #[test]
    fn a_symlinked_read_root_grants_exactly_its_target() {
        // covers F2.1-a: roots are canonicalised too, so a root named
        // through a link grants the target folder — and nothing beside it.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("ok.nii"), b"x").unwrap();
        let link = tmp.path().join("link");
        if !symlink(&real, &link) {
            return;
        }
        let c = AgentConfig { read_roots: vec![link.clone()], ..AgentConfig::default() };
        assert!(c.check_source(&link.join("ok.nii")).is_ok());
        assert!(c.check_source(&real.join("ok.nii")).is_ok(), "the target itself is inside the root");
        std::fs::write(tmp.path().join("sibling.nii"), b"x").unwrap();
        assert_eq!(c.check_source(&tmp.path().join("sibling.nii")).unwrap_err().code, ErrorCode::Forbidden);
    }

    #[test]
    fn workspace_symlink_escape_is_refused() {
        // covers F2.1-b: the workspace root holds the same boundary, and
        // a workspace that does not exist yet still resolves.
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        if !symlink(&outside, &ws.join("escape")) {
            return;
        }
        let c = AgentConfig { workspace_root: Some(ws.clone()), ..AgentConfig::default() };
        assert_eq!(c.resolve_workspace(Path::new("escape")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.resolve_workspace(Path::new("escape/new")).unwrap_err().code, ErrorCode::Forbidden);
        assert_eq!(c.resolve_workspace(Path::new("ct1")).unwrap(), ws.join("ct1"));
    }
}
