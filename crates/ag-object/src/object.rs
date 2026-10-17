use ag_types::{BlobId, ObjectId};
use sha2::{Digest, Sha256};

/// Domain-separation prefix mixed into every object hash.
const DOMAIN: &[u8] = b"agentgit.object\0";
/// Magic bytes at the start of a stored object envelope.
const MAGIC: &[u8; 4] = b"AGOB";

/// The type of a stored object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    /// Raw file content.
    Blob,
    /// A canonically encoded changeset (see `ag-changeset`).
    Changeset,
    /// A canonically encoded tree manifest. Trees are *derived* data: they are
    /// never the source of truth, but their identifier (the "tree id") names
    /// an exact repository state for validation and attestation.
    Tree,
}

impl ObjectKind {
    /// Stable numeric tag (part of the hash preimage).
    pub fn tag(self) -> u8 {
        match self {
            ObjectKind::Blob => 1,
            ObjectKind::Changeset => 2,
            ObjectKind::Tree => 3,
        }
    }

    /// Inverse of [`ObjectKind::tag`].
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(ObjectKind::Blob),
            2 => Some(ObjectKind::Changeset),
            3 => Some(ObjectKind::Tree),
            _ => None,
        }
    }

    /// Lowercase name for logs.
    pub fn name(self) -> &'static str {
        match self {
            ObjectKind::Blob => "blob",
            ObjectKind::Changeset => "changeset",
            ObjectKind::Tree => "tree",
        }
    }
}

/// Errors from decoding or verifying a stored object.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ObjectError {
    /// The envelope is shorter than the fixed header.
    #[error("object envelope is truncated")]
    Truncated,
    /// The magic bytes do not match.
    #[error("object envelope has bad magic")]
    BadMagic,
    /// The kind tag is unknown.
    #[error("unknown object kind tag {0}")]
    UnknownKind(u8),
    /// The recomputed hash differs from the expected identifier.
    #[error("object hash mismatch: expected {expected}, computed {actual}")]
    HashMismatch {
        /// Identifier the caller expected.
        expected: ObjectId,
        /// Identifier computed from the bytes.
        actual: ObjectId,
    },
}

/// Computes the identifier of an object.
pub fn object_id(kind: ObjectKind, version: u16, payload: &[u8]) -> ObjectId {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update([kind.tag()]);
    h.update(version.to_be_bytes());
    h.update((payload.len() as u64).to_be_bytes());
    h.update(payload);
    let digest = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    ObjectId::from_bytes(out)
}

/// Computes the [`BlobId`] of raw file content (format version 1).
pub fn hash_blob(content: &[u8]) -> BlobId {
    BlobId::new(object_id(ObjectKind::Blob, 1, content))
}

/// An object as it is stored: kind, format version and payload bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawObject {
    /// Object type.
    pub kind: ObjectKind,
    /// Format version of the payload.
    pub version: u16,
    /// Payload bytes.
    pub payload: Vec<u8>,
}

impl RawObject {
    /// Builds a blob object from file content.
    pub fn blob(content: Vec<u8>) -> Self {
        Self { kind: ObjectKind::Blob, version: 1, payload: content }
    }

    /// The content identifier of this object.
    pub fn id(&self) -> ObjectId {
        object_id(self.kind, self.version, &self.payload)
    }

    /// Serializes to the on-disk envelope: `AGOB | kind | version | payload`.
    pub fn encode_envelope(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(7 + self.payload.len());
        out.extend_from_slice(MAGIC);
        out.push(self.kind.tag());
        out.extend_from_slice(&self.version.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// Parses an on-disk envelope. Does not verify any hash; use
    /// [`RawObject::verify`] against the expected identifier.
    pub fn decode_envelope(bytes: &[u8]) -> Result<Self, ObjectError> {
        if bytes.len() < 7 {
            return Err(ObjectError::Truncated);
        }
        if &bytes[..4] != MAGIC {
            return Err(ObjectError::BadMagic);
        }
        let kind = ObjectKind::from_tag(bytes[4]).ok_or(ObjectError::UnknownKind(bytes[4]))?;
        let version = u16::from_be_bytes([bytes[5], bytes[6]]);
        Ok(Self { kind, version, payload: bytes[7..].to_vec() })
    }

    /// Checks that the object hashes to `expected`.
    pub fn verify(&self, expected: &ObjectId) -> Result<(), ObjectError> {
        let actual = self.id();
        if &actual == expected { Ok(()) } else { Err(ObjectError::HashMismatch { expected: *expected, actual }) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separation_by_kind_and_version() {
        let payload = b"same bytes";
        let a = object_id(ObjectKind::Blob, 1, payload);
        let b = object_id(ObjectKind::Changeset, 1, payload);
        let c = object_id(ObjectKind::Blob, 2, payload);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn envelope_roundtrip_and_verify() {
        let obj = RawObject::blob(b"hello".to_vec());
        let id = obj.id();
        let back = RawObject::decode_envelope(&obj.encode_envelope()).unwrap();
        assert_eq!(back, obj);
        assert!(back.verify(&id).is_ok());
        let mut tampered = back.clone();
        tampered.payload[0] ^= 1;
        assert!(matches!(tampered.verify(&id), Err(ObjectError::HashMismatch { .. })));
    }

    #[test]
    fn rejects_malformed_envelopes() {
        assert_eq!(RawObject::decode_envelope(b"AG"), Err(ObjectError::Truncated));
        assert_eq!(RawObject::decode_envelope(b"XXXX\x01\x00\x01"), Err(ObjectError::BadMagic));
        assert_eq!(RawObject::decode_envelope(b"AGOB\x09\x00\x01"), Err(ObjectError::UnknownKind(9)));
    }

    #[test]
    fn blob_id_matches_independent_computation() {
        // Golden values computed independently (Python hashlib) from the
        // preimage in the crate docs. Changing the hashing scheme is a
        // format break and must be a deliberate, versioned decision.
        assert_eq!(hash_blob(b"hello\n").to_hex(), "6fdfa63ccd7aa33065dcfab8c0f7b1b605226c87833b50de3345f921122a4545");
        assert_eq!(hash_blob(b"").to_hex(), "2a0a334d0719f25cfd2cf4a6594fb55a5ce0be075ce4a23ba7b4ad95ce6d800b");
    }
}
