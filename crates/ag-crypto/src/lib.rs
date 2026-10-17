//! Signatures and randomness.
//!
//! AgentGit uses Ed25519 for attestations (execution-plane results bound to
//! an exact tree) and provenance records (agent claims). Every signature is
//! **domain separated**: the signed message is
//!
//! ```text
//! "agentgit.sig.v1\0" || len(domain):u8 || domain || message
//! ```
//!
//! so a signature made for one purpose can never be replayed for another.
//! Verification uses the strict Ed25519 rules (no malleable encodings).
//!
//! A signature proves *who* said something, not that it is true: trust in a
//! validation comes from the identity and isolation of the signer, which the
//! policy engine checks against an explicit list of trusted issuers.

use std::fmt;
use std::io::Write;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod private_file;

const SIG_PREFIX: &[u8] = b"agentgit.sig.v1\0";

/// Errors from key handling and verification.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// Hex or length problem in a key or signature.
    #[error("malformed {0}")]
    Malformed(&'static str),
    /// The signature does not verify.
    #[error("signature verification failed")]
    BadSignature,
    /// The domain string is empty or too long.
    #[error("signature domain must be 1..=64 bytes")]
    BadDomain,
    /// The system random number generator failed.
    #[error("random number generator failure")]
    Random,
    /// A key file could not be read or written.
    #[error("key file error: {0}")]
    Io(#[from] std::io::Error),
    /// A key file exists but is accessible to other users.
    #[error("key file {0} is readable by other users; run `chmod 600`")]
    InsecurePermissions(String),
    /// A key file has an unexpected structure.
    #[error("invalid key file: {0}")]
    InvalidKeyFile(String),
}

/// Fills an array with cryptographically secure random bytes.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], CryptoError> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|_| CryptoError::Random)?;
    Ok(b)
}

/// A random identifier such as `op_3f9a…`: `prefix` + `_` + 20 hex chars.
pub fn random_id(prefix: &str) -> Result<String, CryptoError> {
    Ok(format!("{prefix}_{}", hex::encode(random_bytes::<10>()?)))
}

fn signed_message(domain: &str, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if domain.is_empty() || domain.len() > 64 {
        return Err(CryptoError::BadDomain);
    }
    let mut m = Vec::with_capacity(SIG_PREFIX.len() + 1 + domain.len() + message.len());
    m.extend_from_slice(SIG_PREFIX);
    m.push(domain.len() as u8);
    m.extend_from_slice(domain.as_bytes());
    m.extend_from_slice(message);
    Ok(m)
}

/// An Ed25519 public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey(VerifyingKey);

impl PublicKey {
    /// Parses 64 hex characters.
    pub fn from_hex(s: &str) -> Result<Self, CryptoError> {
        let mut b = [0u8; 32];
        hex::decode_to_slice(s, &mut b).map_err(|_| CryptoError::Malformed("public key"))?;
        VerifyingKey::from_bytes(&b).map(Self).map_err(|_| CryptoError::Malformed("public key"))
    }

    /// Lowercase hex form.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0.to_bytes())
    }

    /// Short identifier (first 8 bytes, hex) for display.
    pub fn key_id(&self) -> String {
        hex::encode(&self.0.to_bytes()[..8])
    }

    /// Verifies `signature` over `message` under `domain`.
    pub fn verify(&self, domain: &str, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
        let m = signed_message(domain, message)?;
        self.0.verify_strict(&m, &signature.0).map_err(|_| CryptoError::BadSignature)
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({})", self.key_id())
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for PublicKey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for PublicKey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        PublicKey::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// An Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Signature(ed25519_dalek::Signature);

impl Signature {
    /// Parses 128 hex characters.
    pub fn from_hex(s: &str) -> Result<Self, CryptoError> {
        let mut b = [0u8; 64];
        hex::decode_to_slice(s, &mut b).map_err(|_| CryptoError::Malformed("signature"))?;
        Ok(Self(ed25519_dalek::Signature::from_bytes(&b)))
    }

    /// Lowercase hex form.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0.to_bytes())
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Signature({}…)", &self.to_hex()[..16])
    }
}

impl Serialize for Signature {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Signature {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Signature::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// A signing key pair. The secret is zeroized on drop.
pub struct KeyPair {
    signing: SigningKey,
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    version: u32,
    algorithm: String,
    secret: String,
    public: String,
}

impl KeyPair {
    /// Generates a fresh key pair from the OS random number generator.
    pub fn generate() -> Result<Self, CryptoError> {
        Ok(Self { signing: SigningKey::from_bytes(&random_bytes::<32>()?) })
    }

    /// Builds a key pair from a 32-byte secret seed (deterministic; for tests).
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { signing: SigningKey::from_bytes(&seed) }
    }

    /// The public half.
    pub fn public(&self) -> PublicKey {
        PublicKey(self.signing.verifying_key())
    }

    /// Signs `message` under `domain`.
    pub fn sign(&self, domain: &str, message: &[u8]) -> Result<Signature, CryptoError> {
        let m = signed_message(domain, message)?;
        Ok(Signature(self.signing.sign(&m)))
    }

    /// Writes the key to `path`, refusing to overwrite and restricting access to the owner (mode `0600` on
    /// Unix, an owner-only ACL on Windows; see [`private_file`]).
    pub fn save(&self, path: &Path) -> Result<(), CryptoError> {
        let file = KeyFile {
            version: 1,
            algorithm: "ed25519".into(),
            secret: hex::encode(self.signing.to_bytes()),
            public: self.public().to_hex(),
        };
        let json = serde_json::to_vec_pretty(&file).map_err(|e| CryptoError::InvalidKeyFile(e.to_string()))?;
        let mut f = private_file::create(path, true)?;
        f.write_all(&json)?;
        f.sync_all()?;
        Ok(())
    }

    /// Loads a key written by [`KeyPair::save`]. On Unix, refuses files that
    /// are accessible to group or others (not checked on Windows).
    pub fn load(path: &Path) -> Result<Self, CryptoError> {
        match private_file::check_owner_only(path) {
            Ok(()) => {}
            Err(private_file::Insecure::Io(e)) => return Err(e.into()),
            Err(private_file::Insecure::Accessible) => {
                return Err(CryptoError::InsecurePermissions(path.display().to_string()));
            }
        }
        let file: KeyFile =
            serde_json::from_slice(&std::fs::read(path)?).map_err(|e| CryptoError::InvalidKeyFile(e.to_string()))?;
        if file.version != 1 || file.algorithm != "ed25519" {
            return Err(CryptoError::InvalidKeyFile("unsupported version or algorithm".into()));
        }
        let mut seed = [0u8; 32];
        hex::decode_to_slice(&file.secret, &mut seed).map_err(|_| CryptoError::InvalidKeyFile("bad secret".into()))?;
        let kp = Self::from_seed(seed);
        if kp.public().to_hex() != file.public {
            return Err(CryptoError::InvalidKeyFile("public key does not match secret".into()));
        }
        Ok(kp)
    }
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KeyPair({})", self.public().key_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let kp = KeyPair::generate().unwrap();
        let sig = kp.sign("attestation.v1", b"payload").unwrap();
        assert!(kp.public().verify("attestation.v1", b"payload", &sig).is_ok());
    }

    #[test]
    fn domain_and_message_are_bound() {
        let kp = KeyPair::from_seed([7; 32]);
        let sig = kp.sign("attestation.v1", b"payload").unwrap();
        let pk = kp.public();
        assert!(pk.verify("provenance.v1", b"payload", &sig).is_err());
        assert!(pk.verify("attestation.v1", b"payload2", &sig).is_err());
        let other = KeyPair::from_seed([8; 32]).public();
        assert!(other.verify("attestation.v1", b"payload", &sig).is_err());
        assert!(matches!(kp.sign("", b"x"), Err(CryptoError::BadDomain)));
    }

    #[test]
    fn signatures_are_deterministic_and_hex_roundtrip() {
        let kp = KeyPair::from_seed([1; 32]);
        let a = kp.sign("d", b"m").unwrap();
        let b = kp.sign("d", b"m").unwrap();
        assert_eq!(a, b);
        assert_eq!(Signature::from_hex(&a.to_hex()).unwrap(), a);
        let pk = kp.public();
        assert_eq!(PublicKey::from_hex(&pk.to_hex()).unwrap(), pk);
        assert!(Signature::from_hex("zz").is_err());
        assert!(PublicKey::from_hex("00").is_err());
    }

    #[test]
    fn serde_uses_hex() {
        let kp = KeyPair::from_seed([2; 32]);
        let json = serde_json::to_string(&kp.public()).unwrap();
        assert_eq!(json, format!("\"{}\"", kp.public().to_hex()));
        let back: PublicKey = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kp.public());
    }

    #[test]
    fn key_file_roundtrip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.json");
        let kp = KeyPair::generate().unwrap();
        kp.save(&path).unwrap();
        assert!(matches!(kp.save(&path), Err(CryptoError::Io(_))), "must not overwrite");
        let back = KeyPair::load(&path).unwrap();
        assert_eq!(back.public(), kp.public());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(matches!(KeyPair::load(&path), Err(CryptoError::InsecurePermissions(_))));
        }
    }

    #[test]
    fn tampered_key_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("k.json");
        let kp = KeyPair::from_seed([3; 32]);
        kp.save(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let other = KeyPair::from_seed([4; 32]).public().to_hex();
        let tampered = text.replace(&kp.public().to_hex(), &other);
        std::fs::write(&path, tampered).unwrap();
        assert!(matches!(KeyPair::load(&path), Err(CryptoError::InvalidKeyFile(_))));
    }

    #[test]
    fn random_ids_are_unique_and_valid() {
        let a = random_id("op").unwrap();
        let b = random_id("op").unwrap();
        assert_ne!(a, b);
        assert!(ag_types::OperationId::new(a).is_ok());
    }
}
