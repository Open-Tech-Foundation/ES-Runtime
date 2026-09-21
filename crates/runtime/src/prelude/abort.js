// AbortController / AbortSignal (SPEC §2.6). AbortSignal extends EventTarget and
// fires an "abort" event once. `AbortSignal.timeout` uses the timer builtins;
// `AbortSignal.any` follows a set of source signals.
(() => {
  "use strict";
  const INTERNAL = Symbol("AbortSignal.construct");
  // `esdev test --dom` replaces the public Event constructor with its
  // realm-local DOM implementation after this prelude has installed
  // AbortSignal. AbortSignal itself remains an EventTarget from this realm, so
  // retain its matching constructor instead of looking up the subsequently
  // replaced global when an abort is dispatched.
  const AbortEvent = Event;

  class AbortSignal extends EventTarget {
    #aborted = false;
    #reason = undefined;
    #onabort = null;
    #dependents = new Set();

    constructor(key) {
      if (key !== INTERNAL) {
        throw new TypeError("Illegal constructor");
      }
      super();
    }

    get aborted() {
      return this.#aborted;
    }
    get reason() {
      return this.#reason;
    }
    get onabort() {
      return this.#onabort;
    }
    set onabort(handler) {
      if (this.#onabort) this.removeEventListener("abort", this.#onabort);
      this.#onabort = typeof handler === "function" ? handler : null;
      if (this.#onabort) this.addEventListener("abort", this.#onabort);
    }

    throwIfAborted() {
      if (this.#aborted) throw this.#reason;
    }

    // Internal: abort this signal with `reason` (default AbortError).
    _signalAbort(reason) {
      // Mark the whole dependent graph before dispatching any event. A listener
      // may inspect or compose another signal while handling an abort, and it
      // must already observe every dependent as aborted. Breadth-first marking
      // also keeps event order at each dependency level deterministic.
      if (this.#aborted) return;
      this.#aborted = true;
      this.#reason =
        reason !== undefined
          ? reason
          : new DOMException("signal is aborted without reason", "AbortError");
      const pending = [this];
      for (let index = 0; index < pending.length; index += 1) {
        const signal = pending[index];
        for (const dependent of signal.#dependents) {
          if (dependent.#aborted) continue;
          dependent.#aborted = true;
          dependent.#reason = signal.#reason;
          pending.push(dependent);
        }
      }
      for (const signal of pending) {
        signal.dispatchEvent(new AbortEvent("abort")[__internal.trustEvent]());
      }
    }

    static abort(reason) {
      const signal = new AbortSignal(INTERNAL);
      signal._signalAbort(reason);
      return signal;
    }

    static timeout(milliseconds) {
      const signal = new AbortSignal(INTERNAL);
      setTimeout(() => {
        signal._signalAbort(
          new DOMException("signal timed out", "TimeoutError"),
        );
      }, milliseconds);
      return signal;
    }

    static any(signals) {
      const result = new AbortSignal(INTERNAL);
      for (const source of signals) {
        if (source.aborted) {
          result._signalAbort(source.reason);
          break;
        }
        source.#dependents.add(result);
      }
      return result;
    }
  }

  class AbortController {
    #signal = new AbortSignal(INTERNAL);

    get signal() {
      return this.#signal;
    }

    abort(reason) {
      this.#signal._signalAbort(reason);
    }
  }

  for (const Interface of [AbortSignal, AbortController]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
