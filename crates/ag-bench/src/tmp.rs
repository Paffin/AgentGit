//! A scratch directory removed on drop. (Not the `tempfile` crate: it is a test-only dependency of the workspace,
//! and a normal dependency would add its operating-system crates to the licence inventory of the binaries.)

use std::path::{Path, PathBuf};

/// A fresh directory under `parent` (default: the system temporary directory).
pub struct TempDir(PathBuf);

impl TempDir {
    /// Creates `ag-bench-<random>` under `parent`.
    pub fn new(parent: Option<&Path>) -> std::io::Result<Self> {
        let name = format!("ag-bench-{}", crate::load::random_hex().map_err(std::io::Error::other)?);
        let path = parent.map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir).join(name);
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    /// The directory.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
