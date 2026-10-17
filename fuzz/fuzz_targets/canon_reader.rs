#![no_main]
//! The canonical-encoding reader: the first byte of the input picks a sequence of primitive reads and the
//! rest is the buffer. Whatever is accepted must re-encode to the bytes it came from (one value, one
//! encoding); nothing may panic or allocate in proportion to a declared length.

use ag_object::canon::{Reader, Writer};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((script, buf)) = data.split_first() else { return };
    let mut r = Reader::new(buf);
    let mut w = Writer::new();
    // Bytes consumed by the steps that succeeded (a failed read may have advanced the reader).
    let mut consumed = 0;
    for step in 0..8 {
        match (script >> step) & 1 {
            0 => match r.varint() {
                Ok(v) => {
                    w.varint(v);
                }
                Err(_) => break,
            },
            _ => match r.bytes(1 << 16) {
                Ok(b) => {
                    w.bytes(b);
                }
                Err(_) => break,
            },
        }
        consumed = buf.len() - r.remaining();
    }
    let written = w.finish();
    assert_eq!(&buf[..consumed], &written[..], "an accepted prefix must re-encode identically");
    let _ = r.finish();

    let mut r = Reader::new(buf);
    let _ = r.u16();
    let _ = r.i64();
    let _ = r.fixed32();
    let _ = r.str(1 << 16);
    let _ = r.bounded_varint(1000);
});
