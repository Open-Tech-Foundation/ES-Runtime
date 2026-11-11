//! The HTTP/1.1 request-head parser (crates/http, D217).
//!
//! Beyond not panicking, two properties the server's driver relies on:
//! - A head that parses completely parses as *partial* at every shorter
//!   prefix: refusing early never fires on a good head, and nothing completes
//!   before its blank line.
//! - Every span lies inside the head, every field name is a token and every
//!   value passes the shared validator, trimmed.
#![no_main]

use es_runtime_http::fields::{is_field_value, is_token};
use es_runtime_http::h1::{Limits, Parsed, parse_request};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_head_bytes: 4096,
        max_fields: 32,
        max_target_bytes: 1024,
    };
    let mut fields = Vec::new();
    let Ok(Parsed::Complete(head)) = parse_request(data, &limits, &mut fields) else {
        return;
    };
    assert!(head.len <= data.len());
    for span in [head.method, head.target, head.host] {
        assert!(span.end <= head.len);
    }
    for f in &fields {
        assert!(f.name.end <= head.len && f.value.end <= head.len);
        assert!(is_token(f.name.of(data)));
        let v = f.value.of(data);
        assert!(is_field_value(v));
        assert!(!v.starts_with(b" ") && !v.ends_with(b" "));
    }
    let fields_found = fields.clone();
    // Re-parsing the head alone gives the same head.
    assert_eq!(
        parse_request(&data[..head.len], &limits, &mut fields),
        Ok(Parsed::Complete(head))
    );
    assert_eq!(fields, fields_found);
    for n in 0..head.len {
        assert_eq!(
            parse_request(&data[..n], &limits, &mut fields),
            Ok(Parsed::Partial),
            "prefix {n} of a good head"
        );
    }
});
