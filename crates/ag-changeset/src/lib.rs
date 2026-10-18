//! `ChangesetV1`: the immutable unit of history.
//!
//! A changeset records an ordered list of parents, the author and message,
//! and a *delta* of file operations that transforms the state of its
//! **first parent** into its own resulting state. A changeset with several
//! parents is a merge; it still carries exactly one unambiguous resulting
//! state, expressed relative to the first parent. A changeset with no
//! parents is a root and its initial state is empty.
//!
//! Provenance of agent runs (models, prompts, checks) is **not** part of the
//! changeset: it lives in separate signed records so that history stays
//! immutable and orchestration state can evolve.
//!
//! # Canonical encoding (payload of an `ObjectKind::Changeset` object)
//!
//! ```text
//! format_version : u16be            (= 1)
//! namespace      : str
//! parents        : varint n, n * fixed32
//! author         : str
//! timestamp_ms   : i64be
//! message        : str
//! ops            : varint n, n * op
//! op             : 0x01 path:str file_type:u8 executable:u8 content:fixed32   (put)
//!                | 0x02 path:str                                              (delete)
//! ```
//!
//! Operations are sorted strictly ascending by path (bytewise); a path
//! appears at most once. The decoder rejects anything else, so every
//! changeset has exactly one valid byte representation.

use ag_object::canon::{CanonError, Reader, Writer};
use ag_object::{ObjectKind, RawObject};
use ag_types::{BlobId, ChangesetId, FileType, Identity, Limits, Namespace, RepoPath, Timestamp};

/// The changeset format version implemented by this crate.
pub const FORMAT_VERSION: u16 = 1;

const TAG_PUT: u8 = 1;
const TAG_DELETE: u8 = 2;
const MAX_AUTHOR_LEN: usize = 160;
/// The operation list is allocated up front for at most this many operations whatever the (untrusted) count in the
/// header says; it grows normally past that, as the operations are actually decoded.
const MAX_PREALLOCATED_OPS: usize = 4096;
const MAX_NAMESPACE_LEN: usize = 128;

/// One file operation inside a changeset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Create or replace the entry at `path`.
    Put {
        /// Target path.
        path: RepoPath,
        /// Content blob.
        content: BlobId,
        /// Entry type.
        file_type: FileType,
        /// Executable bit (only meaningful for regular files).
        executable: bool,
    },
    /// Remove the entry at `path`.
    Delete {
        /// Target path.
        path: RepoPath,
    },
}

impl Op {
    /// The path this operation affects.
    pub fn path(&self) -> &RepoPath {
        match self {
            Op::Put { path, .. } | Op::Delete { path } => path,
        }
    }
}

/// Errors from building, encoding or decoding a changeset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChangesetError {
    /// Low-level encoding error.
    #[error("malformed changeset encoding: {0}")]
    Canon(#[from] CanonError),
    /// The object is not a changeset.
    #[error("object is not a changeset")]
    WrongKind,
    /// The format version is not supported.
    #[error("unsupported changeset format version {0}")]
    UnsupportedVersion(u16),
    /// A field failed validation.
    #[error("invalid {field}: {reason}")]
    InvalidField {
        /// Field name.
        field: &'static str,
        /// Human-readable reason.
        reason: String,
    },
    /// More operations than [`Limits::max_ops`].
    #[error("too many operations: {0}")]
    TooManyOps(usize),
    /// More parents than [`Limits::max_parents`].
    #[error("too many parents: {0}")]
    TooManyParents(usize),
    /// The message is longer than [`Limits::max_message_len`].
    #[error("message too long: {0} bytes")]
    MessageTooLong(usize),
    /// The same parent was listed twice.
    #[error("duplicate parent {0}")]
    DuplicateParent(ChangesetId),
    /// Operations are not sorted strictly ascending by path.
    #[error("operations are not in canonical order near '{0}'")]
    OpsNotCanonical(RepoPath),
    /// An executable bit was set on a symlink.
    #[error("symlink '{0}' cannot be executable")]
    ExecutableSymlink(RepoPath),
}

fn invalid<E: std::fmt::Display>(field: &'static str, e: E) -> ChangesetError {
    ChangesetError::InvalidField { field, reason: e.to_string() }
}

/// An immutable changeset (format version 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changeset {
    /// Repository namespace; part of the hash.
    pub namespace: Namespace,
    /// Ordered parents. The first parent is the base of `ops`.
    pub parents: Vec<ChangesetId>,
    /// Author identity.
    pub author: Identity,
    /// Author timestamp.
    pub timestamp: Timestamp,
    /// Commit message.
    pub message: String,
    /// Canonically ordered operations relative to the first parent.
    pub ops: Vec<Op>,
}

impl Changeset {
    /// Builds a changeset from unordered operations: sorts them canonically
    /// and validates every invariant against `limits`.
    pub fn build(
        namespace: Namespace,
        parents: Vec<ChangesetId>,
        author: Identity,
        timestamp: Timestamp,
        message: impl Into<String>,
        mut ops: Vec<Op>,
        limits: &Limits,
    ) -> Result<Self, ChangesetError> {
        ops.sort_by(|a, b| a.path().cmp(b.path()));
        let cs = Self { namespace, parents, author, timestamp, message: message.into(), ops };
        cs.validate(limits)?;
        Ok(cs)
    }

    /// Checks every structural invariant.
    pub fn validate(&self, limits: &Limits) -> Result<(), ChangesetError> {
        if self.parents.len() > limits.max_parents {
            return Err(ChangesetError::TooManyParents(self.parents.len()));
        }
        for (i, p) in self.parents.iter().enumerate() {
            if self.parents[..i].contains(p) {
                return Err(ChangesetError::DuplicateParent(*p));
            }
        }
        if self.message.len() > limits.max_message_len {
            return Err(ChangesetError::MessageTooLong(self.message.len()));
        }
        if self.ops.len() > limits.max_ops {
            return Err(ChangesetError::TooManyOps(self.ops.len()));
        }
        for w in self.ops.windows(2) {
            if w[0].path() >= w[1].path() {
                return Err(ChangesetError::OpsNotCanonical(w[1].path().clone()));
            }
        }
        for op in &self.ops {
            if let Op::Put { path, file_type: FileType::Symlink, executable: true, .. } = op {
                return Err(ChangesetError::ExecutableSymlink(path.clone()));
            }
        }
        Ok(())
    }

    /// Canonical payload bytes (without the object envelope).
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u16(FORMAT_VERSION);
        w.str(self.namespace.as_str());
        w.varint(self.parents.len() as u64);
        for p in &self.parents {
            w.fixed32(p.as_bytes());
        }
        w.str(self.author.as_str());
        w.i64(self.timestamp.as_millis());
        w.str(&self.message);
        w.varint(self.ops.len() as u64);
        for op in &self.ops {
            match op {
                Op::Put { path, content, file_type, executable } => {
                    w.u8(TAG_PUT);
                    w.str(path.as_str());
                    w.u8(file_type.tag());
                    w.u8(u8::from(*executable));
                    w.fixed32(content.as_bytes());
                }
                Op::Delete { path } => {
                    w.u8(TAG_DELETE);
                    w.str(path.as_str());
                }
            }
        }
        w.finish()
    }

    /// Strictly decodes and validates a payload.
    pub fn decode(payload: &[u8], limits: &Limits) -> Result<Self, ChangesetError> {
        let mut r = Reader::new(payload);
        let version = r.u16()?;
        if version != FORMAT_VERSION {
            return Err(ChangesetError::UnsupportedVersion(version));
        }
        let namespace = Namespace::new(r.str(MAX_NAMESPACE_LEN)?).map_err(|e| invalid("namespace", e))?;
        let n_parents = r.bounded_varint(limits.max_parents as u64)? as usize;
        let mut parents = Vec::with_capacity(n_parents);
        for _ in 0..n_parents {
            parents.push(ChangesetId::new(ag_types::ObjectId::from_bytes(r.fixed32()?)));
        }
        let author = Identity::new(r.str(MAX_AUTHOR_LEN)?).map_err(|e| invalid("author", e))?;
        let timestamp = Timestamp::from_millis(r.i64()?);
        let message = r.str(limits.max_message_len)?.to_string();
        let n_ops = r.bounded_varint(limits.max_ops as u64)? as usize;
        let mut ops = Vec::with_capacity(n_ops.min(MAX_PREALLOCATED_OPS));
        for _ in 0..n_ops {
            let tag = r.u8()?;
            let path = RepoPath::new(r.str(ag_types::path_limits::MAX_PATH_LEN)?).map_err(|e| invalid("path", e))?;
            match tag {
                TAG_PUT => {
                    let file_type = FileType::from_tag(r.u8()?).ok_or_else(|| invalid("file_type", "unknown tag"))?;
                    let executable = match r.u8()? {
                        0 => false,
                        1 => true,
                        t => return Err(CanonError::InvalidTag(t).into()),
                    };
                    let content = BlobId::new(ag_types::ObjectId::from_bytes(r.fixed32()?));
                    ops.push(Op::Put { path, content, file_type, executable });
                }
                TAG_DELETE => ops.push(Op::Delete { path }),
                t => return Err(CanonError::InvalidTag(t).into()),
            }
        }
        r.finish()?;
        let cs = Self { namespace, parents, author, timestamp, message, ops };
        cs.validate(limits)?;
        Ok(cs)
    }

    /// Wraps the canonical payload in a storable object.
    pub fn to_raw(&self) -> RawObject {
        RawObject { kind: ObjectKind::Changeset, version: FORMAT_VERSION, payload: self.encode() }
    }

    /// Decodes a changeset from a stored object.
    pub fn from_raw(raw: &RawObject, limits: &Limits) -> Result<Self, ChangesetError> {
        if raw.kind != ObjectKind::Changeset {
            return Err(ChangesetError::WrongKind);
        }
        if raw.version != FORMAT_VERSION {
            return Err(ChangesetError::UnsupportedVersion(raw.version));
        }
        Self::decode(&raw.payload, limits)
    }

    /// The content identifier of this changeset.
    pub fn id(&self) -> ChangesetId {
        ChangesetId::new(self.to_raw().id())
    }

    /// The first parent, which is the base state of `ops`.
    pub fn first_parent(&self) -> Option<ChangesetId> {
        self.parents.first().copied()
    }

    /// True if the changeset has more than one parent.
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }

    /// Every blob referenced by this changeset.
    pub fn blobs(&self) -> impl Iterator<Item = BlobId> + '_ {
        self.ops.iter().filter_map(|op| match op {
            Op::Put { content, .. } => Some(*content),
            Op::Delete { .. } => None,
        })
    }
}

#[cfg(test)]
mod tests;

mod source;
pub use source::{ChangesetSource, MemoryHistory, SourceError};
