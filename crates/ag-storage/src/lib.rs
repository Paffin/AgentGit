//! Durable storage: an immutable object store and a transactional metadata
//! store.
//!
//! * [`ObjectStore`] holds blobs and changesets, addressed by content id.
//!   Objects are immutable and verified on every read. Backends declare what
//!   durability they actually provide via [`Durability`].
//! * [`Meta`] is the strongly consistent metadata store (SQLite in WAL mode
//!   with `synchronous=FULL`): the changeset index, streams, the reflog,
//!   publication intents, the outbox, the audit chain, policies, proposals,
//!   attestations and agent coordination state. Every multi-row state change
//!   happens inside one SQL transaction, which is what makes the stream
//!   compare-and-swap atomic.
//!
//! The two stores cannot share a transaction. The publication protocol in
//! `ag-integrate` therefore makes objects durable and verified *before* the
//! metadata transaction that makes them reachable.

pub mod clock;
mod error;
pub mod history;
pub mod meta;
mod object_store;
pub mod testing;

pub use clock::{Clock, ManualClock, SystemClock};
pub use error::StoreError;
pub use history::{MetaTreeCache, StoreHistory};
pub use meta::Meta;
pub use object_store::{
    Durability, FsObjectStore, MemoryObjectStore, ObjectInfo, ObjectStatus, ObjectStore,
};
