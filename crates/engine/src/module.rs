//! ES module graph support: compile, instantiate, evaluate — all V8-coupled and
//! therefore confined here, exposed to `runtime` only through the opaque
//! [`ModuleId`] (ARCHITECTURE.md §3, DECISIONS.md D3).
//!
//! V8's module-resolution callback is **synchronous**: when a module is
//! instantiated, V8 calls back for each import specifier and expects an
//! already-compiled module in return. So the async work — reading/fetching each
//! dependency's source — must happen *before* instantiation. `runtime` owns that
//! load phase; it walks the graph by repeatedly [`compile`]ing a source and
//! reading its [`requests`], then hands a fully-resolved id map to
//! [`instantiate`]. The resolve callback here is then a pure registry lookup
//! (ARCHITECTURE.md §5's "resolve through a capability-checked hook").
//!
//! Module evaluation returns a promise (top-level await), tracked like an async
//! op: `runtime` ticks the loop until [`eval_state`] reports the graph settled.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::convert::{describe_exception, js_to_string};
use crate::error::{Error, Result};

/// An opaque handle to a compiled module in the engine's registry. Names no V8
/// type, so it crosses the engine boundary unchanged (DECISIONS.md D3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModuleId(u32);

/// The outcome of the most recently started module evaluation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleEvalState {
    /// Nothing evaluating, or the evaluation promise is still pending (e.g.
    /// top-level await waiting on async ops).
    Pending,
    /// The module graph evaluated to completion.
    Completed,
    /// Evaluation threw / rejected; carries the stringified reason.
    Failed(String),
}

/// Per-isolate module registry, shared with the in-isolate resolve and
/// import-meta callbacks via an isolate slot (mirrors [`OpState`](crate::op)).
pub(crate) struct ModuleRegistry {
    next_id: u32,
    by_id: HashMap<ModuleId, v8::Global<v8::Module>>,
    /// Identity hash → id, for mapping a referrer/`import.meta` module back to
    /// its id. V8 identity hashes are not *guaranteed* unique, but collisions
    /// among the handful of modules in one graph are astronomically unlikely and
    /// at worst yield a resolution mismatch V8 rejects — never unsoundness.
    id_by_hash: HashMap<i32, ModuleId>,
    /// Canonical specifier per module — becomes `import.meta.url`.
    specifier_by_id: HashMap<ModuleId, String>,
    /// `(referrer, raw specifier) → target`, populated only for the duration of
    /// an [`instantiate`] call so the synchronous resolve callback can consult it.
    resolve: HashMap<(ModuleId, String), ModuleId>,
    /// The evaluation promise of the most recent [`evaluate`], if any.
    eval_promise: Option<v8::Global<v8::Promise>>,
}

impl ModuleRegistry {
    pub(crate) fn new() -> Self {
        ModuleRegistry {
            next_id: 0,
            by_id: HashMap::new(),
            id_by_hash: HashMap::new(),
            specifier_by_id: HashMap::new(),
            resolve: HashMap::new(),
            eval_promise: None,
        }
    }

    fn insert(&mut self, module: v8::Global<v8::Module>, hash: i32, specifier: String) -> ModuleId {
        let id = ModuleId(self.next_id);
        self.next_id += 1;
        self.by_id.insert(id, module);
        self.id_by_hash.insert(hash, id);
        self.specifier_by_id.insert(id, specifier);
        id
    }

    fn module(&self, id: ModuleId) -> Result<v8::Global<v8::Module>> {
        self.by_id
            .get(&id)
            .cloned()
            .ok_or_else(|| Error::Internal(format!("unknown module id {}", id.0)))
    }
}

/// Reads the shared registry out of the isolate slot (cloned `Rc` so the access
/// doesn't borrow the isolate the scope already holds).
fn registry(scope: &v8::PinScope<'_, '_>) -> Option<Rc<RefCell<ModuleRegistry>>> {
    scope.get_slot::<Rc<RefCell<ModuleRegistry>>>().cloned()
}

/// Compiles `source` as an ES module named `specifier`, registers it, and
/// returns its id. The specifier is recorded for `import.meta.url` and for the
/// resolve map keyed in [`instantiate`].
pub(crate) fn compile(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    registry: &Rc<RefCell<ModuleRegistry>>,
    interrupt: &v8::IsolateHandle,
    specifier: &str,
    source: &str,
) -> Result<ModuleId> {
    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);
    v8::tc_scope!(let scope, scope);

    let Some(name) = v8::String::new(scope, specifier) else {
        return Err(Error::Internal(
            "module specifier exceeds V8 maximum".into(),
        ));
    };
    let Some(code) = v8::String::new(scope, source) else {
        return Err(Error::Internal(
            "module source exceeds V8's maximum length".into(),
        ));
    };
    // is_module = true; the rest are defaults (no source map, not cross-origin).
    let origin = v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        0,
        false,
        -1,
        None,
        false,
        false,
        true,
        None,
    );
    let mut src = v8::script_compiler::Source::new(code, Some(&origin));

    match v8::script_compiler::compile_module(scope, &mut src) {
        Some(module) => {
            let hash = module.get_identity_hash().get();
            let global = v8::Global::new(scope, module);
            Ok(registry
                .borrow_mut()
                .insert(global, hash, specifier.to_string()))
        }
        None if interrupt.is_execution_terminating() => Err(Error::Terminated {
            reason: "execution terminated".into(),
        }),
        None => Err(Error::Compile {
            message: describe_exception(scope, "module compilation failed"),
        }),
    }
}

/// The import specifiers a compiled module requests, in source order — for
/// `runtime` to resolve and load before instantiation.
pub(crate) fn requests(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    registry: &Rc<RefCell<ModuleRegistry>>,
    id: ModuleId,
) -> Result<Vec<String>> {
    let module_global = registry.borrow().module(id)?;

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);

    let module = v8::Local::new(scope, &module_global);
    let array = module.get_module_requests();
    let mut out = Vec::with_capacity(array.length());
    for i in 0..array.length() {
        let entry = array.get(scope, i).expect("index < length");
        let request = v8::Local::<v8::ModuleRequest>::try_from(entry)
            .expect("module-requests array holds ModuleRequests");
        out.push(request.get_specifier().to_rust_string_lossy(scope));
    }
    Ok(out)
}

/// Instantiates module `id`, resolving each `(referrer, specifier)` through
/// `resolved` (every referenced id must already be compiled). Wires the import
/// graph; runs no user code (that is [`evaluate`]).
pub(crate) fn instantiate(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    registry: &Rc<RefCell<ModuleRegistry>>,
    interrupt: &v8::IsolateHandle,
    id: ModuleId,
    resolved: &HashMap<(ModuleId, String), ModuleId>,
) -> Result<()> {
    let module_global = registry.borrow().module(id)?;
    // Make the resolution map visible to the synchronous resolve callback for
    // the duration of this call only.
    registry.borrow_mut().resolve = resolved.clone();

    let result = {
        v8::scope!(let scope, isolate);
        let context = v8::Local::new(scope, context);
        let scope = &mut v8::ContextScope::new(scope, context);
        v8::tc_scope!(let scope, scope);

        let module = v8::Local::new(scope, &module_global);
        let outcome = module.instantiate_module(scope, resolve_module_callback);
        match outcome {
            Some(true) => Ok(()),
            _ if interrupt.is_execution_terminating() => Err(Error::Terminated {
                reason: "execution terminated".into(),
            }),
            _ => Err(Error::Execution {
                message: describe_exception(scope, "module instantiation failed"),
            }),
        }
    };

    registry.borrow_mut().resolve.clear();
    result
}

/// Begins evaluating instantiated module `id`. Module evaluation returns a
/// promise (top-level await); it is stored and observed via [`eval_state`] as the
/// driven loop settles. A synchronous top-level throw rejects that promise.
pub(crate) fn evaluate(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    registry: &Rc<RefCell<ModuleRegistry>>,
    interrupt: &v8::IsolateHandle,
    id: ModuleId,
) -> Result<()> {
    let module_global = registry.borrow().module(id)?;

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);

    let module = v8::Local::new(scope, &module_global);
    match module.evaluate(scope) {
        Some(value) => {
            let promise = v8::Local::<v8::Promise>::try_from(value)
                .map_err(|_| Error::Internal("module evaluation returned no promise".into()))?;
            registry.borrow_mut().eval_promise = Some(v8::Global::new(scope, promise));
            Ok(())
        }
        None if interrupt.is_execution_terminating() => Err(Error::Terminated {
            reason: "execution terminated".into(),
        }),
        None => Err(Error::Internal("module evaluation failed to start".into())),
    }
}

/// Inspects the most recent evaluation promise: pending, completed, or failed
/// (with the rejection stringified). [`ModuleEvalState::Pending`] when nothing
/// has been evaluated yet.
pub(crate) fn eval_state(
    isolate: &mut v8::OwnedIsolate,
    context: &v8::Global<v8::Context>,
    registry: &Rc<RefCell<ModuleRegistry>>,
) -> ModuleEvalState {
    let Some(promise_global) = registry.borrow().eval_promise.clone() else {
        return ModuleEvalState::Pending;
    };

    v8::scope!(let scope, isolate);
    let context = v8::Local::new(scope, context);
    let scope = &mut v8::ContextScope::new(scope, context);

    let promise = v8::Local::new(scope, &promise_global);
    match promise.state() {
        v8::PromiseState::Pending => ModuleEvalState::Pending,
        v8::PromiseState::Fulfilled => ModuleEvalState::Completed,
        v8::PromiseState::Rejected => {
            let reason = promise.result(scope);
            ModuleEvalState::Failed(js_to_string(scope, reason))
        }
    }
}

/// The synchronous resolve callback V8 invokes during instantiation. Reaches the
/// registry via the isolate slot (the callback type is `UnitType` — it cannot
/// capture), maps the referrer + specifier to a target id through the active
/// resolve map, and returns the already-compiled module. A miss returns `None`,
/// which V8 turns into an instantiation exception.
fn resolve_module_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);

    let registry = registry(scope)?;
    let specifier = specifier.to_rust_string_lossy(scope);
    let referrer_hash = referrer.get_identity_hash().get();

    let target = {
        let reg = registry.borrow();
        let referrer_id = reg.id_by_hash.get(&referrer_hash).copied()?;
        let target_id = reg.resolve.get(&(referrer_id, specifier)).copied()?;
        reg.by_id.get(&target_id).cloned()?
    };
    Some(v8::Local::new(scope, &target))
}

/// Installs the `import.meta` initializer on `isolate`, setting `import.meta.url`
/// to the module's specifier. Isolate-level config (like the promise-reject
/// callback), so it is applied in [`wire`](crate::engine) for fresh and
/// snapshot-restored isolates alike and is never serialized into a snapshot.
pub(crate) fn install_import_meta_callback(isolate: &mut v8::OwnedIsolate) {
    isolate.set_host_initialize_import_meta_object_callback(import_meta_callback);
}

extern "C" fn import_meta_callback(
    context: v8::Local<v8::Context>,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    // Contain any panic: V8 invokes this, so an unwind would cross C++ frames
    // (UB), as elsewhere on the V8 boundary (DECISIONS.md D15).
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        import_meta_inner(context, module, meta);
    }));
}

fn import_meta_inner(
    context: v8::Local<v8::Context>,
    module: v8::Local<v8::Module>,
    meta: v8::Local<v8::Object>,
) {
    v8::callback_scope!(unsafe scope, context);

    let Some(registry) = registry(scope) else {
        return;
    };
    let hash = module.get_identity_hash().get();
    let url = {
        let reg = registry.borrow();
        reg.id_by_hash
            .get(&hash)
            .and_then(|id| reg.specifier_by_id.get(id).cloned())
    };
    if let Some(url) = url
        && let (Some(key), Some(value)) =
            (v8::String::new(scope, "url"), v8::String::new(scope, &url))
    {
        meta.create_data_property(scope, key.into(), value.into());
    }
}
