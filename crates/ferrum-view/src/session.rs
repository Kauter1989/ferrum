//! One open workspace kept alive for the harness.
//!
//! A session owns the [`Viewer`], its workspace and the frame source, and it
//! translates the state document of `ferrum-view/1` into use cases; the
//! server is only transport. Every value comes from the voxels through the
//! application layer.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ferrum_app::Viewer;
use ferrum_domain::{ResultStore, ViewEvent, VoxelBox, WindowLevel, MASK_FILL};
use ferrum_io::workspace::changes::{WorkspaceChange, WorkspaceWatcher};
use ferrum_io::{CompositeRepository, WorkspaceStore};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cpu::CpuFrameSource;
use crate::frame::{Backend, BackendPreference, FrameSource};
use crate::gpu::GpuFrameSource;
use crate::state::{annotations_json, events_json, state_json, Input};

/// The lower-case hex SHA-256 of `bytes`, as the workspace format writes it.
fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

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

/// A masked frame cached with the revision and the mask it was drawn for.
struct MaskedFrame {
    rev: u64,
    boxes: Vec<VoxelBox>,
    frame: Frame,
}

/// The wire form of the volume voxels (Stage 29, criterion 29.3-a).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VolumeStorage {
    /// The volume's `u16` storage, little-endian; the client reconstructs the
    /// physical value as `min + stored × (max − min) / 65535` from `value_range`.
    /// Half the bytes of `f32`, and lossless (the storage is the source of truth).
    #[default]
    U16,
    /// Physical `f32` values, little-endian (the original form).
    F32,
}

/// A view host for one workspace.
///
/// The view state is the [`Viewer`]'s own (Stage 29): the session keeps no
/// second copy to drift — it reads `viewer.view_state()` for the document
/// and applies every input and every written artifact through
/// `Viewer::apply_view_state`. Only the revision counter, the event ring and
/// the frame cache are the host's.
pub struct Session {
    workspace: PathBuf,
    store: Arc<dyn ResultStore>,
    viewer: Viewer,
    source: Box<dyn FrameSource>,
    rev: u64,
    events: Vec<(u64, ViewEvent)>,
    seq: u64,
    dropped: u64,
    frame: Option<Frame>,
    masked_frame: Option<MaskedFrame>,
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
        let source = build_source(preference)?;
        let watcher = WorkspaceWatcher::new(workspace);
        Ok(Self {
            workspace: workspace.to_path_buf(),
            store,
            viewer,
            source,
            rev: 0,
            events: Vec::new(),
            seq: 0,
            dropped: 0,
            frame: None,
            masked_frame: None,
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

    /// The state document (`ferrum-view/1`): the full domain view state,
    /// with the mapping of the last frame when there is one.
    pub fn state_json(&self) -> Result<Value, ViewError> {
        let mut state = state_json(&self.viewer, self.rev)?;
        if let Some(frame) = &self.frame {
            state["frame"] = json!({
                "rev": frame.rev,
                "size": [frame.width, frame.height],
                "mapping": frame.mapping,
            });
        }
        Ok(state)
    }

    /// The study's voxels for a client that renders them itself: the header
    /// (dims, spacing, value range, storage form) as JSON and the voxels, `i`
    /// fastest, in the chosen [`VolumeStorage`] form.
    ///
    /// A browser panel takes this once and draws with the GPU of the person's
    /// machine, so nothing but the volume crosses the wire (ADR 0014
    /// decision 4, the local backend). The default `u16` form halves the wire;
    /// `f32` stays available for a client that asks for it.
    pub fn volume(&self, storage: VolumeStorage) -> Result<(Value, Vec<u8>), ViewError> {
        let dataset = self.viewer.dataset().ok_or(ViewError::NoStudy)?;
        let volume = &dataset.volume;
        let dims = volume.dims();
        let spacing = volume.spacing();
        let range = volume.range();
        let values: Vec<u8> = match storage {
            VolumeStorage::U16 => volume.data().iter().flat_map(|s| s.to_le_bytes()).collect(),
            VolumeStorage::F32 => {
                let mut out = Vec::with_capacity(volume.data().len() * 4);
                for k in 0..dims.z {
                    for j in 0..dims.y {
                        for i in 0..dims.x {
                            let value = volume.physical(i, j, k).unwrap_or(0.0);
                            out.extend_from_slice(&value.to_le_bytes());
                        }
                    }
                }
                out
            }
        };
        let header = json!({
            "format": "ferrum-volume/1",
            "storage": match storage { VolumeStorage::U16 => "u16", VolumeStorage::F32 => "f32" },
            "dims": [dims.x, dims.y, dims.z],
            "spacing_mm": [spacing.x, spacing.y, spacing.z],
            "value_range": [range.min, range.max],
            "value_unit": crate::state::value_unit(&self.viewer),
            "frame": "LPS: voxel i, j, k are the stored order of the canonical grid",
            "bytes": values.len(),
            "sha256": hex_sha256(&values),
        });
        Ok((header, values))
    }

    /// The segment label map for a client that draws the overlay itself: one
    /// `u8` label per voxel (`i` fastest, the volume's grid) and the segment
    /// metadata. A study with no segmentation gives a zero grid (Stage 29,
    /// criterion 29.2-a's wire).
    pub fn labels(&self) -> Result<(Value, Vec<u8>), ViewError> {
        let dataset = self.viewer.dataset().ok_or(ViewError::NoStudy)?;
        let dims = dataset.volume.dims();
        let set = self.viewer.segmentation().set();
        let values: Vec<u8> = match set {
            Some(set) => set.labels().data().to_vec(),
            None => vec![0u8; dims.voxel_count()],
        };
        let items: Vec<Value> = set
            .map(|s| {
                s.segments()
                    .iter()
                    .map(|g| json!({ "label": g.label, "name": g.name, "visible": g.visible, "opacity": g.opacity }))
                    .collect()
            })
            .unwrap_or_default();
        let header = json!({
            "format": "ferrum-labels/1",
            "dims": [dims.x, dims.y, dims.z],
            "bytes": values.len(),
            "sha256": hex_sha256(&values),
            "segments": items,
        });
        Ok((header, values))
    }

    /// The current frame of the state's component, drawn again only when
    /// the state changed.
    pub fn frame(&mut self) -> Result<&Frame, ViewError> {
        let fresh = self.frame.as_ref().is_some_and(|f| f.rev == self.rev);
        if !fresh {
            let component = self.viewer.view_state().component;
            let raw = self.source.render(&mut self.viewer, component)?;
            let png = raw.png()?;
            self.frame = Some(Frame {
                png,
                width: raw.width,
                height: raw.height,
                window: raw.window,
                mapping: raw.mapping,
                rev: self.rev,
            });
        }
        self.frame.as_ref().ok_or_else(|| ViewError::Internal("no frame was drawn".into()))
    }

    /// The current frame drawn over a **masked copy** of the data (Clean
    /// Pixel Data, Stage 31, criterion 31.3-c): the voxels of `boxes` are
    /// replaced by [`MASK_FILL`] before drawing; the view state, window and
    /// overlay are unchanged. Masking is a pixel transform on the data
    /// ([`ferrum_app::Viewer::render_masked`]) — both backends draw exactly
    /// as before, so the rendering rule is untouched. Cached per revision
    /// and mask, separately from the unmasked frame. Returns the frame and
    /// the effective boxes (clipped to the grid, empty ones dropped), so
    /// the caller reports exactly what was masked.
    pub fn frame_masked(&mut self, boxes: Vec<VoxelBox>) -> Result<(&Frame, Vec<VoxelBox>), ViewError> {
        let dims = self.viewer.dataset().ok_or(ViewError::NoStudy)?.volume.dims().as_uvec3();
        let boxes: Vec<VoxelBox> = boxes
            .into_iter()
            .map(|b| VoxelBox::new(b.min.min(dims), b.max.min(dims)))
            .filter(|b| !b.is_empty())
            .collect();
        let fresh = self.masked_frame.as_ref().is_some_and(|m| m.rev == self.rev && m.boxes == boxes);
        if !fresh {
            let component = self.viewer.view_state().component;
            let Self { viewer, source, .. } = self;
            let raw = viewer
                .render_masked(&boxes, MASK_FILL, |v| source.render(v, component))
                .ok_or(ViewError::NoStudy)??;
            let png = raw.png()?;
            self.masked_frame = Some(MaskedFrame {
                rev: self.rev,
                boxes,
                frame: Frame {
                    png,
                    width: raw.width,
                    height: raw.height,
                    window: raw.window,
                    mapping: raw.mapping,
                    rev: self.rev,
                },
            });
        }
        let cached =
            self.masked_frame.as_ref().ok_or_else(|| ViewError::Internal("no masked frame was drawn".into()))?;
        Ok((&cached.frame, cached.boxes.clone()))
    }

    /// The mask a `?masked=1` frame request applies when it names no region:
    /// the whole volume when any DICOM source carries `BurnedInAnnotation =
    /// YES` (the conservative default — the text's location is unknown),
    /// else nothing. The caller reports which case held.
    pub fn default_mask_boxes(&self) -> Result<Vec<VoxelBox>, ViewError> {
        let dataset = self.viewer.dataset().ok_or(ViewError::NoStudy)?;
        let ws = ferrum_io::Workspace::open(&self.workspace).map_err(|e| ViewError::Workspace(e.to_string()))?;
        let sources = ws.manifest().source.file_paths();
        if ferrum_io::dicom::clean_pixel::sources_burned_in(&sources) {
            return Ok(vec![VoxelBox::full(dataset.volume.dims())]);
        }
        Ok(Vec::new())
    }

    /// Applies one input object (see [`Input`]) through the viewer's use
    /// cases and records the events it produced.
    pub fn input(&mut self, body: &[u8]) -> Result<(), ViewError> {
        let input = Input::parse(body)?;
        let events = input.apply(&mut self.viewer)?;
        for event in events {
            self.record(event);
        }
        self.rev += 1;
        self.persist_events();
        Ok(())
    }

    /// The events after `since` (all of them for `None`), as JSON.
    pub fn events_json(&self, since: Option<u64>) -> Value {
        events_json(&self.events, self.seq, self.dropped, since)
    }

    /// The measurements drawn on the study, as the annotations document
    /// (`ferrum-view/1`): the values the host's tool machine computed, for a
    /// panel that displays them (Stage 29, criterion 29.2-c).
    pub fn annotations_json(&self) -> Value {
        annotations_json(&self.viewer)
    }

    /// Reloads the workspace when another process changed it: a view state
    /// written by an agent is applied in full (Stage 29 — the host honours
    /// every field of the contract), and segments and annotations are
    /// reloaded. Returns `true` when the next frame differs.
    pub fn refresh(&mut self) -> bool {
        let changes = self.watcher.poll();
        if changes.is_empty() {
            return false;
        }
        let mut changed = false;
        let view_changed = changes.iter().any(|c| matches!(c, WorkspaceChange::ViewStateChanged { .. }));
        let data_changed = changes.iter().any(|c| !matches!(c, WorkspaceChange::ViewStateChanged { .. }));

        if view_changed {
            if let Ok(ws) = ferrum_io::Workspace::open(&self.workspace) {
                if let Ok(Some((view, _revision, _hash))) = ws.load_view_state() {
                    for event in self.viewer.apply_view_state(&view) {
                        self.record(event);
                    }
                    self.rev += 1;
                    changed = true;
                }
            }
        }

        if data_changed {
            if let Some(dataset) = self.viewer.dataset().cloned() {
                match self.store.load(&dataset.volume) {
                    Ok((annotations, segments)) => {
                        // The annotations an agent wrote reach the panel the same
                        // way the segments do: the workspace is the source of
                        // truth, and the host's own draws are saved there too
                        // (29.2-c). An absent file means no annotations.
                        self.viewer.set_annotations(annotations.unwrap_or_default());
                        self.viewer.adopt_segments(segments);
                        self.rev += 1;
                        changed = true;
                    }
                    Err(e) => {
                        log::warn!("ferrum-view: the workspace changed but could not be read again: {e}");
                    }
                }
            }
        }
        if changed {
            self.persist_events();
        }
        changed
    }

    /// Writes the event ring to the workspace's `view-events.json`, so an
    /// agent's `view events` reads the same sequence the host serves. The
    /// events are the domain's own, so they persist without a round trip.
    fn persist_events(&self) {
        let Ok(ws) = ferrum_io::Workspace::open(&self.workspace) else {
            return;
        };
        if let Err(e) = ws.save_view_events(&self.events, self.seq, self.dropped) {
            log::warn!("ferrum-view: could not persist the view events: {e}");
        }
    }

    fn record(&mut self, event: ViewEvent) {
        self.seq += 1;
        self.events.push((self.seq, event));
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
