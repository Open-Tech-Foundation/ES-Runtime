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

  globalThis.reportError = (error) => {
    // Minimal (SPEC §7): surface via console.error. A full implementation
    // dispatches an ErrorEvent on the global; that lands with the event loop's
    // error handling.
    const message =
      error && typeof error === "object" && "stack" in error
        ? error.stack
        : String(error);
    globalThis.console.error(message);
  };

  // WinterTC exposes the global as `globalThis`; `self` is a common alias.
  if (typeof globalThis.self === "undefined") {
    globalThis.self = globalThis;
  }
})();
