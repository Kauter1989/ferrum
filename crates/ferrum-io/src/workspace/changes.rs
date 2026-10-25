//! Change notifications of a workspace (16.8, issue #72).
//!
//! The agent, the viewer and a host's review panel may work on one
//! workspace at the same time. A change is computed by comparing the
//! workspace's files with the state read before, so it is the same
//! whether the writer is this process or another one:
//!
//! - **In process**, [`subscribe`] returns a receiver. Every save through
//!   [`Workspace`](super::Workspace) sends its changes to the subscribers of
//!   that workspace before the save returns.
//! - **Across processes**, a [`WorkspaceWatcher`] is polled by its owner.
//!   It reads the small JSON files on every poll, and the label map only
//!   when its size or modification time changed.
//!
//! A change holds labels, ids, review statuses and the volume key only:
//! never names, texts, UIDs or paths, which may identify a patient or a
//! person.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use ferrum_domain::ReviewStatus;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::files;
use crate::error::IoError;

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
    /// An annotation's name, points, text or slice changed.
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
        /// The volume's key (`b1000`, `e2`, …; the part after `#v=`), or
        /// `None` for a single-volume series.
        key: Option<String>,
    },
}

/// One item: its review status and a hash of everything else that a
/// change of the item can alter.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ItemState {
    status: Option<ReviewStatus>,
    content: [u8; 32],
}

/// The label map's size and modification time: when both are unchanged, a
/// poll does not decode it again.
type Stamp = Option<(u64, SystemTime)>;

/// The state of a workspace as far as changes are reported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceState {
    annotations: BTreeMap<u64, ItemState>,
    segments: BTreeMap<u8, ItemState>,
    /// Per label, a hash of the voxels it holds in the label map.
    masks: BTreeMap<u8, [u8; 32]>,
    volume: Option<String>,
    mask_stamp: Stamp,
}

fn stamp(path: &Path) -> Stamp {
    let md = std::fs::metadata(path).ok()?;
    Some((md.len(), md.modified().ok()?))
}

fn read_json(path: &Path) -> Result<Option<Value>, IoError> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| IoError::parse(path, e)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(IoError::os(path, e)),
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

/// A hash of the item's fields that are not its review: the provenance
/// (status, reviewer, times) and informative values are left out.
fn content_of(item: &Value, skip: &[&str]) -> [u8; 32] {
    let mut v = item.clone();
    if let Some(o) = v.as_object_mut() {
        o.remove("provenance");
        for k in skip {
            o.remove(*k);
        }
    }
    Sha256::digest(v.to_string().as_bytes()).into()
}

fn items<K: Ord>(
    doc: Option<Value>,
    list: &str,
    key: impl Fn(&Value) -> Option<K>,
    skip: &[&str],
) -> BTreeMap<K, ItemState> {
    let entries = doc.as_ref().and_then(|d| d[list].as_array()).cloned().unwrap_or_default();
    entries
        .iter()
        .filter_map(|e| Some((key(e)?, ItemState { status: status_of(e), content: content_of(e, skip) })))
        .collect()
}

/// Per label, a hash of the indices of its voxels.
fn masks(path: &Path) -> Result<BTreeMap<u8, [u8; 32]>, IoError> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let (values, _) = crate::nifti::read_values(path, 0)?;
    let mut hashers: BTreeMap<u8, Sha256> = BTreeMap::new();
    for (i, v) in values.iter().enumerate() {
        // labels are 1..=255 in a uint8 map
        let label = v.round().clamp(0.0, 255.0) as u8;
        if label != 0 {
            hashers.entry(label).or_default().update((i as u64).to_le_bytes());
        }
    }
    Ok(hashers.into_iter().map(|(l, h)| (l, h.finalize().into())).collect())
}

fn volume_key(manifest: Option<Value>) -> Option<String> {
    let id = manifest?["source"]["volume"].as_str()?.to_owned();
    // the id is `<series id>#v=<key>`: the series id is a UID, only the key is kept
    id.rsplit_once("#v=").map(|(_, k)| k.to_owned())
}

impl WorkspaceState {
    /// Reads the state of the workspace in `root`.
    pub fn read(root: &Path) -> Result<Self, IoError> {
        Self::default().refreshed(root, true)
    }

    /// The state now. The label map is decoded again only when `force` is
    /// set or its size or modification time changed since `self`.
    fn refreshed(&self, root: &Path, force: bool) -> Result<Self, IoError> {
        let annotation_id = |e: &Value| e["id"].as_u64();
        let segment_label = |e: &Value| e["label"].as_u64().and_then(|l| u8::try_from(l).ok());
        let doc = |name: &str| read_json(&root.join(name));
        let map = root.join(files::SEGMENTS_NIFTI);
        let mask_stamp = stamp(&map);
        let masks = if force || mask_stamp.is_none() || mask_stamp != self.mask_stamp {
            masks(&map)?
        } else {
            self.masks.clone()
        };
        Ok(Self {
            // the points in voxels and the slice are the content; millimetres and the value follow from them
            annotations: items(doc(files::ANNOTATIONS)?, "annotations", annotation_id, &["value", "points_mm"]),
            segments: items(doc(files::SEGMENTS_JSON)?, "segments", segment_label, &["voxels", "volume_ml"]),
            masks,
            volume: volume_key(doc(files::MANIFEST)?),
            mask_stamp,
        })
    }

    /// The changes from `self` to `next`.
    pub fn changes_to(&self, next: &Self) -> Vec<WorkspaceChange> {
        use WorkspaceChange as C;
        let mut out = Vec::new();
        diff_items(&self.annotations, &next.annotations, &mut out, |id, kind| match kind {
            Kind::Added => C::AnnotationAdded { id },
            Kind::Removed => C::AnnotationRemoved { id },
            Kind::Revised => C::AnnotationRevised { id },
            Kind::Reviewed(status) => C::AnnotationReviewed { id, status },
        });
        // a segment's voxels are part of its content
        let with_mask = |s: &BTreeMap<u8, ItemState>, m: &BTreeMap<u8, [u8; 32]>| -> BTreeMap<u8, ItemState> {
            let labels = s.keys().chain(m.keys()).copied().collect::<std::collections::BTreeSet<_>>();
            labels
                .into_iter()
                .map(|l| {
                    let item = s.get(&l).cloned().unwrap_or(ItemState { status: None, content: [0; 32] });
                    let mut h = Sha256::new();
                    h.update(item.content);
                    h.update(m.get(&l).copied().unwrap_or([0; 32]));
                    (l, ItemState { status: item.status, content: h.finalize().into() })
                })
                .collect()
        };
        let (before, after) = (with_mask(&self.segments, &self.masks), with_mask(&next.segments, &next.masks));
        diff_items(&before, &after, &mut out, |label, kind| match kind {
            Kind::Added => C::SegmentAdded { label },
            Kind::Removed => C::SegmentRemoved { label },
            Kind::Revised => C::SegmentRevised { label },
            Kind::Reviewed(status) => C::SegmentReviewed { label, status },
        });
        if self.volume != next.volume {
            out.push(C::VolumeSwitched { key: next.volume.clone() });
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
    pub fn new(root: &Path) -> Result<Self, IoError> {
        Ok(Self { root: root.to_path_buf(), last: WorkspaceState::read(root)? })
    }

    /// The changes since the last call (or since [`WorkspaceWatcher::new`]).
    pub fn poll(&mut self) -> Result<Vec<WorkspaceChange>, IoError> {
        let next = self.last.refreshed(&self.root, false)?;
        let changes = self.last.changes_to(&next);
        self.last = next;
        Ok(changes)
    }
}

/// The subscribers of one workspace and the state they last heard of.
struct Hub {
    last: WorkspaceState,
    subscribers: Vec<Sender<WorkspaceChange>>,
}

fn hubs() -> &'static Mutex<HashMap<PathBuf, Hub>> {
    static HUBS: OnceLock<Mutex<HashMap<PathBuf, Hub>>> = OnceLock::new();
    HUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn key(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// Subscribes to the changes of the workspace in `root` made through
/// [`Workspace`](super::Workspace) saves in this process. The receiver
/// gets them before each save returns; dropping it unsubscribes.
pub fn subscribe(root: &Path) -> Result<Receiver<WorkspaceChange>, IoError> {
    let (tx, rx) = channel();
    let key = key(root);
    let mut hubs = hubs().lock().map_err(|_| IoError::invalid(root, "change hub poisoned".to_owned()))?;
    match hubs.get_mut(&key) {
        Some(hub) => hub.subscribers.push(tx),
        None => {
            let last = WorkspaceState::read(&key)?;
            hubs.insert(key, Hub { last, subscribers: vec![tx] });
        }
    }
    Ok(rx)
}

/// Sends the changes of the workspace in `root` to its subscribers; called
/// after every save. Without subscribers it does nothing.
pub(crate) fn publish(root: &Path) {
    let Ok(mut hubs) = hubs().lock() else { return };
    if hubs.is_empty() {
        return;
    }
    let key = key(root);
    let Some(hub) = hubs.get_mut(&key) else { return };
    // a save just happened: read everything, whatever the time stamps say
    let next = match hub.last.refreshed(&key, true) {
        Ok(n) => n,
        Err(e) => {
            log::warn!("workspace changes of {}: {e}", key.display());
            return;
        }
    };
    let changes = hub.last.changes_to(&next);
    hub.last = next;
    hub.subscribers.retain(|tx| changes.iter().all(|c| tx.send(c.clone()).is_ok()));
    if hub.subscribers.is_empty() {
        hubs.remove(&key);
    }
}
