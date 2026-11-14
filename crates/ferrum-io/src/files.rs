//! File-system helpers: recursive collection of candidate files and cheap
//! format sniffing.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use ferrum_domain::RepositoryError;

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

/// Deepest folder level below a passed root that the walk descends into.
pub const MAX_WALK_DEPTH: usize = 8;

/// Most entries (files and folders) the walk visits below the passed
/// roots.
pub const MAX_WALK_ENTRIES: usize = 100_000;

/// Recursively collects regular files below `paths` (files are passed
/// through). Hidden entries (starting with `.`) found while descending
/// are skipped; explicitly passed paths are always honoured — the caller
/// checked them (the agent's read-root check resolves symlinks first).
///
/// Symlinks found while descending are skipped, not followed: a link
/// inside a source folder must not pull in files from outside it (#82).
/// The walk is bounded by [`MAX_WALK_DEPTH`] and [`MAX_WALK_ENTRIES`];
/// exceeding either is an error, never a silent cut. The result is
/// sorted for deterministic behaviour.
pub fn collect_files(paths: &[PathBuf]) -> Result<Vec<PathBuf>, RepositoryError> {
    collect_bounded(paths, MAX_WALK_DEPTH, MAX_WALK_ENTRIES)
}

/// The walk with explicit bounds, so tests can trigger them on small
/// trees.
fn collect_bounded(paths: &[PathBuf], max_depth: usize, max_entries: usize) -> Result<Vec<PathBuf>, RepositoryError> {
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, usize)> = paths.iter().map(|p| (p.clone(), 0)).collect();
    let mut seen = 0usize;
    while let Some((p, depth)) = stack.pop() {
        // An explicit root is honoured as given; a discovered entry is
        // known to be a real directory (its type was checked on the way
        // in, without following symlinks).
        if depth == 0 && !p.is_dir() {
            if p.is_file() {
                out.push(p);
            }
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&p) else { continue };
        for entry in rd.filter_map(|e| e.ok()) {
            seen += 1;
            if seen > max_entries {
                return Err(RepositoryError::Limit(format!(
                    "{}: more than {max_entries} entries below the source roots",
                    p.display()
                )));
            }
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            let child = entry.path();
            if kind.is_dir() {
                if depth + 1 > max_depth {
                    return Err(RepositoryError::Limit(format!(
                        "{}: deeper than {max_depth} levels below the source roots",
                        child.display()
                    )));
                }
                stack.push((child, depth + 1));
            } else if kind.is_file() {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
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
        let files = collect_files(&[dir.path().to_path_buf()]).unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn walk_survives_a_symlink_loop() {
        // covers F2.2-a: a link back into the tree must not loop the walk.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("x.dcm"), b"1").unwrap();
        if !symlink(&root, &root.join("loop")) {
            return;
        }
        let files = collect_files(&[root.clone()]).unwrap();
        assert_eq!(files, vec![root.join("x.dcm")]);
    }

    #[test]
    fn walk_bounds_depth_and_entries() {
        // covers F2.2-b: the bounds trigger on small trees, at the exact
        // boundary, and are errors — not a silent cut.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/file.dcm"), b"1").unwrap();
        // a/b is depth 2: a file inside it is collected at the bound…
        let files = collect_bounded(&[root.clone()], 2, 100).unwrap();
        assert_eq!(files, vec![root.join("a/b/file.dcm")]);
        // …and one level more is refused.
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        let err = collect_bounded(&[root.clone()], 2, 100).unwrap_err();
        assert!(matches!(err, RepositoryError::Limit(_)), "{err}");
        assert!(err.to_string().contains("deeper than 2 levels"), "{err}");
        // The entry bound: visiting a third entry is refused.
        let err = collect_bounded(&[root.clone()], 8, 2).unwrap_err();
        assert!(matches!(err, RepositoryError::Limit(_)), "{err}");
        assert!(err.to_string().contains("more than 2 entries"), "{err}");
        // Explicitly passed files are honoured, symlinks or not: the
        // caller checked them (the agent resolves the target first).
        let outside = dir.path().join("outside.dcm");
        std::fs::write(&outside, b"1").unwrap();
        if symlink(&outside, &root.join("explicit-link.dcm")) {
            let files = collect_files(&[root.join("explicit-link.dcm")]).unwrap();
            assert_eq!(files, vec![root.join("explicit-link.dcm")]);
        }
    }

    /// Creates a symlink; on Windows this needs a privilege the test
    /// runner may not hold, so a failed creation reports and skips the
    /// case. CI runs Linux, where every case runs.
    fn symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let r = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let r = if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        match r {
            Ok(()) => true,
            Err(e) => {
                eprintln!("skipping a symlink case: {e}");
                false
            }
        }
    }

    #[test]
    fn walk_does_not_follow_symlinks() {
        // covers F2.2-a: a folder inside a source tree that holds a link
        // to the outside must not pull the outside files in.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("a/x.dcm"), b"1").unwrap();
        std::fs::write(outside.join("y.dcm"), b"1").unwrap();
        if !symlink(&outside, &root.join("link-dir")) {
            return;
        }
        assert!(symlink(&outside.join("y.dcm"), &root.join("link-file")));
        let files = collect_files(&[root.clone()]).unwrap();
        assert_eq!(files, vec![root.join("a/x.dcm")]);
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
