//! File-system helpers whose behaviour does not depend on the platform.
//!
//! `std::fs::rename` is atomic and replaces an existing file on every platform AgentGit supports, but on
//! Windows it can fail for reasons that do not exist on Unix and that go away by themselves: the destination
//! is open in another process without delete sharing (a virus scanner, a search indexer, an editor) or carries
//! the read-only attribute. [`rename_replace`] is the one place that knows how to deal with that.

use std::io;
use std::path::Path;

/// Renames `from` over `to`, replacing it atomically.
///
/// On Unix this is exactly `std::fs::rename`. On Windows a sharing violation or "access denied" is retried
/// with a short back-off (the other process usually lets go within milliseconds), and a read-only destination
/// has its attribute cleared first. Any other error, and a persistent one, is returned unchanged.
pub fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
    #[cfg(windows)]
    {
        windows::rename_replace(from, to)
    }
}

/// Removes a file.
///
/// On Windows a read-only file cannot be deleted and a file somebody has open may be busy for a moment, so
/// this clears the attribute and retries like [`rename_replace`]; elsewhere it is `std::fs::remove_file`.
pub fn remove_file(path: &Path) -> io::Result<()> {
    #[cfg(not(windows))]
    {
        std::fs::remove_file(path)
    }
    #[cfg(windows)]
    {
        windows::remove_file(path)
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::path::Path;
    use std::time::Duration;

    /// How long the retries last before the error is returned: 8 attempts, 5 ms doubling to 320 ms (~0.6 s).
    const ATTEMPTS: u32 = 8;

    /// ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION: all mean "somebody has it open".
    fn is_busy(e: &io::Error) -> bool {
        matches!(e.raw_os_error(), Some(5 | 32 | 33))
    }

    pub(super) fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
        let mut delay = Duration::from_millis(5);
        for attempt in 0..ATTEMPTS {
            match std::fs::rename(from, to) {
                Ok(()) => return Ok(()),
                Err(e) if is_busy(&e) && attempt + 1 < ATTEMPTS => {
                    clear_readonly(to);
                    std::thread::sleep(delay);
                    delay *= 2;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!("the last attempt returns")
    }

    pub(super) fn remove_file(path: &Path) -> io::Result<()> {
        let mut delay = Duration::from_millis(5);
        for attempt in 0..ATTEMPTS {
            match std::fs::remove_file(path) {
                Ok(()) => return Ok(()),
                Err(e) if is_busy(&e) && attempt + 1 < ATTEMPTS => {
                    clear_readonly(path);
                    std::thread::sleep(delay);
                    delay *= 2;
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!("the last attempt returns")
    }

    /// A read-only destination cannot be replaced on Windows; Unix does not care about the file's own mode.
    fn clear_readonly(path: &Path) {
        if let Ok(md) = std::fs::metadata(path) {
            let mut perms = md.permissions();
            if perms.readonly() {
                #[allow(clippy::permissions_set_readonly_false)] // Windows: only clears FILE_ATTRIBUTE_READONLY
                perms.set_readonly(false);
                let _ = std::fs::set_permissions(path, perms);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_an_existing_file_atomically_and_moves_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (dir.path().join("a"), dir.path().join("b"), dir.path().join("c"));
        std::fs::write(&a, "new").unwrap();
        std::fs::write(&b, "old").unwrap();
        rename_replace(&a, &b).unwrap();
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "new");
        assert!(!a.exists());
        rename_replace(&b, &c).unwrap();
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "new");
    }

    #[test]
    fn a_missing_source_is_an_error_not_a_retry_loop() {
        let dir = tempfile::tempdir().unwrap();
        let t = std::time::Instant::now();
        let err = rename_replace(&dir.path().join("nope"), &dir.path().join("x")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(t.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn a_read_only_destination_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
        std::fs::write(&src, "new").unwrap();
        std::fs::write(&dst, "old").unwrap();
        let mut perms = std::fs::metadata(&dst).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&dst, perms).unwrap();
        rename_replace(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
    }

    #[test]
    fn removes_a_read_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("ro");
        std::fs::write(&f, "x").unwrap();
        let mut perms = std::fs::metadata(&f).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&f, perms).unwrap();
        remove_file(&f).unwrap();
        assert!(!f.exists());
        assert_eq!(remove_file(&f).unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    /// A destination another process holds open without delete sharing blocks the rename until it is closed.
    #[cfg(windows)]
    #[test]
    fn a_destination_that_is_briefly_open_is_waited_for() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
        std::fs::write(&src, "new").unwrap();
        std::fs::write(&dst, "old").unwrap();
        // FILE_SHARE_READ only: nobody may rename or delete the file while this handle lives.
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(&dst).unwrap();
        assert!(std::fs::rename(&src, &dst).is_err(), "the plain rename fails while the file is held");
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(60));
            drop(held);
        });
        rename_replace(&src, &dst).unwrap();
        releaser.join().unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
    }

    /// A destination that stays open is reported, after the bounded wait, as the error it is.
    #[cfg(windows)]
    #[test]
    fn a_destination_that_stays_open_is_an_error_after_a_bounded_wait() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("dst"));
        std::fs::write(&src, "new").unwrap();
        std::fs::write(&dst, "old").unwrap();
        let _held = std::fs::OpenOptions::new().read(true).share_mode(1).open(&dst).unwrap();
        let t = std::time::Instant::now();
        assert!(rename_replace(&src, &dst).is_err());
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "old");
        assert!(src.exists(), "the source is left for the caller to clean up");
    }
}
