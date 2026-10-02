//! Which tools and actions each view mode offers, and where the user is in
//! the segmentation workflow.
//!
//! Every tool acts on slices, so tools work in the 2D and MPR views only;
//! the 3D view navigates and erases. Segmentation follows one path in every
//! method: choose a segmentation tool, draw on a slice, check the result
//! (2D, MPR, 3D) and keep it. [`Viewer::segmentation_step`] tells the UI
//! which step comes next.

use crate::tools::ToolKind;

use super::{ViewMode, Viewer};

/// The next step of the segmentation workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentationStep {
    /// Segmentation is not possible here; the reason says what to do.
    Unavailable(String),
    /// No segmentation tool is selected.
    ChooseTool,
    /// A segmentation tool is selected; the user draws with it on a slice.
    Draw(ToolKind),
    /// The engine is computing.
    Working,
    /// An AI object is being refined: more prompts, or accept or discard.
    ReviewObject,
}

impl ViewMode {
    /// `true` for layouts with slice views (where tools work).
    pub fn has_slices(self) -> bool {
        matches!(self, ViewMode::Slice2d | ViewMode::Mpr)
    }

    /// `true` for layouts with a 3D view.
    pub fn has_volume(self) -> bool {
        matches!(self, ViewMode::Volume3d | ViewMode::Mpr)
    }
}

impl Viewer {
    /// Active layout.
    pub fn view_mode(&self) -> ViewMode {
        self.view_mode
    }

    /// Switches the layout. Drawing in progress is cancelled. The volume
    /// eraser belongs to the 3D view and is turned off when leaving it, so
    /// the 3D cell of the MPR layout never erases unnoticed.
    pub fn set_view_mode(&mut self, mode: ViewMode) {
        if mode == self.view_mode {
            return;
        }
        self.view_mode = mode;
        self.tool_ctl.cancel();
        self.pending_text = None;
        if mode != ViewMode::Volume3d {
            self.volume.eraser_enabled = false;
        }
    }

    /// Whether `tool` can be used now, or why not (for the UI to show).
    pub fn tool_availability(&self, tool: ToolKind) -> Result<(), String> {
        if self.dataset.is_none() {
            return Err("Open a study first".into());
        }
        if !self.view_mode.has_slices() {
            return Err(if tool.is_segmentation() {
                "Segments are drawn on slices: switch to the 2D or MPR view".into()
            } else {
                "Works on slices: switch to the 2D or MPR view".into()
            });
        }
        let Some(kind) = tool.prompt_kind() else {
            return Ok(());
        };
        let ai = &self.ai;
        if !ai.status().is_connected() {
            return Err("Needs a segmentation engine: connect one under Segmentation in the settings panel".into());
        }
        let name = ai.info().map(|i| i.name.clone()).unwrap_or_else(|| "The engine".into());
        if !ai.info().is_some_and(|i| i.capabilities.interactive) {
            return Err(format!("{name} segments automatically only: use Segment in the settings panel"));
        }
        if !ai.supports(kind) {
            return Err(format!("{name} does not take {} prompts", kind.as_str()));
        }
        Ok(())
    }

    /// Selects a tool (cancelling any drawing in progress). Returns `false`
    /// and keeps the current tool when `tool` is not available (see
    /// [`Viewer::tool_availability`]).
    pub fn select_tool(&mut self, tool: ToolKind) -> bool {
        if let Err(reason) = self.tool_availability(tool) {
            self.status.message = reason;
            return false;
        }
        self.tool = tool;
        self.tool_ctl.cancel();
        true
    }

    /// `Ok` if segments can be created now: a study is open and slices are
    /// shown. Segmentation is never started from the 3D view.
    pub fn can_segment(&self) -> Result<(), String> {
        self.tool_availability(ToolKind::Region)
    }

    /// The next step of the segmentation workflow.
    pub fn segmentation_step(&self) -> SegmentationStep {
        if let Err(reason) = self.can_segment() {
            return SegmentationStep::Unavailable(reason);
        }
        if self.ai.is_busy() {
            return SegmentationStep::Working;
        }
        if self.ai.target().is_some() {
            return SegmentationStep::ReviewObject;
        }
        if self.tool.is_segmentation() {
            SegmentationStep::Draw(self.tool)
        } else {
            SegmentationStep::ChooseTool
        }
    }
}
