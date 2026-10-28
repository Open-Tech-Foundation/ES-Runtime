//! Parses arbitrary text as JSONC and JSON5 (DECISIONS D191). Nesting is
//! bounded, strings carry escapes and lone surrogates, and a hex literal can
//! be any length. JSONC is a subset of JSON5, so every text JSONC reads must
//! read the same in JSON5; and `validate` must agree with `parse`.
#![no_main]

use es_runtime_json::{Dialect, Failure, Sink, Text, parse, validate};
use libfuzzer_sys::fuzz_target;

/// Every event, as text, so two parses can be compared.
#[derive(Default)]
struct Events(Vec<String>);

impl Sink for Events {
    type Abort = ();
    fn null(&mut self) -> Result<(), ()> {
        self.0.push("null".into());
        Ok(())
    }
    fn bool(&mut self, value: bool) -> Result<(), ()> {
        self.0.push(value.to_string());
        Ok(())
    }
    fn number(&mut self, value: f64) -> Result<(), ()> {
        self.0.push(format!("n{:x}", value.to_bits()));
        Ok(())
    }
    fn string(&mut self, value: Text<'_>) -> Result<(), ()> {
        self.0.push(format!("s{value:?}"));
        Ok(())
    }
    fn begin_array(&mut self) -> Result<(), ()> {
        self.0.push("[".into());
        Ok(())
    }
    fn end_array(&mut self) -> Result<(), ()> {
        self.0.push("]".into());
        Ok(())
    }
    fn begin_object(&mut self) -> Result<(), ()> {
        self.0.push("{".into());
        Ok(())
    }
    fn key(&mut self, key: Text<'_>) -> Result<(), ()> {
        self.0.push(format!("k{key:?}"));
        Ok(())
    }
    fn end_object(&mut self) -> Result<(), ()> {
        self.0.push("}".into());
        Ok(())
    }
}

fn read(text: &str, dialect: Dialect) -> Option<Vec<String>> {
    let mut events = Events::default();
    let parsed = match parse(text, dialect, &mut events) {
        Ok(()) => Some(events.0),
        Err(Failure::Error(error)) => {
            assert!(error.offset <= text.len());
            None
        }
        Err(Failure::Abort(())) => unreachable!("the sink never stops"),
    };
    assert_eq!(parsed.is_some(), validate(text, dialect).is_ok());
    parsed
}

fuzz_target!(|input: &[u8]| {
    let Ok(text) = std::str::from_utf8(input) else {
        return;
    };
    let json5 = read(text, Dialect::Json5);
    if let Some(jsonc) = read(text, Dialect::Jsonc) {
        assert_eq!(Some(jsonc), json5, "JSONC read what JSON5 did not");
    }
});
