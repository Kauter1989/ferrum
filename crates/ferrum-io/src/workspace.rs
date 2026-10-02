//! Workspaces (`ferrum-workspace` v1): a directory holding the results of
//! working on one series, next to (never inside) the source data.
//!
//! ```text
//! ws/ct1/
//! ├── workspace.json    # format, version, source paths with size and SHA-256, series id
//! ├── annotations.json  # ferrum-annotations v2
//! ├── segments.nii.gz   # label map on the volume grid
//! ├── segments.json     # ferrum-segments v1: names, colours, provenance
//! ├── renders/          # images written by the agent interface
//! └── audit.jsonl       # one JSON object per line
//! ```
//!
//! Source data is referenced and hashed, never copied or modified;
//! [`Workspace::verify_sources`] fails with [`IoError::SourceChanged`] when
//! a file differs from its recorded hash. Files are replaced atomically
//! (written next to the target, then renamed). See `docs/agent-skill.md` §6.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ferrum_domain::{
    AnnotationReport, AnnotationSet, ResultStore, ReviewDecision, ReviewItem, ReviewStatus, SegmentationSet,
    SeriesDescriptor, Timestamp, Volume,
};
use rayon::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::annotations::{read_annotations, write_annotation_report};
use crate::error::IoError;
use crate::nifti::{read_label_nifti, write_label_nifti};
use crate::segments::{read_segments, write_segments};

/// Value of the `format` field of `workspace.json`.
pub const WORKSPACE_FORMAT: &str = "ferrum-workspace";
/// Version of the workspace layout.
pub const WORKSPACE_VERSION: u32 = 1;

/// File names inside a workspace.
pub mod files {
    /// Manifest.
    pub const MANIFEST: &str = "workspace.json";
    /// Annotations (`ferrum-annotations` v2).
    pub const ANNOTATIONS: &str = "annotations.json";
    /// Label map.
    pub const SEGMENTS_NIFTI: &str = "segments.nii.gz";
    /// Segment metadata (`ferrum-segments` v1).
    pub const SEGMENTS_JSON: &str = "segments.json";
    /// Directory of rendered images.
    pub const RENDERS: &str = "renders";
    /// Audit log (JSON lines).
    pub const AUDIT: &str = "audit.jsonl";
}

/// One source file with the size and hash recorded at creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Path relative to [`WorkspaceSource::path`] when the file lies below
    /// it, absolute otherwise.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256, lower-case hex.
    pub sha256: String,
}

/// The series a workspace works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSource {
    /// Folder (DICOM) or file (NIfTI) that was opened.
    pub path: PathBuf,
    /// Series id within the source (see [`SeriesDescriptor::id`]).
    pub series_id: String,
    /// Format of the series (`DICOM`, `NIfTI`).
    pub format: String,
    /// Files that make up the series.
    pub files: Vec<SourceFile>,
}

impl WorkspaceSource {
    /// Absolute paths of the source files.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|f| if f.path.is_absolute() { f.path.clone() } else { self.path.join(&f.path) }).collect()
    }
}

/// Contents of `workspace.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceManifest {
    /// Application that created the workspace, e.g. `FERRUM 0.2.0`.
    pub generator: String,
    /// Creation time.
    pub created: Timestamp,
    /// The series.
    pub source: WorkspaceSource,
}

impl WorkspaceManifest {
    fn to_json(&self) -> Value {
        let s = &self.source;
        json!({
            "format": WORKSPACE_FORMAT,
            "version": WORKSPACE_VERSION,
            "generator": self.generator,
            "created": self.created.to_rfc3339(),
            "source": {
                "path": s.path.to_string_lossy(),
                "series_id": s.series_id,
                "format": s.format,
                "files": s.files.iter().map(|f| json!({
                    "path": f.path.to_string_lossy(), "size": f.size, "sha256": f.sha256,
                })).collect::<Vec<_>>(),
            },
        })
    }

    fn from_json(v: &Value) -> Result<Self, String> {
        if v["format"] != WORKSPACE_FORMAT {
            return Err(format!("not a {WORKSPACE_FORMAT} manifest"));
        }
        if v["version"].as_u64() != Some(u64::from(WORKSPACE_VERSION)) {
            return Err(format!("unsupported {WORKSPACE_FORMAT} version {}", v["version"]));
        }
        let text = |v: &Value, key: &str| v[key].as_str().map(str::to_owned).ok_or(format!("{key} is missing"));
        let s = &v["source"];
        let files = s["files"]
            .as_array()
            .ok_or("source.files is missing")?
            .iter()
            .map(|f| {
                let sha256 = text(f, "sha256")?;
                if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(format!("bad sha256 {sha256:?}"));
                }
                Ok(SourceFile {
                    path: text(f, "path")?.into(),
                    size: f["size"].as_u64().ok_or("size is missing")?,
                    sha256,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            generator: text(v, "generator").unwrap_or_default(),
            created: Timestamp::parse_rfc3339(&text(v, "created")?).ok_or("created: expected an RFC 3339 time")?,
            source: WorkspaceSource {
                path: text(s, "path")?.into(),
                series_id: text(s, "series_id")?,
                format: text(s, "format").unwrap_or_default(),
                files,
            },
        })
    }
}

/// An open workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
    manifest: WorkspaceManifest,
}

/// SHA-256 of a file (lower-case hex) and its size.
pub fn sha256_file(path: &Path) -> Result<(String, u64), IoError> {
    let mut f = std::fs::File::open(path).map_err(|e| IoError::os(path, e))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| IoError::os(path, e))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    let hex = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    Ok((hex, size))
}

/// Writes `bytes` to `path` atomically.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), IoError> {
    let tmp = partial_path(path);
    std::fs::write(&tmp, bytes).map_err(|e| IoError::os(&tmp, e))?;
    std::fs::rename(&tmp, path).map_err(|e| IoError::os(path, e))
}

/// `dir/name.ext` → `dir/.partial-name.ext` (keeps the extension, so
/// writers that look at it behave the same).
fn partial_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".partial-{name}"))
}

/// Removes `path` if it exists.
fn remove_if_exists(path: &Path) -> Result<(), IoError> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(IoError::os(path, e)),
        _ => Ok(()),
    }
}

impl Workspace {
    /// Creates a workspace in `root` (created if needed) for `series`,
    /// opened from `source`. Hashes every file of the series. Fails if
    /// `root` already holds a workspace or lies inside the source folder.
    pub fn create(root: &Path, source: &Path, series: &SeriesDescriptor, generator: &str) -> Result<Self, IoError> {
        let manifest_path = root.join(files::MANIFEST);
        if manifest_path.exists() {
            return Err(IoError::invalid(root, "already a workspace; open it instead"));
        }
        let source_abs = std::path::absolute(source).map_err(|e| IoError::os(source, e))?;
        let root_abs = std::path::absolute(root).map_err(|e| IoError::os(root, e))?;
        if source_abs.is_dir() && root_abs.starts_with(&source_abs) {
            return Err(IoError::invalid(root, "a workspace must not lie inside the source data"));
        }
        let files = series
            .sources
            .par_iter()
            .map(|f| {
                let abs = std::path::absolute(f).map_err(|e| IoError::os(f, e))?;
                let (sha256, size) = sha256_file(&abs)?;
                let path = abs.strip_prefix(&source_abs).ok().filter(|p| !p.as_os_str().is_empty());
                Ok(SourceFile { path: path.map_or(abs.clone(), Path::to_path_buf), size, sha256 })
            })
            .collect::<Result<Vec<_>, IoError>>()?;
        std::fs::create_dir_all(root_abs.join(files::RENDERS)).map_err(|e| IoError::os(root, e))?;
        let manifest = WorkspaceManifest {
            generator: generator.to_owned(),
            created: Timestamp::now(),
            source: WorkspaceSource {
                path: source_abs,
                series_id: series.id.clone(),
                format: series.format.clone(),
                files,
            },
        };
        let text =
            serde_json::to_string_pretty(&manifest.to_json()).map_err(|e| IoError::invalid(root, e.to_string()))?;
        write_atomic(&manifest_path, text.as_bytes())?;
        Ok(Self { root: root_abs, manifest })
    }

    /// Opens the workspace in `root`.
    pub fn open(root: &Path) -> Result<Self, IoError> {
        let path = root.join(files::MANIFEST);
        let text = std::fs::read_to_string(&path).map_err(|e| IoError::os(&path, e))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| IoError::parse(&path, e))?;
        let manifest = WorkspaceManifest::from_json(&v).map_err(|m| IoError::invalid(&path, m))?;
        let root = std::path::absolute(root).map_err(|e| IoError::os(root, e))?;
        Ok(Self { root, manifest })
    }

    /// Workspace directory (absolute).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The manifest.
    pub fn manifest(&self) -> &WorkspaceManifest {
        &self.manifest
    }

    /// Path of `name` inside the workspace (see [`files`]).
    pub fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Checks that every source file still exists with the recorded size
    /// and SHA-256.
    pub fn verify_sources(&self) -> Result<(), IoError> {
        let s = &self.manifest.source;
        s.files.par_iter().zip(s.file_paths()).try_for_each(|(f, path)| {
            let changed = |reason: String| IoError::SourceChanged { path: path.clone(), reason };
            let meta = std::fs::metadata(&path).map_err(|e| changed(e.to_string()))?;
            if meta.len() != f.size {
                return Err(changed(format!("size {} instead of {}", meta.len(), f.size)));
            }
            let (sha, _) = sha256_file(&path)?;
            if sha != f.sha256 {
                return Err(changed("content differs from the recorded SHA-256".into()));
            }
            Ok(())
        })
    }

    /// Saves the annotations (an empty report removes the file).
    pub fn save_annotations(&self, report: &AnnotationReport, generator: &str) -> Result<(), IoError> {
        let path = self.path(files::ANNOTATIONS);
        if report.annotations.is_empty() {
            return remove_if_exists(&path);
        }
        let tmp = partial_path(&path);
        write_annotation_report(report, generator, &tmp)?;
        std::fs::rename(&tmp, &path).map_err(|e| IoError::os(&path, e))
    }

    /// Loads the annotations of a volume of `dims`, or `None` if there are
    /// none.
    pub fn load_annotations(&self, dims: [u32; 3]) -> Result<Option<AnnotationSet>, IoError> {
        let path = self.path(files::ANNOTATIONS);
        if !path.exists() {
            return Ok(None);
        }
        read_annotations(&path, Some(dims)).map(Some)
    }

    /// Saves the label map and segment metadata of `set` (on the grid of
    /// `volume`). A set without segments removes both files.
    pub fn save_segments(&self, set: &SegmentationSet, volume: &Volume, generator: &str) -> Result<(), IoError> {
        let (nifti, meta) = (self.path(files::SEGMENTS_NIFTI), self.path(files::SEGMENTS_JSON));
        if set.segments().is_empty() {
            remove_if_exists(&nifti)?;
            return remove_if_exists(&meta);
        }
        let (tmp_nifti, tmp_meta) = (partial_path(&nifti), partial_path(&meta));
        write_label_nifti(set.labels(), volume, &tmp_nifti)?;
        write_segments(set, volume.spacing(), generator, &tmp_meta)?;
        std::fs::rename(&tmp_nifti, &nifti).map_err(|e| IoError::os(&nifti, e))?;
        std::fs::rename(&tmp_meta, &meta).map_err(|e| IoError::os(&meta, e))
    }

    /// Loads the segments on the grid of `volume`, or `None` if there are
    /// none. Labels without metadata get default segments.
    pub fn load_segments(&self, volume: &Volume) -> Result<Option<SegmentationSet>, IoError> {
        let (nifti, meta) = (self.path(files::SEGMENTS_NIFTI), self.path(files::SEGMENTS_JSON));
        if !nifti.exists() {
            return Ok(None);
        }
        let labels = read_label_nifti(&nifti, volume)?;
        let segments = if meta.exists() { read_segments(&meta)? } else { Vec::new() };
        Ok(Some(SegmentationSet::from_labels(labels, segments)))
    }

    /// Appends one entry to the audit log; a `time` field is added if
    /// missing. `entry` must be a JSON object.
    pub fn append_audit(&self, entry: Value) -> Result<(), IoError> {
        let path = self.path(files::AUDIT);
        let Value::Object(mut map) = entry else {
            return Err(IoError::invalid(&path, "audit entries must be JSON objects"));
        };
        if !map.contains_key("time") {
            let mut ordered = serde_json::Map::new();
            ordered.insert("time".into(), Value::String(Timestamp::now().to_rfc3339()));
            ordered.extend(map);
            map = ordered;
        }
        let mut line =
            serde_json::to_string(&Value::Object(map)).map_err(|e| IoError::invalid(&path, e.to_string()))?;
        line.push('\n');
        let mut f =
            std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| IoError::os(&path, e))?;
        f.write_all(line.as_bytes()).map_err(|e| IoError::os(&path, e))
    }

    /// All audit entries, oldest first.
    pub fn read_audit(&self) -> Result<Vec<Value>, IoError> {
        let path = self.path(files::AUDIT);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(IoError::os(&path, e)),
        };
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).map_err(|e| IoError::parse(&path, e)))
            .collect()
    }
}

/// A workspace as the desktop app's [`ResultStore`]: review decisions are
/// written to the workspace and its audit log.
#[derive(Debug, Clone)]
pub struct WorkspaceStore {
    workspace: Workspace,
    generator: String,
}

impl WorkspaceStore {
    /// Opens the workspace in `root`; `generator` names the application in
    /// the files it writes.
    pub fn open(root: &Path, generator: &str) -> Result<Self, IoError> {
        Ok(Self { workspace: Workspace::open(root)?, generator: generator.to_owned() })
    }

    /// The workspace.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }
}

impl ResultStore for WorkspaceStore {
    fn describe(&self) -> String {
        self.workspace.root().display().to_string()
    }

    fn source_paths(&self) -> Vec<PathBuf> {
        vec![self.workspace.manifest().source.path.clone()]
    }

    fn series_id(&self) -> String {
        self.workspace.manifest().source.series_id.clone()
    }

    fn verify(&self) -> Result<(), String> {
        self.workspace.verify_sources().map_err(|e| e.to_string())
    }

    fn load(&self, volume: &Volume) -> Result<(Option<AnnotationSet>, Option<SegmentationSet>), String> {
        let d = volume.dims();
        let annotations = self.workspace.load_annotations([d.x, d.y, d.z]).map_err(|e| e.to_string())?;
        let segments = self.workspace.load_segments(volume).map_err(|e| e.to_string())?;
        Ok((annotations, segments))
    }

    fn save(&self, annotations: &AnnotationReport, segments: &SegmentationSet, volume: &Volume) -> Result<(), String> {
        self.workspace.save_annotations(annotations, &self.generator).map_err(|e| e.to_string())?;
        self.workspace.save_segments(segments, volume, &self.generator).map_err(|e| e.to_string())
    }

    fn log(&self, d: &ReviewDecision) -> Result<(), String> {
        let (kind, id) = match d.item {
            ReviewItem::Annotation(id) => ("annotation", id),
            ReviewItem::Segment(label) => ("segment", u64::from(label)),
        };
        let command = match d.status {
            ReviewStatus::Confirmed => "review confirm",
            ReviewStatus::Rejected => "review reject",
            ReviewStatus::Proposed => "review reopen",
        };
        let entry = json!({
            "command": command,
            "source": "desktop",
            "params": { kind: id, "name": d.name, "by": d.by },
            "ok": true,
        });
        self.workspace.append_audit(entry).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Annotation, Dims3, LabelMap, Provenance, ReviewStatus, SliceAxis, SliceKey, StudyInfo};
    use glam::{Vec2, Vec3};

    struct Fixture {
        _dir: tempfile::TempDir,
        source: PathBuf,
        root: PathBuf,
        series: SeriesDescriptor,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("series");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        let a = source.join("a.dcm");
        let b = source.join("sub/b.dcm");
        std::fs::write(&a, b"abc").unwrap();
        std::fs::write(&b, vec![7u8; 100_000]).unwrap();
        let series = SeriesDescriptor {
            id: "1.2.3".into(),
            format: "DICOM".into(),
            description: String::new(),
            modality: "CT".into(),
            dims: Dims3::new(2, 2, 2),
            sources: vec![a, b],
        };
        let root = dir.path().join("ws/ct1");
        Fixture { _dir: dir, source, root, series }
    }

    fn volume() -> Volume {
        let dims = Dims3::new(2, 2, 2);
        Volume::from_physical(dims, Vec3::ONE, &vec![0.0; dims.voxel_count()]).unwrap()
    }

    #[test]
    fn create_open_and_verify() {
        let f = fixture();
        let ws = Workspace::create(&f.root, &f.source, &f.series, "FERRUM test").unwrap();
        let files = &ws.manifest().source.files;
        assert_eq!(files[0].path, PathBuf::from("a.dcm"));
        assert_eq!(files[1].path, PathBuf::from("sub/b.dcm"));
        assert_eq!(files[0].sha256, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(files[1].size, 100_000);
        assert!(ws.path(files::RENDERS).is_dir());
        let opened = Workspace::open(&f.root).unwrap();
        assert_eq!(opened, ws);
        assert_eq!(opened.manifest().source.file_paths()[1], f.source.join("sub/b.dcm"));
        opened.verify_sources().unwrap();
        assert!(matches!(Workspace::create(&f.root, &f.source, &f.series, "x"), Err(IoError::Invalid { .. })));
        assert!(matches!(
            Workspace::create(&f.source.join("ws"), &f.source, &f.series, "x"),
            Err(IoError::Invalid { .. })
        ));

        std::fs::write(&f.series.sources[0], b"abd").unwrap();
        let err = opened.verify_sources().unwrap_err();
        assert!(matches!(&err, IoError::SourceChanged { reason, .. } if reason.contains("SHA-256")), "{err}");
        std::fs::write(&f.series.sources[0], b"abcd").unwrap();
        assert!(
            matches!(opened.verify_sources(), Err(IoError::SourceChanged { reason, .. }) if reason.contains("size"))
        );
        std::fs::remove_file(&f.series.sources[0]).unwrap();
        assert!(matches!(opened.verify_sources(), Err(IoError::SourceChanged { .. })));
    }

    #[test]
    fn files_outside_the_source_keep_absolute_paths() {
        let f = fixture();
        let elsewhere = f.root.parent().unwrap().join("other.nii");
        std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
        std::fs::write(&elsewhere, b"x").unwrap();
        let series = SeriesDescriptor { sources: vec![elsewhere.clone()], ..f.series.clone() };
        let ws = Workspace::create(&f.root, &elsewhere, &series, "t").unwrap();
        assert_eq!(ws.manifest().source.files[0].path, std::path::absolute(&elsewhere).unwrap());
        assert_eq!(ws.manifest().source.file_paths(), vec![std::path::absolute(&elsewhere).unwrap()]);
        ws.verify_sources().unwrap();
    }

    #[test]
    fn annotations_segments_and_audit() {
        let f = fixture();
        let ws = Workspace::create(&f.root, &f.source, &f.series, "FERRUM test").unwrap();
        let v = volume();
        assert_eq!(ws.load_annotations([2, 2, 2]).unwrap(), None);
        assert_eq!(ws.load_segments(&v).unwrap(), None);

        let mut set = AnnotationSet::default();
        let id = set.add_with(
            SliceKey::new(SliceAxis::Axial, 1),
            Annotation::Distance { a: Vec2::ZERO, b: Vec2::ONE },
            Provenance::agent(Some("run-1".into()), Timestamp(10)),
        );
        let report = |set: &AnnotationSet| AnnotationReport::build(f.source.clone(), StudyInfo::default(), &v, set);
        ws.save_annotations(&report(&set), "t").unwrap();
        assert_eq!(ws.load_annotations([2, 2, 2]).unwrap(), Some(set.clone()));
        assert!(ws.load_annotations([3, 2, 2]).is_err());
        set.review(id, ReviewStatus::Confirmed, Some("dr.k"), Timestamp(20));
        ws.save_annotations(&report(&set), "t").unwrap();
        assert_eq!(ws.load_annotations([2, 2, 2]).unwrap().unwrap().pending(), 0);
        ws.save_annotations(&report(&AnnotationSet::default()), "t").unwrap();
        assert_eq!(ws.load_annotations([2, 2, 2]).unwrap(), None);

        let mut seg = SegmentationSet::from_labels(
            LabelMap::from_data(Dims3::new(2, 2, 2), vec![0, 3, 3, 0, 0, 0, 0, 1]).unwrap(),
            vec![],
        );
        seg.rename(3, "Liver");
        seg.set_provenance(3, Provenance::engine("TotalSegmentator", "2.18", false, Timestamp(5))).unwrap();
        ws.save_segments(&seg, &v, "t").unwrap();
        let back = ws.load_segments(&v).unwrap().unwrap();
        assert_eq!((back.labels(), back.segments()), (seg.labels(), seg.segments()));
        std::fs::remove_file(ws.path(files::SEGMENTS_JSON)).unwrap();
        assert_eq!(ws.load_segments(&v).unwrap().unwrap().segment(3).unwrap().name, "Segment 3");
        ws.save_segments(&SegmentationSet::new(Dims3::new(2, 2, 2)), &v, "t").unwrap();
        assert_eq!(ws.load_segments(&v).unwrap(), None);
        let leftovers: Vec<_> = std::fs::read_dir(ws.root())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".partial"))
            .collect();
        assert!(leftovers.is_empty());

        assert_eq!(ws.read_audit().unwrap(), Vec::<Value>::new());
        ws.append_audit(json!({ "command": "study open", "ok": true })).unwrap();
        ws.append_audit(json!({ "time": "2026-10-01T00:00:00Z", "command": "probe" })).unwrap();
        assert!(ws.append_audit(json!([1])).is_err());
        let log = ws.read_audit().unwrap();
        assert_eq!(log.len(), 2);
        assert!(Timestamp::parse_rfc3339(log[0]["time"].as_str().unwrap()).is_some());
        assert_eq!(log[0].as_object().unwrap().keys().next().map(String::as_str), Some("time"));
        assert_eq!(log[1]["time"], "2026-10-01T00:00:00Z");
        std::fs::write(ws.path(files::AUDIT), "{\n").unwrap();
        assert!(ws.read_audit().is_err());
    }

    #[test]
    fn workspace_store_reads_writes_and_logs() {
        let f = fixture();
        Workspace::create(&f.root, &f.source, &f.series, "t").unwrap();
        let store = WorkspaceStore::open(&f.root, "FERRUM test").unwrap();
        assert_eq!(store.describe(), store.workspace().root().display().to_string());
        assert_eq!(
            (store.series_id(), store.source_paths()),
            ("1.2.3".to_owned(), vec![std::path::absolute(&f.source).unwrap()])
        );
        store.verify().unwrap();
        let v = volume();
        assert_eq!(store.load(&v).unwrap(), (None, None));
        let mut set = AnnotationSet::default();
        set.add_with(
            SliceKey::new(SliceAxis::Axial, 0),
            Annotation::Text { pos: Vec2::ZERO, text: "x".into() },
            Provenance::agent(None, Timestamp(1)),
        );
        let mut seg = SegmentationSet::new(Dims3::new(2, 2, 2));
        seg.add_segment("S").unwrap();
        let report = AnnotationReport::build(f.source.clone(), StudyInfo::default(), &v, &set);
        store.save(&report, &seg, &v).unwrap();
        let (a, s) = store.load(&v).unwrap();
        assert_eq!((a.unwrap().pending(), s.unwrap().segments().len()), (1, 1));
        let d = ReviewDecision {
            item: ReviewItem::Segment(1),
            name: "S".into(),
            status: ReviewStatus::Rejected,
            by: Some("dr.k".into()),
        };
        store.log(&d).unwrap();
        store
            .log(&ReviewDecision { item: ReviewItem::Annotation(0), status: ReviewStatus::Proposed, by: None, ..d })
            .unwrap();
        let log = store.workspace().read_audit().unwrap();
        assert_eq!((log[0]["command"].as_str(), log[0]["source"].as_str()), (Some("review reject"), Some("desktop")));
        assert_eq!((log[0]["params"]["segment"].as_u64(), log[0]["params"]["by"].as_str()), (Some(1), Some("dr.k")));
        assert_eq!(log[1]["command"], "review reopen");
        std::fs::write(&f.series.sources[0], b"changed").unwrap();
        assert!(store.verify().unwrap_err().contains("source changed"));
        assert!(WorkspaceStore::open(&f.source, "t").is_err());
    }

    #[test]
    fn bad_manifests() {
        let f = fixture();
        let ws = Workspace::create(&f.root, &f.source, &f.series, "t").unwrap();
        let path = ws.path(files::MANIFEST);
        let good: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(matches!(Workspace::open(&f.source), Err(IoError::Os { .. })));
        std::fs::write(&path, "nope").unwrap();
        assert!(matches!(Workspace::open(&f.root), Err(IoError::Parse { .. })));
        let edits: [(&str, Value); 5] = [
            ("/format", json!("x")),
            ("/version", json!(2)),
            ("/created", json!("today")),
            ("/source/files/0/sha256", json!("abc")),
            ("/source/series_id", Value::Null),
        ];
        for (pointer, value) in edits {
            let mut doc = good.clone();
            *doc.pointer_mut(pointer).unwrap() = value;
            std::fs::write(&path, doc.to_string()).unwrap();
            assert!(matches!(Workspace::open(&f.root), Err(IoError::Invalid { .. })), "{pointer}");
        }
    }
}
