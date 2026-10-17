//! Strict, deterministic binary encoding primitives.
//!
//! The format is intentionally tiny and unambiguous:
//!
//! * fixed-width integers are big-endian;
//! * variable-length unsigned integers are unsigned LEB128 and **must** be
//!   minimal (no redundant continuation bytes);
//! * byte strings and UTF-8 strings are `varint length || bytes`;
//! * a [`Reader`] rejects trailing bytes, so each value has exactly one
//!   valid encoding and `decode(encode(x)) == x` is also
//!   `encode(decode(b)) == b` for every accepted `b`.

/// Errors produced by [`Reader`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CanonError {
    /// The input ended before the value was complete.
    #[error("unexpected end of input")]
    Eof,
    /// A varint did not fit in 64 bits.
    #[error("varint overflow")]
    VarintOverflow,
    /// A varint had redundant trailing zero groups.
    #[error("non-minimal varint")]
    NonMinimalVarint,
    /// A length prefix exceeded the allowed maximum.
    #[error("length {len} exceeds limit {max}")]
    TooLong {
        /// Declared length.
        len: u64,
        /// Allowed maximum.
        max: u64,
    },
    /// A string was not valid UTF-8.
    #[error("invalid UTF-8")]
    InvalidUtf8,
    /// Bytes remained after the value was fully decoded.
    #[error("{0} trailing bytes after value")]
    TrailingBytes(usize),
    /// A tag or flag byte had an unknown value.
    #[error("invalid tag {0}")]
    InvalidTag(u8),
}

/// Append-only encoder.
#[derive(Default, Debug)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one byte.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// Appends a big-endian `u16`.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends a big-endian `i64`.
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends a minimal unsigned LEB128 varint.
    pub fn varint(&mut self, mut v: u64) -> &mut Self {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return self;
            }
            self.buf.push(byte | 0x80);
        }
    }

    /// Appends a length-prefixed byte string.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.varint(b.len() as u64);
        self.buf.extend_from_slice(b);
        self
    }

    /// Appends a length-prefixed UTF-8 string.
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }

    /// Appends exactly 32 raw bytes (no length prefix).
    pub fn fixed32(&mut self, b: &[u8; 32]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Finishes and returns the encoded bytes.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

/// Strict decoder over a byte slice.
#[derive(Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Creates a reader positioned at the start of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], CanonError> {
        let end = self.pos.checked_add(n).ok_or(CanonError::Eof)?;
        if end > self.buf.len() {
            return Err(CanonError::Eof);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Reads one byte.
    pub fn u8(&mut self) -> Result<u8, CanonError> {
        Ok(self.take(1)?[0])
    }

    /// Reads a big-endian `u16`.
    pub fn u16(&mut self) -> Result<u16, CanonError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    /// Reads a big-endian `i64`.
    pub fn i64(&mut self) -> Result<i64, CanonError> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(i64::from_be_bytes(a))
    }

    /// Reads a minimal unsigned LEB128 varint.
    pub fn varint(&mut self) -> Result<u64, CanonError> {
        let mut result: u64 = 0;
        for i in 0..10 {
            let byte = self.u8()?;
            let payload = u64::from(byte & 0x7f);
            if i == 9 && payload > 1 {
                return Err(CanonError::VarintOverflow);
            }
            result |= payload << (7 * i);
            if byte & 0x80 == 0 {
                if i > 0 && payload == 0 {
                    return Err(CanonError::NonMinimalVarint);
                }
                return Ok(result);
            }
        }
        Err(CanonError::VarintOverflow)
    }

    /// Reads a varint and checks it against `max`.
    pub fn bounded_varint(&mut self, max: u64) -> Result<u64, CanonError> {
        let v = self.varint()?;
        if v > max {
            return Err(CanonError::TooLong { len: v, max });
        }
        Ok(v)
    }

    /// Reads a length-prefixed byte string no longer than `max` bytes.
    pub fn bytes(&mut self, max: usize) -> Result<&'a [u8], CanonError> {
        let len = self.bounded_varint(max as u64)? as usize;
        self.take(len)
    }

    /// Reads a length-prefixed UTF-8 string no longer than `max` bytes.
    pub fn str(&mut self, max: usize) -> Result<&'a str, CanonError> {
        std::str::from_utf8(self.bytes(max)?).map_err(|_| CanonError::InvalidUtf8)
    }

    /// Reads exactly 32 raw bytes.
    pub fn fixed32(&mut self) -> Result<[u8; 32], CanonError> {
        let mut a = [0u8; 32];
        a.copy_from_slice(self.take(32)?);
        Ok(a)
    }

    /// Number of unread bytes.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Asserts that the whole input was consumed.
    pub fn finish(self) -> Result<(), CanonError> {
        match self.remaining() {
            0 => Ok(()),
            n => Err(CanonError::TrailingBytes(n)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn varint_known_values() {
        for (v, enc) in [
            (0u64, vec![0x00]),
            (1, vec![0x01]),
            (127, vec![0x7f]),
            (128, vec![0x80, 0x01]),
            (300, vec![0xac, 0x02]),
            (u64::MAX, vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
        ] {
            let mut w = Writer::new();
            w.varint(v);
            assert_eq!(w.finish(), enc, "{v}");
            assert_eq!(Reader::new(&enc).varint().unwrap(), v);
        }
    }

    #[test]
    fn rejects_non_minimal_and_overflowing_varints() {
        assert_eq!(Reader::new(&[0x80, 0x00]).varint(), Err(CanonError::NonMinimalVarint));
        assert_eq!(Reader::new(&[0xff, 0x80, 0x00]).varint(), Err(CanonError::NonMinimalVarint));
        let overflow = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
        assert_eq!(Reader::new(&overflow).varint(), Err(CanonError::VarintOverflow));
        let too_long = [0x80; 11];
        assert_eq!(Reader::new(&too_long).varint(), Err(CanonError::VarintOverflow));
        assert_eq!(Reader::new(&[0x80]).varint(), Err(CanonError::Eof));
    }

    #[test]
    fn bounded_reads_do_not_allocate_huge_buffers() {
        let mut w = Writer::new();
        w.varint(u64::MAX >> 1);
        let bytes = w.finish();
        assert!(matches!(Reader::new(&bytes).bytes(1024), Err(CanonError::TooLong { .. })));
        // length within the limit but beyond the input
        let mut w = Writer::new();
        w.varint(100);
        let bytes = w.finish();
        assert_eq!(Reader::new(&bytes).bytes(1024), Err(CanonError::Eof));
    }

    #[test]
    fn trailing_bytes_rejected() {
        let mut r = Reader::new(&[1, 2]);
        r.u8().unwrap();
        assert_eq!(r.finish(), Err(CanonError::TrailingBytes(1)));
    }

    #[test]
    fn invalid_utf8_rejected() {
        let mut w = Writer::new();
        w.bytes(&[0xff, 0xfe]);
        let b = w.finish();
        assert_eq!(Reader::new(&b).str(16), Err(CanonError::InvalidUtf8));
    }

    proptest! {
        #[test]
        fn varint_roundtrip(v in any::<u64>()) {
            let mut w = Writer::new();
            w.varint(v);
            let b = w.finish();
            let mut r = Reader::new(&b);
            prop_assert_eq!(r.varint().unwrap(), v);
            prop_assert!(r.finish().is_ok());
        }

        #[test]
        fn accepted_varints_reencode_identically(bytes in proptest::collection::vec(any::<u8>(), 0..12)) {
            let mut r = Reader::new(&bytes);
            if let Ok(v) = r.varint() {
                let used = bytes.len() - r.remaining();
                let mut w = Writer::new();
                w.varint(v);
                prop_assert_eq!(w.finish(), bytes[..used].to_vec());
            }
        }

        #[test]
        fn string_roundtrip(s in "\\PC{0,64}", n in any::<i64>()) {
            let mut w = Writer::new();
            w.str(&s).i64(n);
            let b = w.finish();
            let mut r = Reader::new(&b);
            prop_assert_eq!(r.str(1024).unwrap(), s.as_str());
            prop_assert_eq!(r.i64().unwrap(), n);
            prop_assert!(r.finish().is_ok());
        }
    }
}
