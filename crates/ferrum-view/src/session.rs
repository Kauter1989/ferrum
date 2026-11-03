//! One open workspace kept alive for the harness.
//!
//! A session owns the [`Viewer`], its workspace and the frame source, and it
//! translates the state document of `ferrum-view/1` into use cases; the
//! server is only transport. Every value comes from the voxels through the
//! application layer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferrum_app::Viewer;
use ferrum_domain::{ResultStore, SliceAxis, WindowLevel};
use ferrum_io::workspace::changes::WorkspaceWatcher;
use ferrum_io::{CompositeRepository, WorkspaceStore};
use serde_json::{json, Value};

use crate::cpu::CpuFrameSource;
use crate::frame::{Backend, BackendPreference, FrameSource};
use crate::gpu::GpuFrameSource;
use crate::state::{state_json, Event, Input, ViewState, FORMAT};

/// Everything a view host can fail at.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    /// No study is open (yet).
    #[error("no study is open")]
    NoStudy,
    /// The slice index lies outside the volume.
    #[error("the slice is outside the volume")]
    NoSlice,
    /// The workspace could not be opened or read.
    #[error("workspace: {0}")]
    Workspace(String),
    /// The request was malformed.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// No GPU could be created, or a frame could not be read back.
    #[error("gpu: {0}")]
    Gpu(String),
    /// Something else went wrong.
    #[error("{0}")]
    Internal(String),
}

/// A frame as the host serves it: PNG bytes with the window and the pixel
/// mapping of the state revision it belongs to.
#[derive(Debug, Clone)]
pub struct Frame {
    /// The PNG a harness shows.
    pub png: Vec<u8>,
    /// Width in pixels (the plane's width in voxels).
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The window it was drawn with.
    pub window: WindowLevel,
    /// Pixel to voxel and to patient millimetres.
    pub mapping: Value,
    /// The state revision it was drawn for.
    pub rev: u64,
}

/// How many events a session keeps (oldest dropped first).
const MAX_EVENTS: usize = 512;

/// A view host for one workspace.
pub struct Session {
    workspace: PathBuf,
    store: Arc<dyn ResultStore>,
    viewer: Viewer,
    source: Box<dyn FrameSource>,
    state: ViewState,
    events: Vec<Event>,
    seq: u64,
    dropped: u64,
    frame: Option<Frame>,
    watcher: WorkspaceWatcher,
}

impl Session {
    /// Opens the workspace's study and prepares a frame source: the GPU when
    /// one exists, the CPU reference otherwise (unless `preference` says
    /// which).
    pub fn open(workspace: &Path, preference: BackendPreference) -> Result<Self, ViewError> {
        let generator = format!("FERRUM {}", env!("CARGO_PKG_VERSION"));
        let store = WorkspaceStore::open(workspace, &generator).map_err(|e| ViewError::Workspace(e.to_string()))?;
        let store: Arc<dyn ResultStore> = Arc::new(store);
        let mut viewer = Viewer::new(Arc::new(CompositeRepository::default()));
        viewer.open_workspace(store.clone()).map_err(ViewError::Workspace)?;
        viewer.wait_idle();
        if viewer.dataset().is_none() {
            return Err(ViewError::Workspace("the workspace's study could not be loaded".into()));
        }
        let axis = SliceAxis::Axial;
        let state = ViewState {
            axis,
            index: viewer.slice_index(axis),
            window: viewer.window(),
            segments: viewer.segmentation().overlay_active(),
            rev: 0,
            probe: None,
        };
        let source = build_source(preference)?;
        let watcher = WorkspaceWatcher::new(workspace);
        Ok(Self {
            workspace: workspace.to_path_buf(),
            store,
            viewer,
            source,
            state,
            events: Vec::new(),
            seq: 0,
            dropped: 0,
            frame: None,
            watcher,
        })
    }

    /// The workspace the session serves.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Which backend draws the frames.
    pub fn backend(&self) -> Backend {
        self.source.backend()
    }

    /// The state document (`ferrum-view/1`), with the mapping of the last
    /// frame when there is one.
    pub fn state_json(&self) -> Result<Value, ViewError> {
        let mut state = state_json(&self.viewer, &self.state)?;
        if let Some(frame) = &self.frame {
            state["frame"] = json!({
                "rev": frame.rev,
                "size": [frame.width, frame.height],
                "mapping": frame.mapping,
            });
        }
        Ok(state)
    }

    /// The current frame, drawn again only when the state changed.
    pub fn frame(&mut self) -> Result<&Frame, ViewError> {
        let fresh = self.frame.as_ref().is_some_and(|f| f.rev == self.state.rev);
        if !fresh {
            let raw = self.source.render(&self.viewer, self.state.axis)?;
            let png = raw.png()?;
            self.frame = Some(Frame {
                png,
                width: raw.width,
                height: raw.height,
                window: raw.window,
                mapping: raw.mapping,
                rev: self.state.rev,
            });
        }
        self.frame.as_ref().ok_or_else(|| ViewError::Internal("no frame was drawn".into()))
    }

    /// Applies one input object (see [`Input`]) and records its event.
    pub fn input(&mut self, body: &[u8]) -> Result<(), ViewError> {
        let input = Input::parse(body)?;
        let (kind, data) = input.apply(&mut self.viewer, &mut self.state)?;
        self.record(kind, data);
        Ok(())
    }

    /// The events after `since` (all of them for `None`), as JSON.
    pub fn events_json(&self, since: Option<u64>) -> Value {
        let since = since.unwrap_or(0);
        let events: Vec<Value> = self.events.iter().filter(|e| e.seq > since).map(Event::to_json).collect();
        json!({
            "format": FORMAT,
            "events": events,
            "next": self.seq,
            "dropped": self.dropped,
        })
    }

    /// Reloads the workspace's segments when another process changed it.
    /// Returns `true` when the next frame differs.
    pub fn refresh(&mut self) -> bool {
        let changes = self.watcher.poll();
        if changes.is_empty() {
            return false;
        }
        let Some(dataset) = self.viewer.dataset().cloned() else {
            return false;
        };
        match self.store.load(&dataset.volume) {
            Ok((_annotations, segments)) => {
                self.viewer.adopt_segments(segments);
                self.state.segments = self.viewer.segmentation().overlay_active();
                self.state.rev += 1;
                true
            }
            Err(e) => {
                log::warn!("ferrum-view: the workspace changed but could not be read again: {e}");
                false
            }
        }
    }

    fn record(&mut self, kind: &'static str, data: Value) {
        self.seq += 1;
        self.events.push(Event { seq: self.seq, kind, data });
        while self.events.len() > MAX_EVENTS {
            self.events.remove(0);
            self.dropped += 1;
        }
    }
}

/// The frame source of `preference`.
fn build_source(preference: BackendPreference) -> Result<Box<dyn FrameSource>, ViewError> {
    match preference {
        BackendPreference::Cpu => Ok(Box::new(CpuFrameSource::new())),
        BackendPreference::Gpu => Ok(Box::new(GpuFrameSource::new()?)),
        BackendPreference::Auto => match GpuFrameSource::new() {
            Ok(gpu) => {
                log::info!("ferrum-view: frames on the GPU ({})", gpu.describe());
                Ok(Box::new(gpu))
            }
            Err(e) => {
                log::warn!("ferrum-view: no GPU ({e}); frames on the CPU reference");
                Ok(Box::new(CpuFrameSource::new()))
            }
        },
    }
}
