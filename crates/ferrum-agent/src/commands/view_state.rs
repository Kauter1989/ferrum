//! `view set`, `view state`, `view events` (Stage 28, #70): an agent drives
//! what the person sees by writing the workspace's view-state artifact, and
//! reads back the state and the events. The live clients apply the artifact
//! ([`ferrum_app::Viewer::apply_view_state`]); the agent never links the app
//! or the host (ADR 0018). No command creates a decision.

use ferrum_domain::{storage_histogram, SliceAxis, ViewState, WindowLevel, AUTO_WINDOW_BINS};
use serde_json::{json, Value};

use super::Ctx;
use crate::decisions::DecisionLock;
use crate::envelope::{AgentError, ErrorCode, Output};
use crate::params::Params;
use crate::study::Study;

/// The fields a view state may carry. `probe` is read-only: a set refuses it
/// (the schema omits it too, so CLI and MCP agree).
const STATE_FIELDS: &[&str] =
    &["component", "plane", "slice_index", "window", "overlays", "selected_segment", "tool", "camera"];

/// `view state`: the saved state (or the viewer's own default), with the
/// current revision and source hash the host passes back to `view set`.
pub fn state(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let (state, revision) = match study.workspace.load_view_state()? {
        Some((state, revision, _)) => (state, revision),
        None => (default_state(study), 0),
    };
    Ok(Output::new(json!({
        "format": ferrum_io::VIEW_STATE_FORMAT,
        "revision": revision,
        "source_hash": study.source_sha256,
        "state": ferrum_io::state_to_json(&state),
    })))
}

/// `view set`: validates a state, refuses a stale `source_hash` (fail-closed,
/// as `decisions::record`), and writes the artifact with the next revision
/// under the workspace lock (a concurrent set cannot lose an update or write
/// a revision the watcher would not report). The person takes over at any
/// time — this locks nothing against the person. Returns the stored state.
pub fn set(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let root = ctx.workspace_path(p)?;
    let study = ctx.study(p)?;
    let value = p.req("state")?;
    let object = value.as_object().ok_or_else(|| AgentError::bad_request("state must be an object"))?;
    for key in object.keys() {
        if !STATE_FIELDS.contains(&key.as_str()) {
            return Err(AgentError::bad_request(format!("state.{key} is not a view-state field")));
        }
    }
    let mut state = ferrum_io::state_from_json(value).map_err(AgentError::bad_request)?;
    state.probe = None; // read-only: a set never carries a pick

    let given = p.req_str("source_hash")?;
    if given != study.source_sha256 {
        return Err(AgentError::new(
            ErrorCode::SourceChanged,
            "the study changed since the state was read; nothing was applied",
        )
        .hint("read view state again and let the person decide on the current study"));
    }

    let _lock = DecisionLock::take(&root)?;
    let revision = study.workspace.load_view_state()?.map(|(_, r, _)| r).unwrap_or(0) + 1;
    study.workspace.save_view_state(&state, revision, &study.source_sha256)?;
    Ok(Output::new(json!({
        "format": ferrum_io::VIEW_STATE_FORMAT,
        "revision": revision,
        "source_hash": study.source_sha256,
        "state": ferrum_io::state_to_json(&state),
    })))
}

/// `view events`: the events the person produced, after `since` (a sequence
/// number), with the last sequence and the dropped count. No event is a
/// decision.
pub fn events(ctx: &mut Ctx, p: &Params) -> Result<Output, AgentError> {
    let study = ctx.study(p)?;
    let since = p.u64("since")?.unwrap_or(0);
    let (events, next, dropped) = study.workspace.load_view_events()?;
    let list: Vec<Value> =
        events.iter().filter(|(seq, _)| *seq > since).map(|(seq, e)| ferrum_io::event_to_json(*seq, e)).collect();
    Ok(Output::new(json!({
        "format": ferrum_io::VIEW_STATE_FORMAT,
        "events": list,
        "next": next,
        "dropped": dropped,
    })))
}

/// The default state matches what the viewer shows after opening the study:
/// the axial mid-slice and the series' own window (else the auto window), so
/// a host passing `view state` back to `view set` changes nothing unintended.
fn default_state(study: &Study) -> ViewState {
    let volume = &study.volume;
    let window = study
        .metadata
        .default_window
        .unwrap_or_else(|| WindowLevel::auto(volume.range(), &storage_histogram(volume.data(), AUTO_WINDOW_BINS)));
    ViewState { plane: SliceAxis::Axial, slice_index: volume.dims().z / 2, window, ..Default::default() }
}
