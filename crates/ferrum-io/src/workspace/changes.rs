//! Change notifications of a workspace (16.8, issue #72).
//!
//! The agent, the viewer and a host's review panel may work on one
//! workspace at the same time. A change is computed by comparing the
//! workspace's files with the state read before, so it is the same
//! whichever process wrote them:
//!
//! - **In process**, [`subscribe`] returns a [`Subscription`]. Every save
//!   through [`Workspace`](super::Workspace) sends its changes to the
//!   subscribers of that workspace before the save returns. Changes another
//!   process made meanwhile arrive with the next save in this process.
//! - **Across processes**, a [`WorkspaceWatcher`] is polled by its owner. It
//!   reads a file again only when its size, modification time or (on Unix)
//!   inode changed. A watcher also reports this process's own saves, so a
//!   host that uses both receives those twice.
//!
//! A change holds labels, ids, review statuses and a volume key of the form
//! FERRUM generates (`b1000`, `te80-e2`, `n3`); anything else in the
//! manifest becomes `other`. It never holds names, texts, UIDs or paths,
//! which may identify a patient or a person.
//!
//! Only regular files are read, each with a size limit; a file that cannot
//! be read is reported once as [`WorkspaceChange::FileUnreadable`] and the
//! other files are still compared. Channels are unbounded: a host drains its
//! subscription, or drops it to unsubscribe.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender, TryIter};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

use ferrum_domain::ReviewStatus;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::files;

/// What changed in a workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceChange {
    /// A segment was added.
    SegmentAdded {
        /// Its label.
        label: u8,
    },
    /// A segment was removed.
    SegmentRemoved {
        /// Its label.
        label: u8,
    },
    /// A segment's name, colour, visibility, opacity or voxels changed.
    SegmentRevised {
        /// Its label.
        label: u8,
    },
    /// A segment's review status changed.
    SegmentReviewed {
        /// Its label.
        label: u8,
        /// The new status.
        status: ReviewStatus,
    },
    /// An annotation was added.
    AnnotationAdded {
        /// Its id.
        id: u64,
    },
    /// An annotation was removed.
    AnnotationRemoved {
        /// Its id.
        id: u64,
    },
    /// An annotation's kind, plane, slice, points, text or name changed.
    AnnotationRevised {
        /// Its id.
        id: u64,
    },
    /// An annotation's review status changed.
    AnnotationReviewed {
        /// Its id.
        id: u64,
        /// The new status.
        status: ReviewStatus,
    },
    /// Another volume of a multi-volume series was opened.
    VolumeSwitched {
        /// The volume's key (`b1000`, `te80-e2`, `n3`; the part after `#v=`),
        /// `other` for a key FERRUM does not generate, or `None` for a
        /// single-volume series.
        key: Option<String>,
    },
    /// A workspace file could not be read (not a regular file, too large,
    /// malformed or half written); its items keep their last known state
    /// until it can be read again.
    FileUnreadable {
        /// The file's name in the workspace (`annotations.json`, …).
        file: &'static str,
    },
}

/// The file a save wrote, so that only it is read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Touched {
    Annotations,
    Segments,
    Manifest,
}

/// Largest JSON file read (annotations, segment metadata, manifest).
const MAX_JSON_BYTES: u64 = 64 << 20;
/// Largest label map read, compressed and decompressed.
const MAX_LABEL_MAP_BYTES: u64 = 2 << 30;

/// One item: its review status and a hash of the fields a revision changes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemState {
    status: Option<ReviewStatus>,
    content: [u8; 32],
}

/// Size, modification time and inode: unchanged, the file is not read again.
type Stamp = Option<(u64, SystemTime, u64)>;

/// The state of one file: what it held when last read, and whether it
/// could be read then.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Part<T> {
    value: T,
    stamp: Stamp,
    unreadable: bool,
}

/// The state of a workspace as far as changes are reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceState {
    annotations: Part<BTreeMap<u64, ItemState>>,
    segments: Part<Option<BTreeMap<u8, ItemState>>>,
    /// Per label, a hash of the voxels it holds in the label map.
    masks: Part<BTreeMap<u8, [u8; 32]>>,
    volume: Part<Option<String>>,
    /// The last poll held back a label map that changed without its
    /// metadata (a save may have been between its two renames).
    deferred: bool,
}

fn stamp(path: &Path) -> Stamp {
    let md = std::fs::symlink_metadata(path).ok()?;
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&md);
    #[cfg(not(unix))]
    let ino = 0;
    Some((md.len(), md.modified().ok()?, ino))
}

/// The bytes of a regular file of at most `max` bytes (decompressed when
/// `gz`), or `None` when it does not exist. Symbolic links, FIFOs and
/// devices are refused, so a planted file cannot block or flood the reader.
fn read_regular(path: &Path, max: u64, gz: bool) -> Result<Option<Vec<u8>>, String> {
    let md = match std::fs::symlink_metadata(path) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    if !md.file_type().is_file() {
        return Err("not a regular file".into());
    }
    if md.len() > max {
        return Err(format!("larger than {max} bytes"));
    }
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    let read = if gz {
        flate2::read::GzDecoder::new(std::io::BufReader::new(file)).take(max + 1).read_to_end(&mut bytes)
    } else {
        file.take(max + 1).read_to_end(&mut bytes)
    };
    read.map_err(|e| e.to_string())?;
    if bytes.len() as u64 > max {
        return Err(format!("larger than {max} bytes"));
    }
    Ok(Some(bytes))
}

fn read_json(path: &Path) -> Result<Option<Value>, String> {
    match read_regular(path, MAX_JSON_BYTES, false)? {
        Some(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

fn status_of(item: &Value) -> Option<ReviewStatus> {
    match item["provenance"]["status"].as_str()? {
        "proposed" => Some(ReviewStatus::Proposed),
        "confirmed" => Some(ReviewStatus::Confirmed),
        "rejected" => Some(ReviewStatus::Rejected),
        _ => None,
    }
}

/// A hash of the named fields, in that order: independent of the key order
/// and of fields another writer adds.
fn content_of(item: &Value, fields: &[&str]) -> [u8; 32] {
    let mut h = Sha256::new();
    for f in fields {
        let text = item.get(*f).map(Value::to_string).unwrap_or_default();
        h.update((text.len() as u64).to_le_bytes());
        h.update(text.as_bytes());
    }
    h.finalize().into()
}

/// The fields of an annotation that a revision changes (the millimetres,
/// the value and the unit follow from them).
const ANNOTATION_FIELDS: [&str; 6] = ["type", "plane", "slice_index", "points_voxel", "text", "name"];
/// The fields of a segment's metadata that a revision changes.
const SEGMENT_FIELDS: [&str; 4] = ["name", "color", "visible", "opacity"];

fn items<K: Ord>(
    doc: &Value,
    list: &str,
    key: impl Fn(&Value) -> Option<K>,
    fields: &[&str],
) -> BTreeMap<K, ItemState> {
    let entries = doc[list].as_array().map(Vec::as_slice).unwrap_or_default();
    entries
        .iter()
        .filter_map(|e| Some((key(e)?, ItemState { status: status_of(e), content: content_of(e, fields) })))
        .collect()
}

/// Per label, a hash of the indices of its voxels. Only `uint8` label maps
/// (what FERRUM writes) are decoded.
fn masks(path: &Path) -> Result<BTreeMap<u8, [u8; 32]>, String> {
    let Some(bytes) = read_regular(path, MAX_LABEL_MAP_BYTES, true)? else {
        return Ok(BTreeMap::new());
    };
    let h = crate::nifti::NiftiHeader::parse(&bytes, path).map_err(|e| e.to_string())?;
    if h.datatype != 2 {
        return Err(format!("datatype {} is not a uint8 label map", h.datatype));
    }
    let end = h.vox_offset.checked_add(h.dims.voxel_count()).filter(|e| *e <= bytes.len());
    let voxels = end.map(|e| &bytes[h.vox_offset..e]).ok_or("voxel data truncated")?;
    let mut hashers: BTreeMap<u8, Sha256> = BTreeMap::new();
    for (i, label) in voxels.iter().enumerate().filter(|(_, l)| **l != 0) {
        hashers.entry(*label).or_default().update((i as u64).to_le_bytes());
    }
    Ok(hashers.into_iter().map(|(l, h)| (l, h.finalize().into())).collect())
}

/// One part of a volume key: a prefix of 1 to 3 letters and a number with
/// at most one decimal point (or `x`, unknown).
fn key_part(p: &str) -> bool {
    let prefix_ok = |s: &str| (1..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase());
    if let Some(prefix) = p.strip_suffix('x').filter(|s| prefix_ok(s)) {
        return !prefix.is_empty();
    }
    let digits = p.trim_start_matches(|c: char| c.is_ascii_lowercase());
    let number = matches!(digits.split_once('.'), Some((a, b)) if is_digits(a, 12) && is_digits(b, 6))
        || is_digits(digits, 12);
    prefix_ok(&p[..p.len() - digits.len()]) && number
}

fn is_digits(s: &str, max: usize) -> bool {
    (1..=max).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

/// The key of the open volume, if it has the form FERRUM generates;
/// `other` otherwise, so that no UID or name a crafted manifest holds is
/// passed on.
fn volume_key(manifest: &Value) -> Option<String> {
    let id = manifest["source"]["volume"].as_str()?;
    let key = id.rsplit_once(crate::dicom::VOLUME_SEPARATOR).map_or("", |(_, k)| k);
    let ok = key.len() <= 64 && !key.is_empty() && key.split('-').all(key_part);
    Some(if ok { key.to_owned() } else { "other".to_owned() })
}

/// Reads one file into `part` when `force` is set or its stamp changed;
/// on failure the last value stays and the failure is reported once.
fn refresh<T>(part: &mut Part<T>, path: &Path, force: bool, read: impl FnOnce(&Path) -> Result<T, String>) -> bool {
    let now = stamp(path);
    if !force && now == part.stamp && now.is_some() && !part.unreadable {
        return false;
    }
    part.stamp = now;
    match read(path) {
        Ok(v) => {
            part.value = v;
            part.unreadable = false;
            true
        }
        Err(e) => {
            if !part.unreadable {
                // the file's name only: a workspace path may name the patient
                log::warn!("workspace changes: {} unreadable: {e}", path.file_name().unwrap_or_default().display());
            }
            part.unreadable = true;
            false
        }
    }
}

impl WorkspaceState {
    /// Reads the state of the workspace in `root`.
    pub fn read(root: &Path) -> Self {
        let mut s = Self::default();
        s.refresh(root, None, true);
        s
    }

    /// Reads again the files whose stamp changed, the file `touched` (a save
    /// just wrote it) and, with `all`, every file. Returns whether the
    /// label map was read while its metadata was not.
    fn refresh(&mut self, root: &Path, touched: Option<Touched>, all: bool) -> bool {
        let annotation_id = |e: &Value| e["id"].as_u64();
        let segment_label = |e: &Value| e["label"].as_u64().and_then(|l| u8::try_from(l).ok());
        refresh(
            &mut self.annotations,
            &root.join(files::ANNOTATIONS),
            all || touched == Some(Touched::Annotations),
            |p| {
                Ok(read_json(p)?
                    .map(|d| items(&d, "annotations", annotation_id, &ANNOTATION_FIELDS))
                    .unwrap_or_default())
            },
        );
        let segments = touched == Some(Touched::Segments) || all;
        let meta = refresh(&mut self.segments, &root.join(files::SEGMENTS_JSON), segments, |p| {
            Ok(read_json(p)?.map(|d| items(&d, "segments", segment_label, &SEGMENT_FIELDS)))
        });
        let map = refresh(&mut self.masks, &root.join(files::SEGMENTS_NIFTI), segments, masks);
        refresh(&mut self.volume, &root.join(files::MANIFEST), all || touched == Some(Touched::Manifest), |p| {
            Ok(read_json(p)?.as_ref().and_then(volume_key))
        });
        map && !meta
    }

    /// The segments as compared: metadata and voxels together. A label in
    /// the map without metadata counts only when there is no metadata file
    /// (a label map imported alone).
    fn segment_items(&self) -> BTreeMap<u8, ItemState> {
        let meta = self.segments.value.as_ref();
        let masks = &self.masks.value;
        let labels: BTreeSet<u8> = match meta {
            Some(m) => m.keys().copied().collect(),
            None => masks.keys().copied().collect(),
        };
        labels
            .into_iter()
            .map(|l| {
                let item = meta.and_then(|m| m.get(&l)).cloned();
                let mut h = Sha256::new();
                h.update(item.as_ref().map_or([0; 32], |i| i.content));
                h.update(masks.get(&l).copied().unwrap_or([0; 32]));
                (l, ItemState { status: item.and_then(|i| i.status), content: h.finalize().into() })
            })
            .collect()
    }

    /// The changes from `self` to `next`.
    pub fn changes_to(&self, next: &Self) -> Vec<WorkspaceChange> {
        use WorkspaceChange as C;
        let mut out = Vec::new();
        diff_items(&self.annotations.value, &next.annotations.value, &mut out, |id, kind| match kind {
            Kind::Added => C::AnnotationAdded { id },
            Kind::Removed => C::AnnotationRemoved { id },
            Kind::Revised => C::AnnotationRevised { id },
            Kind::Reviewed(status) => C::AnnotationReviewed { id, status },
        });
        diff_items(&self.segment_items(), &next.segment_items(), &mut out, |label, kind| match kind {
            Kind::Added => C::SegmentAdded { label },
            Kind::Removed => C::SegmentRemoved { label },
            Kind::Revised => C::SegmentRevised { label },
            Kind::Reviewed(status) => C::SegmentReviewed { label, status },
        });
        if self.volume.value != next.volume.value {
            out.push(C::VolumeSwitched { key: next.volume.value.clone() });
        }
        let unreadable = |s: &Self| {
            [
                (s.annotations.unreadable, files::ANNOTATIONS),
                (s.segments.unreadable, files::SEGMENTS_JSON),
                (s.masks.unreadable, files::SEGMENTS_NIFTI),
                (s.volume.unreadable, files::MANIFEST),
            ]
        };
        for ((was, file), (now, _)) in unreadable(self).into_iter().zip(unreadable(next)) {
            if now && !was {
                out.push(C::FileUnreadable { file });
            }
        }
        out
    }
}

enum Kind {
    Added,
    Removed,
    Revised,
    Reviewed(ReviewStatus),
}

fn diff_items<K: Ord + Copy>(
    before: &BTreeMap<K, ItemState>,
    after: &BTreeMap<K, ItemState>,
    out: &mut Vec<WorkspaceChange>,
    make: impl Fn(K, Kind) -> WorkspaceChange,
) {
    for (k, b) in before {
        match after.get(k) {
            None => out.push(make(*k, Kind::Removed)),
            Some(a) => {
                if a.content != b.content {
                    out.push(make(*k, Kind::Revised));
                }
                if let (Some(status), true) = (a.status, a.status != b.status) {
                    out.push(make(*k, Kind::Reviewed(status)));
                }
            }
        }
    }
    for k in after.keys().filter(|k| !before.contains_key(k)) {
        out.push(make(*k, Kind::Added));
    }
}

/// Polls a workspace for changes made by any process.
#[derive(Debug)]
pub struct WorkspaceWatcher {
    root: PathBuf,
    last: WorkspaceState,
}

impl WorkspaceWatcher {
    /// Starts watching the workspace in `root` from its current state.
    pub fn new(root: &Path) -> Self {
        let root = normalized(root);
        let last = WorkspaceState::read(&root);
        Self { root, last }
    }

    /// The changes since the last call (or since [`WorkspaceWatcher::new`]).
    pub fn poll(&mut self) -> Vec<WorkspaceChange> {
        let mut next = self.last.clone();
        let map_alone = next.refresh(&self.root, None, false);
        // the label map changed without its metadata: a save may be between its two
        // renames, so the segments are compared one poll later (once: a program that
        // writes only the label map is reported then)
        if map_alone && !self.last.deferred && next.segments.value.is_some() {
            next.segments = self.last.segments.clone();
            next.masks = self.last.masks.clone();
            next.deferred = true;
        } else {
            next.deferred = false;
        }
        let changes = self.last.changes_to(&next);
        self.last = next;
        changes
    }
}

/// The subscribers of one workspace and the state they last heard of.
#[derive(Default)]
struct Hub {
    last: Option<WorkspaceState>,
    subscribers: Vec<(u64, Sender<WorkspaceChange>)>,
}

type Hubs = HashMap<PathBuf, Arc<Mutex<Hub>>>;

fn hubs() -> MutexGuard<'static, Hubs> {
    static HUBS: OnceLock<Mutex<Hubs>> = OnceLock::new();
    // a panic elsewhere must not silence notifications for the rest of the process
    HUBS.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(PoisonError::into_inner)
}

fn lock(hub: &Mutex<Hub>) -> MutexGuard<'_, Hub> {
    hub.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `path` absolute and with symbolic links resolved as far as it exists, so
/// a subscription made before the workspace is created matches its saves.
fn normalized(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut existing = abs.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(existing) {
            return rest.iter().rev().fold(c, |p, part: &Component| p.join(part));
        }
        match (existing.parent(), existing.components().next_back()) {
            (Some(parent), Some(last)) => {
                rest.push(last);
                existing = parent;
            }
            _ => return abs,
        }
    }
}

/// A subscription to the changes of one workspace; dropping it unsubscribes.
#[derive(Debug)]
pub struct Subscription {
    rx: Receiver<WorkspaceChange>,
    key: PathBuf,
    id: u64,
}

impl Subscription {
    /// The changes received so far, without waiting.
    pub fn try_iter(&self) -> TryIter<'_, WorkspaceChange> {
        self.rx.try_iter()
    }

    /// The next change, waiting at most `timeout`.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<WorkspaceChange, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut hubs = hubs();
        let empty = hubs.get(&self.key).is_some_and(|hub| {
            let mut hub = lock(hub);
            hub.subscribers.retain(|(id, _)| *id != self.id);
            hub.subscribers.is_empty()
        });
        if empty {
            hubs.remove(&self.key);
        }
    }
}

/// Subscribes to the changes of the workspace in `root` made through
/// [`Workspace`](super::Workspace) saves in this process (and, with them,
/// what other processes changed meanwhile). The workspace need not exist
/// yet.
pub fn subscribe(root: &Path) -> Subscription {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let (tx, rx) = channel();
    let key = normalized(root);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let hub = hubs().entry(key.clone()).or_default().clone();
    let mut hub = lock(&hub);
    if hub.last.is_none() {
        // read outside the global lock: only this workspace waits
        hub.last = Some(WorkspaceState::read(&key));
    }
    hub.subscribers.push((id, tx));
    Subscription { rx, key, id }
}

/// Sends the changes of the workspace in `root` to its subscribers; called
/// after every save with the file it wrote. Without subscribers it does
/// nothing.
pub(crate) fn publish(root: &Path, touched: Touched) {
    let hub = {
        let hubs = hubs();
        if hubs.is_empty() {
            return;
        }
        match hubs.get(&normalized(root)) {
            Some(h) => h.clone(),
            None => return,
        }
    };
    // the global lock is released: reading the files blocks only this workspace's saves
    let mut hub = lock(&hub);
    let Some(last) = hub.last.clone() else { return };
    let mut next = last.clone();
    next.refresh(&normalized(root), Some(touched), false);
    next.deferred = false;
    let changes = last.changes_to(&next);
    hub.last = Some(next);
    hub.subscribers.retain(|(_, tx)| changes.iter().all(|c| tx.send(c.clone()).is_ok()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // covers 16.8-d
    #[test]
    fn only_generated_volume_keys_are_passed_on() {
        let key = |id: &str| volume_key(&json!({ "source": { "volume": id } }));
        for k in ["b1000", "te80-e2", "n3", "iso1", "b1000-n2", "tr12.5", "ex"] {
            assert_eq!(key(&format!("1.2.3#v={k}")).as_deref(), Some(k), "{k}");
        }
        for crafted in ["1.2.840.113619.2.55.3.1234", "Ivanov^Petr", "b1000/../x", "", "b1.2.3", "abcd1"] {
            assert_eq!(key(&format!("1.2.3#v={crafted}")).as_deref(), Some("other"), "{crafted:?}");
        }
        assert_eq!(key("1.2.840.10008.5"), Some("other".into()), "no key at all: the id is a UID");
        assert_eq!(volume_key(&json!({ "source": {} })), None);
    }

    // covers 16.8-a
    #[test]
    fn content_does_not_depend_on_key_order_or_other_fields() {
        let a = json!({ "name": "x", "color": [1, 2, 3], "visible": true, "opacity": 0.5 });
        let b = json!({ "opacity": 0.5, "visible": true, "voxels": 10, "color": [1, 2, 3], "name": "x", "new": 1 });
        assert_eq!(content_of(&a, &SEGMENT_FIELDS), content_of(&b, &SEGMENT_FIELDS));
        let c = json!({ "name": "y", "color": [1, 2, 3], "visible": true, "opacity": 0.5 });
        assert_ne!(content_of(&a, &SEGMENT_FIELDS), content_of(&c, &SEGMENT_FIELDS));
    }

    // covers 16.8-a
    #[test]
    fn items_are_added_removed_revised_and_reviewed() {
        let item = |status, content| ItemState { status: Some(status), content: [content; 32] };
        let before = BTreeMap::from([(1u64, item(ReviewStatus::Proposed, 0)), (2, item(ReviewStatus::Proposed, 0))]);
        let after = BTreeMap::from([(2u64, item(ReviewStatus::Confirmed, 1)), (3, item(ReviewStatus::Proposed, 0))]);
        let mut out = Vec::new();
        diff_items(&before, &after, &mut out, |id, kind| match kind {
            Kind::Added => WorkspaceChange::AnnotationAdded { id },
            Kind::Removed => WorkspaceChange::AnnotationRemoved { id },
            Kind::Revised => WorkspaceChange::AnnotationRevised { id },
            Kind::Reviewed(status) => WorkspaceChange::AnnotationReviewed { id, status },
        });
        assert_eq!(
            out,
            vec![
                WorkspaceChange::AnnotationRemoved { id: 1 },
                WorkspaceChange::AnnotationRevised { id: 2 },
                WorkspaceChange::AnnotationReviewed { id: 2, status: ReviewStatus::Confirmed },
                WorkspaceChange::AnnotationAdded { id: 3 },
            ]
        );
    }

    // covers 16.8-c
    #[test]
    fn only_regular_files_of_bounded_size_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let big = dir.path().join("big.json");
        std::fs::write(&big, vec![b' '; 2048]).unwrap();
        assert!(read_regular(&big, 1024, false).unwrap_err().contains("larger"));
        assert_eq!(read_regular(&dir.path().join("missing"), 10, false).unwrap(), None);
        assert!(read_regular(dir.path(), 10, false).unwrap_err().contains("regular"), "a directory");
        #[cfg(unix)]
        {
            let link = dir.path().join("link.json");
            std::os::unix::fs::symlink(&big, &link).unwrap();
            assert!(read_regular(&link, 1 << 20, false).unwrap_err().contains("regular"), "a symbolic link");
        }
    }
}
