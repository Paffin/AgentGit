//! Publish an already prepared directory without exposing a half-populated destination.
use std::{io, path::Path};

/// Atomically move a prepared directory into a previously absent destination.
/// Existing destinations, including empty directories, are never replaced. Both paths must be on
/// the same filesystem. On Unix, parent directories are synced before success is acknowledged.
pub fn publish_new_directory(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "prepared directory does not exist"));
    }
    move_exclusively(source, destination)?;
    sync_parent(source)?;
    sync_parent(destination)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn move_exclusively(source: &Path, destination: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, source, CWD, destination, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

#[cfg(windows)]
fn move_exclusively(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    // atomicwrites uses MoveFileExW without REPLACE_EXISTING, unlike std::fs::rename.
    // Validate before its NUL-terminated Win32 path conversion.
    if [source, destination].iter().any(|path| path.as_os_str().encode_wide().any(|unit| unit == 0)) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "directory path contains NUL"));
    }
    atomicwrites::move_atomic(source, destination)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn move_exclusively(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "atomic directory publication is unavailable on this platform"))
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> io::Result<()> {
    Ok(())
}
