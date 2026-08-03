//! Checking a receiver out of a registry, and getting it back whatever happens.
//!
//! Several provider calls wait on a channel that lives in a shared registry.
//! None of them can hold the registry's lock across the await, so each takes the
//! receiver *out*, awaits, and puts it back. Written directly, the put-back only
//! happens on the paths that reach it — so a caller who abandons the future
//! (`tokio::time::timeout`, a losing `select!` branch) takes the receiver with
//! it, and the resource is left in the registry looking alive while reporting
//! "closed" or "end of stream" to every later call. Silent, permanent, and
//! nothing in the type system objects.
//!
//! Guest JavaScript cannot trigger this — the engine polls every pending op
//! future to completion and drops none — but the provider traits are a public
//! integration seam, so an embedder wrapping one of these calls in a timeout can.
//!
//! [`Checkout`] makes the put-back a destructor instead of a code path: it
//! happens on completion, on cancellation, and on a panic in between. A caller
//! that genuinely wants the receiver gone — the channel has ended, the resource
//! is finished — says so with [`Checkout::keep_out`].

/// A value taken out of a registry, returned when this guard is dropped.
pub(crate) struct Checkout<T> {
    value: Option<T>,
    /// How to put it back. `None` once [`keep_out`](Self::keep_out) has decided
    /// it should not go back.
    restore: Option<Box<dyn FnOnce(T) + Send>>,
}

impl<T> Checkout<T> {
    /// Guards `value`, calling `restore` with it when this guard is dropped.
    pub(crate) fn new(value: T, restore: impl FnOnce(T) + Send + 'static) -> Self {
        Self {
            value: Some(value),
            restore: Some(Box::new(restore)),
        }
    }

    /// The checked-out value, to await on.
    pub(crate) fn get_mut(&mut self) -> &mut T {
        self.value
            .as_mut()
            .expect("a Checkout holds its value until it is dropped")
    }

    /// Drops the value instead of returning it — the channel has ended and
    /// putting it back would leave a dead receiver for the next caller to wait
    /// on forever.
    pub(crate) fn keep_out(mut self) {
        self.value = None;
        self.restore = None;
    }
}

impl<T> Drop for Checkout<T> {
    fn drop(&mut self) {
        if let (Some(value), Some(restore)) = (self.value.take(), self.restore.take()) {
            restore(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn registry() -> Arc<Mutex<Option<u32>>> {
        Arc::new(Mutex::new(Some(7)))
    }

    fn check_out(reg: &Arc<Mutex<Option<u32>>>) -> Checkout<u32> {
        let value = reg.lock().unwrap().take().expect("checked out twice");
        let back = reg.clone();
        Checkout::new(value, move |v| *back.lock().unwrap() = Some(v))
    }

    #[test]
    fn a_completed_use_puts_the_value_back() {
        let reg = registry();
        {
            let mut guard = check_out(&reg);
            assert_eq!(*guard.get_mut(), 7);
            assert!(reg.lock().unwrap().is_none(), "checked out while in use");
        }
        assert_eq!(*reg.lock().unwrap(), Some(7));
    }

    /// The case the guard exists for: the caller never reaches its own
    /// put-back, and the value comes home anyway.
    #[test]
    fn an_abandoned_use_puts_the_value_back() {
        let reg = registry();
        let guard = check_out(&reg);
        drop(guard); // as an abandoned future would
        assert_eq!(*reg.lock().unwrap(), Some(7));
    }

    #[test]
    fn keep_out_leaves_the_registry_empty() {
        let reg = registry();
        check_out(&reg).keep_out();
        assert_eq!(*reg.lock().unwrap(), None);
    }

    /// A panic between checkout and put-back must not strand the value either —
    /// this is a destructor, not a code path.
    #[test]
    fn a_panic_still_puts_the_value_back() {
        let reg = registry();
        let taken = reg.clone();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = check_out(&taken);
            panic!("mid-use");
        }));
        assert_eq!(*reg.lock().unwrap(), Some(7));
    }
}
