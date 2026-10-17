#![no_main]
//! Attestation and provenance JSON as they arrive from workers and agents, plus the hex forms of keys and
//! signatures.
//!
//! Invariants: parsing never panics; a parsed record serialises and re-parses to itself; the bytes that are
//! signed are a pure function of the record (stable across re-parsing); verifying a signature never panics
//! and a mutated record does not verify under the original signature.

use ag_agent::provenance::ProvenanceRecord;
use ag_crypto::{PublicKey, Signature};
use ag_policy::Attestation;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(att) = serde_json::from_slice::<Attestation>(data) {
        let json = serde_json::to_string(&att).unwrap();
        let back: Attestation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, att);
        let _ = att.verify_signature();
        let signed = |a: &Attestation| {
            Attestation::signing_bytes(
                &a.id,
                &a.tree_id,
                &a.check,
                a.result,
                a.policy_version,
                &a.environment_digest,
                &a.issuer,
                a.issued_at,
            )
        };
        assert_eq!(signed(&att), signed(&back));
        if att.verify_signature().is_ok() {
            // A valid signature covers every field.
            let mut moved = att.clone();
            moved.policy_version = moved.policy_version.wrapping_add(1);
            assert!(moved.verify_signature().is_err(), "signature must cover the policy version");
            let mut renamed = att.clone();
            renamed.check.push('!');
            assert!(renamed.verify_signature().is_err(), "signature must cover the check name");
        }
    }

    if let Ok(rec) = serde_json::from_slice::<ProvenanceRecord>(data) {
        let bytes = rec.canonical_bytes().expect("a parsed record serialises");
        let back: ProvenanceRecord = serde_json::from_slice(&bytes).expect("canonical bytes parse");
        assert_eq!(back, rec);
        assert_eq!(back.canonical_bytes().unwrap(), bytes, "canonical bytes must be a fixed point");
    }

    if let Ok(s) = std::str::from_utf8(data) {
        let (a, b) = s.split_at(s.len() / 2);
        if let Ok(k) = PublicKey::from_hex(a) {
            assert_eq!(PublicKey::from_hex(&k.to_hex()).unwrap(), k);
            if let Ok(sig) = Signature::from_hex(b) {
                assert!(k.verify("fuzz", data, &sig).is_err() || k.verify("fuzz", data, &sig).is_ok());
                assert_eq!(Signature::from_hex(&sig.to_hex()).unwrap(), sig);
            }
        }
    }
});
