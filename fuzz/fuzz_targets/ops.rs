//! Every op, called with whatever arguments the input describes (D253).
//!
//! The other targets reach one parser each. This one drives the op boundary
//! itself — the marshaler, the typed-argument readers, and each handler's
//! treatment of what it is given — through a live runtime that serves many
//! inputs (`es_runtime::op_fuzz`). A Rust panic anywhere under an op is the
//! finding: libFuzzer's panic hook aborts on it, before the engine can turn it
//! into a JS error, so the crash points at the handler.
#![no_main]

use std::cell::RefCell;

use es_runtime::op_fuzz::OpFuzzer;
use libfuzzer_sys::fuzz_target;

thread_local! {
    // Leaked, never dropped: a thread-local destructor runs while the
    // thread's other thread-locals — V8's and the engine's among them — are
    // being torn down, and dropping an isolate then panics at exit.
    static FUZZER: RefCell<Option<&'static mut OpFuzzer>> = const { RefCell::new(None) };
}

fuzz_target!(|data: &[u8]| {
    FUZZER.with_borrow_mut(|fuzzer| {
        let fuzzer = fuzzer.get_or_insert_with(|| Box::leak(Box::new(OpFuzzer::new())));
        let found = fuzzer.call(data);
        assert!(found.is_empty(), "an op panicked: {found:?}");
    });
});
