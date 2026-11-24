//! Work that must not run on the thread driving the loop (D241).
//!
//! That thread is the one the `--timeout` watchdog's termination lands on and
//! the one that sees a `SIGTERM`. V8 can interrupt JavaScript there, but not
//! Rust: a synchronous op runs to its end before either is noticed. So an op
//! whose running time the guest can push past a bound — a PBKDF2 iteration
//! count, an RSA key, a password hash's cost, a large buffer to encrypt — is
//! an async op whose body runs on the embedder's [`TaskSpawner`] instead. The
//! loop stays free to stop the program, or to shut it down, while the work
//! finishes elsewhere.
//!
//! Only for APIs that are already asynchronous, where moving the body off the
//! thread changes nothing the guest can observe: `crypto.subtle`, and
//! `runtime:hashing`'s `password`. A synchronous API (`hash()`, `hmac()`, a
//! webhook's `sign()`) stays synchronous by contract.

use std::sync::Arc;

use es_runtime_common::ExceptionClass;
use es_runtime_engine::{OpDecl, OpError, OpResult, Value};
use es_runtime_providers::TaskSpawner;

/// Runs `work` on the spawner, or inline without one — an embedder that
/// supplies no spawner gets today's behaviour, not a refusal.
pub(crate) async fn offload<T: Send + 'static>(
    spawner: Option<Arc<dyn TaskSpawner>>,
    work: impl FnOnce() -> T + Send + 'static,
) -> std::result::Result<T, OpError> {
    let Some(spawner) = spawner else {
        return Ok(work());
    };
    let (tx, rx) = futures_channel::oneshot::channel();
    spawner
        .spawn_blocking(Box::new(move || {
            let _ = tx.send(work());
        }))
        .await;
    // A dropped sender means the work never finished: the pool shut down, or
    // the work panicked.
    rx.await
        .map_err(|_| OpError::new(ExceptionClass::Error, "the work did not complete"))
}

/// An async op named `name` whose synchronous `body` runs off the loop thread.
///
/// The body is the one a synchronous op would have had: it takes the marshaled
/// arguments and returns the result, so converting an op is the declaration
/// and an `await` at its call site.
pub(crate) fn blocking_op(
    name: &str,
    spawner: &Option<Arc<dyn TaskSpawner>>,
    body: impl Fn(Vec<Value>) -> OpResult + Send + Sync + 'static,
) -> OpDecl {
    let body = Arc::new(body);
    let spawner = spawner.clone();
    OpDecl::r#async(name, move |args| {
        let body = body.clone();
        let spawner = spawner.clone();
        Box::pin(async move { offload(spawner, move || body(args)).await? })
    })
}

/// The most data a symmetric operation (a digest, HMAC, AES) works on inline.
///
/// Below this the work is microseconds — too short to hold a timeout or a
/// signal back by anything a person could notice — and the hop to a pool
/// thread and back (~10 µs, measured by `bench/subtle-small.js`) would cost
/// more than the work itself. Above it the work grows with the data, so it
/// moves off the thread like every operation whose cost can grow (D241).
pub(crate) const INLINE_MAX_BYTES: usize = 64 * 1024;

/// An async op like [`blocking_op`], except that it runs `body` inline when its
/// byte arguments hold at most [`INLINE_MAX_BYTES`] between them — for a
/// symmetric operation whose cost is only ever the size of what it is given.
/// All of them count, not just the message: an HMAC key is hashed too.
pub(crate) fn sized_blocking_op(
    name: &str,
    spawner: &Option<Arc<dyn TaskSpawner>>,
    body: impl Fn(Vec<Value>) -> OpResult + Send + Sync + 'static,
) -> OpDecl {
    let body = Arc::new(body);
    let spawner = spawner.clone();
    OpDecl::r#async(name, move |args| {
        if byte_len(&args) <= INLINE_MAX_BYTES {
            let result = body(args);
            return Box::pin(std::future::ready(result));
        }
        let body = body.clone();
        let spawner = spawner.clone();
        Box::pin(async move { offload(spawner, move || body(args)).await? })
    })
}

/// The bytes the arguments carry between them.
fn byte_len(args: &[Value]) -> usize {
    args.iter()
        .map(|arg| match arg {
            Value::Bytes(bytes) => bytes.len(),
            _ => 0,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A spawner that runs each piece of work on a thread of its own and
    /// reports where it ran.
    struct ThreadSpawner;

    impl TaskSpawner for ThreadSpawner {
        fn spawn_blocking(
            &self,
            work: Box<dyn FnOnce() + Send + 'static>,
        ) -> es_runtime_providers::BoxFuture<()> {
            std::thread::spawn(work).join().expect("work thread");
            Box::pin(std::future::ready(()))
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
    }

    #[test]
    fn work_runs_on_the_spawner_and_not_the_calling_thread() {
        let caller = std::thread::current().id();
        let spawner: Option<Arc<dyn TaskSpawner>> = Some(Arc::new(ThreadSpawner));
        let ran_on = block_on(offload(spawner, || std::thread::current().id())).expect("ran");
        assert_ne!(ran_on, caller);
    }

    #[test]
    fn without_a_spawner_work_runs_inline() {
        let caller = std::thread::current().id();
        let ran_on = block_on(offload(None, || std::thread::current().id())).expect("ran");
        assert_eq!(ran_on, caller);
    }

    #[test]
    fn every_byte_argument_counts_towards_the_inline_bound() {
        let half = Value::Bytes(vec![0; INLINE_MAX_BYTES / 2]);
        assert_eq!(
            byte_len(&[Value::String("SHA-256".into()), half.clone()]),
            INLINE_MAX_BYTES / 2
        );
        // A key and a message under the bound each, over it together.
        assert!(byte_len(&[half.clone(), half.clone(), Value::Bytes(vec![0])]) > INLINE_MAX_BYTES);
    }

    #[test]
    fn work_that_never_reports_is_an_error_not_a_hang() {
        /// A spawner that drops the work without running it — a pool that
        /// shut down.
        struct Dropping;
        impl TaskSpawner for Dropping {
            fn spawn_blocking(
                &self,
                _work: Box<dyn FnOnce() + Send + 'static>,
            ) -> es_runtime_providers::BoxFuture<()> {
                Box::pin(std::future::ready(()))
            }
        }
        let spawner: Option<Arc<dyn TaskSpawner>> = Some(Arc::new(Dropping));
        assert!(block_on(offload(spawner, || 1)).is_err());
    }
}
