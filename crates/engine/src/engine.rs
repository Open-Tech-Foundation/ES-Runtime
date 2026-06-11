//! Isolate + context lifecycle and script evaluation.

use es_runtime_common::Limits;

use crate::convert::{describe_exception, marshal};
use crate::error::{Error, Result};
use crate::value::Value;

/// An embedded V8 instance: one isolate and one persistent context.
///
/// Created with [`Engine::new`] (fresh context) or [`Engine::with_snapshot`]
/// (context restored from a startup snapshot). [`Engine::eval`] runs source
/// against that context; state persists across calls, since the context is held
/// for the engine's lifetime rather than rebuilt per evaluation.
///
/// The isolate is `!Send`/`!Sync` (V8's threading model): an `Engine` is driven
/// by a single thread, which is exactly the embedder's drive model
/// (ARCHITECTURE.md §5).
pub struct Engine {
    isolate: v8::OwnedIsolate,
    /// The persistent context evaluations run in, held across the isolate's life.
    context: v8::Global<v8::Context>,
}

impl Engine {
    /// Creates an engine with a fresh, empty context.
    ///
    /// `limits` are validated up front ([`Limits::validate`]); the heap ceiling
    /// is installed on the isolate so V8 enforces it (ARCHITECTURE.md §7). The
    /// near-limit graceful-termination callback is a later hardening item
    /// (Phase 9) — here the cap simply exists.
    pub fn new(limits: Limits) -> Result<Self> {
        limits.validate()?;
        crate::ensure_v8_initialized();

        let params = v8::CreateParams::default().heap_limits(0, limits.heap_limit_bytes);
        let mut isolate = v8::Isolate::new(params);
        let context = Self::make_context(&mut isolate);

        Ok(Engine { isolate, context })
    }

    /// Restores an engine from a startup snapshot built by
    /// [`snapshot::build`](crate::snapshot::build).
    ///
    /// With a snapshot blob installed, the new context is deserialized from the
    /// snapshot's default context — including any prelude baked into it
    /// (DECISIONS.md D8) — so global state captured at build time is present
    /// immediately.
    pub fn with_snapshot(limits: Limits, snapshot: Vec<u8>) -> Result<Self> {
        limits.validate()?;
        crate::ensure_v8_initialized();

        let params = v8::CreateParams::default()
            .heap_limits(0, limits.heap_limit_bytes)
            .snapshot_blob(snapshot.into());
        let mut isolate = v8::Isolate::new(params);
        let context = Self::make_context(&mut isolate);

        Ok(Engine { isolate, context })
    }

    /// Builds a context in `isolate` and globalizes a handle to it. When the
    /// isolate was created with a snapshot blob, this restores the snapshot's
    /// default context rather than an empty one.
    fn make_context(isolate: &mut v8::OwnedIsolate) -> v8::Global<v8::Context> {
        v8::scope!(let scope, isolate);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        v8::Global::new(scope, context)
    }

    /// Compiles and runs `source` in the engine's context, returning the
    /// marshaled result.
    ///
    /// Errors are typed (DECISIONS.md D12): a compile failure is
    /// [`Error::Compile`]; an uncaught JS exception is [`Error::Execution`].
    /// Both are caught via a V8 `TryCatch`, so an exception in evaluated code is
    /// surfaced as a Rust `Err`, never an unwind across the boundary.
    pub fn eval(&mut self, source: &str) -> Result<Value> {
        v8::scope!(let scope, &mut self.isolate);
        let context = v8::Local::new(scope, &self.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);

        let Some(code) = v8::String::new(scope, source) else {
            return Err(Error::Internal(
                "source string exceeds V8's maximum length".into(),
            ));
        };

        let Some(script) = v8::Script::compile(scope, code, None) else {
            return Err(Error::Compile {
                message: describe_exception(scope, "compilation failed"),
            });
        };

        let Some(result) = script.run(scope) else {
            return Err(Error::Execution {
                message: describe_exception(scope, "execution failed"),
            });
        };

        Ok(marshal(scope, result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(Limits::default()).expect("engine construction")
    }

    #[test]
    fn evaluates_one_plus_one() {
        // The Phase 1 end-to-end acceptance check (SPEC.md §6.1).
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let result = engine.eval("1 + 1").expect("eval");
        assert_eq!(result, Value::Number(2.0));
    }

    #[test]
    fn marshals_primitive_kinds() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        assert_eq!(engine.eval("undefined").unwrap(), Value::Undefined);
        assert_eq!(engine.eval("null").unwrap(), Value::Null);
        assert_eq!(engine.eval("true").unwrap(), Value::Bool(true));
        assert_eq!(engine.eval("2 + 3").unwrap(), Value::Number(5.0));
        assert_eq!(
            engine.eval("'a' + 'b'").unwrap(),
            Value::String("ab".into())
        );
    }

    #[test]
    fn non_primitive_falls_back_to_other() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        match engine.eval("({})").unwrap() {
            Value::Other(s) => assert_eq!(s, "[object Object]"),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn context_state_persists_across_evals() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        engine.eval("globalThis.counter = 41").unwrap();
        assert_eq!(
            engine.eval("globalThis.counter + 1").unwrap(),
            Value::Number(42.0)
        );
    }

    #[test]
    fn syntax_error_is_typed_compile_error() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let err = engine.eval("function (").unwrap_err();
        assert!(matches!(err, Error::Compile { .. }), "got {err:?}");
    }

    #[test]
    fn thrown_exception_is_typed_execution_error() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let err = engine.eval("throw new Error('boom')").unwrap_err();
        match err {
            Error::Execution { message } => assert!(message.contains("boom"), "{message}"),
            other => panic!("expected Execution, got {other:?}"),
        }
    }

    #[test]
    fn invalid_limits_rejected_before_v8() {
        let bad = Limits::default().with_heap_limit_bytes(0);
        assert!(matches!(Engine::new(bad), Err(Error::Common(_))));
    }
}
