//! The PostgreSQL, MySQL and MongoDB drivers, answered by the input: every
//! read a driver makes hands it the next piece, so the handshake,
//! authentication, framing and value decoding all meet what a hostile or
//! broken server could send (es_runtime::script_fuzz). Seeds are sessions
//! recorded from real servers (fuzz/record-db).
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
        let fuzzer =
            fuzzer.get_or_insert_with(|| Box::leak(Box::new(ScriptFuzzer::new(Harness::DbWire))));
        let found = fuzzer.run(data);
        assert!(found.is_empty(), "{found:?}");
    });
});
