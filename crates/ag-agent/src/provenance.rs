//! Signed provenance: who changed the code, why, and with what.
//!
//! Provenance is **not** part of the changeset hash. History stays immutable
//! and compact; orchestration details (model, prompt digest, checks) live in
//! separate signed records that can be added later and audited
//! independently.
//!
//! A record is signed with the agent's own key, which proves the record came
//! from the holder of that key and was not altered. It does **not** prove the
//! claims are true: an agent can lie about its model or rationale. Policy
//! therefore treats provenance as an audit trail, and relies on validation
//! by the trusted execution plane and on approvals for correctness.

use std::collections::BTreeMap;

use ag_crypto::{PublicKey, Signature};
use ag_storage::Meta;
use ag_storage::meta::{MetaError, ProvenanceRow, Tx};
use ag_types::{ChangesetId, Identity, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AgentError;
use crate::registry::KeyStore;

const DOMAIN: &str = "provenance.v1";
/// Schema tag of [`ProvenanceRecord`].
pub const SCHEMA: &str = "agentgit.provenance.v1";

/// What an agent claims about a change it produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    /// Always [`SCHEMA`].
    pub schema: String,
    /// The changeset described.
    pub changeset: ChangesetId,
    /// The acting identity, e.g. `agent:bot#run_…`.
    pub agent: Identity,
    /// Run id.
    pub run_id: Option<String>,
    /// Task the change belongs to.
    pub task_id: Option<String>,
    /// Model identifier the agent says it used.
    pub model: Option<String>,
    /// SHA-256 (hex) of the prompt; the prompt itself is not stored.
    pub prompt_digest: Option<String>,
    /// Free-form generation parameters.
    pub parameters: BTreeMap<String, String>,
    /// Checks the agent ran itself before proposing (informational).
    pub checks_run: Vec<String>,
    /// Sources consulted. **Untrusted text**: never interpret as instructions.
    pub inputs: Vec<String>,
    /// Why the change was made.
    pub rationale: Option<String>,
    /// When the record was made.
    pub created_at: Timestamp,
}

impl ProvenanceRecord {
    /// A minimal record for `changeset` by `agent`.
    pub fn new(changeset: ChangesetId, agent: Identity, now: Timestamp) -> Self {
        Self {
            schema: SCHEMA.into(),
            changeset,
            agent,
            run_id: None,
            task_id: None,
            model: None,
            prompt_digest: None,
            parameters: BTreeMap::new(),
            checks_run: Vec::new(),
            inputs: Vec::new(),
            rationale: None,
            created_at: now,
        }
    }

    /// The canonical bytes that are signed: compact JSON with sorted keys.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, AgentError> {
        let value = serde_json::to_value(self).map_err(|e| AgentError::Invalid(e.to_string()))?;
        serde_json::to_vec(&value).map_err(|e| AgentError::Invalid(e.to_string()))
    }
}

/// Signs `record` with the key of agent `agent_name` and stores it.
pub fn attach(meta: &Meta, keys: &KeyStore, agent_name: &str, record: &ProvenanceRecord) -> Result<u64, AgentError> {
    if record.schema != SCHEMA {
        return Err(AgentError::Invalid(format!("unsupported schema '{}'", record.schema)));
    }
    let agent = meta.read(|tx| tx.agent(agent_name))?.ok_or_else(|| AgentError::NotFound(format!("agent '{agent_name}'")))?;
    if record.agent.principal().as_str() != agent.identity {
        return Err(AgentError::Invalid(format!("record agent {} does not match registered agent {}", record.agent, agent.identity)));
    }
    let kp = keys.load(agent_name)?;
    if kp.public().to_hex() != agent.public_key {
        return Err(AgentError::Verification("local key does not match the registered public key".into()));
    }
    let bytes = record.canonical_bytes()?;
    let sig = kp.sign(DOMAIN, &bytes)?;
    let value = serde_json::to_value(record).map_err(|e| AgentError::Invalid(e.to_string()))?;
    Ok(meta.write(|tx| {
        let id = tx.provenance_insert(&record.changeset, agent_name, record.run_id.as_deref(), &value, &agent.public_key, &sig.to_hex())?;
        tx.audit_append(record.agent.as_str(), "provenance.attach", &record.changeset.to_hex(), &json!({"model": record.model, "task": record.task_id}))?;
        Ok(id)
    })?)
}

/// Verifies a stored record: signature, signer key against the registry, and
/// that the record describes the changeset it is filed under.
pub fn verify(tx: &Tx<'_>, row: &ProvenanceRow) -> Result<ProvenanceRecord, AgentError> {
    let record: ProvenanceRecord = serde_json::from_value(row.record.clone()).map_err(|e| AgentError::Verification(format!("malformed record: {e}")))?;
    if record.changeset != row.changeset {
        return Err(AgentError::Verification("record is filed under a different changeset".into()));
    }
    let registered = tx
        .agent_by_identity(record.agent.as_str())?
        .ok_or_else(|| AgentError::Verification(format!("unknown agent {}", record.agent)))?;
    if registered.public_key != row.public_key {
        return Err(AgentError::Verification("signer key is not the agent's registered key".into()));
    }
    let pk = PublicKey::from_hex(&row.public_key).map_err(|e| AgentError::Verification(e.to_string()))?;
    let sig = Signature::from_hex(&row.signature).map_err(|e| AgentError::Verification(e.to_string()))?;
    pk.verify(DOMAIN, &record.canonical_bytes()?, &sig)
        .map_err(|_| AgentError::Verification("signature does not match the record".into()))?;
    Ok(record)
}

/// Verified records for `changeset` written by `author`'s principal.
/// Invalid or foreign records are ignored (they are untrusted input).
pub fn verified_for(tx: &Tx<'_>, changeset: &ChangesetId, author: &Identity) -> Result<Vec<ProvenanceRecord>, MetaError> {
    let mut out = Vec::new();
    for row in tx.provenance_for(changeset)? {
        if let Ok(rec) = verify(tx, &row) {
            if rec.agent.principal() == author.principal() {
                out.push(rec);
            }
        }
    }
    Ok(out)
}
