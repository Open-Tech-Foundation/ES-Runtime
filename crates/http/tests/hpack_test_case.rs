//! hpack-test-case: header sequences encoded by independent HPACK encoders,
//! decoded by ours (crates/http/tests/corpus/hpack-test-case, and the whole
//! corpus when scripts/fetch-hpack-test-case.sh has fetched it).
//!
//! Each story is one connection: its cases share a decoder, in order, and a
//! case may change the table size the encoder was allowed.

// The summary line is the point of running this by hand.
#![allow(clippy::print_stderr)]

use std::path::Path;

use es_runtime_http::hpack::{Decoder, Encoder};

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Decodes every story under `root`; returns (stories, cases) checked.
fn run(root: &Path) -> (usize, usize) {
    let mut stories = 0;
    let mut cases = 0;
    let mut encoders: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    encoders.sort();
    for encoder in encoders.iter().filter(|p| p.is_dir()) {
        let mut files: Vec<_> = std::fs::read_dir(encoder)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for file in files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
        {
            let story: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
            let mut decoder = Decoder::new(4096);
            for case in story["cases"].as_array().unwrap() {
                let at = format!("{} case {}", file.display(), case["seqno"]);
                if let Some(size) = case.get("header_table_size").and_then(|v| v.as_u64()) {
                    decoder.set_allowed_table_size(size as usize);
                }
                // `raw-data` holds the inputs the encoders were given: header
                // lists with nothing encoded to decode.
                let Some(wire) = case.get("wire").and_then(|w| w.as_str()) else {
                    continue;
                };
                let wire = hex(wire);
                let mut out = Vec::new();
                decoder
                    .decode(&wire, usize::MAX, &mut out)
                    .unwrap_or_else(|e| panic!("{at}: {e}"));
                let want: Vec<(Vec<u8>, Vec<u8>)> = case["headers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        let (n, v) = h.as_object().unwrap().iter().next().unwrap();
                        (
                            n.as_bytes().to_vec(),
                            v.as_str().unwrap().as_bytes().to_vec(),
                        )
                    })
                    .collect();
                assert_eq!(out, want, "{at}");
                cases += 1;
            }
            stories += 1;
        }
    }
    (stories, cases)
}

#[test]
fn independent_encoders_decode_to_their_header_lists() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let (stories, cases) = run(&corpus.join("hpack-test-case"));
    assert!(stories >= 32, "vendored corpus missing: {stories} stories");
    eprintln!("hpack-test-case (vendored): {stories} stories, {cases} header blocks");
    let full = corpus.join("hpack-test-case-full");
    if full.is_dir() {
        let (stories, cases) = run(&full);
        eprintln!("hpack-test-case (full): {stories} stories, {cases} header blocks");
        let blocks = round_trip(&full.join("raw-data"));
        eprintln!("hpack-test-case (raw-data through our encoder): {blocks} header blocks");
    }
}

/// The corpus's unencoded inputs, through our encoder and back through our
/// decoder, at the default table size and at a small one that forces
/// eviction: the same headers come out and both tables stay the same size.
fn round_trip(raw: &Path) -> usize {
    let mut blocks = 0;
    let mut files: Vec<_> = std::fs::read_dir(raw)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    for table in [4096, 256] {
        for file in &files {
            let story: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
            let mut encoder = Encoder::new(table);
            let mut decoder = Decoder::new(table);
            for case in story["cases"].as_array().unwrap() {
                let list: Vec<(Vec<u8>, Vec<u8>)> = case["headers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|h| {
                        let (n, v) = h.as_object().unwrap().iter().next().unwrap();
                        (
                            n.as_bytes().to_vec(),
                            v.as_str().unwrap().as_bytes().to_vec(),
                        )
                    })
                    .collect();
                let mut block = Vec::new();
                encoder.encode(
                    list.iter().map(|(n, v)| (n.as_slice(), v.as_slice())),
                    &mut block,
                );
                let mut out = Vec::new();
                decoder.decode(&block, usize::MAX, &mut out).unwrap();
                assert_eq!(out, list, "{} at table size {table}", file.display());
                blocks += 1;
            }
        }
    }
    blocks
}
