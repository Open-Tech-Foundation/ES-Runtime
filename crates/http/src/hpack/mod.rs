//! HPACK, HTTP/2's header compression (RFC 7541), sans-I/O.
//!
//! Each direction of a connection has one [`Encoder`] or [`Decoder`], whose
//! dynamic tables must stay in step with the peer's: every header block is
//! decoded in full, in order, even one whose headers are then discarded.
//!
//! The decoder is the half an attacker feeds, so it is the half that is
//! bounded:
//! - integers longer than a few bytes are refused rather than overflowing;
//! - a string's length is checked against the block before anything is read;
//! - Huffman padding must be short, all ones, and never the EOS symbol;
//! - a dynamic-table size update must come first in a block and stay within
//!   what we allowed;
//! - a header list growing past its limit — the "HPACK bomb", a small block
//!   that indexes one large entry over and over — stops being collected the
//!   moment it passes the limit, while the block is still decoded so the
//!   table stays in step (RFC 9113 §6.5.2).
//!
//! The static table and Huffman code are generated from the RFC's text
//! (`scripts/gen-hpack-tables.py`), and the decoder is checked against the
//! RFC's own worked examples, Appendix C.

mod huffman;
mod tables;

#[cfg(test)]
mod rfc_vectors;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;

pub use huffman::{huffman_decode, huffman_encode, huffman_encoded_len};

/// The size accounting of a table entry (RFC 7541 §4.1): its name and value
/// in octets, plus 32.
const ENTRY_OVERHEAD: usize = 32;

/// The most continuation bytes an integer may carry after its prefix: enough
/// for any value up to 2^28, which bounds every size this module accepts.
const MAX_INTEGER_BYTES: usize = 4;

/// Why a header block could not be decoded. Every one is a connection error
/// (`COMPRESSION_ERROR`): the dynamic table can no longer be trusted to match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HpackError {
    /// The block ended inside a representation.
    Truncated,
    /// An integer too large to be a valid size or index.
    IntegerOverflow,
    /// An index past the end of the static and dynamic tables, or zero.
    InvalidIndex,
    /// A Huffman string with an EOS symbol or bad padding.
    InvalidHuffman,
    /// A table-size update after a header, or larger than allowed.
    InvalidTableSizeUpdate,
}

impl std::fmt::Display for HpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            HpackError::Truncated => "HPACK: the header block ends mid-field",
            HpackError::IntegerOverflow => "HPACK: integer too large",
            HpackError::InvalidIndex => "HPACK: index outside the tables",
            HpackError::InvalidHuffman => "HPACK: invalid Huffman string",
            HpackError::InvalidTableSizeUpdate => "HPACK: invalid dynamic table size update",
        })
    }
}

impl std::error::Error for HpackError {}

// ---- primitives -------------------------------------------------------------

/// Appends `value` as an HPACK integer with an `n`-bit prefix (RFC 7541
/// §5.1); `flags` are the bits above the prefix in the first byte.
pub(crate) fn encode_integer(out: &mut Vec<u8>, value: usize, n: u8, flags: u8) {
    let max = (1usize << n) - 1;
    if value < max {
        out.push(flags | value as u8);
        return;
    }
    out.push(flags | max as u8);
    let mut rest = value - max;
    while rest >= 128 {
        out.push((rest % 128 + 128) as u8);
        rest /= 128;
    }
    out.push(rest as u8);
}

/// Reads an HPACK integer with an `n`-bit prefix from the start of `buf`;
/// returns it and how many bytes it took.
pub(crate) fn decode_integer(buf: &[u8], n: u8) -> Result<(usize, usize), HpackError> {
    let first = *buf.first().ok_or(HpackError::Truncated)?;
    let max = (1usize << n) - 1;
    let mut value = usize::from(first) & max;
    if value < max {
        return Ok((value, 1));
    }
    let mut shift = 0;
    for (i, &b) in buf[1..].iter().enumerate() {
        if i == MAX_INTEGER_BYTES {
            return Err(HpackError::IntegerOverflow);
        }
        value += usize::from(b & 0x7f) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return Ok((value, i + 2));
        }
    }
    Err(HpackError::Truncated)
}

/// Reads a string literal (RFC 7541 §5.2) into `out`; returns how many bytes
/// of `buf` it took.
fn decode_string(buf: &[u8], out: &mut Vec<u8>) -> Result<usize, HpackError> {
    let first = *buf.first().ok_or(HpackError::Truncated)?;
    let (len, used) = decode_integer(buf, 7)?;
    let data = buf
        .get(used..used.checked_add(len).ok_or(HpackError::IntegerOverflow)?)
        .ok_or(HpackError::Truncated)?;
    out.clear();
    if first & 0x80 != 0 {
        huffman_decode(data, out)?;
    } else {
        out.extend_from_slice(data);
    }
    Ok(used + len)
}

/// Appends `s` as a string literal, Huffman-coded when that is shorter.
fn encode_string(out: &mut Vec<u8>, s: &[u8]) {
    let huffman = huffman_encoded_len(s);
    if huffman < s.len() {
        encode_integer(out, huffman, 7, 0x80);
        huffman_encode(s, out);
    } else {
        encode_integer(out, s.len(), 7, 0);
        out.extend_from_slice(s);
    }
}

// ---- the dynamic table --------------------------------------------------------

/// A dynamic table (RFC 7541 §2.3.2): newest entry first, evicted from the
/// back when an insert or a smaller maximum needs the room.
#[derive(Debug, Default)]
struct Table {
    entries: VecDeque<(Vec<u8>, Vec<u8>)>,
    size: usize,
    max_size: usize,
}

impl Table {
    fn new(max_size: usize) -> Self {
        Table {
            entries: VecDeque::new(),
            size: 0,
            max_size,
        }
    }

    fn set_max_size(&mut self, max_size: usize) {
        self.max_size = max_size;
        self.evict_to(max_size);
    }

    fn evict_to(&mut self, limit: usize) {
        while self.size > limit {
            let (n, v) = self.entries.pop_back().expect("size counts entries");
            self.size -= n.len() + v.len() + ENTRY_OVERHEAD;
        }
    }

    /// Inserts an entry; one larger than the whole table empties it instead
    /// (RFC 7541 §4.4).
    fn insert(&mut self, name: Vec<u8>, value: Vec<u8>) {
        let size = name.len() + value.len() + ENTRY_OVERHEAD;
        if size > self.max_size {
            self.entries.clear();
            self.size = 0;
            return;
        }
        self.evict_to(self.max_size - size);
        self.size += size;
        self.entries.push_front((name, value));
    }

    /// The entry at HPACK index `index` (1-based: the static table, then this).
    fn get(&self, index: usize) -> Result<(&[u8], &[u8]), HpackError> {
        if index == 0 {
            return Err(HpackError::InvalidIndex);
        }
        if let Some((n, v)) = tables::STATIC.get(index - 1) {
            return Ok((n.as_bytes(), v.as_bytes()));
        }
        self.entries
            .get(index - 1 - tables::STATIC.len())
            .map(|(n, v)| (n.as_slice(), v.as_slice()))
            .ok_or(HpackError::InvalidIndex)
    }
}

// ---- decoding -----------------------------------------------------------------

/// What [`Decoder::decode`] found beyond the headers themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded {
    /// The header list passed the size limit; the fields after that point
    /// were decoded (to keep the table in step) but not kept.
    pub too_large: bool,
}

/// The receiving half of a connection's HPACK state.
#[derive(Debug)]
pub struct Decoder {
    table: Table,
    /// The largest table size the peer's encoder may ask for: what our
    /// `SETTINGS_HEADER_TABLE_SIZE` allows.
    allowed: usize,
}

impl Decoder {
    /// A decoder whose peer may use up to `max_table_size` bytes of table
    /// (our `SETTINGS_HEADER_TABLE_SIZE`; 4096 is the protocol default).
    #[must_use]
    pub fn new(max_table_size: usize) -> Self {
        Decoder {
            table: Table::new(max_table_size),
            allowed: max_table_size,
        }
    }

    /// Decodes one complete header block, appending its fields to `out` until
    /// their total size (RFC 9113 §6.5.2: name + value + 32 each) passes
    /// `max_list_size`.
    ///
    /// # Errors
    ///
    /// An [`HpackError`]; the connection must then end with `COMPRESSION_ERROR`.
    pub fn decode(
        &mut self,
        block: &[u8],
        max_list_size: usize,
        out: &mut Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<Decoded, HpackError> {
        let mut at = 0;
        let mut list_size = 0usize;
        let mut too_large = false;
        let mut fields_seen = false;
        let mut name = Vec::new();
        let mut value = Vec::new();
        while at < block.len() {
            let b = block[at];
            let rest = &block[at..];
            if b & 0x80 != 0 {
                // Indexed header field (§6.1).
                let (index, used) = decode_integer(rest, 7)?;
                at += used;
                let (n, v) = self.table.get(index)?;
                name.clear();
                name.extend_from_slice(n);
                value.clear();
                value.extend_from_slice(v);
            } else if b & 0xe0 == 0x20 {
                // Dynamic table size update (§6.3): only before any field.
                let (size, used) = decode_integer(rest, 5)?;
                if fields_seen || size > self.allowed {
                    return Err(HpackError::InvalidTableSizeUpdate);
                }
                self.table.set_max_size(size);
                at += used;
                continue;
            } else {
                // A literal (§6.2): with incremental indexing (01), without
                // (0000), or never indexed (0001).
                let (prefix, index_it) = if b & 0x40 != 0 { (6, true) } else { (4, false) };
                let (index, used) = decode_integer(rest, prefix)?;
                at += used;
                if index == 0 {
                    at += decode_string(&block[at..], &mut name)?;
                } else {
                    let (n, _) = self.table.get(index)?;
                    name.clear();
                    name.extend_from_slice(n);
                }
                at += decode_string(&block[at..], &mut value)?;
                if index_it {
                    self.table.insert(name.clone(), value.clone());
                }
            }
            fields_seen = true;
            if too_large {
                continue;
            }
            list_size = list_size.saturating_add(name.len() + value.len() + ENTRY_OVERHEAD);
            if list_size > max_list_size {
                too_large = true;
                continue;
            }
            out.push((std::mem::take(&mut name), std::mem::take(&mut value)));
        }
        Ok(Decoded { too_large })
    }

    /// Changes the largest table the peer may ask for — once a changed
    /// `SETTINGS_HEADER_TABLE_SIZE` of ours is acknowledged.
    pub fn set_allowed_table_size(&mut self, size: usize) {
        self.allowed = size;
    }

    /// The dynamic table's current size, by RFC 7541's accounting.
    #[must_use]
    pub fn table_size(&self) -> usize {
        self.table.size
    }
}

// ---- encoding -----------------------------------------------------------------

/// Names whose values are credentials: never put in a table, so a value
/// cannot be inferred by watching compression (RFC 7541 §7.1.3).
fn is_sensitive(name: &[u8]) -> bool {
    matches!(
        name,
        b"authorization" | b"proxy-authorization" | b"cookie" | b"set-cookie"
    )
}

/// The sending half of a connection's HPACK state.
#[derive(Debug)]
pub struct Encoder {
    table: Table,
    /// The smallest table maximum since the last block, and the current one:
    /// both must be signalled at the start of the next block (RFC 7541 §4.2).
    pending: Option<(usize, usize)>,
}

impl Encoder {
    /// An encoder with a `max_table_size`-byte table (the peer's
    /// `SETTINGS_HEADER_TABLE_SIZE`, at most what we choose to use).
    #[must_use]
    pub fn new(max_table_size: usize) -> Self {
        Encoder {
            table: Table::new(max_table_size),
            pending: None,
        }
    }

    /// Changes the table's maximum, signalled at the start of the next block.
    pub fn set_max_table_size(&mut self, size: usize) {
        let smallest = self.pending.map_or(size, |(s, _)| s.min(size));
        self.pending = Some((smallest, size));
        self.table.set_max_size(size);
    }

    /// Appends one header block for `fields` (names lowercase, as HTTP/2
    /// requires) to `out`.
    pub fn encode<'a>(
        &mut self,
        fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
        out: &mut Vec<u8>,
    ) {
        if let Some((smallest, current)) = self.pending.take() {
            if smallest < current {
                encode_integer(out, smallest, 5, 0x20);
            }
            encode_integer(out, current, 5, 0x20);
        }
        for (name, value) in fields {
            self.encode_field(name, value, out);
        }
    }

    fn encode_field(&mut self, name: &[u8], value: &[u8], out: &mut Vec<u8>) {
        let sensitive = is_sensitive(name);
        let mut name_index = 0;
        // The static table first (§2.3.1), then the dynamic one.
        for (i, (n, v)) in tables::STATIC.iter().enumerate() {
            if n.as_bytes() == name {
                if !sensitive && v.as_bytes() == value {
                    encode_integer(out, i + 1, 7, 0x80);
                    return;
                }
                if name_index == 0 {
                    name_index = i + 1;
                }
            }
        }
        for (i, (n, v)) in self.table.entries.iter().enumerate() {
            if n.as_slice() == name {
                let index = tables::STATIC.len() + 1 + i;
                if !sensitive && v.as_slice() == value {
                    encode_integer(out, index, 7, 0x80);
                    return;
                }
                if name_index == 0 {
                    name_index = index;
                }
            }
        }
        let size = name.len() + value.len() + ENTRY_OVERHEAD;
        // Never index a credential; and an entry that would take most of the
        // table would only evict everything useful in it.
        let (prefix, flags, index_it) = if sensitive {
            (4, 0x10, false)
        } else if size <= self.table.max_size * 3 / 4 {
            (6, 0x40, true)
        } else {
            (4, 0x00, false)
        };
        encode_integer(out, name_index, prefix, flags);
        if name_index == 0 {
            encode_string(out, name);
        }
        encode_string(out, value);
        if index_it {
            self.table.insert(name.to_vec(), value.to_vec());
        }
    }
}
