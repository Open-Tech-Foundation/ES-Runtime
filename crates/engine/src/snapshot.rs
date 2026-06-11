//! Startup-snapshot build/load scaffolding (DECISIONS.md D8, ARCHITECTURE.md §9).
//!
//! Baking the pure-JS prelude into a V8 startup snapshot makes context creation
//! cheap, which directly serves Layer B's density goals. Phase 1 stands up the
//! mechanism end-to-end — build a blob (optionally running prelude source into
//! its default context) and restore a [`V8Engine`](crate::V8Engine) from it via
//! [`V8Engine::with_snapshot`](crate::V8Engine::with_snapshot). The actual
//! min-common prelude is authored and baked in Phase 8; here the prelude is a
//! caller-supplied parameter so the round-trip is real and testable.

use crate::convert::describe_exception;
use crate::error::{Error, Result};

/// Builds a V8 startup-snapshot blob.
///
/// If `prelude` is `Some`, the source is evaluated into the snapshot's default
/// context before capture, so its global side effects are present when an engine
/// is later restored from the blob. A `None` prelude captures a clean default
/// context.
///
/// Returns the serialized blob, suitable for
/// [`V8Engine::with_snapshot`](crate::V8Engine::with_snapshot).
///
/// # Concurrency
///
/// V8 does not permit snapshot creation to run concurrently with other isolate
/// creation in the same process. Callers must not invoke `build` while another
/// thread is constructing an isolate (e.g. a [`V8Engine`](crate::V8Engine)). In a
/// typical embedding the snapshot is built once at startup, before any isolate
/// exists, so this is naturally satisfied. (Recorded as a D3a leak note.)
pub fn build(prelude: Option<&str>) -> Result<Vec<u8>> {
    crate::ensure_v8_initialized();

    // A snapshot-creator isolate. V8 *requires* `create_blob` be called on it
    // before it drops, so we must reach that call even when the prelude fails —
    // hence the prelude outcome is captured and surfaced only afterward.
    let mut creator = v8::Isolate::snapshot_creator(None, None);
    let prelude_result = {
        v8::scope!(let scope, &mut creator);
        let context = v8::Context::new(scope, v8::ContextOptions::default());
        let scope = &mut v8::ContextScope::new(scope, context);

        let result = match prelude {
            Some(source) => run_prelude(scope, source),
            None => Ok(()),
        };

        // Mark this context as the default (index 0), restored on load. Done
        // even on prelude failure so the required `create_blob` has a context.
        scope.set_default_context(context);
        result
    };

    // Always consume the creator (V8 invariant), then honor a prelude failure.
    let blob = creator.create_blob(v8::FunctionCodeHandling::Keep);
    prelude_result?;
    blob.map(|data| data.to_vec())
        .ok_or_else(|| Error::Internal("V8 returned no snapshot blob".into()))
}

/// Compiles and runs prelude `source` in `scope`, mapping failures to typed
/// engine errors. Prelude faults are build-time bugs, surfaced loudly.
fn run_prelude(scope: &mut v8::PinScope<'_, '_>, source: &str) -> Result<()> {
    v8::tc_scope!(let scope, scope);

    let Some(code) = v8::String::new(scope, source) else {
        return Err(Error::Internal(
            "prelude source exceeds V8's maximum length".into(),
        ));
    };

    let Some(script) = v8::Script::compile(scope, code, None) else {
        return Err(Error::Compile {
            message: describe_exception(scope, "prelude compilation failed"),
        });
    };

    if script.run(scope).is_none() {
        return Err(Error::Execution {
            message: describe_exception(scope, "prelude execution failed"),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;
    use crate::{Engine, V8Engine};
    use es_runtime_common::Limits;

    #[test]
    fn builds_a_non_empty_blob() {
        let _v8 = crate::v8_test_guard();
        let blob = build(None).expect("snapshot build");
        assert!(!blob.is_empty(), "snapshot blob should carry data");
    }

    #[test]
    fn prelude_state_survives_round_trip() {
        // Build a snapshot whose default context already ran the prelude, then
        // restore an engine from it and observe the baked-in global state. This
        // is the end-to-end proof the snapshot pipeline works (DECISIONS.md D8).
        let _v8 = crate::v8_test_guard();
        let blob = build(Some("globalThis.marker = 40 + 2;")).expect("build");
        let mut engine = V8Engine::with_snapshot(Limits::default(), blob).expect("restore");
        assert_eq!(
            engine.eval("globalThis.marker").unwrap(),
            Value::Number(42.0)
        );
    }

    #[test]
    fn prelude_syntax_error_is_reported() {
        let _v8 = crate::v8_test_guard();
        let err = build(Some("function (")).unwrap_err();
        assert!(matches!(err, Error::Compile { .. }), "got {err:?}");
    }
}
