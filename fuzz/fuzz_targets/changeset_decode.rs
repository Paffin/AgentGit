#![no_main]
//! `ChangesetV1` payloads and objects. A decoded changeset is valid, re-encodes to exactly the input (the
//! format has one representation per value), and gets the same id every time.

use ag_changeset::Changeset;
use ag_object::RawObject;
use ag_types::Limits;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok(cs) = Changeset::decode(data, &limits) {
        assert!(cs.validate(&limits).is_ok(), "decode must only return valid changesets");
        assert_eq!(cs.encode(), data, "non-canonical bytes were accepted");
        assert_eq!(cs.id(), cs.id());
        let raw = cs.to_raw();
        assert_eq!(Changeset::from_raw(&raw, &limits).unwrap(), cs);
        // Everything derived for display must be total.
        let _ = cs.timestamp.to_rfc3339();
        let _ = cs.blobs().count();
        let _ = format!("{cs:?}");
    }
    // The same bytes as a whole stored object (envelope included).
    if let Ok(raw) = RawObject::decode_envelope(data) {
        let _ = Changeset::from_raw(&raw, &limits);
    }
    // Tight limits must reject, never panic.
    let tight = Limits { max_ops: 2, max_parents: 1, max_message_len: 8, ..Limits::default() };
    let _ = Changeset::decode(data, &tight);
});
