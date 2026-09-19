//! The wire primitives: varints, zigzag and a string table.
//!
//! This is the whole reason the crate does not put `serde` on the hot path.
//! The same bytes go from the tick thread to the webview through a
//! `tauri::ipc::Channel`, and later from the room server to a visitor, at
//! 10 Hz. JSON would spend more bytes on quotation marks than this spends on
//! the entire diff.
//!
//! # Format, byte by byte
//!
//! Every blob starts with the same six-byte header:
//!
//! ```text
//! offset size field
//! 0      4    magic, the ASCII bytes "OGW1"
//! 4      1    kind: 1 = snapshot, 2 = diff
//! 5      1    format version, currently 1
//! ```
//!
//! After the header come the bodies described in [`crate::snapshot`] and
//! [`crate::snapshot::diff`]. Three encodings are used throughout:
//!
//! - **varint**: LEB128, little-endian groups of seven bits, high bit set on
//!   every byte but the last. At most ten bytes. Used for every unsigned
//!   number, including `u64` ticks and hashes.
//! - **zigzag varint**: a signed number mapped with `(n << 1) ^ (n >> 31)`
//!   before the varint, so small negatives are as cheap as small positives.
//!   Used for coordinates, which are `Fixed` raw `i32`.
//! - **string table**: every string appears once, in first-use order, and is
//!   referenced by its index as a varint. Written as a varint count followed
//!   by that many (varint byte length, UTF-8 bytes) pairs.
//!
//! Fixed-width fields (`u8`, `u16`) are written raw, little-endian.

use std::collections::BTreeMap;

use crate::error::{Result, WorldError};

/// First four bytes of every blob.
pub const MAGIC: [u8; 4] = *b"OGW1";
/// Current format version. A decoder refuses anything else.
pub const FORMAT_VERSION: u8 = 1;
/// Blob kinds.
pub const KIND_SNAPSHOT: u8 = 1;
pub const KIND_DIFF: u8 = 2;
/// Bytes of the header.
pub const HEADER_LEN: usize = 6;
/// Longest string the table accepts, so a runaway model cannot blow the wire.
pub const MAX_STRING_BYTES: usize = 4096;
/// Most strings one blob may carry.
pub const MAX_STRINGS: usize = 4096;
/// Most entities or events one blob may carry, so a corrupt length cannot make
/// a decoder allocate gigabytes before it fails.
pub const MAX_ITEMS: usize = 1 << 20;

/// Byte sink with the encodings above.
#[derive(Clone, Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Writer {
        Writer::default()
    }

    /// Reuse an existing buffer, which is how the tick thread avoids
    /// allocating once per diff.
    pub fn with_buffer(mut buf: Vec<u8>) -> Writer {
        buf.clear();
        Writer { buf }
    }

    pub fn header(&mut self, kind: u8) {
        self.buf.extend_from_slice(&MAGIC);
        self.buf.push(kind);
        self.buf.push(FORMAT_VERSION);
    }

    #[inline]
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    #[inline]
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    #[inline]
    pub fn varint(&mut self, mut v: u64) {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return;
            }
            self.buf.push(byte | 0x80);
        }
    }

    #[inline]
    pub fn zigzag(&mut self, v: i32) {
        self.varint(((v << 1) ^ (v >> 31)) as u32 as u64);
    }

    pub fn bytes(&mut self, b: &[u8]) {
        self.varint(b.len() as u64);
        self.buf.extend_from_slice(b);
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn finish(self) -> Vec<u8> {
        self.buf
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
}

/// Byte source, with a bounds check on every read.
#[derive(Clone, Debug)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Reader<'a> {
        Reader { buf, pos: 0 }
    }

    /// Check the header and return the blob kind.
    pub fn header(&mut self) -> Result<u8> {
        if self.buf.len() < HEADER_LEN {
            return Err(WorldError::Truncated {
                at: self.buf.len(),
                need: HEADER_LEN - self.buf.len(),
            });
        }
        if self.buf[..4] != MAGIC {
            return Err(WorldError::BadMagic);
        }
        let kind = self.buf[4];
        let version = self.buf[5];
        if version != FORMAT_VERSION {
            return Err(WorldError::BadVersion {
                got: version,
                want: FORMAT_VERSION,
            });
        }
        if kind != KIND_SNAPSHOT && kind != KIND_DIFF {
            return Err(WorldError::BadKind(kind));
        }
        self.pos = HEADER_LEN;
        Ok(kind)
    }

    #[inline]
    pub fn u8(&mut self) -> Result<u8> {
        let b = *self.buf.get(self.pos).ok_or(WorldError::Truncated {
            at: self.pos,
            need: 1,
        })?;
        self.pos += 1;
        Ok(b)
    }

    #[inline]
    pub fn u16(&mut self) -> Result<u16> {
        if self.pos + 2 > self.buf.len() {
            return Err(WorldError::Truncated {
                at: self.pos,
                need: 2,
            });
        }
        let v = u16::from_le_bytes([self.buf[self.pos], self.buf[self.pos + 1]]);
        self.pos += 2;
        Ok(v)
    }

    #[inline]
    pub fn varint(&mut self) -> Result<u64> {
        let start = self.pos;
        let mut result: u64 = 0;
        for i in 0..10 {
            let b = self.u8()?;
            result |= ((b & 0x7f) as u64) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(result);
            }
        }
        Err(WorldError::Varint(start))
    }

    #[inline]
    pub fn zigzag(&mut self) -> Result<i32> {
        let v = self.varint()? as u32;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }

    /// A length-prefixed count, refused when it is beyond anything the world
    /// could hold.
    pub fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.varint()? as usize;
        if n > max || n > self.buf.len() - self.pos + 1 {
            return Err(WorldError::TooLong {
                what: "count",
                got: n,
                max,
            });
        }
        Ok(n)
    }

    pub fn bytes(&mut self) -> Result<&'a [u8]> {
        let n = self.varint()? as usize;
        if self.pos + n > self.buf.len() {
            return Err(WorldError::Truncated {
                at: self.pos,
                need: n,
            });
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Refuse a blob with bytes left over: it means the writer and the reader
    /// disagree about the format, and silently ignoring the tail hides that.
    pub fn finish(self) -> Result<()> {
        if self.remaining() > 0 {
            return Err(WorldError::Trailing(self.remaining()));
        }
        Ok(())
    }
}

/// Strings, deduplicated, in first-use order.
#[derive(Clone, Debug, Default)]
pub struct StringTable {
    items: Vec<String>,
    index: BTreeMap<String, u32>,
}

impl StringTable {
    pub fn new() -> StringTable {
        StringTable::default()
    }

    /// Index of a string, adding it if new. An over-long string is cut on a
    /// char boundary rather than rejected: a name is not worth failing a tick
    /// over.
    pub fn intern(&mut self, s: &str) -> u32 {
        if let Some(i) = self.index.get(s) {
            return *i;
        }
        let mut owned = s.to_string();
        if owned.len() > MAX_STRING_BYTES {
            let mut end = MAX_STRING_BYTES;
            while end > 0 && !owned.is_char_boundary(end) {
                end -= 1;
            }
            owned.truncate(end);
            if let Some(i) = self.index.get(&owned) {
                return *i;
            }
        }
        let idx = self.items.len() as u32;
        self.items.push(owned.clone());
        self.index.insert(owned, idx);
        idx
    }

    pub fn get(&self, idx: u32) -> Result<&str> {
        self.items
            .get(idx as usize)
            .map(|s| s.as_str())
            .ok_or(WorldError::BadString(idx as usize))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn write(&self, w: &mut Writer) {
        w.varint(self.items.len() as u64);
        for s in &self.items {
            w.bytes(s.as_bytes());
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Result<StringTable> {
        let n = r.count(MAX_STRINGS)?;
        let mut t = StringTable {
            items: Vec::with_capacity(n),
            index: BTreeMap::new(),
        };
        for i in 0..n {
            let raw = r.bytes()?;
            if raw.len() > MAX_STRING_BYTES {
                return Err(WorldError::TooLong {
                    what: "string",
                    got: raw.len(),
                    max: MAX_STRING_BYTES,
                });
            }
            let s = std::str::from_utf8(raw)
                .map_err(|_| WorldError::BadString(i))?
                .to_string();
            t.index.insert(s.clone(), i as u32);
            t.items.push(s);
        }
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_across_the_whole_range() {
        let cases = [
            0u64,
            1,
            127,
            128,
            300,
            16_383,
            16_384,
            u32::MAX as u64,
            u64::MAX / 2,
            u64::MAX,
        ];
        let mut w = Writer::new();
        for c in cases {
            w.varint(c);
        }
        let buf = w.finish();
        let mut r = Reader::new(&buf);
        for c in cases {
            assert_eq!(r.varint().unwrap(), c);
        }
        r.finish().unwrap();
    }

    #[test]
    fn varint_sizes_are_what_the_budget_assumes() {
        let size = |v: u64| {
            let mut w = Writer::new();
            w.varint(v);
            w.len()
        };
        assert_eq!(size(0), 1);
        assert_eq!(size(127), 1);
        assert_eq!(size(128), 2);
        assert_eq!(size(16_383), 2);
        assert_eq!(size(u64::MAX), 10);
    }

    #[test]
    fn zigzag_keeps_small_negatives_small() {
        let cases = [0i32, -1, 1, -64, 63, i32::MIN, i32::MAX, -4096, 4096];
        let mut w = Writer::new();
        for c in cases {
            w.zigzag(c);
        }
        let buf = w.finish();
        let mut r = Reader::new(&buf);
        for c in cases {
            assert_eq!(r.zigzag().unwrap(), c);
        }
        let mut w = Writer::new();
        w.zigzag(-1);
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn the_header_is_checked() {
        let mut w = Writer::new();
        w.header(KIND_SNAPSHOT);
        let buf = w.finish();
        assert_eq!(buf.len(), HEADER_LEN);
        assert_eq!(Reader::new(&buf).header().unwrap(), KIND_SNAPSHOT);

        let mut bad = buf.clone();
        bad[0] = b'X';
        assert_eq!(Reader::new(&bad).header(), Err(WorldError::BadMagic));

        let mut bad = buf.clone();
        bad[5] = 9;
        assert_eq!(
            Reader::new(&bad).header(),
            Err(WorldError::BadVersion { got: 9, want: 1 })
        );

        let mut bad = buf.clone();
        bad[4] = 7;
        assert_eq!(Reader::new(&bad).header(), Err(WorldError::BadKind(7)));

        assert!(matches!(
            Reader::new(&buf[..3]).header(),
            Err(WorldError::Truncated { .. })
        ));
    }

    #[test]
    fn a_truncated_blob_is_an_error_not_a_panic() {
        let mut w = Writer::new();
        w.varint(u64::MAX);
        w.bytes(b"hello");
        let buf = w.finish();
        for cut in 0..buf.len() {
            let mut r = Reader::new(&buf[..cut]);
            let _ = r.varint().and_then(|_| r.bytes().map(|_| ()));
        }
        let mut r = Reader::new(&buf[..buf.len() - 2]);
        r.varint().unwrap();
        assert!(matches!(r.bytes(), Err(WorldError::Truncated { .. })));
    }

    #[test]
    fn a_varint_that_never_ends_is_refused() {
        let buf = vec![0xff; 12];
        assert_eq!(Reader::new(&buf).varint(), Err(WorldError::Varint(0)));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut w = Writer::new();
        w.varint(1);
        let mut buf = w.finish();
        buf.push(0);
        let mut r = Reader::new(&buf);
        r.varint().unwrap();
        assert_eq!(r.finish(), Err(WorldError::Trailing(1)));
    }

    #[test]
    fn an_absurd_count_is_refused_before_allocating() {
        let mut w = Writer::new();
        w.varint(u32::MAX as u64);
        let buf = w.finish();
        assert!(matches!(
            Reader::new(&buf).count(MAX_ITEMS),
            Err(WorldError::TooLong { .. })
        ));
    }

    #[test]
    fn the_string_table_dedupes_in_first_use_order() {
        let mut t = StringTable::new();
        assert_eq!(t.intern("omni"), 0);
        assert_eq!(t.intern("ada"), 1);
        assert_eq!(t.intern("omni"), 0);
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(1).unwrap(), "ada");
        assert!(t.get(9).is_err());
    }

    #[test]
    fn the_string_table_round_trips() {
        let mut t = StringTable::new();
        for s in ["object/workbench", "omni", "olá, mundo", "日本語"] {
            t.intern(s);
        }
        let mut w = Writer::new();
        t.write(&mut w);
        let buf = w.finish();
        let mut r = Reader::new(&buf);
        let back = StringTable::read(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(back.len(), 4);
        assert_eq!(back.get(2).unwrap(), "olá, mundo");
        assert_eq!(back.get(3).unwrap(), "日本語");
    }

    #[test]
    fn invalid_utf8_in_the_table_is_an_error() {
        let mut w = Writer::new();
        w.varint(1);
        w.bytes(&[0xff, 0xfe]);
        let buf = w.finish();
        let mut r = Reader::new(&buf);
        assert_eq!(
            StringTable::read(&mut r).unwrap_err(),
            WorldError::BadString(0)
        );
    }

    #[test]
    fn over_long_strings_are_cut_not_rejected() {
        let mut t = StringTable::new();
        let idx = t.intern(&"á".repeat(4000)); // 8000 bytes
        assert_eq!(idx, 0);
        assert!(t.get(0).unwrap().len() <= MAX_STRING_BYTES);
    }

    #[test]
    fn a_writer_can_reuse_its_buffer() {
        let mut w = Writer::new();
        w.varint(1);
        let buf = w.finish();
        let cap = buf.capacity();
        let mut w2 = Writer::with_buffer(buf);
        assert!(w2.is_empty());
        w2.varint(2);
        assert_eq!(w2.finish().capacity(), cap);
    }
}
