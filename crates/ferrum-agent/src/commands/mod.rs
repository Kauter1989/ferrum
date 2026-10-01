//! Command implementations. Each command reads its parameters from a JSON
//! object and returns [`Output`] data or an [`AgentError`].

use std::path::PathBuf;

use crate::config::AgentConfig;
use crate::envelope::{AgentError, Output};
use crate::params::Params;
use crate::study::Study;

pub mod annotate;
pub mod inspect;
pub mod review;
pub mod segment;
pub mod study;
pub mod view;

/// State of one command call.
pub struct Ctx<'a> {
    /// Operator configuration.
    pub config: &'a AgentConfig,
    /// The study, once a command loaded it.
    pub study: Option<Study>,
}

impl Ctx<'_> {
    /// The workspace path of the call (resolved and checked).
    pub fn workspace_path(&self, p: &Params) -> Result<PathBuf, AgentError> {
        let ws = p.req_str("workspace")?;
        self.config.resolve_workspace(std::path::Path::new(ws))
    }

    /// Loads the study of the call's workspace.
    pub fn study(&mut self, p: &Params) -> Result<&mut Study, AgentError> {
        if self.study.is_none() {
            let root = self.workspace_path(p)?;
            self.study = Some(Study::load(self.config, &root)?);
        }
        self.study.as_mut().ok_or_else(|| AgentError::internal("study not loaded"))
    }
}

/// A command function.
pub type Command = fn(&mut Ctx, &Params) -> Result<Output, AgentError>;

/// Every command: name and function.
pub const COMMANDS: &[(&str, Command)] = &[
    ("study scan", study::scan),
    ("study open", study::open),
    ("study info", study::info),
    ("view slice", view::slice),
    ("probe", inspect::probe),
    ("stats", inspect::stats),
    ("measure distance", inspect::measure_distance),
    ("measure angle", inspect::measure_angle),
    ("measure area", inspect::measure_area),
    ("annotate add", annotate::add),
    ("annotate list", annotate::list),
    ("annotate rename", annotate::rename),
    ("annotate delete", annotate::delete),
    ("segment list", segment::list),
    ("segment threshold", segment::threshold),
    ("segment rename", segment::rename),
    ("segment delete", segment::delete),
    ("review list", review::list),
    ("review confirm", review::confirm),
    ("review reject", review::reject),
];
