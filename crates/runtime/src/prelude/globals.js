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

  // `onerror` is a single handler slot layered over the global EventTarget,
  // so assigning it twice replaces rather than accumulates.
  let onerror = null;
  Object.defineProperty(globalThis, "onerror", {
    get() {
      return onerror;
    },
    set(handler) {
      if (onerror) globalThis.removeEventListener("error", onerror);
      onerror = typeof handler === "function" ? handler : null;
      if (onerror) globalThis.addEventListener("error", onerror);
    },
    enumerable: true,
    configurable: true,
  });

  // Guards against recursion: an "error" listener that itself throws is routed
  // back here by EventTarget.dispatchEvent, which would otherwise re-dispatch
  // forever. The nested report goes straight to the console.
  let reporting = false;

  globalThis.reportError = (error) => {
    // Report an exception the way the platform does: dispatch an "error"
    // ErrorEvent on the global scope, and fall back to console.error only if
    // nothing handled it (no listener called preventDefault). That makes
    // `addEventListener("error", …)` and `onerror` work, and keeps an
    // unhandled report visible on the console as before.
    if (reporting) {
      globalThis.console.error(
        error && typeof error === "object" && "stack" in error
          ? error.stack
          : String(error),
      );
      return;
    }
    const event = new ErrorEvent("error", {
      cancelable: true,
      message: error && typeof error === "object" && "message" in error
        ? String(error.message)
        : String(error),
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
      const message =
        error && typeof error === "object" && "stack" in error
          ? error.stack
          : String(error);
      globalThis.console.error(message);
    }
  };

  // WinterTC exposes the global as `globalThis`; `self` is a common alias.
  if (typeof globalThis.self === "undefined") {
    globalThis.self = globalThis;
  }
})();
