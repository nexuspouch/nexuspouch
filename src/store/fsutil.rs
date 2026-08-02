//! Cross-platform filesystem helpers for store durability on Windows.
//!
//! Unix rename is atomic and replaces; Windows often needs replace + retries
//! when editors/antivirus briefly lock the destination.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

/// True for symlinks (all platforms) and Windows reparse points (junctions, etc.).
pub fn is_symlink_or_reparse(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Rename `from` → `to`, replacing an existing file when needed, with short
/// retries for transient Windows sharing violations.
pub fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 8;
    let mut delay = Duration::from_millis(20);
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match rename_replace_once(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if is_retryable(&e) && attempt + 1 < ATTEMPTS => {
                last = Some(e);
                std::thread::sleep(delay);
                delay = (delay * 2).min(Duration::from_millis(400));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("rename_replace exhausted retries")))
}

fn rename_replace_once(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if to.exists() {
            // Windows `rename` does not replace an existing file.
            let _ = fs::remove_file(to);
        }
    }
    fs::rename(from, to)
}

fn is_retryable(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::PermissionDenied
            | io::ErrorKind::TimedOut
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::ResourceBusy
            | io::ErrorKind::Interrupted
    ) || err.raw_os_error() == Some(32) // ERROR_SHARING_VIOLATION
        || err.raw_os_error() == Some(33) // ERROR_LOCK_VIOLATION
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn rename_replace_overwrites() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"new").unwrap();
        fs::write(&b, b"old").unwrap();
        rename_replace(&a, &b).unwrap();
        assert_eq!(fs::read_to_string(&b).unwrap(), "new");
        assert!(!a.exists());
    }

    #[test]
    fn plain_file_not_reparse() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        fs::write(&p, b"x").unwrap();
        assert!(!is_symlink_or_reparse(&p));
    }

    #[cfg(unix)]
    #[test]
    fn unix_symlink_detected() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("t.txt");
        let link = dir.path().join("l.txt");
        fs::write(&target, b"x").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(is_symlink_or_reparse(&link));
    }
}
