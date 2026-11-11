//! The chunked-body decoder (crates/http, D217): fed all at once or one byte
//! at a time, it must produce the same data, stop at the same place, and
//! refuse the same inputs.
#![no_main]

use es_runtime_http::h1::{Chunk, ChunkedDecoder, Limits};
use libfuzzer_sys::fuzz_target;

#[derive(Debug, PartialEq)]
enum Outcome {
    End { data: Vec<u8>, used: usize, trailers: Vec<(Vec<u8>, Vec<u8>)> },
    NeedMore { data: Vec<u8> },
    Refused,
}

fn run(input: &[u8], step: usize) -> Outcome {
    let limits = Limits {
        max_head_bytes: 2048,
        max_fields: 16,
        max_target_bytes: 1024,
    };
    let mut d = ChunkedDecoder::new(limits);
    let mut data = Vec::new();
    let mut at = 0;
    let mut available = step.min(input.len());
    loop {
        match d.decode(&input[at..available]) {
            Err(_) => return Outcome::Refused,
            Ok((used, chunk)) => {
                assert!(at + used <= available);
                at += used;
                match chunk {
                    Chunk::Data(bytes) => data.extend_from_slice(bytes),
                    Chunk::End => {
                        let trailers = d.trailers().map(|(n, v)| (n.to_vec(), v.to_vec())).collect();
                        return Outcome::End { data, used: at, trailers };
                    }
                    Chunk::NeedMore => {
                        if available == input.len() {
                            return Outcome::NeedMore { data };
                        }
                        available = (available + step).min(input.len());
                    }
                }
            }
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let whole = run(data, data.len().max(1));
    let bytewise = run(data, 1);
    assert_eq!(whole, bytewise);
});
