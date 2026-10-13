//! The runtime's YAML reader, and a write-and-reread of whatever it accepts:
//! an indentation-sensitive grammar read by hand, where a mis-tracked column or
//! a writer that quotes too little are the interesting failures.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &str| {
    es_runtime::fuzz::parse_yaml(input);
});
