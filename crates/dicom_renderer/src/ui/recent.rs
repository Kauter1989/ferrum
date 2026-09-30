//! "Recently opened" list shown on the start screen, persisted as a plain
//! text file in the user's configuration directory.

use std::path::{Path, PathBuf};

/// Most-recent-first list of opened paths.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecentFiles {
    store: Option<PathBuf>,
    items: Vec<PathBuf>,
}

impl RecentFiles {
    /// Maximum number of entries.
    pub const CAPACITY: usize = 8;

    /// A list that is never persisted (tests, headless tools).
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Loads the list from `<config dir>/dicom_renderer/recent.txt`.
    pub fn load_default() -> Self {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
        match base {
            Some(dir) => Self::load(&dir.join("dicom_renderer").join("recent.txt")),
            None => Self::in_memory(),
        }
    }

    /// Loads the list from `store` (missing file = empty list).
    pub fn load(store: &Path) -> Self {
        let items = std::fs::read_to_string(store)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(PathBuf::from)
            .take(Self::CAPACITY)
            .collect();
        Self { store: Some(store.to_path_buf()), items }
    }

    /// Entries, most recent first.
    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }

    /// Records opened paths (a single entry per open action: the folder or
    /// the first file) and persists the list.
    pub fn record(&mut self, paths: &[PathBuf]) {
        let Some(first) = paths.first() else {
            return;
        };
        let entry = if paths.len() > 1 {
            first.parent().map(Path::to_path_buf).unwrap_or(first.clone())
        } else {
            first.clone()
        };
        self.items.retain(|p| p != &entry);
        self.items.insert(0, entry);
        self.items.truncate(Self::CAPACITY);
        if let Some(store) = &self.store {
            if let Some(dir) = store.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let text: Vec<String> = self.items.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            if let Err(e) = std::fs::write(store, text.join("\n")) {
                log::warn!("cannot save recent files: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_dedups_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("sub/recent.txt");
        let mut r = RecentFiles::load(&store);
        assert!(r.items().is_empty());
        r.record(&[PathBuf::from("/a/scan.nii")]);
        r.record(&[PathBuf::from("/b/1.dcm"), PathBuf::from("/b/2.dcm")]);
        r.record(&[PathBuf::from("/a/scan.nii")]);
        assert_eq!(r.items(), &[PathBuf::from("/a/scan.nii"), PathBuf::from("/b")]);
        let again = RecentFiles::load(&store);
        assert_eq!(again.items(), r.items());
        for i in 0..20 {
            r.record(&[PathBuf::from(format!("/x/{i}"))]);
        }
        assert_eq!(r.items().len(), RecentFiles::CAPACITY);
        r.record(&[]);
        assert_eq!(r.items()[0], PathBuf::from("/x/19"));
    }
}
