//! Content addressing and the canonical binary encoding toolkit.
//!
//! Every persisted object has a [`ObjectKind`], a format version and a
//! payload. Its identifier is
//!
//! ```text
//! SHA-256( "agentgit.object\0" || kind:u8 || version:u16be || len:u64be || payload )
//! ```
//!
//! so identifiers are domain-separated by object type and format version,
//! and a blob can never collide with a changeset that happens to have the
//! same bytes. Protobuf (or any other transport format) never participates
//! in hashing: payloads are produced by the strict encoders built on
//! [`canon`].

pub mod canon;
mod object;

pub use object::{ObjectError, ObjectKind, RawObject, hash_blob, object_id};
