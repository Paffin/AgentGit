//! AGWP v1 — the AgentGit Wire Protocol.
//!
//! The protocol is defined once, in `proto/agwp/v1/agwp.proto` (this crate builds from an identical copy in
//! `crates/ag-protocol/proto/`, kept in sync by `scripts/sync_crate_files.py`). This crate
//! compiles it (no external `protoc` needed) and exposes
//!
//! * the **message types** ([`v1`]), which are plain Rust structs that also
//!   derive `serde` and OpenAPI schemas, so the very same types are spoken by
//!   gRPC (protobuf) and by the HTTP/JSON gateway;
//! * the **gRPC services** (client and server stubs, via tonic);
//! * the **error model** ([`error`]): stable machine-readable codes and their
//!   mapping to gRPC and HTTP statuses;
//! * protocol **constants** ([`VERSION`], [`features`], default [`limits`]).
//!
//! Content addressing is *not* defined here: protobuf is only a transport.
//! Object IDs, canonical encodings and signatures are specified by the
//! byte-level formats in `docs/architecture/object-format.md`.

pub mod error;

/// Generated messages and services of package `agwp.v1`.
#[allow(missing_docs, clippy::all, clippy::large_enum_variant, clippy::doc_lazy_continuation)]
pub mod v1 {
    tonic::include_proto!("agwp.v1");
}

pub use error::{ErrorCode, ProtocolError};

/// Protocol version string negotiated by `Negotiate`.
pub const VERSION: &str = "agwp.v1";

/// Optional protocol features a server may advertise.
pub mod features {
    /// Durable event log with replay by sequence number.
    pub const EVENTS_REPLAY: &str = "events.replay";
    /// Server-side workspaces edited through the API (no checkout).
    pub const WORKSPACES_VIRTUAL: &str = "workspaces.virtual";
    /// Job queue and execution-plane workers.
    pub const EXECUTION_JOBS: &str = "execution.jobs";
    /// Objects larger than one message are streamed in chunks.
    pub const OBJECTS_CHUNKED: &str = "objects.chunked";
    /// OIDC bearer tokens are accepted.
    pub const AUTH_OIDC: &str = "auth.oidc";
    /// Scoped, short-lived credentials can be minted.
    pub const AUTH_SCOPED_CREDENTIALS: &str = "auth.scoped_credentials";
    /// Credentials can be bound to one run (`CredentialScope.run_id`, ADR-0010): they act only as that run and
    /// die with it.
    pub const AUTH_RUN_CREDENTIALS: &str = "auth.run_credentials";
    /// The `delegator` role: minting credentials for `agent:*` subjects without administrator rights.
    pub const AUTH_DELEGATION: &str = "auth.delegation";
    /// Operators can finish tasks and reap stale runs (`agent.operate`, `ReapRuns`).
    pub const AGENTS_OPERATE: &str = "agents.operate";

    /// Bounded literal content search with tree-bound continuation tokens.
    pub const TREE_SEARCH: &str = "tree.search";
    /// Repository issue tracker with durable discussion and optimistic editing.
    pub const ISSUES: &str = "issues.v1";

    /// Review submission can atomically require the exact head inspected by the caller.
    pub const PROPOSALS_REVIEW_HEAD: &str = "proposals.review_head";

    /// Every feature this implementation knows.
    pub const ALL: &[&str] = &[
        EVENTS_REPLAY,
        WORKSPACES_VIRTUAL,
        EXECUTION_JOBS,
        OBJECTS_CHUNKED,
        AUTH_OIDC,
        AUTH_SCOPED_CREDENTIALS,
        AUTH_RUN_CREDENTIALS,
        AUTH_DELEGATION,
        AGENTS_OPERATE,
        PROPOSALS_REVIEW_HEAD,
        TREE_SEARCH,
        ISSUES,
    ];
}

/// Default protocol limits advertised by `Negotiate`.
pub mod limits {
    /// Largest accepted object envelope (matches the engine's blob limit plus the envelope header).
    pub const MAX_OBJECT_BYTES: u64 = 256 * 1024 * 1024 + 64;
    /// Largest accepted non-streaming request body.
    pub const MAX_REQUEST_BYTES: u64 = 8 * 1024 * 1024;
    /// Largest number of items in one batch request.
    pub const MAX_BATCH: u32 = 10_000;
    /// Largest page size.
    pub const MAX_PAGE_SIZE: u32 = 1_000;
    /// Default page size.
    pub const DEFAULT_PAGE_SIZE: u32 = 100;
    /// Size of one object chunk on streams.
    pub const CHUNK_BYTES: usize = 1024 * 1024;
}

/// `serde` helper: `Vec<u8>` as a base64 string.
pub mod serde_b64 {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    /// Serializes bytes as standard base64.
    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(bytes))
    }

    /// Deserializes standard base64 (padding optional).
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        let trimmed = s.trim_end_matches('=');
        base64::engine::general_purpose::STANDARD_NO_PAD.decode(trimmed).map_err(serde::de::Error::custom)
    }
}

/// `serde` helper: a string that holds a JSON document, exposed as a JSON value.
pub mod json_string {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_json::Value;

    /// Serializes the document as JSON (an unparsable string stays a string).
    pub fn serialize<S: Serializer>(doc: &str, s: S) -> Result<S::Ok, S::Error> {
        if doc.is_empty() {
            return Value::Null.serialize(s);
        }
        match serde_json::from_str::<Value>(doc) {
            Ok(v) => v.serialize(s),
            Err(_) => doc.serialize(s),
        }
    }

    /// Deserializes any JSON value into its compact text.
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(match v {
            Value::Null => String::new(),
            other => other.to_string(),
        })
    }
}
