#![no_main]
//! AGWP v1 request bodies as the HTTP/JSON gateway receives them (the same message types as gRPC, with
//! `serde`). The first two bytes choose the message type, the rest is the body. Decoding never panics;
//! whatever decodes serialises to JSON that decodes to the same message (base64 `bytes` fields and the
//! JSON-in-a-string fields included).

use agentgit_fuzz::agwp_types;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let idx = usize::from(u16::from_le_bytes([data[0], data[1]]));
    agwp_types::check_json(idx, &data[2..]);
});
