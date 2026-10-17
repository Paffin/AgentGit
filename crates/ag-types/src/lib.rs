//! Core value types shared by every AgentGit crate.
//!
//! This crate is deliberately free of I/O and hashing. It defines the
//! validated, canonical forms of identifiers, repository paths, identities
//! and limits so that the rest of the system can rely on them being well
//! formed.

mod id;
mod identity;
mod limits;
mod path;
mod time;

pub use id::{BlobId, ChangesetId, IdParseError, ObjectId, TreeId};
pub use identity::{
    Identity, IdentityError, IdentityKind, Namespace, NamespaceError, OperationId, OperationIdError, StreamName,
    StreamNameError,
};
pub use limits::Limits;
pub use path::{PathError, RepoPath};

/// Path length limits.
pub mod path_limits {
    pub use crate::path::{MAX_COMPONENT_LEN, MAX_PATH_LEN};
}
pub use time::Timestamp;

/// Kind of a filesystem entry that a repository can record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileType {
    /// A regular file whose bytes are the content blob.
    Regular,
    /// A symbolic link whose target path is the content blob.
    Symlink,
}

impl FileType {
    /// Stable numeric tag used by the canonical binary encodings.
    pub fn tag(self) -> u8 {
        match self {
            FileType::Regular => 0,
            FileType::Symlink => 1,
        }
    }

    /// Inverse of [`FileType::tag`].
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(FileType::Regular),
            1 => Some(FileType::Symlink),
            _ => None,
        }
    }
}

impl std::fmt::Display for FileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FileType::Regular => "file",
            FileType::Symlink => "symlink",
        })
    }
}
