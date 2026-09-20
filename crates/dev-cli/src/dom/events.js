// Realm-local UI Events dispatch for esdev's DOM. The propagation route is
// captured before listeners run, so a listener moving or removing a node does
// not rewrite the remainder of the dispatch.

const STATE = Symbol("esdev event state");
const LISTENERS = Symbol("esdev event listeners");

export function createEvents() {
  function retarget(original, current) {
    let target = original;
    while (target?.getRootNode) {
      const root = target.getRootNode();
      if (!root?.host || current === root || current?.getRootNode?.() === root) return target;
      target = root.host;
    }
    return target;
  }

  class Event {
    static NONE = 0;
    static CAPTURING_PHASE = 1;
    static AT_TARGET = 2;
    static BUBBLING_PHASE = 3;

    constructor(type, options = {}) {
      if (arguments.length === 0) throw new TypeError("Event constructor requires a type");
      Object.defineProperty(this, STATE, {
        value: {
          type: String(type), bubbles: Boolean(options.bubbles), cancelable: Boolean(options.cancelable), composed: Boolean(options.composed),
          target: null, currentTarget: null, phase: Event.NONE, path: [], defaultPrevented: false,
          propagationStopped: false, immediateStopped: false, passive: false, dispatching: false,
          timeStamp: globalThis.performance?.now?.() ?? Date.now(),
        },
      });
    }
    get type() { return this[STATE].type; }
    get bubbles() { return this[STATE].bubbles; }
    get cancelable() { return this[STATE].cancelable; }
    get composed() { return this[STATE].composed; }
    get target() { return this[STATE].target; }
    get currentTarget() { return this[STATE].currentTarget; }
    get eventPhase() { return this[STATE].phase; }
    get defaultPrevented() { return this[STATE].defaultPrevented; }
    get timeStamp() { return this[STATE].timeStamp; }
    get isTrusted() { return false; }
    composedPath() { return [...this[STATE].path]; }
    stopPropagation() { this[STATE].propagationStopped = true; }
    stopImmediatePropagation() { this[STATE].propagationStopped = true; this[STATE].immediateStopped = true; }
    preventDefault() { if (this.cancelable && !this[STATE].passive) this[STATE].defaultPrevented = true; }
  }

  class CustomEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.detail = options.detail ?? null; }
  }
  class UIEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.view = options.view ?? null; this.detail = Number(options.detail ?? 0); }
  }
  class MouseEvent extends UIEvent {
    constructor(type, options = {}) {
      super(type, options);
      this.screenX = Number(options.screenX ?? 0); this.screenY = Number(options.screenY ?? 0);
      this.clientX = Number(options.clientX ?? 0); this.clientY = Number(options.clientY ?? 0);
      this.ctrlKey = Boolean(options.ctrlKey); this.shiftKey = Boolean(options.shiftKey); this.altKey = Boolean(options.altKey); this.metaKey = Boolean(options.metaKey);
      this.button = Number(options.button ?? 0); this.buttons = Number(options.buttons ?? 0); this.relatedTarget = options.relatedTarget ?? null;
    }
  }
  class KeyboardEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); this.key = String(options.key ?? ""); this.code = String(options.code ?? ""); this.repeat = Boolean(options.repeat); }
  }
  class InputEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); this.data = options.data ?? null; this.inputType = String(options.inputType ?? ""); this.isComposing = Boolean(options.isComposing); }
  }
  class FocusEvent extends UIEvent {
    constructor(type, options = {}) { super(type, options); this.relatedTarget = options.relatedTarget ?? null; }
  }
  class PointerEvent extends MouseEvent {
    constructor(type, options = {}) { super(type, options); this.pointerId = Number(options.pointerId ?? 0); this.width = Number(options.width ?? 1); this.height = Number(options.height ?? 1); this.pressure = Number(options.pressure ?? 0); this.pointerType = String(options.pointerType ?? ""); this.isPrimary = Boolean(options.isPrimary); }
  }
  class SubmitEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.submitter = options.submitter ?? null; }
  }
  class ErrorEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.message = String(options.message ?? ""); this.filename = String(options.filename ?? ""); this.lineno = Number(options.lineno ?? 0); this.colno = Number(options.colno ?? 0); this.error = options.error ?? null; }
  }
  class PromiseRejectionEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.promise = options.promise; this.reason = options.reason; }
  }

  class EventTarget {
    constructor() { Object.defineProperty(this, LISTENERS, { value: new Map() }); }
    addEventListener(type, callback, options = {}) {
      if (callback == null) return;
      if (typeof callback !== "function" && typeof callback.handleEvent !== "function") throw new TypeError("The listener must be a function or EventListener object");
      const capture = typeof options === "boolean" ? options : Boolean(options.capture);
      const list = this[LISTENERS].get(String(type)) ?? [];
      if (list.some((listener) => listener.callback === callback && listener.capture === capture)) return;
      const listener = { callback, capture, once: Boolean(options.once), passive: Boolean(options.passive), signal: options.signal ?? null };
      if (listener.signal?.aborted) return;
      if (listener.signal) listener.abort = () => this.removeEventListener(type, callback, { capture });
      listener.signal?.addEventListener?.("abort", listener.abort, { once: true });
      list.push(listener);
      this[LISTENERS].set(String(type), list);
    }
    removeEventListener(type, callback, options = {}) {
      const capture = typeof options === "boolean" ? options : Boolean(options.capture);
      const list = this[LISTENERS].get(String(type));
      if (!list) return;
      const index = list.findIndex((listener) => listener.callback === callback && listener.capture === capture);
      if (index < 0) return;
      const [listener] = list.splice(index, 1);
      listener.signal?.removeEventListener?.("abort", listener.abort);
      if (list.length === 0) this[LISTENERS].delete(String(type));
    }
    dispatchEvent(event) {
      if (!(event instanceof Event)) throw new TypeError("dispatchEvent expects an Event from this DOM realm");
      const state = event[STATE];
      if (state.dispatching) throw new DOMException("The event is already being dispatched.", "InvalidStateError");
      const path = [this];
      for (let current = this._eventParent?.(event) ?? null; current; current = current._eventParent?.(event) ?? null) path.push(current);
      state.dispatching = true; state.target = this; state.path = path;
      try {
        for (let index = path.length - 1; index > 0 && !state.propagationStopped; index -= 1) this._invoke(path[index], event, Event.CAPTURING_PHASE, true);
        if (!state.propagationStopped) {
          this._invoke(this, event, Event.AT_TARGET, true);
          if (!state.immediateStopped) this._invoke(this, event, Event.AT_TARGET, false);
        }
        if (state.bubbles) for (let index = 1; index < path.length && !state.propagationStopped; index += 1) this._invoke(path[index], event, Event.BUBBLING_PHASE, false);
      } finally {
        state.target = this; state.currentTarget = null; state.phase = Event.NONE; state.passive = false; state.dispatching = false;
      }
      return !state.defaultPrevented;
    }
    _invoke(target, event, phase, capture) {
      const state = event[STATE];
      state.target = retarget(this, target); state.currentTarget = target; state.phase = phase;
      const listeners = [...(target[LISTENERS].get(event.type) ?? [])];
      for (const listener of listeners) {
        if (listener.capture !== capture || !target[LISTENERS].get(event.type)?.includes(listener)) continue;
        if (listener.once) target.removeEventListener(event.type, listener.callback, { capture: listener.capture });
        state.passive = listener.passive;
        if (typeof listener.callback === "function") listener.callback.call(target, event);
        else listener.callback.handleEvent.call(listener.callback, event);
        state.passive = false;
        if (state.immediateStopped) break;
      }
      const handler = !capture && target[`on${event.type}`];
      if (!state.immediateStopped && typeof handler === "function") handler.call(target, event);
    }
  }

  return { EventTarget, Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, InputEvent, FocusEvent, PointerEvent, SubmitEvent, ErrorEvent, PromiseRejectionEvent };
}
