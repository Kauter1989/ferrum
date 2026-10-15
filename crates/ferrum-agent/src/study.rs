//! A study loaded from a workspace: the volume plus the annotations and
//! segments saved next to it.
//!
//! The command line is stateless: every call opens the workspace, checks
//! the source hashes, loads the series and saves what it changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use ferrum_domain::{
    AnnotationReport, AnnotationSet, NoProgress, SegmentationSet, SeriesDescriptor, SeriesMetadata, StudyInfo, Volume,
    VolumeRepository,
};
use ferrum_io::{CompositeRepository, Workspace};
use sha2::{Digest, Sha256};

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};

/// Name and version written into the files FERRUM produces.
pub fn generator() -> String {
    format!("FERRUM {}", env!("CARGO_PKG_VERSION"))
}

/// Stable, salted pseudonym of an identifier (`anon-` + 16 hex digits).
pub fn pseudonym(salt: &str, id: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update([0]);
    h.update(id.as_bytes());
    let hex: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("anon-{hex}")
}

/// How series are named in outputs: their id, or its pseudonym.
pub fn series_key(config: &AgentConfig, id: &str) -> String {
    if config.pseudonymise_uids {
        pseudonym(&config.salt, id)
    } else {
        id.to_owned()
    }
}

/// Finds all series under `paths`.
pub fn scan(paths: &[PathBuf]) -> Result<Vec<SeriesDescriptor>, AgentError> {
    CompositeRepository::default().scan(paths, &NoProgress).map_err(|e| match e {
        ferrum_domain::RepositoryError::NothingFound => {
            AgentError::not_found("no DICOM or NIfTI series found").hint("give a DICOM folder or a .nii/.nii.gz file")
        }
        other => AgentError::internal(other.to_string()),
    })
}

/// Loaded series kept in memory between calls of a long-running server
/// (MCP), keyed by workspace. An entry is reused while every source file
/// keeps its size and modification time; otherwise the sources are hashed
/// and loaded again. Annotations and segments are never cached: they are
/// read from the workspace on every call, so decisions made meanwhile in
/// the desktop app are not overwritten.
#[derive(Debug, Default)]
pub struct VolumeCache {
    entries: HashMap<PathBuf, CachedSeries>,
    /// Engine sessions of interactive objects (see [`crate::sessions`]).
    pub engines: crate::sessions::EngineSessions,
}

#[derive(Debug, Clone)]
struct CachedSeries {
    series_id: String,
    open_volume: Option<String>,
    volumes: Vec<SeriesDescriptor>,
    stamps: Vec<(u64, Option<SystemTime>)>,
    volume: Arc<Volume>,
    metadata: Arc<SeriesMetadata>,
}

impl VolumeCache {
    /// Number of cached series.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` if nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn get(&self, root: &Path, ws: &Workspace) -> Option<CachedSeries> {
        let e = self.entries.get(root)?;
        let src = &ws.manifest().source;
        (e.series_id == src.series_id && e.open_volume == src.volume && e.stamps == stamps(&src.file_paths()))
            .then(|| e.clone())
    }
}

/// Size and modification time of each file (`(0, None)` if missing).
fn stamps(files: &[PathBuf]) -> Vec<(u64, Option<SystemTime>)> {
    files.iter().map(|f| std::fs::metadata(f).map_or((0, None), |m| (m.len(), m.modified().ok()))).collect()
}

/// An open study.
#[derive(Debug)]
pub struct Study {
    /// The workspace.
    pub workspace: Workspace,
    /// The volume in the canonical LPS frame.
    pub volume: Arc<Volume>,
    /// Metadata of the series.
    pub metadata: Arc<SeriesMetadata>,
    /// Annotations saved in the workspace.
    pub annotations: AnnotationSet,
    /// Segments saved in the workspace (an empty set if there are none).
    pub segments: SegmentationSet,
    /// SHA-256 over the source file hashes: identifies the data the
    /// results belong to.
    pub source_sha256: String,
    /// The open volume of a multi-volume series: its descriptor (id and
    /// [`ferrum_domain::VolumeInfo`]); `None` for a single-volume series.
    pub open_volume: Option<SeriesDescriptor>,
    /// Every volume of the series, when it holds several.
    pub volumes: Vec<SeriesDescriptor>,
    /// What `study open` chose for the caller (a default volume); repeated
    /// as warnings, not saved.
    pub notes: Vec<String>,
}

impl Study {
    /// Opens `source` into the workspace at `root` (creating it), choosing
    /// `series` (an id or its pseudonym) when the source holds several.
    /// An existing workspace is reused if it holds the same series.
    pub fn open(
        config: &AgentConfig,
        root: &Path,
        source: &Path,
        series: Option<&str>,
        cache: Option<&mut VolumeCache>,
    ) -> Result<Self, AgentError> {
        let source = config.check_source(source)?;
        if !source.exists() {
            return Err(AgentError::not_found(format!("{} does not exist", source.display())));
        }
        let found = scan(std::slice::from_ref(&source))?;
        let choice = choose(config, &found, series)?;
        let wanted = choice.chosen.volume.as_ref().map(|_| choice.chosen.id.clone());
        let mut notes = Vec::new();
        if root.join(ferrum_io::workspace::files::MANIFEST).exists() {
            let mut ws = Workspace::open(root)?;
            let m = &ws.manifest().source;
            if m.path != source || m.series_id != choice.group.id {
                return Err(AgentError::bad_request(format!("{} already holds another series", root.display()))
                    .hint("use a new workspace for each series; the volumes of one series share it"));
            }
            match (&wanted, choice.defaulted) {
                // the caller named no volume: keep the workspace's
                (Some(_), true) if m.volume.is_some() => {}
                (Some(id), _) if m.volume.as_ref() != Some(id) => {
                    check_switch(&choice, m.volume.as_deref())?;
                    ws.set_volume(Some(id))?;
                }
                _ => {}
            }
            let mut study = Self::load(config, root, cache)?;
            if choice.defaulted && study.open_volume.is_some() {
                notes.push(default_note(&study));
            }
            study.notes = notes;
            return Ok(study);
        }
        check_size(config, &choice.chosen)?;
        let mut workspace = Workspace::create(root, &source, &choice.group, &generator())?;
        if let Some(id) = &wanted {
            workspace.set_volume(Some(id))?;
        }
        let loaded = load_series(&choice.chosen)?;
        if let Some(c) = cache {
            c.entries.insert(root.to_path_buf(), cached(&workspace, &loaded, &choice.volumes));
        }
        let mut study = Self::from_workspace(workspace, loaded.volume, loaded.metadata)?;
        study.volumes = choice.volumes;
        study.open_volume = wanted.and_then(|id| study.volumes.iter().find(|v| v.id == id).cloned());
        if choice.defaulted && study.open_volume.is_some() {
            study.notes.push(default_note(&study));
        }
        Ok(study)
    }

    /// Loads the study of the workspace at `root`, reusing a cached series
    /// when its source files are unchanged.
    pub fn load(config: &AgentConfig, root: &Path, cache: Option<&mut VolumeCache>) -> Result<Self, AgentError> {
        if !root.join(ferrum_io::workspace::files::MANIFEST).exists() {
            return Err(AgentError::new(ErrorCode::NoStudy, format!("{} is not a workspace", root.display()))
                .hint("open a study first: study open --workspace <ws> <path>"));
        }
        let workspace = Workspace::open(root)?;
        let src = workspace.manifest().source.clone();
        config.check_source(&src.path)?;
        if let Some(hit) = cache.as_deref().and_then(|c| c.get(root, &workspace)) {
            return Self::from_workspace(workspace, hit.volume, hit.metadata)?.with_volumes(hit.volumes, src.volume);
        }
        workspace.verify_sources()?;
        let found = scan(std::slice::from_ref(&src.path))?;
        let series = found.iter().find(|s| s.id == src.series_id).ok_or_else(|| {
            AgentError::new(ErrorCode::SourceChanged, "the workspace's series is no longer in its source")
        })?;
        // The open volume of a multi-volume series, else the series.
        let volumes = match &src.volume {
            Some(_) => volumes_of(series)?,
            None => Vec::new(),
        };
        let chosen = match &src.volume {
            Some(id) => volumes.iter().find(|v| &v.id == id).ok_or_else(|| {
                AgentError::new(ErrorCode::SourceChanged, "the workspace's volume is no longer in its series")
            })?,
            None => series,
        };
        check_size(config, chosen)?;
        let loaded = load_series(chosen)?;
        if let Some(c) = cache {
            c.entries.insert(root.to_path_buf(), cached(&workspace, &loaded, &volumes));
        }
        Self::from_workspace(workspace, loaded.volume, loaded.metadata)?.with_volumes(volumes, src.volume)
    }

    /// Sets the series' volumes and the open one (by id).
    fn with_volumes(mut self, volumes: Vec<SeriesDescriptor>, open: Option<String>) -> Result<Self, AgentError> {
        self.open_volume = open.and_then(|id| volumes.iter().find(|v| v.id == id).cloned());
        self.volumes = volumes;
        Ok(self)
    }

    fn from_workspace(
        workspace: Workspace,
        volume: Arc<Volume>,
        metadata: Arc<SeriesMetadata>,
    ) -> Result<Self, AgentError> {
        let metadata = match &workspace.manifest().modality {
            Some(m) if metadata.modality.is_empty() => {
                Arc::new(SeriesMetadata { modality: m.clone(), ..(*metadata).clone() })
            }
            _ => metadata,
        };
        let d = volume.dims();
        let annotations = workspace.load_annotations([d.x, d.y, d.z])?.unwrap_or_default();
        let segments = workspace.load_segments(&volume)?.unwrap_or_else(|| SegmentationSet::new(d));
        let mut h = Sha256::new();
        for f in &workspace.manifest().source.files {
            h.update(f.sha256.as_bytes());
        }
        let source_sha256 = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        Ok(Self {
            workspace,
            volume,
            open_volume: None,
            volumes: Vec::new(),
            notes: Vec::new(),
            metadata,
            annotations,
            segments,
            source_sha256,
        })
    }

    /// Saves the annotations into the workspace (study identification
    /// filtered by the operator's privacy settings).
    pub fn save_annotations(&self, config: &AgentConfig) -> Result<(), AgentError> {
        Ok(self.workspace.save_annotations(&self.annotation_report(config), &generator())?)
    }

    /// The annotation report as the agent may write it: study identifiers
    /// pass [`shared_study`].
    pub fn annotation_report(&self, config: &AgentConfig) -> AnnotationReport {
        AnnotationReport::build(
            self.workspace.manifest().source.path.clone(),
            shared_study(config, &self.metadata.study),
            &self.volume,
            &self.annotations,
        )
    }

    /// Saves the segments into the workspace.
    pub fn save_segments(&self) -> Result<(), AgentError> {
        Ok(self.workspace.save_segments(&self.segments, &self.volume, &generator())?)
    }

    /// Folder of renders.
    pub fn renders_dir(&self) -> PathBuf {
        self.workspace.path(ferrum_io::workspace::files::RENDERS)
    }

    /// Declares the modality of a source that carries none (NIfTI) and
    /// saves it in the workspace. A source with its own modality keeps it:
    /// declaring another one is a `bad_request`.
    pub fn declare_modality(&mut self, modality: &str) -> Result<(), AgentError> {
        let m =
            ferrum_domain::parse_modality(modality).map_err(|e| AgentError::bad_request(e).hint("e.g. CT, MR, PT"))?;
        let declared = self.workspace.manifest().modality.is_some();
        if !declared && !self.metadata.modality.is_empty() {
            if self.metadata.modality.eq_ignore_ascii_case(&m) {
                return Ok(());
            }
            return Err(AgentError::bad_request(format!(
                "the series carries the modality {}; it cannot be declared {m}",
                self.metadata.modality
            )));
        }
        self.workspace.set_modality(Some(&m))?;
        self.metadata = Arc::new(SeriesMetadata { modality: m, ..(*self.metadata).clone() });
        Ok(())
    }

    /// `true` if the modality was declared rather than read from the source.
    pub fn modality_declared(&self) -> bool {
        self.workspace.manifest().modality.is_some()
    }

    /// Unit of the voxel values: `HU` for CT, empty otherwise.
    pub fn value_unit(&self) -> &'static str {
        if self.metadata.modality.eq_ignore_ascii_case("CT") {
            "HU"
        } else {
            ""
        }
    }
}

/// Study identification as the operator allows it to leave FERRUM: UIDs
/// pseudonymised (unless `pseudonymise_uids` is off), dates only with
/// `expose_dates`, the accession number only with `expose_identifiers`.
pub fn shared_study(config: &AgentConfig, s: &StudyInfo) -> StudyInfo {
    let uid = |id: &str| if id.is_empty() { String::new() } else { series_key(config, id) };
    let keep = |allowed: bool, v: &str| if allowed { v.to_owned() } else { String::new() };
    StudyInfo {
        study_instance_uid: uid(&s.study_instance_uid),
        series_instance_uid: uid(&s.series_instance_uid),
        study_date: keep(config.expose_dates, &s.study_date),
        study_time: keep(config.expose_dates, &s.study_time),
        accession_number: keep(config.expose_identifiers, &s.accession_number),
        ..s.clone()
    }
}

/// A series as loaded from its source.
struct Loaded {
    volume: Arc<Volume>,
    metadata: Arc<SeriesMetadata>,
}

fn load_series(series: &SeriesDescriptor) -> Result<Loaded, AgentError> {
    let loaded = CompositeRepository::default()
        .load(series, &NoProgress)
        .map_err(|e| AgentError::internal(format!("cannot load the series: {e}")))?;
    Ok(Loaded { volume: Arc::new(loaded.volume), metadata: Arc::new(loaded.metadata) })
}

fn cached(ws: &Workspace, loaded: &Loaded, volumes: &[SeriesDescriptor]) -> CachedSeries {
    let src = &ws.manifest().source;
    CachedSeries {
        series_id: src.series_id.clone(),
        open_volume: src.volume.clone(),
        volumes: volumes.to_vec(),
        stamps: stamps(&src.file_paths()),
        volume: loaded.volume.clone(),
        metadata: loaded.metadata.clone(),
    }
}

/// What `study open` resolved `series` to.
struct Choice {
    /// The series (group) the workspace is bound to.
    group: SeriesDescriptor,
    /// What is loaded: a volume of the group, or the group itself.
    chosen: SeriesDescriptor,
    /// Every volume of the group (empty for a single-volume series).
    volumes: Vec<SeriesDescriptor>,
    /// `true` when the caller named no volume and volume 1 was taken.
    defaulted: bool,
}

/// The volumes of `series`; empty when it holds only one.
pub(crate) fn volumes_of(series: &SeriesDescriptor) -> Result<Vec<SeriesDescriptor>, AgentError> {
    let volumes = CompositeRepository::default()
        .volumes(series, &NoProgress)
        .map_err(|e| AgentError::internal(format!("cannot list the volumes of the series: {e}")))?;
    Ok(if volumes.len() > 1 { volumes } else { Vec::new() })
}

/// The whole series `group`: volume 1 when it holds several.
fn choose_group(group: &SeriesDescriptor) -> Result<Choice, AgentError> {
    let volumes = volumes_of(group)?;
    let chosen = volumes.first().cloned().unwrap_or_else(|| group.clone());
    let defaulted = !volumes.is_empty();
    Ok(Choice { group: group.clone(), chosen, volumes, defaulted })
}

/// Resolves `wanted` (a series or volume id, or its pseudonym) in `found`.
fn choose(config: &AgentConfig, found: &[SeriesDescriptor], wanted: Option<&str>) -> Result<Choice, AgentError> {
    let w = match (wanted, found) {
        (None, [one]) => return choose_group(one),
        (None, _) => {
            return Err(AgentError::bad_request(format!("the source holds {} series", found.len()))
                .hint("choose one with series (see study scan)"))
        }
        (Some(w), _) => w,
    };
    let named = |id: &str| id == w || series_key(config, id) == w;
    if let Some(group) = found.iter().find(|s| named(&s.id)) {
        return choose_group(group);
    }
    for group in found {
        let volumes = volumes_of(group)?;
        if let Some(v) = volumes.iter().find(|v| named(&v.id)) {
            let chosen = v.clone();
            return Ok(Choice { group: group.clone(), chosen, volumes, defaulted: false });
        }
    }
    Err(AgentError::not_found(format!("no series or volume {w} in the source")).hint("see study scan"))
}

/// A workspace switches only between complete volumes: they share its grid.
fn check_switch(choice: &Choice, current: Option<&str>) -> Result<(), AgentError> {
    let complete = |v: &SeriesDescriptor| v.volume.as_ref().is_none_or(|i| i.complete);
    let current_complete = current.and_then(|id| choice.volumes.iter().find(|v| v.id == id)).is_none_or(complete);
    if complete(&choice.chosen) && current_complete {
        return Ok(());
    }
    Err(AgentError::bad_request("the volume lacks slices of its series and does not share the workspace's grid")
        .hint("open it in a workspace of its own"))
}

/// The warning of a volume taken because the caller named none.
fn default_note(study: &Study) -> String {
    let number = study.open_volume.as_ref().and_then(|v| v.volume.as_ref()).map_or(1, |i| i.index + 1);
    format!(
        "no volume was named: the series holds {} volumes and volume {number} is open; open another with series \
         set to its id (see volumes in study info or study scan); the workspace keeps segments and annotations",
        study.volumes.len()
    )
}

fn check_size(config: &AgentConfig, s: &SeriesDescriptor) -> Result<(), AgentError> {
    let n = s.dims.voxel_count() as u64;
    if n > config.max_voxels {
        return Err(AgentError::new(
            ErrorCode::Limit,
            format!("the series has {n} voxels; the limit is {}", config.max_voxels),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pseudonyms_are_stable_and_salted() {
        let a = pseudonym("s", "1.2.3");
        assert_eq!(a, pseudonym("s", "1.2.3"));
        assert_ne!(a, pseudonym("t", "1.2.3"));
        assert!(a.starts_with("anon-") && a.len() == 21);
        let open = AgentConfig { pseudonymise_uids: false, ..AgentConfig::default() };
        assert_eq!(series_key(&open, "1.2.3"), "1.2.3");
        assert_eq!(series_key(&AgentConfig::default(), "1.2.3"), pseudonym("ferrum", "1.2.3"));
        assert!(generator().starts_with("FERRUM "));
    }
}
