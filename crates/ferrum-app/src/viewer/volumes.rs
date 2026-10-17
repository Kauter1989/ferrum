//! Volumes of a multi-volume series (echoes, b-values, phases, PET frames;
//! Stage 20.2): the list the Image panel shows, switching without reloading
//! the study, and a cache of volumes loaded before.
//!
//! The volumes of a complete series share its grid, so a switch replaces
//! only the voxels: slices, zoom, view mode, segments and annotations stay.

use ferrum_domain::{LoadedSeries, SeriesDescriptor, WindowLevel};

use super::Viewer;
use crate::dataset::Dataset;
use crate::jobs::LoadedStudy;

/// Voxels kept by the volume cache: 1 GiB (ADR 0013, `docs/multi-volume.md`).
pub const VOLUME_CACHE_BYTES: usize = 1 << 30;

/// A volume of the open series as the Image panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeEntry {
    /// `b = 1000`, `echo 2, TE = 80 ms`, `phase 3/20`, `volume 2/5`, …
    pub label: String,
    /// Shown now.
    pub current: bool,
    /// Why it cannot be shown in place of the current one (`None`: it can).
    pub disabled: Option<String>,
}

/// The open series when it holds several volumes.
#[derive(Debug, Clone, Default)]
pub(super) struct OpenSeries {
    volumes: Vec<SeriesDescriptor>,
    current: usize,
    /// A volume being loaded for a switch.
    pending: Option<usize>,
}

/// Loaded volumes, least recently used first, within a budget of bytes.
#[derive(Debug, Clone)]
pub(super) struct VolumeCache {
    entries: Vec<(String, Dataset)>,
    budget: usize,
}

impl Default for VolumeCache {
    fn default() -> Self {
        Self { entries: Vec::new(), budget: VOLUME_CACHE_BYTES }
    }
}

fn bytes(d: &Dataset) -> usize {
    d.volume.dims().voxel_count() * std::mem::size_of::<u16>()
}

impl VolumeCache {
    /// The dataset of volume `id`, now the most recently used.
    fn take(&mut self, id: &str) -> Option<Dataset> {
        let i = self.entries.iter().position(|(k, _)| k == id)?;
        let entry = self.entries.remove(i);
        let d = entry.1.clone();
        self.entries.push(entry);
        Some(d)
    }

    /// Keeps `d` as volume `id`, dropping the least recently used volumes
    /// beyond the budget (a volume larger than the budget is not kept).
    fn put(&mut self, id: &str, d: &Dataset) {
        self.entries.retain(|(k, _)| k != id);
        if bytes(d) > self.budget {
            return;
        }
        self.entries.push((id.to_owned(), d.clone()));
        while self.entries.iter().map(|(_, d)| bytes(d)).sum::<usize>() > self.budget {
            self.entries.remove(0);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Viewer {
    /// The volumes of the open series; empty for a single-volume series.
    /// A volume missing slices of the series lies on another grid and is
    /// disabled with the reason.
    pub fn volumes(&self) -> Vec<VolumeEntry> {
        let Some(open) = &self.series else { return Vec::new() };
        let dims = open.volumes.get(open.current).map(|v| v.dims);
        open.volumes
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let info = v.volume.as_ref();
                let complete = info.is_none_or(|i| i.complete);
                let disabled = if i == open.current {
                    None
                } else if !complete {
                    Some("misses slices of the series, so it does not share the grid of the others".to_owned())
                } else if Some(v.dims) != dims {
                    Some("lies on another grid than the volume shown".to_owned())
                } else {
                    None
                };
                VolumeEntry {
                    label: info.map_or_else(|| format!("volume {}", i + 1), |i| i.label.clone()),
                    current: i == open.current,
                    disabled,
                }
            })
            .collect()
    }

    /// Index of the volume shown, when the series holds several.
    pub fn current_volume(&self) -> Option<usize> {
        self.series.as_ref().map(|s| s.current)
    }

    /// Label of the volume shown, when the series holds several.
    pub fn current_volume_label(&self) -> Option<String> {
        let i = self.current_volume()?;
        self.volumes().into_iter().nth(i).map(|e| e.label)
    }

    /// `true` while a switch waits for its volume to load.
    pub fn is_switching_volume(&self) -> bool {
        self.series.as_ref().is_some_and(|s| s.pending.is_some())
    }

    /// Sets the budget of the volume cache in bytes of voxels (default
    /// [`VOLUME_CACHE_BYTES`]).
    pub fn set_volume_cache_budget(&mut self, bytes: usize) {
        self.volume_cache.budget = bytes;
        self.volume_cache.entries.clear();
    }

    /// Number of volumes in the cache.
    pub fn cached_volumes(&self) -> usize {
        self.volume_cache.entries.len()
    }

    /// Shows volume `index` of the open series in place of the current
    /// one: at once when it was loaded before, else after a background
    /// load. Slices, zoom, view mode, segments and annotations stay; the
    /// window follows the new volume unless the person set one.
    pub fn switch_volume(&mut self, index: usize) -> Result<(), String> {
        let entry = self.volumes().into_iter().nth(index).ok_or("the series has no such volume")?;
        if entry.current {
            return Ok(());
        }
        if let Some(reason) = entry.disabled {
            return Err(format!("{} cannot be shown: it {reason}", entry.label));
        }
        let open = self.series.as_mut().ok_or("the series holds one volume")?;
        let descriptor = open.volumes[index].clone();
        if let Some(d) = self.volume_cache.take(&descriptor.id) {
            open.pending = None;
            self.show_volume(index, d);
            return Ok(());
        }
        open.pending = Some(index);
        self.status.message = format!("Loading {}…", entry.label);
        self.status.progress = Some(0.0);
        self.jobs.load_volume(self.repo.clone(), descriptor);
        Ok(())
    }

    /// Steps to the next (`+1`) or previous (`-1`) volume that can be shown.
    pub fn step_volume(&mut self, delta: i32) -> Result<(), String> {
        let entries = self.volumes();
        let Some(current) = self.current_volume() else { return Ok(()) };
        let n = entries.len() as i64;
        let mut i = current as i64;
        for _ in 1..n {
            i = (i + i64::from(delta.signum())).rem_euclid(n);
            if entries[i as usize].disabled.is_none() {
                return self.switch_volume(i as usize);
            }
        }
        Ok(())
    }

    /// A series was loaded: keeps its volumes and installs the loaded one.
    pub(super) fn install_study(&mut self, study: LoadedStudy) {
        let LoadedStudy { volumes, index, mut loaded, .. } = study;
        self.volume_cache.clear();
        if let Some(v) = volumes.get(index) {
            let label = v.volume.as_ref().map_or("", |i| i.label.as_str());
            loaded.metadata.warnings.insert(
                0,
                format!(
                    "the series holds {n} volumes; volume {k} of {n} is shown ({label}); choose another in the Volume row of the Image panel",
                    n = volumes.len(),
                    k = index + 1
                ),
            );
        }
        self.install(loaded);
        self.series = (!volumes.is_empty()).then_some(OpenSeries { volumes, current: index, pending: None });
        if let (Some(open), Some(d)) = (&self.series, &self.dataset) {
            let id = open.volumes[index].id.clone();
            self.volume_cache.put(&id, d);
            if let Some(label) = self.current_volume_label() {
                self.status.message = format!("{} — {label}", self.status.message);
            }
        }
    }

    /// A volume asked for by [`Viewer::switch_volume`] was loaded.
    pub(super) fn finish_volume(&mut self, id: &str, loaded: LoadedSeries) {
        let Some(open) = &mut self.series else { return };
        let Some(index) = open.pending.filter(|i| open.volumes[*i].id == id) else { return };
        open.pending = None;
        let d = Dataset::new(loaded.volume, loaded.metadata, 0);
        self.volume_cache.put(id, &d);
        self.show_volume(index, d);
    }

    /// The switch failed: the current volume stays.
    pub(super) fn fail_volume(&mut self) {
        if let Some(open) = &mut self.series {
            open.pending = None;
        }
    }

    fn show_volume(&mut self, index: usize, d: Dataset) {
        let Some(open) = &mut self.series else { return };
        if self.dataset.as_ref().is_none_or(|old| old.volume.dims() != d.volume.dims()) {
            return;
        }
        open.current = index;
        let id = open.volumes[index].id.clone();
        let revision = self.bump_revision();
        let mut d = Dataset { revision, ..d };
        let old_revision = self.dataset.as_ref().map(|o| o.revision);
        self.keep_image_type(old_revision, &mut d);
        // the window follows the volume unless the person chose one
        if self.window_followed == Some(self.slices.window) {
            // the automatic window, not the stored one: a file often stores one window for all its
            // volumes, chosen for the first (b = 0), and a b = 1500 volume would be black under it
            let window = Self::auto_window_for(&d);
            self.slices.window = window;
            self.window_followed = Some(window);
        }
        self.load_warnings_unseen = !d.metadata.warnings.is_empty();
        self.dataset = Some(d);
        self.volume.ao = None;
        self.volume.ao_requested = None;
        self.probe = None;
        self.reset_ai_session();
        let label = self.current_volume_label().unwrap_or_default();
        self.status.progress = None;
        self.status.message = format!("Showing {label}");
        if let Some(ws) = &self.workspace {
            if let Err(e) = ws.store.set_volume(&id) {
                self.status.errors.push(format!("The workspace did not keep the volume: {e}"));
            }
        }
    }

    /// The window the viewer chose on its own for the dataset shown.
    pub(super) fn follow_window(&mut self, window: WindowLevel) {
        self.window_followed = Some(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ferrum_domain::{Dims3, SeriesMetadata, Volume};
    use glam::Vec3;

    fn dataset(n: u32) -> Dataset {
        let dims = Dims3::new(n, n, n);
        let v = Volume::from_physical(dims, Vec3::ONE, &vec![1.0; dims.voxel_count()]).unwrap();
        Dataset::new(v, SeriesMetadata::default(), 0)
    }

    #[test]
    fn the_cache_drops_the_least_recently_used_volume() {
        // covers 20.2-b
        let d = dataset(8); // 1 KiB of voxels
        let mut cache = VolumeCache { entries: Vec::new(), budget: 3 * bytes(&d) };
        for id in ["a", "b", "c"] {
            cache.put(id, &d);
        }
        assert!(cache.take("a").is_some(), "a is now the most recently used");
        cache.put("d", &d);
        let ids: Vec<_> = cache.entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(ids, ["c", "a", "d"], "b went out");
        cache.put("big", &dataset(16));
        assert!(cache.take("big").is_none(), "larger than the budget: not kept");
        assert_eq!(cache.entries.len(), 3);
    }
}
