//! Typed ops (DECISIONS D228): a handler written over Rust types, whose
//! arguments are read straight off V8's call arguments.
//!
//! A [`Value`]-based op pays, on every call, for the generic enum, the `Vec`
//! that holds it, and a chain of type probes per argument — ~22 ns for each
//! number (`crates/engine/examples/op_crossing.rs`). A typed op asks V8 one
//! question per argument, the one its declared type needs, and hands the
//! handler the value.
//!
//! Arguments are **checked, not coerced or defaulted**. Ops are called by the
//! runtime's own prelude, so an argument of the wrong type is a bug there, and
//! a silent default would hide it. A mismatch throws a `TypeError` naming the
//! op and the argument.
//!
//! The conversion traits are sealed: they name V8 types, which no crate but
//! this one may see (ARCHITECTURE.md §3). Other crates only write closures over
//! the types that implement them.

use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;

use crate::convert::{bytes_to_uint8array, js_string, marshal, value_to_js, view_to_vec};
use crate::op::{AsyncOp, OpError};
use crate::value::Value;

/// The largest integer a double holds exactly, `2^53 - 1` — the edge of
/// JavaScript's `Number.isSafeInteger`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Why a typed op's arguments were refused, before the handler ran.
pub(crate) enum ArgError {
    /// Argument `index` (0-based) is not the declared type.
    Type {
        index: usize,
        expected: &'static str,
    },
    /// More arguments than the op declares.
    TooMany { declared: usize, got: usize },
    /// The argument is the declared type but could not be carried — a cyclic
    /// structure, a BigInt too large — with the error that says so.
    Op(OpError),
}

impl ArgError {
    /// The exception this refusal throws, named after the op.
    pub(crate) fn into_op_error(self, op: &str) -> OpError {
        match self {
            ArgError::Type { index, expected } => {
                OpError::type_error(format!("{op}: argument {} must be {expected}", index + 1))
            }
            ArgError::TooMany { declared, got } => OpError::type_error(format!(
                "{op}: takes {declared} argument{}, got {got}",
                if declared == 1 { "" } else { "s" }
            )),
            ArgError::Op(err) => err,
        }
    }
}

pub(crate) mod sealed {
    use super::{ArgError, AsyncOp, OpError, Value};

    /// Reads one argument as `Self`.
    pub trait Arg: Sized {
        /// How a refusal describes the type: "a string", "a boolean".
        const EXPECTED: &'static str;

        /// Reads `value`, or says it is not `Self`. Must not run JS unless
        /// `Self` is [`Value`] (whose marshaling reads object properties), so
        /// it runs before any op state is borrowed.
        fn from_js<'s>(
            scope: &v8::PinScope<'s, '_>,
            value: v8::Local<'s, v8::Value>,
        ) -> Result<Self, Option<OpError>>;
    }

    /// Hands a result back to JS as `Self`.
    pub trait Ret {
        /// Sets the result of a sync op directly on V8's return slot.
        fn set(self, scope: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>);

        /// The result as a [`Value`], for an async op's promise, which settles
        /// later through the ordinary resolution path.
        fn into_value(self) -> Value;
    }

    /// A registered typed handler with its types erased.
    pub trait Sync {
        /// Reads the arguments, calls the handler, and sets the result.
        fn call(
            &self,
            scope: &mut v8::PinScope<'_, '_>,
            args: &v8::FunctionCallbackArguments<'_>,
            rv: &mut v8::ReturnValue<'_, v8::Value>,
        ) -> Result<(), CallError>;
    }

    /// A registered typed async handler with its types erased.
    pub trait Async {
        /// Reads the arguments and calls the handler for its future.
        fn call(
            &self,
            scope: &mut v8::PinScope<'_, '_>,
            args: &v8::FunctionCallbackArguments<'_>,
        ) -> Result<AsyncOp, CallError>;
    }

    /// What stopped a typed call: its arguments, or the handler itself.
    pub enum CallError {
        /// Refused before the handler ran.
        Args(ArgError),
        /// The handler's own error.
        Op(OpError),
    }
}

use sealed::CallError;

/// A type a typed op may take as an argument: `bool`, `i32`, `u32`, `f64`,
/// `i64`, `u64`, `String`, `Vec<u8>`, [`Value`], or an `Option` of one.
///
/// - Integers must be integral and in range — no rounding, no wrapping. `i64`
///   and `u64` take a safe integer (within ±2<sup>53</sup>−1) or a `BigInt`.
/// - `String` takes a string; a lone surrogate becomes U+FFFD, as a WebIDL
///   `USVString` does.
/// - `Vec<u8>` takes a typed array or `DataView`, copied (DECISIONS D3a).
/// - `Option<T>` reads `undefined`, `null`, or a missing argument as `None`.
/// - [`Value`] takes anything, marshaled as a `Value`-based op's arguments are:
///   the escape hatch for a structured argument.
pub trait OpArg: sealed::Arg {}
impl<T: sealed::Arg> OpArg for T {}

/// A type a typed op may return: `()`, `bool`, `i32`, `u32`, `f64`, `String`,
/// `Vec<u8>` (moved into a `Uint8Array` without a copy), [`Value`], or an
/// `Option` of one, `None` being `null`.
pub trait OpRet: sealed::Ret {}
impl<T: sealed::Ret> OpRet for T {}

fn mismatch<T>() -> Result<T, Option<OpError>> {
    Err(None)
}

impl sealed::Arg for bool {
    const EXPECTED: &'static str = "a boolean";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        if value.is_true() {
            Ok(true)
        } else if value.is_false() {
            Ok(false)
        } else {
            mismatch()
        }
    }
}

impl sealed::Arg for f64 {
    const EXPECTED: &'static str = "a number";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        v8::Local::<v8::Number>::try_from(value)
            .map(|n| n.value())
            .or_else(|_| mismatch())
    }
}

/// A number with an integral value, as `i64` (exact: within ±2^53), or a
/// refusal. `-0` is zero. `i64` and not `i128`: a double converts to `i64` in
/// one instruction, to `i128` through a software routine.
fn integral<'s>(value: v8::Local<'s, v8::Value>) -> Result<i64, Option<OpError>> {
    // One read of the number: each `value()` is a call into V8, and on the
    // hot path three of them cost ~7 ns an argument.
    let Ok(n) = v8::Local::<v8::Number>::try_from(value).map(|n| n.value()) else {
        return mismatch();
    };
    // `trunc` rather than `fract`: the same test, compiled to one instruction.
    // NaN and the infinities fail it, and `-0` passes as zero.
    if n.trunc() == n && n.abs() <= MAX_SAFE_INTEGER {
        Ok(n as i64)
    } else {
        mismatch()
    }
}

impl sealed::Arg for u32 {
    const EXPECTED: &'static str = "an unsigned 32-bit integer";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        // V8's own test first, the common case at one call into it; then the
        // general one, which also takes `-0`.
        if let Ok(n) = v8::Local::<v8::Uint32>::try_from(value) {
            return Ok(n.value());
        }
        integral(value).and_then(|n| u32::try_from(n).or_else(|_| mismatch()))
    }
}

impl sealed::Arg for i32 {
    const EXPECTED: &'static str = "a 32-bit integer";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        if let Ok(n) = v8::Local::<v8::Int32>::try_from(value) {
            return Ok(n.value());
        }
        integral(value).and_then(|n| i32::try_from(n).or_else(|_| mismatch()))
    }
}

/// A safe integer from a number, or any integer from a `BigInt`, as `i128` —
/// wide enough to range-check into either 64-bit type.
fn integer<'s>(value: v8::Local<'s, v8::Value>) -> Result<i128, Option<OpError>> {
    if value.is_number() {
        return integral(value).map(i128::from);
    }
    if let Ok(big) = v8::Local::<v8::BigInt>::try_from(value) {
        let (n, lossless) = big.i64_value();
        if lossless {
            return Ok(i128::from(n));
        }
        let (n, lossless) = big.u64_value();
        if lossless {
            return Ok(i128::from(n));
        }
    }
    mismatch()
}

impl sealed::Arg for i64 {
    const EXPECTED: &'static str = "a safe integer or a BigInt in the signed 64-bit range";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        integer(value).and_then(|n| i64::try_from(n).or_else(|_| mismatch()))
    }
}

impl sealed::Arg for u64 {
    const EXPECTED: &'static str =
        "a non-negative safe integer or a BigInt in the unsigned 64-bit range";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        integer(value).and_then(|n| u64::try_from(n).or_else(|_| mismatch()))
    }
}

impl sealed::Arg for String {
    const EXPECTED: &'static str = "a string";
    fn from_js<'s>(
        scope: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        v8::Local::<v8::String>::try_from(value)
            .map(|s| s.to_rust_string_lossy(scope))
            .or_else(|_| mismatch())
    }
}

impl sealed::Arg for Vec<u8> {
    const EXPECTED: &'static str = "a typed array or DataView";
    fn from_js<'s>(
        _: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        v8::Local::<v8::ArrayBufferView>::try_from(value)
            .map(view_to_vec)
            .or_else(|_| mismatch())
    }
}

impl sealed::Arg for Value {
    const EXPECTED: &'static str = "any value";
    fn from_js<'s>(
        scope: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        marshal(scope, value).map_err(Some)
    }
}

impl<T: sealed::Arg> sealed::Arg for Option<T> {
    const EXPECTED: &'static str = T::EXPECTED;
    fn from_js<'s>(
        scope: &v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Result<Self, Option<OpError>> {
        if value.is_null_or_undefined() {
            Ok(None)
        } else {
            T::from_js(scope, value).map(Some)
        }
    }
}

impl sealed::Ret for () {
    fn set(self, _: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        rv.set_undefined();
    }
    fn into_value(self) -> Value {
        Value::Undefined
    }
}

impl sealed::Ret for bool {
    fn set(self, _: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        rv.set_bool(self);
    }
    fn into_value(self) -> Value {
        Value::Bool(self)
    }
}

impl sealed::Ret for i32 {
    fn set(self, _: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        rv.set_int32(self);
    }
    fn into_value(self) -> Value {
        Value::Number(f64::from(self))
    }
}

impl sealed::Ret for u32 {
    fn set(self, _: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        rv.set_uint32(self);
    }
    fn into_value(self) -> Value {
        Value::Number(f64::from(self))
    }
}

impl sealed::Ret for f64 {
    fn set(self, _: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        rv.set_double(self);
    }
    fn into_value(self) -> Value {
        Value::Number(self)
    }
}

impl sealed::Ret for String {
    fn set(self, scope: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        let s = js_string(scope, &self);
        rv.set(s.into());
    }
    fn into_value(self) -> Value {
        Value::String(self)
    }
}

impl sealed::Ret for Vec<u8> {
    fn set(self, scope: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        let bytes = bytes_to_uint8array(scope, self);
        rv.set(bytes.into());
    }
    fn into_value(self) -> Value {
        Value::Bytes(self)
    }
}

impl sealed::Ret for Value {
    fn set(self, scope: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        let js = value_to_js(scope, self);
        rv.set(js);
    }
    fn into_value(self) -> Value {
        self
    }
}

impl<T: sealed::Ret> sealed::Ret for Option<T> {
    fn set(self, scope: &mut v8::PinScope<'_, '_>, rv: &mut v8::ReturnValue<'_, v8::Value>) {
        match self {
            Some(value) => value.set(scope, rv),
            None => rv.set_null(),
        }
    }
    fn into_value(self) -> Value {
        self.map_or(Value::Null, T::into_value)
    }
}

/// A closure over [`OpArg`]s returning `Result<R, OpError>` for an
/// [`OpRet`] `R` — what [`OpDecl::typed`](crate::OpDecl::typed) takes. `M`
/// is inferred; it only tells the closure shapes apart.
pub trait TypedSync<M>: 'static {
    #[doc(hidden)]
    fn into_handler(self) -> TypedHandler;
}

/// A closure over [`OpArg`]s returning a future of `Result<R, OpError>` —
/// what [`OpDecl::typed_async`](crate::OpDecl::typed_async) takes.
pub trait TypedAsync<M>: 'static {
    #[doc(hidden)]
    fn into_handler(self) -> TypedHandler;
}

/// A typed handler, ready to register. Opaque: built only by
/// [`OpDecl::typed`](crate::OpDecl::typed) and
/// [`OpDecl::typed_async`](crate::OpDecl::typed_async).
pub struct TypedHandler(pub(crate) TypedKind);

pub(crate) enum TypedKind {
    Sync(Rc<dyn sealed::Sync>),
    Async(Rc<dyn sealed::Async>),
}

impl Clone for TypedHandler {
    fn clone(&self) -> Self {
        TypedHandler(match &self.0 {
            TypedKind::Sync(op) => TypedKind::Sync(Rc::clone(op)),
            TypedKind::Async(op) => TypedKind::Async(Rc::clone(op)),
        })
    }
}

/// A closure with the argument and result types it was declared with.
///
/// The closure sits in a `RefCell` of its own, so the dispatcher can release
/// the op table before the arguments are read: reading a [`Value`] argument
/// runs JS (a getter, a proxy trap), which may call another op.
struct Typed<F, M> {
    f: RefCell<F>,
    _shape: PhantomData<fn() -> M>,
}

impl<F, M> Typed<F, M> {
    fn new(f: F) -> Self {
        Typed {
            f: RefCell::new(f),
            _shape: PhantomData,
        }
    }
}

/// Reads argument `index` as `T`.
fn arg<'s, T: sealed::Arg>(
    scope: &v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    index: usize,
) -> Result<T, CallError> {
    // Past the end, V8 hands back `undefined` — which is what an `Option`
    // reads as absent and every other type refuses.
    T::from_js(scope, args.get(index as i32)).map_err(|err| {
        CallError::Args(match err {
            Some(err) => ArgError::Op(err),
            None => ArgError::Type {
                index,
                expected: T::EXPECTED,
            },
        })
    })
}

/// Refuses a call with more arguments than declared.
fn arity(args: &v8::FunctionCallbackArguments<'_>, declared: usize) -> Result<(), CallError> {
    let got = args.length().max(0) as usize;
    if got > declared {
        return Err(CallError::Args(ArgError::TooMany { declared, got }));
    }
    Ok(())
}

macro_rules! typed_arity {
    ($count:expr; $($a:ident $i:tt),*) => {
        impl<F, R, $($a,)*> sealed::Sync for Typed<F, (R, $($a,)*)>
        where
            F: FnMut($($a),*) -> Result<R, OpError>,
            R: sealed::Ret,
            $($a: sealed::Arg,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn call(
                &self,
                scope: &mut v8::PinScope<'_, '_>,
                args: &v8::FunctionCallbackArguments<'_>,
                rv: &mut v8::ReturnValue<'_, v8::Value>,
            ) -> Result<(), CallError> {
                arity(args, $count)?;
                $(let $a = arg::<$a>(&*scope, args, $i)?;)*
                let result = (self.f.borrow_mut())($($a),*).map_err(CallError::Op)?;
                result.set(scope, rv);
                Ok(())
            }
        }

        impl<F, R, $($a,)*> TypedSync<(R, $($a,)*)> for F
        where
            F: FnMut($($a),*) -> Result<R, OpError> + 'static,
            R: sealed::Ret + 'static,
            $($a: sealed::Arg + 'static,)*
        {
            fn into_handler(self) -> TypedHandler {
                TypedHandler(TypedKind::Sync(Rc::new(Typed::<F, (R, $($a,)*)>::new(self))))
            }
        }

        impl<F, Fut, R, $($a,)*> sealed::Async for Typed<F, (Fut, R, $($a,)*)>
        where
            F: FnMut($($a),*) -> Fut,
            Fut: Future<Output = Result<R, OpError>> + 'static,
            R: sealed::Ret,
            $($a: sealed::Arg,)*
        {
            #[allow(non_snake_case, unused_variables)]
            fn call(
                &self,
                scope: &mut v8::PinScope<'_, '_>,
                args: &v8::FunctionCallbackArguments<'_>,
            ) -> Result<AsyncOp, CallError> {
                arity(args, $count)?;
                $(let $a = arg::<$a>(&*scope, args, $i)?;)*
                let future = (self.f.borrow_mut())($($a),*);
                Ok(Box::pin(async move { future.await.map(sealed::Ret::into_value) }))
            }
        }

        impl<F, Fut, R, $($a,)*> TypedAsync<(Fut, R, $($a,)*)> for F
        where
            F: FnMut($($a),*) -> Fut + 'static,
            Fut: Future<Output = Result<R, OpError>> + 'static,
            R: sealed::Ret + 'static,
            $($a: sealed::Arg + 'static,)*
        {
            fn into_handler(self) -> TypedHandler {
                TypedHandler(TypedKind::Async(Rc::new(
                    Typed::<F, (Fut, R, $($a,)*)>::new(self),
                )))
            }
        }
    };
}

typed_arity!(0;);
typed_arity!(1; A0 0);
typed_arity!(2; A0 0, A1 1);
typed_arity!(3; A0 0, A1 1, A2 2);
typed_arity!(4; A0 0, A1 1, A2 2, A3 3);
typed_arity!(5; A0 0, A1 1, A2 2, A3 3, A4 4);
typed_arity!(6; A0 0, A1 1, A2 2, A3 3, A4 4, A5 5);
typed_arity!(7; A0 0, A1 1, A2 2, A3 3, A4 4, A5 5, A6 6);
typed_arity!(8; A0 0, A1 1, A2 2, A3 3, A4 4, A5 5, A6 6, A7 7);

#[cfg(test)]
mod tests {
    use es_runtime_common::{Capability, CapabilitySet};

    use crate::engine::{Engine, V8Engine};
    use crate::op::{OpDecl, OpError};
    use crate::value::Value;
    use es_runtime_common::Limits;

    fn engine(ops: Vec<OpDecl>) -> V8Engine {
        let mut engine = V8Engine::new(Limits::default()).expect("engine");
        engine.expose_ops_for_testing().expect("expose ops");
        for op in ops {
            engine.register_op(op).expect("register");
        }
        engine
    }

    fn eval(engine: &mut V8Engine, source: &str) -> Value {
        engine.eval(source).expect("eval")
    }

    /// What a call throws, as `Class: message`, or `ok` if it returns.
    fn thrown(engine: &mut V8Engine, call: &str) -> String {
        let source =
            format!("try {{ {call}; 'ok' }} catch (e) {{ e.constructor.name + ': ' + e.message }}");
        match eval(engine, &source) {
            Value::String(s) => s,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn scalar_arguments_and_results_cross_as_their_types() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("add", |a: u32, b: u32| Ok(a + b)),
            OpDecl::typed("neg", |a: i32| Ok(-a)),
            OpDecl::typed("half", |a: f64| Ok(a / 2.0)),
            OpDecl::typed("not", |a: bool| Ok(!a)),
            OpDecl::typed("nothing", || Ok(())),
        ]);
        assert_eq!(eval(&mut engine, "__ops.add(2, 3)"), Value::Number(5.0));
        // An integral double is the integer it holds.
        assert_eq!(
            eval(&mut engine, "__ops.add(6 / 2, 0.5 * 2)"),
            Value::Number(4.0)
        );
        assert_eq!(
            eval(&mut engine, "__ops.add(4294967295, 0)"),
            Value::Number(4294967295.0)
        );
        assert_eq!(eval(&mut engine, "__ops.neg(-7)"), Value::Number(7.0));
        assert_eq!(eval(&mut engine, "__ops.half(3)"), Value::Number(1.5));
        assert_eq!(eval(&mut engine, "__ops.not(false)"), Value::Bool(true));
        assert_eq!(eval(&mut engine, "__ops.nothing()"), Value::Undefined);
    }

    #[test]
    fn an_integer_argument_is_never_rounded_wrapped_or_coerced() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("u", |a: u32| Ok(a)),
            OpDecl::typed("i", |a: i32| Ok(a)),
        ]);
        let refused = "TypeError: u: argument 1 must be an unsigned 32-bit integer";
        for bad in [
            "-1",
            "1.5",
            "2 ** 32",
            "NaN",
            "Infinity",
            "'1'",
            "1n",
            "undefined",
            "null",
            "{}",
        ] {
            assert_eq!(
                thrown(&mut engine, &format!("__ops.u({bad})")),
                refused,
                "{bad}"
            );
        }
        let refused = "TypeError: i: argument 1 must be a 32-bit integer";
        for bad in ["2 ** 31", "-(2 ** 31) - 1", "0.1", "true"] {
            assert_eq!(
                thrown(&mut engine, &format!("__ops.i({bad})")),
                refused,
                "{bad}"
            );
        }
        assert_eq!(
            eval(&mut engine, "__ops.i(-(2 ** 31))"),
            Value::Number(-2147483648.0)
        );
        // -0 is an integer, and zero.
        assert_eq!(eval(&mut engine, "__ops.u(-0)"), Value::Number(0.0));
    }

    #[test]
    fn a_64_bit_argument_takes_a_safe_integer_or_a_bigint() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("u", |a: u64| Ok(a.to_string())),
            OpDecl::typed("i", |a: i64| Ok(a.to_string())),
        ]);
        assert_eq!(
            eval(&mut engine, "__ops.u(2 ** 53 - 1)"),
            Value::String("9007199254740991".into())
        );
        assert_eq!(
            eval(&mut engine, "__ops.u(2n ** 64n - 1n)"),
            Value::String(u64::MAX.to_string())
        );
        assert_eq!(
            eval(&mut engine, "__ops.i(-(2n ** 63n))"),
            Value::String(i64::MIN.to_string())
        );
        assert_eq!(eval(&mut engine, "__ops.i(-5)"), Value::String("-5".into()));
        // Past 2^53 a number is no longer the integer it looks like.
        assert!(
            thrown(&mut engine, "__ops.u(2 ** 53)").starts_with("TypeError: u: argument 1 must be")
        );
        for bad in ["-1", "-1n", "2n ** 64n", "1.5"] {
            assert!(
                thrown(&mut engine, &format!("__ops.u({bad})")).starts_with("TypeError"),
                "{bad}"
            );
        }
        assert!(thrown(&mut engine, "__ops.i(2n ** 63n)").starts_with("TypeError"));
    }

    #[test]
    fn a_string_argument_takes_only_a_string_and_replaces_lone_surrogates() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![OpDecl::typed("echo", |s: String| Ok(s))]);
        assert_eq!(
            eval(&mut engine, "__ops.echo('héllo ✓')"),
            Value::String("héllo ✓".into())
        );
        assert_eq!(
            eval(&mut engine, "__ops.echo('a\\uD800b')"),
            Value::String("a\u{FFFD}b".into())
        );
        assert_eq!(
            eval(&mut engine, "__ops.echo('')"),
            Value::String(String::new())
        );
        for bad in ["1", "new String('x')", "undefined", "['x']"] {
            assert_eq!(
                thrown(&mut engine, &format!("__ops.echo({bad})")),
                "TypeError: echo: argument 1 must be a string",
                "{bad}"
            );
        }
    }

    #[test]
    fn a_byte_argument_takes_a_view_and_a_byte_result_is_a_uint8array() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![OpDecl::typed("rev", |mut b: Vec<u8>| {
            b.reverse();
            Ok(b)
        })]);
        let out = eval(
            &mut engine,
            "const r = __ops.rev(new Uint8Array([1, 2, 3, 4]).subarray(1, 3)); \
             r instanceof Uint8Array && r.join() === '3,2'",
        );
        assert_eq!(out, Value::Bool(true));
        let out = eval(
            &mut engine,
            "__ops.rev(new DataView(new Uint8Array([5, 6]).buffer)).join()",
        );
        assert_eq!(out, Value::String("6,5".into()));
        assert_eq!(
            eval(&mut engine, "__ops.rev(new Uint8Array(0)).length"),
            Value::Number(0.0)
        );
        // A bare ArrayBuffer is not a view; the prelude wraps one first.
        assert_eq!(
            thrown(&mut engine, "__ops.rev(new ArrayBuffer(2))"),
            "TypeError: rev: argument 1 must be a typed array or DataView"
        );
    }

    #[test]
    fn an_option_reads_absent_undefined_and_null_as_none() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("greet", |name: String, title: Option<String>| {
                Ok(match title {
                    Some(title) => format!("{title} {name}"),
                    None => name,
                })
            }),
            OpDecl::typed("maybe", |give: bool| Ok(give.then_some(7u32))),
        ]);
        assert_eq!(
            eval(&mut engine, "__ops.greet('Ada')"),
            Value::String("Ada".into())
        );
        assert_eq!(
            eval(&mut engine, "__ops.greet('Ada', undefined)"),
            Value::String("Ada".into())
        );
        assert_eq!(
            eval(&mut engine, "__ops.greet('Ada', null)"),
            Value::String("Ada".into())
        );
        assert_eq!(
            eval(&mut engine, "__ops.greet('Ada', 'Dr')"),
            Value::String("Dr Ada".into())
        );
        assert_eq!(
            thrown(&mut engine, "__ops.greet('Ada', 1)"),
            "TypeError: greet: argument 2 must be a string"
        );
        // A required argument may not be left out.
        assert_eq!(
            thrown(&mut engine, "__ops.greet()"),
            "TypeError: greet: argument 1 must be a string"
        );
        assert_eq!(eval(&mut engine, "__ops.maybe(true)"), Value::Number(7.0));
        assert_eq!(eval(&mut engine, "__ops.maybe(false)"), Value::Null);
    }

    #[test]
    fn more_arguments_than_declared_are_refused() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("one", |a: u32| Ok(a)),
            OpDecl::typed("none", || Ok(())),
        ]);
        assert_eq!(
            thrown(&mut engine, "__ops.one(1, 2)"),
            "TypeError: one: takes 1 argument, got 2"
        );
        assert_eq!(
            thrown(&mut engine, "__ops.none(1)"),
            "TypeError: none: takes 0 arguments, got 1"
        );
    }

    #[test]
    fn a_handler_error_throws_as_its_own_class() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![OpDecl::typed("check", |n: u32| {
            if n > 10 {
                Err(OpError::range_error("too big"))
            } else {
                Ok(n)
            }
        })]);
        assert_eq!(
            thrown(&mut engine, "__ops.check(11)"),
            "RangeError: too big"
        );
        assert_eq!(eval(&mut engine, "__ops.check(10)"), Value::Number(10.0));
    }

    #[test]
    fn a_denied_typed_op_never_reads_its_arguments() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("read", |v: Value| Ok(v)).requires(Capability::FileRead),
        ]);
        engine.set_capabilities(CapabilitySet::none());
        let out = eval(
            &mut engine,
            "globalThis.touched = false; \
             const arg = { get x() { globalThis.touched = true; return 1; } }; \
             let denied = false; \
             try { __ops.read(arg); } catch (e) { denied = true; } \
             denied && !globalThis.touched",
        );
        assert_eq!(out, Value::Bool(true));
    }

    /// Reading a `Value` argument runs JS — here a getter — and that JS may
    /// call ops, this one included. The op table must not be borrowed then.
    #[test]
    fn an_argument_getter_may_call_ops_while_its_op_is_dispatching() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![
            OpDecl::typed("wrap", |v: Value| Ok(Value::Array(vec![v]))),
            OpDecl::typed("add", |a: u32, b: u32| Ok(a + b)),
        ]);
        let out = eval(
            &mut engine,
            "const arg = { get x() { return __ops.add(1, 2) + __ops.wrap(5)[0]; } }; \
             JSON.stringify(__ops.wrap(arg))",
        );
        assert_eq!(out, Value::String(r#"[{"x":8}]"#.into()));
    }

    #[test]
    fn a_typed_async_op_resolves_its_promise_with_its_result() {
        let _v8 = crate::v8_test_guard();
        let mut engine = engine(vec![OpDecl::typed_async("double", |n: u32| {
            std::future::ready(Ok::<_, OpError>(n * 2))
        })]);
        eval(
            &mut engine,
            "__ops.double(21).then((v) => { globalThis.got = v; });",
        );
        engine.poll_async_ops();
        engine.run_microtasks();
        assert_eq!(eval(&mut engine, "globalThis.got"), Value::Number(42.0));
        // An argument it cannot take is refused at the call, as a sync op's is.
        assert_eq!(
            thrown(&mut engine, "__ops.double('x')"),
            "TypeError: double: argument 1 must be an unsigned 32-bit integer"
        );
    }
}
