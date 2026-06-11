// AbortController / AbortSignal (SPEC §2.6). AbortSignal extends EventTarget and
// fires an "abort" event once. `AbortSignal.timeout` uses the timer builtins;
// `AbortSignal.any` follows a set of source signals.
(() => {
  "use strict";
  const INTERNAL = Symbol("AbortSignal.construct");

  class AbortSignal extends EventTarget {
    #aborted = false;
    #reason = undefined;
    #onabort = null;

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
      if (this.#aborted) return;
      this.#aborted = true;
      this.#reason =
        reason !== undefined
          ? reason
          : new DOMException("signal is aborted without reason", "AbortError");
      this.dispatchEvent(new Event("abort"));
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
        source.addEventListener("abort", () => result._signalAbort(source.reason), {
          once: true,
        });
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

  globalThis.AbortSignal = AbortSignal;
  globalThis.AbortController = AbortController;
})();
