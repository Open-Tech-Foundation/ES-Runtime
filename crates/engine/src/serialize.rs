//! HTML's StructuredSerialize / StructuredDeserialize, over V8's
//! `ValueSerializer` (SPEC §2.1).
//!
//! # Why this lives in `engine`
//!
//! An op handler receives [`Value`](crate::Value), a closed enum: a `Map`, a
//! cyclic graph or a class instance crossing the op boundary arrives as
//! `Value::Other`, carrying only its `String(value)` coercion. Every existing op
//! takes scalars and bytes, which is exactly why the rich web types are built in
//! JS on top of them — but `postMessage(anyValue)` is the first case whose
//! *payload* is an arbitrary object graph.
//!
//! So the graph never crosses the op boundary. It is flattened to bytes here,
//! where the live `v8::Local` still exists, and the op moves a `Vec<u8>` like
//! every other byte-carrying op. `Value` is unchanged.
//!
//! # Why the algorithm is V8's rather than ours
//!
//! Keeping a hand-written JS clone *and* this would be two implementations of
//! one algorithm, and they would drift — a value that cloned via
//! `structuredClone` would fail via `postMessage`. V8's is also the one that is
//! already correct: cycles, `Map`/`Set`/`Date`/`RegExp`/`BigInt`, typed arrays,
//! ordinary objects deserializing as plain objects, and String keys only.
//!
//! # The format is not a wire format
//!
//! V8's serialization format is engine-specific and versioned. These bytes are
//! valid only between two isolates of the same engine build — never persist
//! them, never send them over a network, never hand them to another engine
//! implementation.
//!
//! # Host objects
//!
//! V8 knows JS types, not ours: `Blob`, `File`, `DOMException`, `MessagePort`.
//! Those ride the delegate hooks, which call back into JS — the prelude
//! registers a codec next to each type, so "what a `Blob` is" stays in JS and
//! this file stays web-agnostic.
//!
//! Instances opt in by carrying [`HOST_CLONE_KEY`] (a registered symbol, so both
//! sides can name it). That keeps the per-object test a single property lookup
//! in Rust rather than a call into JS for every object in the graph.
//!
//! # Re-entering JS from the delegate
//!
//! V8 runs the whole of `WriteValue`/`ReadValue` inside a
//! `DisallowJavascriptExecutionScope`, and its failure mode is `CHECK`-crash,
//! not an exception — so *any* V8 API that could run JS aborts the process from
//! inside a delegate callback, a property read included. Blink never hits this
//! because its host objects are C++ wrappers it reads directly; ours are JS
//! classes with private fields, which only JS can reach.
//!
//! So the callbacks that must reach JS open an `AllowJavascriptExecutionScope`
//! first. That is sound here for the reason V8 installs the guard at all: the
//! guard exists to stop *the embedder* re-entering a half-serialized graph by
//! surprise, and the codecs we call are prelude functions locked in `harden.js`
//! whose only job is to encode one already-produced value.

#![allow(clippy::allow_attributes)]

use es_runtime_common::{ExceptionClass, IntoException};
use v8::{ValueDeserializerHelper, ValueSerializerHelper};

use crate::convert::throw;
use crate::error::{Error, Result};
use crate::op::OpError;

/// The registered symbol tagging a host-cloneable instance. JS reaches the same
/// symbol with `Symbol.for(HOST_CLONE_KEY)`; the value is the codec's tag.
pub(crate) const HOST_CLONE_KEY: &str = "es-runtime.hostClone";

/// Name of the JS hook that encodes a host object to bytes.
const WRITE_HOST_OBJECT: &str = "__structuredWriteHostObject";
/// Name of the JS hook that rebuilds a host object from bytes.
const READ_HOST_OBJECT: &str = "__structuredReadHostObject";

/// Builds the `DataCloneError` the spec requires for an unserializable value.
fn data_clone_error(message: &str) -> OpError {
    OpError::new(ExceptionClass::DomException("DataCloneError"), message)
}

// ---- SharedArrayBuffer ------------------------------------------------------

/// A `SharedArrayBuffer`'s backing store, in transit between two agents.
///
/// Unlike an `ArrayBuffer` — whose contents are copied into the serialized
/// bytes — a `SharedArrayBuffer` cannot be represented in the byte stream at
/// all: *sharing the memory* is the whole point, so what travels is an id
/// against this registry, and the receiving agent rebuilds a
/// `SharedArrayBuffer` over the very same allocation.
struct SharedBuffer(v8::SharedRef<v8::BackingStore>);

// SAFETY: a `SharedArrayBuffer` backing store is the one V8 allocation designed
// to be reached from several isolates on several threads at once — that is what
// makes `SharedArrayBuffer` and `Atomics` work, and V8 hands the same
// `BackingStore` to every isolate that shares one. The handle here is a C++
// `shared_ptr`, whose control block is atomically refcounted, so cloning and
// dropping it from another thread is exactly the operation the type is built
// for. `BackingStore` itself is already `Send` in these bindings; only the
// `SharedRef` wrapper lacks the impl, because it is generic over shared types
// that are not all thread-safe.
//
// The bytes are guest-visible shared memory either way: races over them are
// what `Atomics` exists to arbitrate, and are the guest's to resolve, not a
// memory-safety property of this map.
unsafe impl Send for SharedBuffer {}
// SAFETY: as above — `&SharedRef` is only ever used to clone it, and the
// refcount is atomic.
unsafe impl Sync for SharedBuffer {}

/// Backing stores handed over by a `postMessage`, keyed by the id written into
/// the serialized bytes.
///
/// Process-global because the two ends are different isolates on different
/// threads: the sender registers, the receiver takes. An entry is removed when
/// it is read, so the common path leaves nothing behind; a message that is
/// never delivered — its worker terminated first — leaves its entry until the
/// process ends, which is bounded by how many such messages were in flight.
static SHARED_BUFFERS: std::sync::OnceLock<
    std::sync::Mutex<(u32, std::collections::HashMap<u32, SharedBuffer>)>,
> = std::sync::OnceLock::new();

fn shared_buffers() -> &'static std::sync::Mutex<(u32, std::collections::HashMap<u32, SharedBuffer>)>
{
    SHARED_BUFFERS.get_or_init(|| std::sync::Mutex::new((0, std::collections::HashMap::new())))
}

/// Registers a backing store for transit and returns its id.
fn register_shared(store: v8::SharedRef<v8::BackingStore>) -> Option<u32> {
    let mut guard = shared_buffers().lock().ok()?;
    let id = guard.0.checked_add(1)?;
    guard.0 = id;
    guard.1.insert(id, SharedBuffer(store));
    Some(id)
}

/// Takes a registered backing store back out. `None` for an id that was never
/// registered or has already been claimed — a forged id in a hand-written blob,
/// which becomes a `DataCloneError` rather than a wrong allocation.
fn take_shared(id: u32) -> Option<v8::SharedRef<v8::BackingStore>> {
    let mut guard = shared_buffers().lock().ok()?;
    guard.1.remove(&id).map(|shared| shared.0)
}

/// Looks up one of the prelude's host-object hooks. Absent (or not a function)
/// means the prelude has not installed it, which is a build error rather than
/// something guest code can cause — the caller turns it into a
/// `DataCloneError`, so a host object simply fails to clone.
fn host_hook<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
) -> Option<v8::Local<'s, v8::Function>> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, name)?;
    let value = global.get(scope, key.into())?;
    v8::Local::<v8::Function>::try_from(value).ok()
}

/// The registered symbol instances carry to opt into host-object treatment.
fn host_clone_symbol<'s>(scope: &mut v8::PinScope<'s, '_>) -> Option<v8::Local<'s, v8::Symbol>> {
    let description = v8::String::new(scope, HOST_CLONE_KEY)?;
    Some(v8::Symbol::for_key(scope, description))
}

/// Reads `bytes` out of a JS `Uint8Array`/`ArrayBuffer` view.
fn view_bytes(value: v8::Local<'_, v8::Value>) -> Option<Vec<u8>> {
    let view = v8::Local::<v8::ArrayBufferView>::try_from(value).ok()?;
    let mut out = vec![0u8; view.byte_length()];
    let copied = view.copy_contents(&mut out);
    (copied == out.len()).then_some(out)
}

// ---- serialize --------------------------------------------------------------

struct Serializer;

impl v8::ValueSerializerImpl for Serializer {
    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        message: v8::Local<'s, v8::String>,
    ) {
        // V8 supplies the message ("… could not be cloned."); re-throw it as a
        // real `DOMException` so `err.name === "DataCloneError"` holds, which is
        // what the spec and every test assert on. Building one means calling the
        // prelude's `DOMException` constructor, hence the scope (see the module
        // docs) — without it the class lookup alone aborts the process.
        let text = message.to_rust_string_lossy(scope);
        v8::allow_javascript_execution_scope!(let scope, scope);
        throw(scope, &data_clone_error(&text));
    }

    fn has_custom_host_object(&self, _isolate: &v8::Isolate) -> bool {
        true
    }

    fn is_host_object<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        object: v8::Local<'s, v8::Object>,
    ) -> Option<bool> {
        // Called for every object in the graph, so it stays a plain property
        // read — but even a read is a JS-capable API under V8's guard, hence
        // the scope. Anything untagged is an ordinary JS value V8 serializes
        // itself.
        v8::allow_javascript_execution_scope!(let scope, scope);
        let Some(symbol) = host_clone_symbol(scope) else {
            return Some(false);
        };
        match object.get(scope, symbol.into()) {
            Some(tag) => Some(tag.is_string()),
            None => Some(false),
        }
    }

    fn write_host_object<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        object: v8::Local<'s, v8::Object>,
        serializer: &dyn v8::ValueSerializerHelper,
    ) -> Option<bool> {
        v8::allow_javascript_execution_scope!(let scope, scope);
        let Some(hook) = host_hook(scope, WRITE_HOST_OBJECT) else {
            throw(
                scope,
                &data_clone_error("host object codecs are not installed"),
            );
            return None;
        };
        let undefined = v8::undefined(scope).into();
        // The hook throws its own DataCloneError for a type it cannot encode
        // (an already-transferred port, say); `None` propagates that.
        let encoded = hook.call(scope, undefined, &[object.into()])?;
        let Some(bytes) = view_bytes(encoded) else {
            throw(
                scope,
                &data_clone_error("host object codec returned no bytes"),
            );
            return None;
        };
        // Length first: `read_raw_bytes` needs to know how much to take.
        serializer.write_uint32(u32::try_from(bytes.len()).ok()?);
        serializer.write_raw_bytes(&bytes);
        Some(true)
    }

    fn get_shared_array_buffer_id<'s>(
        &self,
        _scope: &mut v8::PinScope<'s, '_>,
        shared_array_buffer: v8::Local<'s, v8::SharedArrayBuffer>,
    ) -> Option<u32> {
        // Hand the *allocation* over, not a copy of it. Returning `None` here
        // is what made `postMessage(new SharedArrayBuffer(8))` a DataCloneError.
        register_shared(shared_array_buffer.get_backing_store())
    }
}

/// Serializes `value` to V8's structured-clone format.
///
/// Transfer is deliberately **not** routed through V8's
/// `TransferArrayBuffer`: a transferred buffer is serialized by value and the
/// source detached by the caller afterwards. The observable contract — receiver
/// holds the data, sender's buffer is detached — is identical, and it keeps a
/// serialized message a plain `Vec<u8>` at every layer, with no backing store
/// travelling out of band.
pub(crate) fn serialize(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> Result<Vec<u8>> {
    let context = scope.get_current_context();
    let serializer = v8::ValueSerializer::new(scope, Box::new(Serializer));
    serializer.write_header();
    match serializer.write_value(context, value) {
        Some(true) => Ok(serializer.release()),
        // `None`/`Some(false)` means an exception is already pending (the
        // delegate threw a DataCloneError, or the host codec did). Leave it for
        // the caller's scope to surface rather than replacing it.
        _ => Err(Error::Internal("structured clone failed".into())),
    }
}

// ---- deserialize ------------------------------------------------------------

struct Deserializer;

impl v8::ValueDeserializerImpl for Deserializer {
    fn read_host_object<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        deserializer: &dyn v8::ValueDeserializerHelper,
    ) -> Option<v8::Local<'s, v8::Object>> {
        let mut length = 0u32;
        if !deserializer.read_uint32(&mut length) {
            throw(scope, &data_clone_error("truncated host object"));
            return None;
        }
        let bytes = deserializer.read_raw_bytes(length as usize)?.to_vec();

        v8::allow_javascript_execution_scope!(let scope, scope);
        let hook = host_hook(scope, READ_HOST_OBJECT)?;
        let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
        let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
        let view = v8::Uint8Array::new(scope, buffer, 0, buffer.byte_length())?;
        let undefined = v8::undefined(scope).into();
        let rebuilt = hook.call(scope, undefined, &[view.into()])?;
        v8::Local::<v8::Object>::try_from(rebuilt).ok()
    }

    fn get_shared_array_buffer_from_id<'s>(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        transfer_id: u32,
    ) -> Option<v8::Local<'s, v8::SharedArrayBuffer>> {
        // The same allocation the sender handed over, wrapped in this isolate's
        // own `SharedArrayBuffer` object. Two agents, two JS objects, one piece
        // of memory — which is what makes `Atomics` between them mean anything.
        let store = take_shared(transfer_id)?;
        v8::allow_javascript_execution_scope!(let scope, scope);
        Some(v8::SharedArrayBuffer::with_backing_store(scope, &store))
    }
}

/// Rebuilds a value from [`serialize`] output. A blob from a different engine
/// build is rejected by V8's own version check, surfacing as a `DataCloneError`
/// rather than a misparse.
pub(crate) fn deserialize<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: &[u8],
) -> Result<v8::Local<'s, v8::Value>> {
    let context = scope.get_current_context();
    let deserializer = v8::ValueDeserializer::new(scope, Box::new(Deserializer), bytes);
    match deserializer.read_header(context) {
        Some(true) => {}
        _ => return Err(Error::Internal("unrecognized structured clone data".into())),
    }
    deserializer
        .read_value(context)
        .ok_or_else(|| Error::Internal("structured clone data could not be read".into()))
}

// ---- the JS-facing builtins -------------------------------------------------

/// Installs `__structuredSerialize(value)` and `__structuredDeserialize(bytes)`
/// on the global, beside the op shells.
///
/// These are builtins rather than ops for the reason at the top of this file:
/// an op sees a marshaled [`Value`](crate::Value), and the whole point is to
/// reach the live JS value before it is flattened.
pub(crate) fn install_structured_clone(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
) -> Result<()> {
    let global = context.global(scope);
    install_fn(scope, global, "__structuredSerialize", structured_serialize)?;
    install_fn(
        scope,
        global,
        "__structuredDeserialize",
        structured_deserialize,
    )
}

fn install_fn(
    scope: &mut v8::PinScope,
    global: v8::Local<v8::Object>,
    name: &str,
    callback: impl v8::MapFnTo<v8::FunctionCallback>,
) -> Result<()> {
    let func = v8::Function::builder(callback)
        .build(scope)
        .ok_or_else(|| Error::Internal(format!("could not build builtin {name}")))?;
    let key =
        v8::String::new(scope, name).ok_or_else(|| Error::Internal("string alloc".to_string()))?;
    global.set(scope, key.into(), func.into());
    Ok(())
}

/// `__structuredSerialize(value) -> Uint8Array`.
fn structured_serialize(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    // A panic must not unwind into V8 (D15).
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        structured_serialize_inner(&mut *scope, args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(
                ExceptionClass::Error,
                "internal error in __structuredSerialize",
            ),
        );
    }
}

fn structured_serialize_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let value = args.get(0);
    // A failure always leaves an exception pending — V8's `WriteValue` reports
    // `Nothing` only after it or the delegate threw, and the delegate's
    // `throw_data_clone_error` above is what turns that into a real
    // `DOMException`. Returning here lets that exception propagate rather than
    // replacing it with a vaguer one.
    let Ok(bytes) = serialize(scope, value) else {
        return;
    };
    // The Vec *moves* into the backing store — no copy on the way out.
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    let length = buffer.byte_length();
    if let Some(view) = v8::Uint8Array::new(scope, buffer, 0, length) {
        rv.set(view.into());
    }
}

/// `__structuredDeserialize(bytes) -> value`.
fn structured_deserialize(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        structured_deserialize_inner(&mut *scope, args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(
                ExceptionClass::Error,
                "internal error in __structuredDeserialize",
            ),
        );
    }
}

fn structured_deserialize_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Some(bytes) = view_bytes(args.get(0)) else {
        throw(
            scope,
            &data_clone_error("expected the bytes from __structuredSerialize"),
        );
        return;
    };
    // As above: a failure leaves V8's own exception pending (a version
    // mismatch, truncated input, or a host codec that threw).
    if let Ok(value) = deserialize(scope, &bytes) {
        rv.set(value);
    }
}

/// The external references for the two builtins above. V8 matches these by
/// index, so they must appear in the same order at snapshot build and restore
/// (D8); [`crate::op::external_references`] appends them.
pub(crate) fn external_references() -> Vec<v8::ExternalReference> {
    use v8::MapFnTo;
    vec![
        v8::ExternalReference {
            function: structured_serialize.map_fn_to(),
        },
        v8::ExternalReference {
            function: structured_deserialize.map_fn_to(),
        },
    ]
}

/// `IntoException` for [`OpError`] is already implemented in `op`; this module
/// only needs the trait in scope for [`throw`].
const _: fn() = || {
    fn assert_into_exception<T: IntoException>() {}
    assert_into_exception::<OpError>();
};

#[cfg(test)]
mod tests {
    use crate::{Engine, V8Engine, Value};
    use es_runtime_common::Limits;

    fn engine() -> V8Engine {
        V8Engine::new(Limits::default()).expect("engine construction")
    }

    /// Round-trips `expr` and reports the result of `check` against the clone.
    fn round_trip(engine: &mut V8Engine, expr: &str, check: &str) -> Value {
        engine
            .eval(&format!(
                "(() => {{
                   const original = ({expr});
                   const clone = __structuredDeserialize(__structuredSerialize(original));
                   return ({check});
                 }})()"
            ))
            .expect("round trip")
    }

    #[test]
    fn round_trips_the_types_the_js_clone_covered() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        for (expr, check) in [
            ("new Map([[1, 'a']])", "clone.get(1)"),
            ("new Set(['x'])", "clone.has('x') ? 'x' : 'no'"),
            ("new Date(1234)", "String(clone.getTime())"),
            ("/ab+c/gi", "clone.source + ':' + clone.flags"),
            ("new Uint8Array([1, 2, 3])", "String(clone.join(','))"),
            ("123n", "String(clone)"),
            ("new Error('boom')", "clone.name + ':' + clone.message"),
        ] {
            let got = round_trip(&mut engine, expr, check);
            assert!(
                matches!(got, Value::String(_)),
                "{expr} did not round-trip: {got:?}"
            );
        }
    }

    #[test]
    fn cycles_survive_and_stay_shared() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let got = round_trip(
            &mut engine,
            "(() => { const o = { n: 1 }; o.self = o; return o; })()",
            "String(clone.self === clone && clone.self.n === 1)",
        );
        assert_eq!(got, Value::String("true".to_string()));
    }

    #[test]
    fn an_ordinary_object_deserializes_as_a_plain_object() {
        // The hand-written JS clone this replaces threw `DataCloneError` for any
        // prototype other than `Object.prototype`/null. The spec serializes an
        // ordinary object's own enumerable properties and rebuilds it as a plain
        // object — what every other runtime does.
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let got = round_trip(
            &mut engine,
            "new (class Foo { constructor() { this.a = 1; } })()",
            "String(clone.a === 1 && Object.getPrototypeOf(clone) === Object.prototype)",
        );
        assert_eq!(got, Value::String("true".to_string()));
    }

    #[test]
    fn symbol_keys_are_dropped() {
        // StructuredSerialize walks String keys only; the JS clone used
        // `Reflect.ownKeys` and copied enumerable symbols too.
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let got = round_trip(
            &mut engine,
            "(() => { const o = { x: 1 };
                      Object.defineProperty(o, Symbol('k'), { value: 2, enumerable: true });
                      return o; })()",
            "String(Object.getOwnPropertySymbols(clone).length)",
        );
        assert_eq!(got, Value::String("0".to_string()));
    }

    #[test]
    fn a_function_is_a_data_clone_error() {
        // A bare engine has no `DOMException` — that class arrives with the
        // runtime prelude — so `build_exception` falls back to a plain `Error`
        // whose message carries the name. The real `e.name === "DataCloneError"`
        // is asserted at the runtime layer, where the class exists.
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let got = engine
            .eval(
                "(() => { try { __structuredSerialize(() => {}); return 'no throw'; }
                          catch (e) { return e.message; } })()",
            )
            .expect("eval");
        let Value::String(message) = got else {
            panic!("expected a message, got {got:?}");
        };
        assert!(
            message.starts_with("DataCloneError:"),
            "expected a DataCloneError, got: {message}"
        );
    }

    #[test]
    fn garbage_bytes_do_not_deserialize() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine();
        let got = engine
            .eval(
                "(() => { try { __structuredDeserialize(new Uint8Array([9, 9, 9])); return 'no throw'; }
                          catch (e) { return 'threw'; } })()",
            )
            .expect("eval");
        assert_eq!(got, Value::String("threw".to_string()));
    }
}
