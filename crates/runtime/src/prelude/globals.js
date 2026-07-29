// Misc globals (SPEC §2.1): queueMicrotask, reportError, and the `self` alias.
// Loaded after console so reportError can route through it.
(() => {
  "use strict";

  globalThis.queueMicrotask = (callback) => {
    if (typeof callback !== "function") {
      throw new TypeError(
        "Failed to execute 'queueMicrotask': the callback must be a function",
      );
    }
    // Promise.resolve().then schedules a microtask with the right timing.
    Promise.resolve().then(() => {
      callback();
    });
  };

  // `onerror` and friends are single handler slots layered over the global
  // EventTarget, so assigning one twice replaces rather than accumulates.
  const handlerSlot = (name) => {
    let current = null;
    Object.defineProperty(globalThis, `on${name}`, {
      get() {
        return current;
      },
      set(handler) {
        if (current) globalThis.removeEventListener(name, current);
        current = typeof handler === "function" ? handler : null;
        if (current) globalThis.addEventListener(name, current);
      },
      enumerable: true,
      configurable: true,
    });
  };
  handlerSlot("error");
  handlerSlot("unhandledrejection");
  handlerSlot("rejectionhandled");

  // Guards against recursion: an "error" listener that itself throws is routed
  // back here by EventTarget.dispatchEvent, which would otherwise re-dispatch
  // forever. The nested report goes straight to the console.
  let reporting = false;

  // What an ErrorEvent's `message` should read, and what a console report
  // should show — the stack when there is one, since it subsumes the message.
  const messageOf = (error) =>
    error && typeof error === "object" && "message" in error
      ? String(error.message)
      : String(error);
  const stackOf = (error) =>
    error && typeof error === "object" && "stack" in error
      ? error.stack
      : String(error);

  globalThis.reportError = (error) => {
    // Report an exception the way the platform does: dispatch an "error"
    // ErrorEvent on the global scope, and fall back to console.error only if
    // nothing handled it (no listener called preventDefault). That makes
    // `addEventListener("error", …)` and `onerror` work, and keeps an
    // unhandled report visible on the console as before.
    if (reporting) {
      globalThis.console.error(stackOf(error));
      return;
    }
    const event = new ErrorEvent("error", {
      cancelable: true,
      message: messageOf(error),
      error,
    });
    reporting = true;
    let notHandled;
    try {
      notHandled = globalThis.dispatchEvent(event);
    } finally {
      reporting = false;
    }
    if (notHandled) {
      globalThis.console.error(stackOf(error));
    }
  };

  // ---- Host dispatch hooks -------------------------------------------------
  //
  // The engine holds the failing values (a thrown exception, a rejected
  // promise) but knows nothing about `Event`; these three functions are the
  // seam it calls into. Each returns true when a listener claimed the failure
  // with `preventDefault()`, which tells the host not to report it. `harden.js`
  // locks the bindings, so guest code cannot swap a hook for one that lies —
  // though it can, of course, just call `preventDefault()` itself, which is the
  // supported way to take responsibility.

  // An exception that escaped a callback the host invoked (a timer). There is
  // no caller left to throw to, so the platform reports it instead.
  globalThis.__dispatch_error_event = (error) => {
    if (reporting) return false; // see reportError: an error while reporting
    const event = new ErrorEvent("error", {
      cancelable: true,
      message: messageOf(error),
      error,
    });
    reporting = true;
    try {
      return !globalThis.dispatchEvent(event);
    } finally {
      reporting = false;
    }
  };

  globalThis.__dispatch_unhandled_rejection = (reason, promise) => {
    const event = new PromiseRejectionEvent("unhandledrejection", {
      cancelable: true,
      promise,
      reason,
    });
    return !globalThis.dispatchEvent(event);
  };

  // A handler attached after the rejection was already reported: the report is
  // retracted. Not cancelable — it has already happened.
  globalThis.__dispatch_rejection_handled = (promise) => {
    globalThis.dispatchEvent(
      new PromiseRejectionEvent("rejectionhandled", { promise }),
    );
    return false;
  };

  // WinterTC exposes the global as `globalThis`; `self` is a common alias.
  if (typeof globalThis.self === "undefined") {
    globalThis.self = globalThis;
  }
})();
