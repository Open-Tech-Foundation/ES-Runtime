//! HPACK's Huffman code (RFC 7541 §5.2, Appendix B).
//!
//! Decoding walks a table built once from the generated code: for each node
//! of the code tree and each 4-bit nibble, where the walk ends up and which
//! symbol it completed on the way. The code's shortest symbol is 5 bits, so a
//! nibble completes at most one. A string must end on the root or within 7
//! bits of it along the all-ones path (the EOS prefix used as padding), and
//! may never contain EOS itself.

use std::sync::OnceLock;

use super::HpackError;
use super::tables::HUFFMAN;

const EOS: u16 = 256;

/// One step of the decoder: from a node, on a nibble.
#[derive(Clone, Copy, Default)]
struct Step {
    next: u16,
    /// The symbol completed during this nibble, if any.
    emit: Option<u8>,
    /// The nibble walked into EOS, or off the code.
    fail: bool,
}

struct Decoding {
    steps: Vec<[Step; 16]>,
    /// Whether ending a string at this node is valid padding: on the
    /// all-ones path from the root, at most 7 bits deep.
    accepts: Vec<bool>,
}

fn decoding() -> &'static Decoding {
    static TABLE: OnceLock<Decoding> = OnceLock::new();
    TABLE.get_or_init(|| {
        // The code tree: children[node] = [zero, one], each either an
        // internal node (Ok) or a leaf symbol (Err).
        let mut children: Vec<[Option<Result<u16, u16>>; 2]> = vec![[None, None]];
        for (sym, &(code, len)) in HUFFMAN.iter().enumerate() {
            let mut node = 0usize;
            for i in (0..len).rev() {
                let bit = ((code >> i) & 1) as usize;
                if i == 0 {
                    children[node][bit] = Some(Err(sym as u16));
                } else {
                    match children[node][bit] {
                        Some(Ok(next)) => node = next as usize,
                        _ => {
                            children.push([None, None]);
                            let next = children.len() - 1;
                            children[node][bit] = Some(Ok(next as u16));
                            node = next;
                        }
                    }
                }
            }
        }
        // Padding: the all-ones path from the root, up to 7 bits.
        let mut accepts = vec![false; children.len()];
        accepts[0] = true;
        let mut node = 0usize;
        for _ in 0..7 {
            match children[node][1] {
                Some(Ok(next)) => {
                    node = next as usize;
                    accepts[node] = true;
                }
                _ => break,
            }
        }
        let steps = (0..children.len())
            .map(|start| {
                let mut row = [Step::default(); 16];
                for (nibble, step) in row.iter_mut().enumerate() {
                    let mut node = start;
                    let mut emit = None;
                    let mut fail = false;
                    for i in (0..4).rev() {
                        let bit = (nibble >> i) & 1;
                        match children[node][bit] {
                            Some(Ok(next)) => node = next as usize,
                            Some(Err(EOS)) | None => {
                                fail = true;
                                break;
                            }
                            Some(Err(sym)) => {
                                emit = Some(sym as u8);
                                node = 0;
                            }
                        }
                    }
                    *step = Step {
                        next: node as u16,
                        emit,
                        fail,
                    };
                }
                row
            })
            .collect();
        Decoding { steps, accepts }
    })
}

/// Decodes the Huffman string `data`, appending to `out`.
///
/// # Errors
///
/// [`HpackError::InvalidHuffman`] for an EOS symbol, or padding that is longer
/// than 7 bits or not all ones.
pub fn huffman_decode(data: &[u8], out: &mut Vec<u8>) -> Result<(), HpackError> {
    let table = decoding();
    out.reserve(data.len() * 8 / 5);
    let mut node = 0usize;
    for &byte in data {
        for nibble in [byte >> 4, byte & 0x0f] {
            let step = table.steps[node][nibble as usize];
            if step.fail {
                return Err(HpackError::InvalidHuffman);
            }
            if let Some(sym) = step.emit {
                out.push(sym);
            }
            node = step.next as usize;
        }
    }
    if table.accepts[node] {
        Ok(())
    } else {
        Err(HpackError::InvalidHuffman)
    }
}

/// How many bytes `data` takes Huffman-coded.
#[must_use]
pub fn huffman_encoded_len(data: &[u8]) -> usize {
    let bits: usize = data
        .iter()
        .map(|&b| usize::from(HUFFMAN[b as usize].1))
        .sum();
    bits.div_ceil(8)
}

/// Appends `data` Huffman-coded, padded with the EOS prefix (ones).
pub fn huffman_encode(data: &[u8], out: &mut Vec<u8>) {
    let mut acc: u64 = 0;
    let mut bits: u32 = 0;
    for &b in data {
        let (code, len) = HUFFMAN[b as usize];
        acc = (acc << len) | u64::from(code);
        bits += u32::from(len);
        while bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        // Only the bits not yet written are kept.
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        let pad = 8 - bits;
        out.push(((acc << pad) | ((1 << pad) - 1)) as u8);
    }
}
