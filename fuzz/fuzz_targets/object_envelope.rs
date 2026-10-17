#![no_main]
//! The stored-object envelope: `AGOB | kind | version | payload`.
//!
//! Invariants: decoding never panics; an envelope that decodes re-encodes to the same bytes; the object id
//! is stable and `verify` accepts exactly that id (and rejects a different one).

use ag_object::{ObjectError, ObjectKind, RawObject, hash_blob, object_id};
use ag_types::ObjectId;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(obj) = RawObject::decode_envelope(data) {
        assert_eq!(obj.encode_envelope(), data, "decode/encode must round-trip");
        let id = obj.id();
        assert!(obj.verify(&id).is_ok());
        assert_eq!(id, object_id(obj.kind, obj.version, &obj.payload));
        if obj.kind == ObjectKind::Blob && obj.version == 1 {
            assert_eq!(hash_blob(&obj.payload).object_id(), id);
        }
        let other = ObjectId::from_bytes([0xa5; 32]);
        if other != id {
            assert!(matches!(obj.verify(&other), Err(ObjectError::HashMismatch { .. })));
        }
    }
    // Any bytes are hashable as a payload, whatever the kind.
    let _ = object_id(ObjectKind::Blob, 1, data);
});
