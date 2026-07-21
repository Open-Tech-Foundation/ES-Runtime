// Intrinsic-integrity hardening (SPEC §4). Runs LAST, after every global and
// host op is in place.
//
// Scope is deliberately conservative and non-breaking: this does NOT freeze the
// JS primordials (`Object`/`Array`/`Function.prototype`, …). SES-style
// primordial hardening is an opinionated policy with real guest-compat cost, so
// it is left to the embedder / Layer B rather than baked into a general-purpose
// Layer A (DECISIONS D7; SECURITY.md).
//
// The load-bearing integrity guarantee does not live in JS at all: the op table
// and the capability set live in the engine's Rust `OpState`. No amount of guest
// tampering — prototype pollution, reassigning globals, forging `__ops` — can
// grant a capability or dispatch an op the host did not register and gate. What
// this step adds is defense-in-depth around the JS surface.
(() => {
  "use strict";

  // Lock the op-table *binding*: guest code can neither replace nor delete
  // `globalThis.__ops` (which could otherwise confuse later host op
  // registration). The object itself stays extensible so the host can keep
  // registering ops onto it.
  const ops = globalThis.__ops;
  if (ops !== undefined) {
    Object.defineProperty(globalThis, "__ops", {
      value: ops,
      writable: false,
      configurable: false,
      enumerable: false,
    });
  }

  // `__wasm_pending` is deliberately *not* locked here: the engine reinstalls it
  // on every isolate, including one restored from this snapshot, and a
  // non-configurable binding would make that reinstall fail silently. It needs no
  // lock — the WebAssembly wrappers capture it in a closure before any guest code
  // runs, so reassigning the global cannot reach them, and the host counter
  // saturates so a forged call can only keep the loop alive, never end it early.

  // Freeze the runtime's plain namespace objects so their methods can't be
  // swapped out from under code that reaches them by reference. (`crypto` and
  // `performance` are already frozen at their definitions.)
  if (globalThis.console) Object.freeze(globalThis.console);
})();
