use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Error returned when parsing an identifier from text fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdParseError {
    /// The text was not exactly 64 characters long.
    #[error("identifier must be 64 hex characters, got {0}")]
    Length(usize),
    /// The text contained a character that is not lowercase hex.
    #[error("identifier must be lowercase hex")]
    NotHex,
}

/// A 256-bit content identifier (SHA-256 with domain separation).
///
/// See `ag-object` for how identifiers are computed.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId([u8; 32]);

impl ObjectId {
    /// Length of an identifier in bytes.
    pub const LEN: usize = 32;

    /// Wraps raw digest bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Raw digest bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Full lowercase hex form (64 characters).
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Abbreviated hex form (first 12 characters) for human output.
    pub fn short(&self) -> String {
        hex::encode(&self.0[..6])
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl fmt::Debug for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ObjectId({})", self.short())
    }
}

impl FromStr for ObjectId {
    type Err = IdParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 64 {
            return Err(IdParseError::Length(s.len()));
        }
        if !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return Err(IdParseError::NotHex);
        }
        let mut out = [0u8; 32];
        hex::decode_to_slice(s, &mut out).map_err(|_| IdParseError::NotHex)?;
        Ok(Self(out))
    }
}

impl Serialize for ObjectId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

macro_rules! typed_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(ObjectId);

        impl $name {
            /// Wraps an untyped object identifier.
            pub const fn new(id: ObjectId) -> Self {
                Self(id)
            }

            /// The underlying untyped identifier.
            pub const fn object_id(&self) -> ObjectId {
                self.0
            }

            /// Raw digest bytes.
            pub const fn as_bytes(&self) -> &[u8; 32] {
                self.0.as_bytes()
            }

            /// Full lowercase hex form.
            pub fn to_hex(&self) -> String {
                self.0.to_hex()
            }

            /// Abbreviated hex form for human output.
            pub fn short(&self) -> String {
                self.0.short()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0.short())
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                s.parse::<ObjectId>().map(Self)
            }
        }

        impl From<$name> for ObjectId {
            fn from(v: $name) -> Self {
                v.0
            }
        }
    };
}

typed_id!(
    /// Identifier of an immutable content blob.
    BlobId
);
typed_id!(
    /// Identifier of a changeset.
    ChangesetId
);
typed_id!(
    /// Identifier of a derived tree (the resulting state of a changeset).
    TreeId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let id = ObjectId::from_bytes([0xab; 32]);
        let s = id.to_string();
        assert_eq!(s.len(), 64);
        assert_eq!(s.parse::<ObjectId>().unwrap(), id);
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!("abc".parse::<ObjectId>(), Err(IdParseError::Length(3)));
        let upper = "AB".repeat(32);
        assert_eq!(upper.parse::<ObjectId>(), Err(IdParseError::NotHex));
        let nonhex = "zz".repeat(32);
        assert_eq!(nonhex.parse::<ObjectId>(), Err(IdParseError::NotHex));
    }

    #[test]
    fn serde_is_hex_string() {
        let id = ChangesetId::new(ObjectId::from_bytes([1; 32]));
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{}\"", "01".repeat(32)));
        let back: ChangesetId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }
}
