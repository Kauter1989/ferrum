//! The per-user lock directory and its safety checks.
//!
//! A lock directory another user can write would let that user hold a group
//! forever or plant records, so every directory and file is checked: not a
//! symbolic link, owned by the effective user, and (directories) not
//! writable by group or others. FERRUM never falls back to the shared
//! temporary folder.

use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::LockError;

/// The default lock directory of the current user (`docs/gpu-sharing.md`
/// §3.2), or `None` when the environment names no home.
pub fn default_path() -> Option<PathBuf> {
    default_path_from(|k| std::env::var_os(k))
}

/// [`default_path`] for a given environment (tests use it without changing
/// the process's environment).
pub fn default_path_from(env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let dir = |k: &str| env(k).map(PathBuf::from).filter(|p| p.is_absolute());
    let base = if cfg!(windows) {
        dir("LOCALAPPDATA")?
    } else if cfg!(target_os = "macos") {
        dir("HOME")?.join("Library").join("Caches")
    } else {
        dir("XDG_RUNTIME_DIR").or_else(|| dir("HOME").map(|h| h.join(".cache")))?
    };
    Some(base.join("ferrum").join("gpu"))
}

/// The file name of a group's directory: ASCII letters, digits and `-`
/// stay, anything else becomes `_`.
pub fn group_name(group: &str) -> String {
    let safe: String = group.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    if safe.is_empty() {
        "_".into()
    } else {
        safe
    }
}

/// What the checks need to know about a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    /// Owner (Unix user id).
    pub uid: u32,
    /// Permission bits.
    pub mode: u32,
    /// A directory.
    pub is_dir: bool,
    /// A symbolic link (not followed).
    pub is_symlink: bool,
}

/// Why `meta` is unsafe for the effective user `euid`, if it is.
pub fn unsafe_reason(meta: Meta, euid: u32, want_dir: bool) -> Option<String> {
    if meta.is_symlink {
        return Some("is a symbolic link".into());
    }
    if meta.is_dir != want_dir {
        return Some(if want_dir { "is not a directory".into() } else { "is not a regular file".into() });
    }
    if meta.uid != euid {
        return Some(format!("is owned by user {} instead of the current user {euid}", meta.uid));
    }
    if want_dir && meta.mode & 0o022 != 0 {
        return Some(format!("can be written by other users (mode {:o})", meta.mode & 0o777));
    }
    None
}

/// Checks an existing path.
pub fn check(path: &Path, want_dir: bool) -> Result<(), LockError> {
    let md = std::fs::symlink_metadata(path).map_err(|e| LockError::io(path, e))?;
    if let Some(reason) = reason_for(&md, want_dir) {
        return Err(LockError::Unsafe { path: path.to_path_buf(), reason });
    }
    Ok(())
}

#[cfg(unix)]
fn reason_for(md: &std::fs::Metadata, want_dir: bool) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = Meta { uid: md.uid(), mode: md.mode(), is_dir: md.is_dir(), is_symlink: md.file_type().is_symlink() };
    unsafe_reason(meta, rustix::process::geteuid().as_raw(), want_dir)
}

#[cfg(not(unix))]
fn reason_for(md: &std::fs::Metadata, want_dir: bool) -> Option<String> {
    // Windows: %LOCALAPPDATA% is private to the user by its ACL; only the
    // kind of the path is checked.
    let meta = Meta { uid: 0, mode: 0, is_dir: md.is_dir(), is_symlink: md.file_type().is_symlink() };
    unsafe_reason(meta, 0, want_dir)
}

/// Creates `path` (mode 0700) if it is missing, then checks it.
pub fn ensure_dir(path: &Path) -> Result<(), LockError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => create_private_dir(path)?,
        Err(e) => return Err(LockError::io(path, e)),
    }
    check(path, true)
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<(), LockError> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(LockError::io(path, e)),
    }
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<(), LockError> {
    match std::fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(LockError::io(path, e)),
    }
}

/// Opens a file of the lock directory for writing, creating it (mode 0600)
/// when `create` is set, and checks the opened path.
pub fn open_file(path: &Path, create: bool) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(create).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600).custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    o.open(path)
}

/// Creates a new file (fails if it exists).
pub fn create_new(path: &Path) -> io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ME: u32 = 1000;

    fn dir(uid: u32, mode: u32) -> Meta {
        Meta { uid, mode, is_dir: true, is_symlink: false }
    }

    // covers 23.1-c
    #[test]
    fn dir_check_refuses_other_owner_symlink_and_group_writable() {
        assert_eq!(unsafe_reason(dir(ME, 0o40700), ME, true), None);
        assert_eq!(unsafe_reason(dir(ME, 0o40750), ME, true), None, "readable by the group is fine");
        assert!(unsafe_reason(dir(0, 0o40700), ME, true).unwrap().contains("owned by user 0"));
        assert!(unsafe_reason(dir(ME, 0o41777), ME, true).unwrap().contains("written by other users"));
        assert!(unsafe_reason(dir(ME, 0o40720), ME, true).unwrap().contains("written by other users"));
        let link = Meta { is_symlink: true, ..dir(ME, 0o40700) };
        assert!(unsafe_reason(link, ME, true).unwrap().contains("symbolic link"));
        let file = Meta { is_dir: false, ..dir(ME, 0o100600) };
        assert_eq!(unsafe_reason(file, ME, false), None);
        assert!(unsafe_reason(file, ME, true).unwrap().contains("not a directory"));
        assert!(unsafe_reason(Meta { uid: 1001, ..file }, ME, false).unwrap().contains("owned by user 1001"));
    }

    // covers 23.1-c
    #[test]
    fn default_dir_is_per_user_and_never_tmp() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(*v))
        };
        if cfg!(windows) {
            assert_eq!(
                default_path_from(env(&[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local")])),
                Some(PathBuf::from("C:\\Users\\a\\AppData\\Local\\ferrum\\gpu"))
            );
        } else if cfg!(target_os = "macos") {
            assert_eq!(
                default_path_from(env(&[("HOME", "/Users/a")])),
                Some(PathBuf::from("/Users/a/Library/Caches/ferrum/gpu"))
            );
        } else {
            assert_eq!(
                default_path_from(env(&[("XDG_RUNTIME_DIR", "/run/user/1000"), ("HOME", "/home/a")])),
                Some(PathBuf::from("/run/user/1000/ferrum/gpu"))
            );
            assert_eq!(
                default_path_from(env(&[("HOME", "/home/a")])),
                Some(PathBuf::from("/home/a/.cache/ferrum/gpu"))
            );
            assert_eq!(
                default_path_from(env(&[("XDG_RUNTIME_DIR", ""), ("HOME", "/home/a")])),
                Some(PathBuf::from("/home/a/.cache/ferrum/gpu")),
                "an empty runtime dir is ignored"
            );
            assert_eq!(default_path_from(env(&[("XDG_RUNTIME_DIR", "relative")])), None);
        }
        assert_eq!(default_path_from(|_| None), None, "no home: no directory, not /tmp");
    }

    #[test]
    fn group_names_are_safe_file_names() {
        assert_eq!(group_name("a b/c"), "a_b_c");
        assert_eq!(group_name("rtx-3080ti"), "rtx-3080ti");
        assert_eq!(group_name(".."), "__");
        assert_eq!(group_name(""), "_");
    }
}
