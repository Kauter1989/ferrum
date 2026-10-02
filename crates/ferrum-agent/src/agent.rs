//! The agent facade: runs a command by name with JSON parameters and
//! returns the envelope; writes the audit log of the workspace.

use serde_json::{json, Value};

use crate::commands::{Ctx, COMMANDS};
use crate::config::AgentConfig;
use crate::envelope::{error_envelope, ok_envelope, AgentError, Output};
use crate::params::Params;
use crate::study::{generator, VolumeCache};

/// Runs commands under one operator configuration.
#[derive(Debug, Clone, Default)]
pub struct Agent {
    config: AgentConfig,
}

impl Agent {
    /// An agent with the given operator configuration.
    pub fn new(config: AgentConfig) -> Self {
        Self { config }
    }

    /// The operator configuration.
    pub fn config(&self) -> &AgentConfig {
        &self.config
    }

    /// Names of all commands (e.g. `view slice`).
    pub fn commands() -> impl Iterator<Item = &'static str> {
        COMMANDS.iter().map(|(name, _)| *name)
    }

    /// Runs `command` with `params` (a JSON object) and returns the
    /// `ferrum-agent/1` envelope. Never panics on bad input. Every call
    /// loads the series from its source (command line).
    pub fn run(&self, command: &str, params: &Value) -> Value {
        self.execute(None, command, params)
    }

    /// Like [`Agent::run`], but keeps loaded series in `cache` between
    /// calls (long-running servers such as MCP).
    pub fn run_cached(&self, cache: &mut VolumeCache, command: &str, params: &Value) -> Value {
        self.execute(Some(cache), command, params)
    }

    fn execute(&self, cache: Option<&mut VolumeCache>, command: &str, params: &Value) -> Value {
        let mut ctx = Ctx { config: &self.config, study: None, cache };
        let result = self.dispatch(&mut ctx, command, params);
        let envelope = match &result {
            Ok(out) => {
                let mut out = out.clone();
                if self.config.is_default {
                    out = out.warn("no operator configuration: every path is readable (set FERRUM_AGENT_CONFIG)");
                }
                ok_envelope(&out, self.provenance(&ctx, command, params))
            }
            Err(e) => error_envelope(e),
        };
        if let Some(study) = &ctx.study {
            let entry = json!({
                "command": command,
                "params": params,
                "ok": result.is_ok(),
                "error": result.as_ref().err().map(|e| e.code.as_str()),
                "source_sha256": study.source_sha256,
            });
            if let Err(e) = study.workspace.append_audit(entry) {
                log::warn!("audit log: {e}");
            }
        }
        envelope
    }

    fn dispatch(&self, ctx: &mut Ctx, command: &str, params: &Value) -> Result<Output, AgentError> {
        let f = COMMANDS.iter().find(|(name, _)| *name == command).map(|(_, f)| *f).ok_or_else(|| {
            AgentError::bad_request(format!("unknown command {command:?}"))
                .hint(format!("commands: {}", Self::commands().collect::<Vec<_>>().join(", ")))
        })?;
        let p = Params::new(params)?;
        if let Some((_, schema)) = crate::schema::command_schema(command) {
            crate::schema::validate(&schema, params, "params").map_err(|e| {
                AgentError::bad_request(e).hint(format!("see the parameters with: ferrum-cli schema \"{command}\""))
            })?;
        }
        f(ctx, &p)
    }

    fn provenance(&self, ctx: &Ctx, command: &str, params: &Value) -> Value {
        json!({
            "ferrum": generator(),
            "command": command,
            "params": params,
            "source_sha256": ctx.study.as_ref().map(|s| s.source_sha256.clone()),
            "renderer": "cpu",
            "time": ferrum_domain::Timestamp::now().to_rfc3339(),
        })
    }
}
