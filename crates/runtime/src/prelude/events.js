// Event / CustomEvent / EventTarget (SPEC §2.7). Pure JS. A flat dispatch model
// (single target, no DOM tree) — capture/bubble phases exist in the API but there
// is no propagation path, which matches a non-DOM runtime.
(() => {
  "use strict";
  // Fragment-local dispatch slots: EventTarget is the only caller.
  const BEGIN = Symbol("Event begin");
  const END = Symbol("Event end");
  const IMMEDIATE_STOPPED = Symbol("Event immediateStopped");
  // Lets a target dispatch on behalf of another object — used only for the
  // global scope, so `event.target` is `globalThis` rather than the internal
  // EventTarget that actually holds the listeners.
  const FACE = Symbol("dispatch face");

  class Event {
    #type;
    #bubbles;
    #cancelable;
    #composed;
    #defaultPrevented = false;
    #immediateStopped = false;
    #target = null;
    #currentTarget = null;
    #timeStamp;
    #inDispatch = false;
    #stopped = false;
    // `isTrusted`: true for an event the platform fired, false for one script
    // built and dispatched itself. Set through an internal symbol below, so
    // there is no way for guest code to claim it.
    #trusted = false;

    constructor(type, options = {}) {
      if (arguments.length < 1) {
        throw new TypeError("Event constructor requires a type");
      }
      this.#type = String(type);
      this.#bubbles = Boolean(options.bubbles);
      this.#cancelable = Boolean(options.cancelable);
      this.#composed = Boolean(options.composed);
      this.#timeStamp = globalThis.performance ? performance.now() : 0;
    }

    get type() {
      return this.#type;
    }
    get bubbles() {
      return this.#bubbles;
    }
    get cancelable() {
      return this.#cancelable;
    }
    get composed() {
      return this.#composed;
    }
    get defaultPrevented() {
      return this.#defaultPrevented;
    }
    get target() {
      return this.#target;
    }
    get srcElement() {
      return this.#target;
    }
    get currentTarget() {
      return this.#currentTarget;
    }
    get timeStamp() {
      return this.#timeStamp;
    }
    get eventPhase() {
      return this.#inDispatch ? 2 : 0; // AT_TARGET : NONE
    }
    get isTrusted() {
      return this.#trusted;
    }
    composedPath() {
      return this.#currentTarget ? [this.#currentTarget] : [];
    }
    [__internal.trustEvent]() {
      this.#trusted = true;
      return this;
    }
    preventDefault() {
      if (this.#cancelable) this.#defaultPrevented = true;
    }
    stopPropagation() {
      this.#stopped = true;
    }
    stopImmediatePropagation() {
      this.#stopped = true;
      this.#immediateStopped = true;
    }
    // Legacy aliases, still normative in the DOM standard. `cancelBubble`
    // mirrors stopPropagation(); `returnValue` is the inverse of
    // defaultPrevented.
    get cancelBubble() {
      return this.#stopped;
    }
    set cancelBubble(value) {
      if (value) this.#stopped = true;
    }
    get returnValue() {
      return !this.#defaultPrevented;
    }
    set returnValue(value) {
      if (!value) this.preventDefault();
    }
    initEvent(type, bubbles = false, cancelable = false) {
      // A no-op once the event is being dispatched, per the standard.
      if (this.#inDispatch) return;
      this.#type = String(type);
      this.#bubbles = Boolean(bubbles);
      this.#cancelable = Boolean(cancelable);
      this.#defaultPrevented = false;
    }

    // Internal slots for EventTarget.dispatchEvent.
    [BEGIN](target) {
      // Re-dispatching an event that is already being dispatched is an
      // InvalidStateError, not unbounded recursion.
      if (this.#inDispatch) {
        throw new DOMException(
          "The event is already being dispatched.",
          "InvalidStateError",
        );
      }
      this.#target = target;
      this.#currentTarget = target;
      this.#inDispatch = true;
      this.#immediateStopped = false;
      this.#stopped = false;
    }
    [END]() {
      this.#inDispatch = false;
      this.#currentTarget = null;
    }
    get [IMMEDIATE_STOPPED]() {
      return this.#immediateStopped;
    }
  }
  Object.defineProperties(Event, {
    NONE: { value: 0 },
    CAPTURING_PHASE: { value: 1 },
    AT_TARGET: { value: 2 },
    BUBBLING_PHASE: { value: 3 },
  });

  class CustomEvent extends Event {
    #detail;
    constructor(type, options = {}) {
      super(type, options);
      this.#detail = options.detail ?? null;
    }
    get detail() {
      return this.#detail;
    }
  }

  class EventTarget {
    #listeners = new Map();

    addEventListener(type, callback, options) {
      if (callback === null || callback === undefined) return;
      const opts =
        typeof options === "boolean" ? { capture: options } : options || {};
      const entry = {
        callback,
        capture: Boolean(opts.capture),
        once: Boolean(opts.once),
        passive: Boolean(opts.passive),
        signal: opts.signal || null,
      };
      if (entry.signal && entry.signal.aborted) return;

      const key = String(type);
      let list = this.#listeners.get(key);
      if (!list) {
        list = [];
        this.#listeners.set(key, list);
      }
      if (
        list.some((l) => l.callback === callback && l.capture === entry.capture)
      ) {
        return; // duplicate
      }
      list.push(entry);

      if (entry.signal) {
        entry.signal.addEventListener(
          "abort",
          () =>
            this.removeEventListener(type, callback, { capture: entry.capture }),
          { once: true },
        );
      }
    }

    removeEventListener(type, callback, options) {
      const capture =
        typeof options === "boolean"
          ? options
          : Boolean(options && options.capture);
      const list = this.#listeners.get(String(type));
      if (!list) return;
      const i = list.findIndex(
        (l) => l.callback === callback && l.capture === capture,
      );
      if (i !== -1) list.splice(i, 1);
    }

    dispatchEvent(event) {
      if (!(event instanceof Event)) {
        throw new TypeError("dispatchEvent argument must be an Event");
      }
      const list = this.#listeners.get(event.type);
      event[BEGIN](this[FACE] ?? this);
      if (list) {
        for (const entry of list.slice()) {
          if (event[IMMEDIATE_STOPPED]) break;
          if (!list.includes(entry)) continue; // removed mid-dispatch
          if (entry.once) {
            this.removeEventListener(event.type, entry.callback, {
              capture: entry.capture,
            });
          }
          const cb = entry.callback;
          const fn =
            typeof cb === "function"
              ? cb
              : cb && typeof cb.handleEvent === "function"
                ? cb.handleEvent
                : null;
          if (!fn) continue;
          try {
            fn.call(typeof cb === "function" ? this : cb, event);
          } catch (e) {
            globalThis.reportError(e);
          }
        }
      }
      event[END]();
      return !event.defaultPrevented;
    }
  }

  // MessageEvent — carries a `message` payload (used by WebSocket, DECISIONS
  // D29; reusable later for EventSource / worker postMessage).
  class MessageEvent extends Event {
    #data;
    #origin;
    #lastEventId;
    #source;
    #ports;
    constructor(type, options = {}) {
      super(type, options);
      this.#data = options.data ?? null;
      this.#origin = options.origin !== undefined ? String(options.origin) : "";
      this.#lastEventId =
        options.lastEventId !== undefined ? String(options.lastEventId) : "";
      this.#source = options.source ?? null;
      // A FrozenArray in WebIDL: the receiver may read the ports it was sent
      // and may not add to them.
      this.#ports = Object.freeze(options.ports ? [...options.ports] : []);
    }
    get data() {
      return this.#data;
    }
    get origin() {
      return this.#origin;
    }
    get lastEventId() {
      return this.#lastEventId;
    }
    get source() {
      return this.#source;
    }
    get ports() {
      return this.#ports;
    }
  }

  // CloseEvent — the WebSocket closing handshake result (DECISIONS D29).
  class CloseEvent extends Event {
    #wasClean;
    #code;
    #reason;
    constructor(type, options = {}) {
      super(type, options);
      this.#wasClean = Boolean(options.wasClean);
      this.#code = options.code !== undefined ? options.code : 0;
      this.#reason = options.reason !== undefined ? String(options.reason) : "";
    }
    get wasClean() {
      return this.#wasClean;
    }
    get code() {
      return this.#code;
    }
    get reason() {
      return this.#reason;
    }
  }

  // ErrorEvent — an uncaught exception or a `reportError()` call, carrying both
  // the thrown value and where it came from.
  class ErrorEvent extends Event {
    #message;
    #filename;
    #lineno;
    #colno;
    #error;
    constructor(type, options = {}) {
      super(type, options);
      this.#message = options.message !== undefined ? String(options.message) : "";
      this.#filename = options.filename !== undefined ? String(options.filename) : "";
      this.#lineno = options.lineno !== undefined ? Number(options.lineno) : 0;
      this.#colno = options.colno !== undefined ? Number(options.colno) : 0;
      this.#error = options.error;
    }
    get message() {
      return this.#message;
    }
    get filename() {
      return this.#filename;
    }
    get lineno() {
      return this.#lineno;
    }
    get colno() {
      return this.#colno;
    }
    get error() {
      return this.#error;
    }
  }

  // PromiseRejectionEvent — a promise rejection that reached the global scope,
  // carrying both the promise and the reason it rejected with. Fired as
  // `unhandledrejection` (cancelable: preventing the default suppresses the
  // host's report) and as `rejectionhandled` (not cancelable: the report has
  // already gone out, this retracts it).
  class PromiseRejectionEvent extends Event {
    #promise;
    #reason;
    constructor(type, options = {}) {
      super(type, options);
      if (!options || !("promise" in options)) {
        throw new TypeError(
          "Failed to construct 'PromiseRejectionEvent': required member promise is undefined",
        );
      }
      this.#promise = options.promise;
      this.#reason = options.reason;
    }
    get promise() {
      return this.#promise;
    }
    get reason() {
      return this.#reason;
    }
  }

  // ProgressEvent — progress of a length-bounded transfer.
  class ProgressEvent extends Event {
    #lengthComputable;
    #loaded;
    #total;
    constructor(type, options = {}) {
      super(type, options);
      this.#lengthComputable = Boolean(options.lengthComputable);
      this.#loaded = options.loaded !== undefined ? Number(options.loaded) : 0;
      this.#total = options.total !== undefined ? Number(options.total) : 0;
    }
    get lengthComputable() {
      return this.#lengthComputable;
    }
    get loaded() {
      return this.#loaded;
    }
    get total() {
      return this.#total;
    }
  }

  for (const Interface of [
    Event,
    CustomEvent,
    EventTarget,
    MessageEvent,
    CloseEvent,
    ErrorEvent,
    ProgressEvent,
    PromiseRejectionEvent,
  ]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }

  // ---- The global scope is an EventTarget ---------------------------------
  //
  // WindowOrWorkerGlobalScope inherits EventTarget, which is what
  // `addEventListener("error", …)` on the global relies on. The global object
  // cannot itself be an EventTarget instance here (its private fields would
  // have no brand), so listeners live on a hidden target whose dispatch reports
  // `globalThis` as the event target — the observable half of the contract.
  const globalTarget = new EventTarget();
  globalTarget[FACE] = globalThis;
  for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"]) {
    Object.defineProperty(globalThis, method, {
      value: EventTarget.prototype[method].bind(globalTarget),
      writable: true,
      enumerable: false,
      configurable: true,
    });
  }
})();
