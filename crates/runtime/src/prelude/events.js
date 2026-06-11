// Event / CustomEvent / EventTarget (SPEC §2.7). Pure JS. A flat dispatch model
// (single target, no DOM tree) — capture/bubble phases exist in the API but there
// is no propagation path, which matches a non-DOM runtime.
(() => {
  "use strict";

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
      return false;
    }
    composedPath() {
      return this.#currentTarget ? [this.#currentTarget] : [];
    }
    preventDefault() {
      if (this.#cancelable) this.#defaultPrevented = true;
    }
    stopPropagation() {}
    stopImmediatePropagation() {
      this.#immediateStopped = true;
    }

    // Internal hooks for EventTarget.dispatchEvent.
    _begin(target) {
      this.#target = target;
      this.#currentTarget = target;
      this.#inDispatch = true;
      this.#immediateStopped = false;
    }
    _end() {
      this.#inDispatch = false;
      this.#currentTarget = null;
    }
    get _immediateStopped() {
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
      event._begin(this);
      if (list) {
        for (const entry of list.slice()) {
          if (event._immediateStopped) break;
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
      event._end();
      return !event.defaultPrevented;
    }
  }

  globalThis.Event = Event;
  globalThis.CustomEvent = CustomEvent;
  globalThis.EventTarget = EventTarget;
})();
