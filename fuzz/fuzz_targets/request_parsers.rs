//! What a server parses out of a request, in JavaScript: `formData()` in
//! both encodings, `Headers`, cookies, `URLSearchParams` and a body's
//! content type (es_runtime::script_fuzz). cargo-fuzz reaches Rust; this
//! reaches the runtime's own JavaScript.
//!
//! A Rust panic is a finding (libFuzzer's hook aborts on it), and so is an
//! error the code throws only when it has itself gone wrong.
#![no_main]

use std::cell::RefCell;

use es_runtime::script_fuzz::{Harness, ScriptFuzzer};
use libfuzzer_sys::fuzz_target;

thread_local! {
    // Leaked, never dropped, as in the ops target: V8 must not be torn down by
    // a thread-local destructor.
    static FUZZER: RefCell<Option<&'static mut ScriptFuzzer>> = const { RefCell::new(None) };
}

fuzz_target!(|data: &[u8]| {
    FUZZER.with_borrow_mut(|fuzzer| {
        let fuzzer = fuzzer
            .get_or_insert_with(|| Box::leak(Box::new(ScriptFuzzer::new(Harness::RequestParsers))));
        let found = fuzzer.run(data);
        assert!(found.is_empty(), "{found:?}");
    });
});
