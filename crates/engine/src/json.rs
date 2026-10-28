//! `__json_parse(text, dialect)` and `__json_validate(text, dialect)`: JSONC
//! and JSON5 (DECISIONS D191), `dialect` being `"jsonc"` or `"json5"`.
//!
//! # Why this lives in `engine`
//!
//! For the reason BSON's decoder does (D187): an op returns a marshaled
//! [`Value`](crate::Value) tree, which the engine then rebuilds as JavaScript
//! objects. Here `es_runtime_json`'s parser reports each value to a sink that
//! builds the JavaScript value at once, so the text becomes objects with no
//! tree in between, as `JSON.parse` does.
//!
//! # What it builds
//!
//! What `JSON.parse` builds: plain objects and arrays, each object's
//! properties *defined* rather than assigned, so a `__proto__` key is an own
//! property and never sets the prototype, and a repeated key keeps its first
//! place and its last value. Objects stay in V8's fast mode and share shapes,
//! so reading them later is as fast as reading `JSON.parse`'s. `reviver` is
//! applied in JavaScript afterwards.

use es_runtime_common::ExceptionClass;
use es_runtime_json::{Dialect, ErrorKind, Failure, Sink, Text};

use crate::convert::throw;
use crate::error::Result as EngineResult;
use crate::op::OpError;

pub(crate) fn install(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
) -> EngineResult<()> {
    let global = context.global(scope);
    crate::op::install_global_fn(scope, global, "__json_parse", json_parse, None)?;
    crate::op::install_global_fn(scope, global, "__json_validate", json_validate, None)
}

pub(crate) fn external_references() -> Vec<v8::ExternalReference> {
    use v8::MapFnTo;
    vec![
        v8::ExternalReference {
            function: json_parse.map_fn_to(),
        },
        v8::ExternalReference {
            function: json_validate.map_fn_to(),
        },
    ]
}

/// The text and the dialect, or a thrown `TypeError`.
fn arguments(
    scope: &mut v8::PinScope<'_, '_>,
    args: &v8::FunctionCallbackArguments<'_>,
) -> Option<(String, Dialect)> {
    let dialect = args.get(1);
    let dialect = match dialect
        .is_string()
        .then(|| dialect.to_rust_string_lossy(scope))
    {
        Some(name) if name == "jsonc" => Dialect::Jsonc,
        Some(name) if name == "json5" => Dialect::Json5,
        _ => {
            throw(
                scope,
                &OpError::type_error("the dialect is \"jsonc\" or \"json5\""),
            );
            return None;
        }
    };
    let text = args.get(0);
    if !text.is_string() {
        throw(scope, &OpError::type_error("the text must be a string"));
        return None;
    }
    // The wrapper escapes lone surrogates first, so nothing is lost here.
    Some((text.to_rust_string_lossy(scope), dialect))
}

fn name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Jsonc => "JSONC",
        Dialect::Json5 => "JSON5",
    }
}

fn json_parse<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    // A panic must not unwind into V8 (D15).
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        json_parse_inner(&mut *scope, &args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in __json_parse"),
        );
    }
}

fn json_parse_inner<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some((text, dialect)) = arguments(scope, args) else {
        return;
    };
    let mut build = Build {
        scope,
        stack: Vec::new(),
        root: None,
    };
    match es_runtime_json::parse(&text, dialect, &mut build) {
        Ok(()) => {
            let root = build.root.expect("a parse that succeeds reports a value");
            rv.set(root);
        }
        // V8 refused to make a value; its exception is already pending.
        Err(Failure::Abort(())) => {}
        Err(Failure::Error(e)) => {
            let class = match e.kind {
                ErrorKind::Syntax => ExceptionClass::SyntaxError,
                ErrorKind::Depth => ExceptionClass::RangeError,
            };
            throw(
                build.scope,
                &OpError::new(class, format!("{}: {e}", name(dialect))),
            );
        }
    }
}

fn json_validate<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (text, dialect) = arguments(&mut *scope, &args)?;
        Some(
            es_runtime_json::validate(&text, dialect)
                .map_err(|e| format!("{}: {e}", name(dialect))),
        )
    }));
    match caught {
        Ok(None) => {}
        Ok(Some(Ok(()))) => rv.set(v8::Boolean::new(scope, true).into()),
        Ok(Some(Err(message))) => {
            if let Some(message) = v8::String::new(scope, &message) {
                rv.set(message.into());
            }
        }
        Err(_) if scope.is_execution_terminating() => {}
        Err(_) => throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in __json_validate"),
        ),
    }
}

/// A container being filled: an array's values wait for it to close, and an
/// object takes each property as it is read, with the key that precedes it.
enum Frame<'s> {
    Array(Vec<v8::Local<'s, v8::Value>>),
    Object(v8::Local<'s, v8::Object>, Option<v8::Local<'s, v8::Name>>),
}

struct Build<'s, 'i, 'p> {
    scope: &'i mut v8::PinScope<'s, 'p>,
    stack: Vec<Frame<'s>>,
    root: Option<v8::Local<'s, v8::Value>>,
}

impl<'s> Build<'s, '_, '_> {
    fn put(&mut self, value: v8::Local<'s, v8::Value>) -> Result<(), ()> {
        match self.stack.last_mut() {
            Some(Frame::Array(values)) => values.push(value),
            Some(Frame::Object(object, key)) => {
                let key = key.take().expect("a value in an object follows its key");
                // Defined, not assigned, as `JSON.parse` does: a `__proto__`
                // key becomes an own property, and a repeated key keeps its
                // first place and its last value.
                object
                    .create_data_property(self.scope, key, value)
                    .ok_or(())?;
            }
            None => self.root = Some(value),
        }
        Ok(())
    }

    fn text(
        &mut self,
        text: &Text<'_>,
        kind: v8::NewStringType,
    ) -> Result<v8::Local<'s, v8::String>, ()> {
        let made = match text {
            // ASCII is Latin-1, so V8 takes it as a one-byte string with no decoding.
            Text::Str(s) if s.is_ascii() => {
                v8::String::new_from_one_byte(self.scope, s.as_bytes(), kind)
            }
            Text::Str(s) => v8::String::new_from_utf8(self.scope, s.as_bytes(), kind),
            Text::Utf16(units) => v8::String::new_from_two_byte(self.scope, units, kind),
        };
        made.ok_or(())
    }
}

impl<'s> Sink for Build<'s, '_, '_> {
    type Abort = ();

    fn null(&mut self) -> Result<(), ()> {
        let null = v8::null(self.scope).into();
        self.put(null)
    }

    fn bool(&mut self, value: bool) -> Result<(), ()> {
        let value = v8::Boolean::new(self.scope, value).into();
        self.put(value)
    }

    fn number(&mut self, value: f64) -> Result<(), ()> {
        let value = v8::Number::new(self.scope, value).into();
        self.put(value)
    }

    fn string(&mut self, value: Text<'_>) -> Result<(), ()> {
        let value = self.text(&value, v8::NewStringType::Normal)?.into();
        self.put(value)
    }

    fn begin_array(&mut self) -> Result<(), ()> {
        self.stack.push(Frame::Array(Vec::new()));
        Ok(())
    }

    fn end_array(&mut self) -> Result<(), ()> {
        let Some(Frame::Array(values)) = self.stack.pop() else {
            unreachable!("the parser closes what it opened");
        };
        let array = v8::Array::new_with_elements(self.scope, &values).into();
        self.put(array)
    }

    fn begin_object(&mut self) -> Result<(), ()> {
        // An empty object given its properties one at a time stays in fast
        // mode, and objects with the same keys share a shape, as
        // `JSON.parse`'s do. One made with all its properties at once is a
        // dictionary, several times slower to read from.
        let object = v8::Object::new(self.scope);
        self.stack.push(Frame::Object(object, None));
        Ok(())
    }

    fn key(&mut self, key: Text<'_>) -> Result<(), ()> {
        // Internalized, as property names are: one copy per distinct key.
        let name = self.text(&key, v8::NewStringType::Internalized)?.into();
        match self.stack.last_mut() {
            Some(Frame::Object(_, key)) => *key = Some(name),
            _ => unreachable!("a key is reported inside an object"),
        }
        Ok(())
    }

    fn end_object(&mut self) -> Result<(), ()> {
        let Some(Frame::Object(object, _)) = self.stack.pop() else {
            unreachable!("the parser closes what it opened");
        };
        self.put(object.into())
    }
}
