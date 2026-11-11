//! HPACK against RFC 7541's own examples, and the decoder's bounds.

use super::rfc_vectors::VECTORS;
use super::*;

fn decode_all(d: &mut Decoder, block: &[u8]) -> Result<Vec<(String, String)>, HpackError> {
    let mut out = Vec::new();
    d.decode(block, usize::MAX, &mut out)?;
    Ok(out
        .into_iter()
        .map(|(n, v)| (String::from_utf8(n).unwrap(), String::from_utf8(v).unwrap()))
        .collect())
}

#[test]
fn appendix_c_examples_decode_to_their_header_lists_and_tables() {
    // Each section (C.3 … C.6) is one connection: its examples share a decoder.
    let mut decoder = None;
    let mut section = "";
    for v in VECTORS {
        if &v.name[..3] != section {
            section = &v.name[..3];
            decoder = Some(Decoder::new(v.max_table));
        }
        let d = decoder.as_mut().unwrap();
        let headers = decode_all(d, v.encoded).unwrap_or_else(|e| panic!("{}: {e}", v.name));
        let want: Vec<(String, String)> = v
            .headers
            .iter()
            .map(|(n, x)| (n.to_string(), x.to_string()))
            .collect();
        assert_eq!(headers, want, "{}", v.name);
        assert_eq!(d.table_size(), v.table_size, "{} table size", v.name);
        let table: Vec<(String, String)> = d
            .table
            .entries
            .iter()
            .map(|(n, x)| {
                (
                    String::from_utf8_lossy(n).into(),
                    String::from_utf8_lossy(x).into(),
                )
            })
            .collect();
        let want: Vec<(String, String)> = v
            .table
            .iter()
            .map(|(n, x)| (n.to_string(), x.to_string()))
            .collect();
        assert_eq!(table, want, "{} table", v.name);
    }
}

#[test]
fn appendix_c1_integers() {
    let mut out = Vec::new();
    encode_integer(&mut out, 10, 5, 0);
    assert_eq!(out, [0x0a]);
    out.clear();
    encode_integer(&mut out, 1337, 5, 0);
    assert_eq!(out, [0x1f, 0x9a, 0x0a]);
    out.clear();
    encode_integer(&mut out, 42, 8, 0);
    assert_eq!(out, [0x2a]);
    assert_eq!(decode_integer(&[0x1f, 0x9a, 0x0a], 5), Ok((1337, 3)));
    assert_eq!(decode_integer(&[0x1f, 0x9a], 5), Err(HpackError::Truncated));
    assert_eq!(
        decode_integer(&[0x1f, 0xff, 0xff, 0xff, 0xff, 0x0f], 5),
        Err(HpackError::IntegerOverflow)
    );
}

#[test]
fn every_byte_round_trips_through_huffman() {
    let all: Vec<u8> = (0..=255u8).collect();
    let mut coded = Vec::new();
    huffman_encode(&all, &mut coded);
    assert_eq!(coded.len(), huffman_encoded_len(&all));
    let mut back = Vec::new();
    huffman_decode(&coded, &mut back).unwrap();
    assert_eq!(back, all);
    // RFC 7541 C.4.1's value.
    coded.clear();
    huffman_encode(b"www.example.com", &mut coded);
    assert_eq!(
        coded,
        [
            0xf1, 0xe3, 0xc2, 0xe5, 0xf2, 0x3a, 0x6b, 0xa0, 0xab, 0x90, 0xf4, 0xff
        ]
    );
}

#[test]
fn bad_huffman_strings_are_refused() {
    let mut out = Vec::new();
    // EOS (30 ones) inside the string.
    assert_eq!(
        huffman_decode(&[0xff, 0xff, 0xff, 0xfc], &mut out),
        Err(HpackError::InvalidHuffman)
    );
    // Padding of 8 or more ones: a whole byte of padding.
    let mut coded = Vec::new();
    huffman_encode(b"a", &mut coded); // 5 bits + 3 bits of padding
    coded.push(0xff);
    assert_eq!(
        huffman_decode(&coded, &mut out),
        Err(HpackError::InvalidHuffman)
    );
    // Padding that is not all ones: 'a' (00011) padded with zeros.
    assert_eq!(
        huffman_decode(&[0b0001_1000], &mut out),
        Err(HpackError::InvalidHuffman)
    );
}

#[test]
fn a_table_size_update_must_come_first_and_stay_within_the_limit() {
    let mut d = Decoder::new(4096);
    // Indexed :method GET, then a size update: refused.
    assert_eq!(
        decode_all(&mut d, &[0x82, 0x3f, 0xe1, 0x1f]),
        Err(HpackError::InvalidTableSizeUpdate)
    );
    let mut d = Decoder::new(256);
    // An update to 4096 when we allowed 256.
    assert_eq!(
        decode_all(&mut d, &[0x3f, 0xe1, 0x1f]),
        Err(HpackError::InvalidTableSizeUpdate)
    );
    // An update to 0 first, then a field: fine.
    assert_eq!(
        decode_all(&mut d, &[0x20, 0x82]).unwrap(),
        [(":method".into(), "GET".into())]
    );
}

#[test]
fn indexes_outside_the_tables_and_truncated_literals_are_refused() {
    let mut d = Decoder::new(4096);
    assert_eq!(decode_all(&mut d, &[0x80]), Err(HpackError::InvalidIndex));
    assert_eq!(decode_all(&mut d, &[0xbe]), Err(HpackError::InvalidIndex)); // 62, empty table
    // A literal claiming 10 bytes of value with 2 present.
    assert_eq!(
        decode_all(&mut d, &[0x04, 0x0a, b'a', b'b']),
        Err(HpackError::Truncated)
    );
}

/// A small block that indexes one large entry many times stops being
/// collected at the limit, but is still decoded to its end.
#[test]
fn a_header_list_past_its_limit_stops_growing() {
    let mut d = Decoder::new(4096);
    let mut block = Vec::new();
    // Literal with incremental indexing: new name "x", a 1000-byte value.
    block.extend_from_slice(&[0x40, 0x01, b'x']);
    encode_integer(&mut block, 1000, 7, 0);
    block.extend(std::iter::repeat_n(b'v', 1000));
    // Then reference it (index 62) 1000 times: 1000 bytes of block, ~1 MB of list.
    block.extend(std::iter::repeat_n(0xbe, 1000));
    let mut out = Vec::new();
    let decoded = d.decode(&block, 16 * 1024, &mut out).unwrap();
    assert!(decoded.too_large);
    assert!(out.len() < 20, "collected {}", out.len());
    // The table still saw the insert.
    assert_eq!(d.table_size(), 1 + 1000 + 32);
}

#[test]
fn what_the_encoder_writes_the_decoder_reads_back_with_tables_in_step() {
    let mut e = Encoder::new(4096);
    let mut d = Decoder::new(4096);
    let lists: [&[(&[u8], &[u8])]; 3] = [
        &[
            (b":method", b"GET"),
            (b":path", b"/"),
            (b"user-agent", b"esrun"),
            (b"authorization", b"secret"),
        ],
        &[
            (b":method", b"GET"),
            (b":path", b"/next"),
            (b"user-agent", b"esrun"),
            (b"x-big", &[b'z'; 5000]),
        ],
        &[
            (b":status", b"200"),
            (b"content-type", b"text/plain"),
            (b"user-agent", b"esrun"),
        ],
    ];
    for list in lists {
        let mut block = Vec::new();
        e.encode(list.iter().copied(), &mut block);
        let mut out = Vec::new();
        d.decode(&block, usize::MAX, &mut out).unwrap();
        let back: Vec<(&[u8], &[u8])> = out
            .iter()
            .map(|(n, v)| (n.as_slice(), v.as_slice()))
            .collect();
        assert_eq!(back, list);
        assert_eq!(e.table.size, d.table_size());
    }
    // The credential never entered the table.
    assert!(!e.table.entries.iter().any(|(n, _)| n == b"authorization"));
    // A smaller table is signalled first in the next block, and both sides agree.
    e.set_max_table_size(100);
    let mut block = Vec::new();
    e.encode([(&b"user-agent"[..], &b"esrun"[..])], &mut block);
    let mut out = Vec::new();
    d.decode(&block, usize::MAX, &mut out).unwrap();
    assert_eq!(e.table.size, d.table_size());
}
