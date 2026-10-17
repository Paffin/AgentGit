//! Files that only their owner may read: private keys and credentials.
//!
//! * **Unix**: created with mode `0600` (directories `0700`); [`check_owner_only`] refuses a file that group
//!   or others can access.
//! * **Windows**: there are no mode bits. A new secret file inherits whatever the parent directory's ACL
//!   grants (under `C:\` that can include every authenticated user), so [`create`] replaces the file's ACL
//!   with one entry: full control for the current user (`icacls /inheritance:r /grant:r`). Reading the ACL
//!   back is **not** attempted, so [`check_owner_only`] cannot detect a file somebody loosened afterwards;
//!   `docs/operations/windows.md` says so.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Creates (or, with `exclusive == false`, truncates) a file that only the current user can access.
///
/// With `exclusive` the call fails if the path exists, so a key is never overwritten.
pub fn create(path: &Path, exclusive: bool) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true);
    if exclusive {
        opts.create_new(true);
    } else {
        opts.create(true).truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    #[cfg(windows)]
    windows::restrict_to_current_user(path, false)?;
    #[cfg(unix)]
    if !exclusive {
        // The mode only applies to files that are created: tighten one that already existed.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// Creates a directory (and parents) that only the current user can access.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    windows::restrict_to_current_user(path, true)?;
    Ok(())
}

/// Why [`check_owner_only`] refused a file.
#[derive(Debug)]
pub enum Insecure {
    /// The file could not be inspected.
    Io(io::Error),
    /// Group or other users can access the file.
    Accessible,
}

/// On Unix, fails if group or others have any access to `path`. On Windows this cannot be checked and
/// always succeeds (see the module documentation).
pub fn check_owner_only(path: &Path) -> Result<(), Insecure> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map_err(Insecure::Io)?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err(Insecure::Accessible);
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::path::Path;
    use std::process::Command;
    use std::sync::OnceLock;

    /// Replaces the ACL of `path` with a single entry granting the current user full control.
    pub(super) fn restrict_to_current_user(path: &Path, dir: bool) -> io::Result<()> {
        let sid = current_user_sid()?;
        // `*S-1-5-...` makes icacls take a SID rather than look a name up; `(OI)(CI)` inherits into children.
        let grant = format!("*{sid}:{}", if dir { "(OI)(CI)F" } else { "F" });
        let out = Command::new("icacls").arg(path).args(["/inheritance:r", "/grant:r", &grant]).output()?;
        if !out.status.success() {
            return Err(io::Error::other(format!(
                "icacls could not restrict {} to its owner: {}",
                path.display(),
                String::from_utf8_lossy(&out.stdout).trim()
            )));
        }
        Ok(())
    }

    fn current_user_sid() -> io::Result<&'static str> {
        static SID: OnceLock<Result<String, String>> = OnceLock::new();
        SID.get_or_init(|| {
            // `"DOMAIN\user","S-1-5-21-..."`
            let out =
                Command::new("whoami").args(["/user", "/fo", "csv", "/nh"]).output().map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&out.stdout);
            text.split('"')
                .find(|f| f.starts_with("S-1-"))
                .map(str::to_string)
                .ok_or_else(|| format!("cannot find the current user's SID in `whoami` output: {text:?}"))
        })
        .as_ref()
        .map(String::as_str)
        .map_err(|e| io::Error::other(e.clone()))
    }
}
