use ag_crypto::{CryptoError, KeyPair, PublicKey, Signature};
use ag_object::canon::Writer;
use ag_types::{Identity, Timestamp, TreeId};
use serde::{Deserialize, Serialize};

const DOMAIN: &str = "attestation.v1";

/// Outcome of a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckResult {
    /// The check passed.
    Pass,
    /// The check failed.
    Fail,
}

impl CheckResult {
    /// Stored name.
    pub fn as_str(self) -> &'static str {
        match self {
            CheckResult::Pass => "pass",
            CheckResult::Fail => "fail",
        }
    }
}

/// Errors from verifying an attestation.
#[derive(Debug, thiserror::Error)]
pub enum AttestationError {
    /// The signature does not match the content.
    #[error("attestation signature is invalid")]
    BadSignature,
    /// Cryptographic failure.
    #[error(transparent)]
    Crypto(#[from] CryptoError),
}

/// A signed statement "check X on exact tree T under policy version P in
/// environment E had result R", issued by a trusted execution plane.
///
/// The signature covers every field, so an attestation cannot be moved to a
/// different tree, policy version, environment or check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attestation {
    /// Unique id.
    pub id: String,
    /// The exact state that was checked.
    pub tree_id: TreeId,
    /// Name of the check, e.g. `tests`.
    pub check: String,
    /// Result.
    pub result: CheckResult,
    /// Version of the policy the check ran under.
    pub policy_version: u64,
    /// Digest identifying the execution environment.
    pub environment_digest: String,
    /// Issuer identity (an execution-plane service).
    pub issuer: Identity,
    /// When the statement was made.
    pub issued_at: Timestamp,
    /// The issuer's public key.
    pub public_key: PublicKey,
    /// Signature over [`Attestation::signing_bytes`].
    pub signature: Signature,
}

impl Attestation {
    /// The canonical bytes that are signed.
    #[allow(clippy::too_many_arguments)]
    pub fn signing_bytes(
        id: &str,
        tree_id: &TreeId,
        check: &str,
        result: CheckResult,
        policy_version: u64,
        environment_digest: &str,
        issuer: &Identity,
        issued_at: Timestamp,
    ) -> Vec<u8> {
        let mut w = Writer::new();
        w.str(id)
            .fixed32(tree_id.as_bytes())
            .str(check)
            .u8(match result {
                CheckResult::Pass => 1,
                CheckResult::Fail => 2,
            })
            .varint(policy_version)
            .str(environment_digest)
            .str(issuer.as_str())
            .i64(issued_at.as_millis());
        w.finish()
    }

    /// Creates and signs an attestation.
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        key: &KeyPair,
        id: String,
        tree_id: TreeId,
        check: String,
        result: CheckResult,
        policy_version: u64,
        environment_digest: String,
        issuer: Identity,
        issued_at: Timestamp,
    ) -> Result<Self, AttestationError> {
        let bytes =
            Self::signing_bytes(&id, &tree_id, &check, result, policy_version, &environment_digest, &issuer, issued_at);
        let signature = key.sign(DOMAIN, &bytes)?;
        Ok(Self {
            id,
            tree_id,
            check,
            result,
            policy_version,
            environment_digest,
            issuer,
            issued_at,
            public_key: key.public(),
            signature,
        })
    }

    /// Verifies the signature against the embedded public key. This proves
    /// integrity and key possession only; whether the key is *trusted* is the
    /// policy's decision.
    pub fn verify_signature(&self) -> Result<(), AttestationError> {
        let bytes = Self::signing_bytes(
            &self.id,
            &self.tree_id,
            &self.check,
            self.result,
            self.policy_version,
            &self.environment_digest,
            &self.issuer,
            self.issued_at,
        );
        self.public_key.verify(DOMAIN, &bytes, &self.signature).map_err(|_| AttestationError::BadSignature)
    }
}
