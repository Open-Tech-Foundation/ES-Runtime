//! The HTTP/1.1 response-head parser (crates/http, D217) — the client's half.
//! Beyond not panicking: a head that parses completely is partial at every
//! shorter prefix, its spans lie inside it, and its fields pass the shared
//! validator.
#![no_main]

use es_runtime_http::fields::{is_field_value, is_token};
use es_runtime_http::h1::{Limits, ParsedResponse, parse_response};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (bool, &[u8])| {
    let (to_head, data) = data;
    let limits = Limits {
        max_head_bytes: 4096,
        max_fields: 32,
        max_target_bytes: 1024,
    };
    let mut fields = Vec::new();
    let Ok(ParsedResponse::Complete(head)) = parse_response(data, &limits, &mut fields, to_head)
    else {
        return;
    };
    assert!(head.len <= data.len() && head.reason.end <= head.len);
    assert!((100..=999).contains(&head.status));
    for f in &fields {
        assert!(is_token(f.name.of(data)) && is_field_value(f.value.of(data)));
    }
    for n in 0..head.len {
        assert_eq!(
            parse_response(&data[..n], &limits, &mut fields, to_head),
            Ok(ParsedResponse::Partial),
            "prefix {n}"
        );
    }
});
