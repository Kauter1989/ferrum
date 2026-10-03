//! AI segmentation use cases of the [`Viewer`].
//!
//! The engine (any [`SegmentationEngine`], usually a `ferrum-engine/1`
//! client created by the presentation layer) is driven by a worker thread,
//! so uploads and inference never block the UI. Prompts refine the current
//! *object*: the engine's target mask, mirrored into a target segment.
//! *Accept* keeps the segment and starts the next object, *Discard*
//! removes it. Automatic engines run jobs whose label map becomes one
//! segment per structure, named after the engine's labels.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use ferrum_domain::{
    EngineError, EngineInfo, InteractiveSession, JobState, JobStatus, Prompt, PromptKind, PromptResult, Provenance,
    ReviewStatus, SegmentationEngine, Timestamp, Volume,
};

use super::Viewer;

/// Connection state of the AI engine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AiStatus {
    /// No engine connected.
    #[default]
    Disconnected,
    /// Asking the engine for its capabilities.
    Connecting,
    /// Connected; no session yet for the current volume.
    Connected,
    /// Uploading the volume (the engine may pre-compute features).
    Uploading,
    /// Ready for prompts.
    Ready,
    /// Waiting for the engine's answer to a prompt.
    Working,
    /// The last operation failed.
    Failed(String),
}

impl AiStatus {
    /// `true` while connected to an engine (whatever it is doing).
    pub fn is_connected(&self) -> bool {
        matches!(self, AiStatus::Connected | AiStatus::Uploading | AiStatus::Ready | AiStatus::Working)
    }
}

enum Command {
    Open { volume: Arc<Volume>, modality: String, revision: u64 },
    Prompt { prompt: Prompt, target: u8 },
    Undo { target: u8 },
    Reset,
    Automatic { labels: Option<Vec<String>> },
}

enum Event {
    Info(Result<EngineInfo, EngineError>),
    Opened {
        revision: u64,
        result: Result<(), EngineError>,
    },
    Mask {
        target: u8,
        result: Result<(PromptResult, Option<Vec<u8>>), EngineError>,
    },
    Reset(Result<(), EngineError>),
    /// Intermediate job progress (does not complete a request).
    Progress(JobStatus),
    /// Finished job: the label map, or `None` if cancelled.
    LabelMap(Result<Option<Vec<u16>>, EngineError>),
}

/// Interval between job status requests.
const JOB_POLL: Duration = Duration::from_millis(300);

/// Runs an automatic job to completion, reporting progress.
fn run_job(
    s: &mut dyn InteractiveSession,
    labels: Option<&[String]>,
    cancel: &AtomicBool,
    tx: &Sender<Event>,
) -> Result<Option<Vec<u16>>, EngineError> {
    let job = s.start_job(labels)?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            s.cancel_job(&job)?;
            return Ok(None);
        }
        let status = s.job_status(&job)?;
        let state = status.state;
        let message = status.message.clone();
        let _ = tx.send(Event::Progress(status));
        match state {
            JobState::Done => return s.label_map().map(Some),
            JobState::Failed => return Err(EngineError::Internal(message)),
            JobState::Cancelled => return Ok(None),
            JobState::Queued | JobState::Running => std::thread::sleep(JOB_POLL),
        }
    }
}

/// Background thread owning the engine session.
struct Worker {
    tx: Option<Sender<Command>>,
    rx: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(engine: Arc<dyn SegmentationEngine>, cancel: Arc<AtomicBool>) -> Self {
        let (cmd_tx, cmd_rx) = channel::<Command>();
        let (ev_tx, ev_rx) = channel::<Event>();
        let thread = std::thread::Builder::new()
            .name("ferrum-ai-engine".into())
            .spawn(move || run_worker(engine.as_ref(), &cancel, &cmd_rx, &ev_tx))
            .ok();
        Self { tx: Some(cmd_tx), rx: ev_rx, thread }
    }

    fn send(&self, cmd: Command) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(cmd).is_ok())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // closing the channel ends the loop; the session is dropped (and
        // closed on the engine) by the worker thread
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn mask_after(
    session: &mut dyn InteractiveSession,
    r: PromptResult,
) -> Result<(PromptResult, Option<Vec<u8>>), EngineError> {
    match r.changed {
        Some(bx) => Ok((r, Some(session.mask(bx)?))),
        None => Ok((r, None)),
    }
}

fn run_worker(engine: &dyn SegmentationEngine, cancel: &AtomicBool, rx: &Receiver<Command>, tx: &Sender<Event>) {
    let mut session: Option<Box<dyn InteractiveSession>> = None;
    let _ = tx.send(Event::Info(engine.info()));
    let no_session = || EngineError::Protocol("no session".into());
    for cmd in rx {
        let event = match cmd {
            Command::Open { volume, modality, revision } => {
                session = None;
                let result = engine.open_session(&volume, &modality).map(|s| session = Some(s));
                Event::Opened { revision, result }
            }
            Command::Prompt { prompt, target } => {
                let result = match session.as_deref_mut() {
                    Some(s) => s.prompt(&prompt).and_then(|r| mask_after(s, r)),
                    None => Err(no_session()),
                };
                Event::Mask { target, result }
            }
            Command::Undo { target } => {
                let result = match session.as_deref_mut() {
                    Some(s) => s.undo().and_then(|r| mask_after(s, r)),
                    None => Err(no_session()),
                };
                Event::Mask { target, result }
            }
            Command::Reset => Event::Reset(session.as_deref_mut().map_or(Ok(()), |s| s.reset())),
            Command::Automatic { labels } => Event::LabelMap(match session.as_deref_mut() {
                Some(s) => run_job(s, labels.as_deref(), cancel, tx),
                None => Err(no_session()),
            }),
        };
        if tx.send(event).is_err() {
            break;
        }
    }
}

/// State of the *AI segmentation* panel.
#[derive(Default)]
pub struct AiState {
    worker: Option<Worker>,
    status: AiStatus,
    info: Option<EngineInfo>,
    engine_label: String,
    session_revision: Option<u64>,
    pending: usize,
    target: Option<u8>,
    objects: u32,
    job: Option<JobStatus>,
    cancel: Arc<AtomicBool>,
    /// Prompts mark the object (`true`, *Include*) or background (*Exclude*).
    pub positive: bool,
}

impl std::fmt::Debug for AiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiState")
            .field("status", &self.status)
            .field("engine", &self.engine_label)
            .field("pending", &self.pending)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}

impl AiState {
    /// New state: disconnected, prompts include.
    pub fn new() -> Self {
        Self { positive: true, ..Self::default() }
    }

    /// Connection state.
    pub fn status(&self) -> &AiStatus {
        &self.status
    }

    /// Engine description once connected.
    pub fn info(&self) -> Option<&EngineInfo> {
        self.info.as_ref()
    }

    /// Where the engine was reached (e.g. its URL).
    pub fn engine_label(&self) -> &str {
        &self.engine_label
    }

    /// Segment that the current object is written to.
    pub fn target(&self) -> Option<u8> {
        self.target
    }

    /// `true` while requests are in flight.
    pub fn is_busy(&self) -> bool {
        self.pending > 0
    }

    /// `true` if the connected engine supports interactive `kind` prompts.
    pub fn supports(&self, kind: PromptKind) -> bool {
        self.status.is_connected() && self.info.as_ref().is_some_and(|i| i.supports(kind))
    }

    /// `true` if the engine can undo prompts.
    pub fn can_undo(&self) -> bool {
        self.status.is_connected() && self.target.is_some() && self.info.as_ref().is_some_and(|i| i.capabilities.undo)
    }

    /// `true` if the connected engine runs automatic segmentation.
    pub fn supports_automatic(&self) -> bool {
        self.status.is_connected() && self.info.as_ref().is_some_and(|i| i.capabilities.automatic)
    }

    /// Progress of the running automatic job.
    pub fn job(&self) -> Option<&JobStatus> {
        self.job.as_ref()
    }

    /// `true` if the engine marks its results for research use only.
    pub fn research_only(&self) -> bool {
        self.info.as_ref().is_some_and(|i| i.research_only)
    }

    /// Provenance of a result of the connected engine, created now: a
    /// proposal waiting for review.
    pub fn proposal(&self) -> Provenance {
        let (name, version) =
            self.info.as_ref().map_or(("unknown engine", ""), |i| (i.name.as_str(), i.version.as_str()));
        Provenance::engine(name, version, self.research_only(), Timestamp::now())
    }
}

impl Viewer {
    /// AI segmentation state.
    pub fn ai(&self) -> &AiState {
        &self.ai
    }

    /// Sets whether prompts include (mark the object) or exclude.
    pub fn set_ai_positive(&mut self, positive: bool) {
        self.ai.positive = positive;
    }

    /// Connects an engine; `label` describes it (e.g. its URL). The
    /// capabilities arrive asynchronously (see [`AiState::status`]).
    pub fn connect_engine(&mut self, engine: Arc<dyn SegmentationEngine>, label: &str) {
        self.disconnect_engine();
        self.ai.engine_label = label.to_owned();
        self.ai.status = AiStatus::Connecting;
        self.ai.pending = 1;
        self.ai.cancel = Arc::new(AtomicBool::new(false));
        self.ai.worker = Some(Worker::spawn(engine, self.ai.cancel.clone()));
    }

    /// Disconnects the engine (closing its session). The segments stay.
    pub fn disconnect_engine(&mut self) {
        self.ai.worker = None;
        self.ai.info = None;
        self.ai.status = AiStatus::Disconnected;
        self.ai.session_revision = None;
        self.ai.pending = 0;
        self.ai.target = None;
        self.ai.job = None;
        if self.tool.prompt_kind().is_some() {
            self.tool = crate::ToolKind::Pan;
            self.tool_ctl.cancel();
        }
    }

    fn ai_send(&mut self, cmd: Command) -> bool {
        let sent = self.ai.worker.as_ref().is_some_and(|w| w.send(cmd));
        if sent {
            self.ai.pending += 1;
        } else {
            self.ai.status = AiStatus::Failed("engine worker stopped".into());
        }
        sent
    }

    /// Sends a prompt for the current object. Opens a session for the
    /// current volume first if needed, and creates the target segment on
    /// the first prompt of an object. Returns `false` if the prompt cannot
    /// be sent (no engine, unsupported kind, invalid geometry, or the 3D
    /// view, where segmentation does not start).
    pub fn ai_prompt(&mut self, prompt: Prompt) -> bool {
        if self.can_segment().is_err() {
            return false;
        }
        let Some(d) = self.dataset.as_ref() else {
            return false;
        };
        if !self.ai.supports(prompt.kind()) || prompt.validate(d.volume.dims()).is_err() {
            return false;
        }
        if !self.ensure_ai_session() {
            return false;
        }
        let target = match self.ai.target.filter(|t| self.segmentation().set().is_some_and(|s| s.segment(*t).is_some()))
        {
            Some(t) => t,
            None => {
                self.ai.objects += 1;
                match self.add_segment(&format!("AI segment {}", self.ai.objects)) {
                    Ok(t) => {
                        let _ = self.set_segment_provenance(t, self.ai.proposal());
                        t
                    }
                    Err(e) => {
                        self.status.errors.push(e.to_string());
                        return false;
                    }
                }
            }
        };
        self.ai.target = Some(target);
        if self.ai.status == AiStatus::Ready {
            self.ai.status = AiStatus::Working;
        }
        self.ai_send(Command::Prompt { prompt, target })
    }

    /// Opens an engine session for the current volume unless one is open.
    fn ensure_ai_session(&mut self) -> bool {
        let Some(d) = self.dataset.as_ref() else {
            return false;
        };
        let (volume, modality, revision) = (d.volume.clone(), d.metadata.modality.clone(), d.revision);
        if self.ai.session_revision == Some(revision) {
            return true;
        }
        if !self.ai_send(Command::Open { volume, modality, revision }) {
            return false;
        }
        self.ai.session_revision = Some(revision);
        self.ai.status = AiStatus::Uploading;
        true
    }

    /// Runs automatic segmentation of `labels` (engine label names; `None`
    /// for all). Each structure found becomes a segment. Returns `false` if
    /// the engine cannot run it, a job is already running or the 3D view is
    /// shown (segmentation starts from the 2D and MPR views).
    pub fn ai_run_automatic(&mut self, labels: Option<Vec<String>>) -> bool {
        if self.can_segment().is_err()
            || !self.ai.supports_automatic()
            || self.ai.job.is_some()
            || !self.ensure_ai_session()
        {
            return false;
        }
        self.ai.cancel.store(false, Ordering::Relaxed);
        self.ai.job = Some(JobStatus { state: JobState::Queued, progress: 0.0, message: String::new() });
        if self.ai.status == AiStatus::Ready || self.ai.status == AiStatus::Connected {
            self.ai.status = AiStatus::Working;
        }
        self.ai_send(Command::Automatic { labels })
    }

    /// Asks the engine to cancel the running automatic job.
    pub fn ai_cancel_job(&mut self) {
        if self.ai.job.is_some() {
            self.ai.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Undoes the last prompt of the current object (engines with undo).
    pub fn ai_undo(&mut self) -> bool {
        match self.ai.target {
            Some(target) if self.ai.can_undo() => self.ai_send(Command::Undo { target }),
            _ => false,
        }
    }

    /// Keeps the current object's segment and starts a new object.
    pub fn ai_accept(&mut self) {
        if let Some(target) = self.ai.target.take() {
            // the user accepts the engine's proposal; the author stays the engine
            let _ = self.review_segment(target, ReviewStatus::Confirmed, None);
            self.ai_send(Command::Reset);
            self.status.message = "AI segment accepted".into();
        }
    }

    /// Removes the current object's segment and starts a new object.
    pub fn ai_discard(&mut self) {
        if let Some(t) = self.ai.target.take() {
            self.ai_send(Command::Reset);
            if let Err(e) = self.remove_segment(t) {
                self.status.errors.push(e.to_string());
            }
        }
    }

    /// Applies finished engine work. Called from [`Viewer::poll`].
    pub(super) fn poll_ai(&mut self) {
        while let Some(event) = self.ai.worker.as_ref().and_then(|w| w.rx.try_recv().ok()) {
            self.handle_ai_event(event);
        }
    }

    /// Blocks until the engine answered every request (tests, scripts).
    pub fn wait_ai_idle(&mut self) {
        while self.ai.pending > 0 {
            let Some(event) = self.ai.worker.as_ref().and_then(|w| w.rx.recv().ok()) else {
                self.ai.pending = 0;
                break;
            };
            self.handle_ai_event(event);
        }
    }

    fn ai_failed(&mut self, what: &str, e: &EngineError) {
        let msg = format!("{what}: {e}");
        self.status.message = msg.clone();
        self.status.errors.push(msg.clone());
        self.ai.status = AiStatus::Failed(msg);
    }

    fn handle_ai_event(&mut self, event: Event) {
        if !matches!(event, Event::Progress(_)) {
            self.ai.pending = self.ai.pending.saturating_sub(1);
        }
        match event {
            Event::Info(Ok(info)) if info.capabilities.interactive || info.capabilities.automatic => {
                self.status.message = format!("Connected to {} {}", info.name, info.version);
                self.ai.info = Some(info);
                self.ai.status = AiStatus::Connected;
            }
            Event::Info(Ok(info)) => {
                let e = EngineError::Unsupported(format!(
                    "{} offers neither interactive nor automatic segmentation",
                    info.name
                ));
                self.ai_failed("AI engine", &e);
                self.ai.worker = None;
            }
            Event::Info(Err(e)) => {
                self.ai_failed("AI engine", &e);
                self.ai.worker = None;
            }
            Event::Opened { revision, result } => match result {
                Ok(()) if Some(revision) == self.ai.session_revision => self.ai.status = AiStatus::Working,
                Ok(()) => {}
                Err(e) => {
                    self.ai.session_revision = None;
                    self.ai_failed("Volume upload", &e);
                }
            },
            Event::Mask { target, result } => match result {
                Ok((r, mask)) => self.apply_ai_mask(target, r, mask),
                Err(e) => self.ai_failed("AI prompt", &e),
            },
            Event::Reset(Err(e)) => self.ai_failed("AI reset", &e),
            Event::Reset(Ok(())) => {}
            Event::Progress(status) => self.ai.job = Some(status),
            Event::LabelMap(result) => {
                self.ai.job = None;
                match result {
                    Ok(Some(values)) => self.apply_engine_label_map(&values),
                    Ok(None) => self.status.message = "Automatic segmentation cancelled".into(),
                    Err(e) => self.ai_failed("Automatic segmentation", &e),
                }
            }
        }
        if self.ai.pending == 0 && matches!(self.ai.status, AiStatus::Working | AiStatus::Uploading) {
            self.ai.status = AiStatus::Ready;
        }
    }

    /// Creates one segment per engine label present in `values` (named and
    /// coloured after the engine's label list) and fills them; voxels that
    /// already belong to a segment are kept.
    fn apply_engine_label_map(&mut self, values: &[u16]) {
        let labels = self.ai.info.as_ref().map(|i| i.labels.clone()).unwrap_or_default();
        let mut present = vec![false; usize::from(u16::MAX) + 1];
        for &v in values {
            present[usize::from(v)] = true;
        }
        let mut mapping = Vec::new();
        for (value, _) in present.iter().enumerate().skip(1).filter(|(_, p)| **p) {
            let value = value as u16;
            let known = labels.iter().find(|l| l.value == value);
            let name = known.map_or_else(|| format!("Label {value}"), |l| l.name.clone());
            match self.add_segment(&name) {
                Ok(label) => {
                    let _ = self.set_segment_provenance(label, self.ai.proposal());
                    if let Some(c) = known.and_then(|l| l.color) {
                        let _ = self.set_segment_color(label, c);
                    }
                    mapping.push((value, label));
                }
                Err(e) => {
                    self.status.errors.push(format!("{name}: {e}"));
                    break;
                }
            }
        }
        let count = mapping.len();
        let result = match self.segments.set.as_mut() {
            Some(set) => set.apply_label_values(values, &mapping, false).map(|_| ()),
            None => Ok(()),
        };
        match result {
            Ok(()) => self.status.message = format!("Automatic segmentation: {count} structure(s)"),
            Err(e) => self.status.errors.push(e.to_string()),
        }
    }

    fn apply_ai_mask(&mut self, target: u8, r: PromptResult, mask: Option<Vec<u8>>) {
        let (Some(bx), Some(mask)) = (r.changed, mask) else {
            return;
        };
        if let Err(e) = self.apply_segment_mask(target, bx, &mask, false) {
            // the target may have been deleted meanwhile; nothing to update
            log::debug!("AI result for segment {target} dropped: {e}");
        }
    }

    /// Closes the engine session when the dataset is replaced.
    pub(super) fn reset_ai_session(&mut self) {
        self.ai.session_revision = None;
        self.ai.target = None;
        self.ai.objects = 0;
        self.ai.job = None;
        if self.ai.status.is_connected() {
            self.ai.status = AiStatus::Connected;
        }
    }
}
