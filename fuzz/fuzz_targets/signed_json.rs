#![no_main]
//! Attestation and provenance JSON as they arrive from workers and agents, plus the hex forms of keys and
//! signatures.
//!
//! Invariants: parsing never panics; a parsed record serialises and re-parses to itself; the bytes that are
//! signed are a pure function of the record (stable across re-parsing); verifying a signature never panics
//! and a mutated record does not verify under the original signature.

use ag_agent::provenance::ProvenanceRecord;
use ag_crypto::{KeyPair, PublicKey, Signature};
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

    // Hex forms of keys and signatures. The input is arbitrary UTF-8, so the split point has to be moved back
    // to a character boundary (`split_at` panics inside a multi-byte character).
    if let Ok(s) = std::str::from_utf8(data) {
        let mut mid = s.len() / 2;
        while !s.is_char_boundary(mid) {
            mid -= 1;
        }
        let (a, b) = s.split_at(mid);
        if let Ok(k) = PublicKey::from_hex(a) {
            assert_eq!(PublicKey::from_hex(&k.to_hex()).unwrap(), k);
            if let Ok(sig) = Signature::from_hex(b) {
                // Whatever the verdict is, it is the same every time and never a panic.
                assert_eq!(k.verify("fuzz", data, &sig).is_ok(), k.verify("fuzz", data, &sig).is_ok());
                assert_eq!(Signature::from_hex(&sig.to_hex()).unwrap(), sig);
            }
        }
    }

    // A real signature over fuzzer-chosen bytes: it verifies, it is bound to its domain, and one flipped bit in
    // the message breaks it. The first 32 bytes are the key seed, the rest the message.
    if data.len() > 32 {
        let (seed, message) = data.split_at(32);
        let pair = KeyPair::from_seed(seed.try_into().expect("32 bytes"));
        let sig = pair.sign("fuzz", message).expect("signing does not fail");
        pair.public().verify("fuzz", message, &sig).expect("a fresh signature verifies");
        assert!(pair.public().verify("other-domain", message, &sig).is_err(), "domain separation");
        let mut tampered = message.to_vec();
        tampered[0] ^= 1;
        assert!(pair.public().verify("fuzz", &tampered, &sig).is_err(), "a changed message must not verify");
        assert_eq!(Signature::from_hex(&sig.to_hex()).unwrap(), sig);
        assert_eq!(PublicKey::from_hex(&pair.public().to_hex()).unwrap(), pair.public());
    }
});
