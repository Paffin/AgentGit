use std::path::{Path, PathBuf};

use ag_types::{Identity, Namespace, StreamName};
use serde::{Deserialize, Serialize};

use crate::storage::StorageConfig;

/// Name of the repository metadata directory.
pub const DIR_NAME: &str = ".ag";

/// On-disk layout of a repository rooted at a working directory.
#[derive(Clone, Debug)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// Layout for the working directory `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The working directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/.ag`
    pub fn ag_dir(&self) -> PathBuf {
        self.root.join(DIR_NAME)
    }

    /// Repository configuration file.
    pub fn config_path(&self) -> PathBuf {
        self.ag_dir().join("config.json")
    }

    /// Metadata database.
    pub fn db_path(&self) -> PathBuf {
        self.ag_dir().join("meta.db")
    }

    /// Object store directory.
    pub fn objects_dir(&self) -> PathBuf {
        self.ag_dir().join("store")
    }

    /// Directory of agent and issuer private keys. Override with `AG_KEY_DIR`
    /// to keep keys outside the repository.
    pub fn keys_dir(&self) -> PathBuf {
        match std::env::var_os("AG_KEY_DIR") {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => self.ag_dir().join("keys"),
        }
    }

    /// Finds the repository root by walking up from `start`.
    pub fn discover(start: &Path) -> Option<Layout> {
        let mut cur = Some(start);
        while let Some(dir) = cur {
            if dir.join(DIR_NAME).join("config.json").is_file() {
                return Some(Layout::new(dir));
            }
            cur = dir.parent();
        }
        None
    }
}

/// Persistent repository configuration (`.ag/config.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoConfig {
    /// Config format version.
    pub format: u32,
    /// Repository namespace (hashed into every changeset).
    pub namespace: Namespace,
    /// The stream created at init and used by default.
    pub default_stream: StreamName,
    /// Where metadata and objects live. Absent: a SQLite file and a directory under
    /// `.ag/`, as in every earlier version. Additive: `format` stays 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageConfig>,
}

impl RepoConfig {
    /// The effective storage layout (the default one when the section is absent).
    pub fn storage(&self) -> StorageConfig {
        self.storage.clone().unwrap_or_default()
    }
}

/// Options for [`crate::Repository::init`].
#[derive(Clone, Debug)]
pub struct InitOptions {
    /// Repository namespace, e.g. `acme/webapp`.
    pub namespace: Namespace,
    /// Name of the first stream.
    pub default_stream: StreamName,
    /// Who initializes the repository.
    pub actor: Identity,
    /// Where metadata and objects live; `None` is the local default (SQLite + files).
    pub storage: Option<StorageConfig>,
}

impl InitOptions {
    /// Options for a repository with the default (local) storage.
    pub fn new(namespace: Namespace, default_stream: StreamName, actor: Identity) -> Self {
        Self { namespace, default_stream, actor, storage: None }
    }

    /// Uses the given storage layout.
    pub fn with_storage(mut self, storage: StorageConfig) -> Self {
        self.storage = Some(storage);
        self
    }
}
