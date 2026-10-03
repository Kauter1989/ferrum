//! Command implementations. Each command reads its parameters from a JSON
//! object and returns [`Output`] data or an [`AgentError`].

use std::path::PathBuf;

use crate::config::AgentConfig;
use crate::envelope::{AgentError, Output};
use crate::params::Params;
use crate::study::{Study, VolumeCache};

pub mod annotate;
pub mod engine;
pub mod export;
pub mod inspect;
pub mod interactive;
pub mod masks;
pub mod review;
pub mod segment;
pub mod study;
pub mod tiles;
pub mod view;
pub mod volume;

/// Receives the progress (`0..=1`) and a message of a long command.
pub type ProgressFn<'a> = dyn FnMut(f64, &str) + 'a;

/// State of one command call.
pub struct Ctx<'a> {
    /// Operator configuration.
    pub config: &'a AgentConfig,
    /// The study, once a command loaded it.
    pub study: Option<Study>,
    /// Series kept in memory between calls (long-running servers).
    pub cache: Option<&'a mut VolumeCache>,
    /// Receives the progress (`0..=1`) and a message of long commands.
    pub progress: Option<&'a mut ProgressFn<'a>>,
}

impl Ctx<'_> {
    /// Reports progress of a long command, if anyone listens.
    pub fn report(&mut self, progress: f64, message: &str) {
        if let Some(f) = self.progress.as_deref_mut() {
            f(progress, message);
        }
    }

    /// The workspace path of the call (resolved and checked).
    pub fn workspace_path(&self, p: &Params) -> Result<PathBuf, AgentError> {
        let ws = p.req_str("workspace")?;
        self.config.resolve_workspace(std::path::Path::new(ws))
    }

    /// Loads the study of the call's workspace.
    pub fn study(&mut self, p: &Params) -> Result<&mut Study, AgentError> {
        if self.study.is_none() {
            let root = self.workspace_path(p)?;
            self.study = Some(Study::load(self.config, &root, self.cache.as_deref_mut())?);
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
    ("view montage", tiles::montage),
    ("view mpr", tiles::mpr),
    ("view volume", volume::volume),
    ("probe", inspect::probe),
    ("stats", inspect::stats),
    ("profile", inspect::profile),
    ("measure distance", inspect::measure_distance),
    ("measure angle", inspect::measure_angle),
    ("measure area", inspect::measure_area),
    ("annotate add", annotate::add),
    ("annotate list", annotate::list),
    ("annotate rename", annotate::rename),
    ("annotate delete", annotate::delete),
    ("segment list", segment::list),
    ("segment threshold", segment::threshold),
    ("engine info", engine::info),
    ("engine list", engine::list),
    ("segment interactive", interactive::interactive),
    ("segment auto", engine::auto),
    ("segment shape", masks::shape),
    ("segment components", masks::components),
    ("segment compare", masks::compare),
    ("segment edit", masks::edit),
    ("segment rename", segment::rename),
    ("segment delete", segment::delete),
    ("review list", review::list),
    ("review confirm", review::confirm),
    ("review reject", review::reject),
    ("export bundle", export::bundle),
];
