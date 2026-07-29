//! URLPattern constructor strings: the path-to-regexp dialect's lexer and the
//! split across components.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|pattern: &str| {
    es_runtime::fuzz::urlpattern(pattern);
});
