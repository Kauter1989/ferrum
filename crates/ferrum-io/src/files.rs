//! File-system helpers: recursive collection of candidate files and cheap
//! format sniffing.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Detected file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// DICOM Part 10 file (with `DICM` magic).
    Dicom,
    /// NIfTI-1 (`.nii` or `.nii.gz`).
    Nifti,
    /// Anything else.
    Unknown,
}

/// Recursively collects regular files below `paths` (files are passed
/// through). Hidden entries (starting with `.`) found while descending are
/// skipped; explicitly passed paths are always honoured. The result is
/// sorted for deterministic behaviour.
pub fn collect_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(
                    rd.filter_map(|e| e.ok())
                        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
                        .map(|e| e.path()),
                );
            }
        } else if p.is_file() {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Sniffs the format of `path` from its name and first bytes.
pub fn detect(path: &Path) -> FileKind {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_ascii_lowercase();
    if name.ends_with(".nii") || name.ends_with(".nii.gz") {
        return FileKind::Nifti;
    }
    let mut buf = [0u8; 132];
    let Ok(mut f) = File::open(path) else {
        return FileKind::Unknown;
    };
    match f.read_exact(&mut buf) {
        Ok(()) if &buf[128..132] == b"DICM" => FileKind::Dicom,
        _ => FileKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_recursively_and_skips_hidden() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("x.dcm"), b"1").unwrap();
        std::fs::write(dir.path().join("y.nii"), b"1").unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/z"), b"1").unwrap();
        std::fs::write(dir.path().join(".hidden"), b"1").unwrap();
        let files = collect_files(&[dir.path().to_path_buf()]);
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn detects_formats() {
        let dir = tempfile::tempdir().unwrap();
        let dcm = dir.path().join("noext");
        let mut bytes = vec![0u8; 128];
        bytes.extend_from_slice(b"DICM");
        std::fs::write(&dcm, &bytes).unwrap();
        assert_eq!(detect(&dcm), FileKind::Dicom);
        assert_eq!(detect(&dir.path().join("brain.nii.gz")), FileKind::Nifti);
        let txt = dir.path().join("readme.txt");
        std::fs::write(&txt, b"hello").unwrap();
        assert_eq!(detect(&txt), FileKind::Unknown);
        assert_eq!(detect(&dir.path().join("missing")), FileKind::Unknown);
    }
}
