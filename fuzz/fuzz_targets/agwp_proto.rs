#![no_main]
//! AGWP v1 protobuf messages, as a gRPC peer sends them. The first two bytes choose the message type, the
//! rest is the wire data. Decoding must never panic, hang or allocate in proportion to a declared length;
//! whatever decodes must survive an encode/decode round trip.

use agentgit_fuzz::agwp_types;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 2 {
        return;
    }
    let idx = usize::from(u16::from_le_bytes([data[0], data[1]]));
    agwp_types::check_proto(idx, &data[2..]);
});
