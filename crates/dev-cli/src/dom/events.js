// Realm-local UI Events dispatch for esdev's DOM. The propagation route is
// captured before listeners run, so a listener moving or removing a node does
// not rewrite the remainder of the dispatch.

const STATE = Symbol("esdev event state");
const LISTENERS = Symbol("esdev event listeners");
const isTrusted = () => false;

// Whether `node` is inside a closed root that `from` is not also inside.
function visibleFrom(node, from) {
  for (let root = rootOf(node); root?.host; root = rootOf(root.host)) {
    if (root.mode === "closed" && !reaches(from, root)) return false;
  }
  return true;
}

// A ShadowRoot is its own root; anything else asks the tree.
function rootOf(node) {
  if (node?.host) return node;
  return node?.getRootNode?.() ?? null;
}

// Whether walking `node` up through hosts arrives at `root`.
function reaches(node, root) {
  for (let current = rootOf(node); current; current = rootOf(current.host)) {
    if (current === root) return true;
    if (!current.host) return false;
  }
  return false;
}

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
          // An event made by `createEvent` is uninitialized until `initEvent`;
          // one made by a constructor is initialized by construction.
          initialized: true,
          propagationStopped: false, immediateStopped: false, passive: false, dispatching: false,
          // The test clock (and its Date) starts at zero. Event timestamps are
          // positive on construction, so keep a nonzero value until it moves.
          timeStamp: globalThis.performance?.now?.() || Date.now() || Number.EPSILON,
        },
      });
      // Web IDL exposes this as a same-realm own accessor. Keeping one shared
      // getter also makes the descriptor stable across Event instances.
      Object.defineProperty(this, "isTrusted", { get: isTrusted });
    }
    get type() { return this[STATE].type; }
    get bubbles() { return this[STATE].bubbles; }
    get cancelable() { return this[STATE].cancelable; }
    get composed() { return this[STATE].composed; }
    get target() { return this[STATE].target; }
    get srcElement() { return this[STATE].target; }
    get currentTarget() { return this[STATE].currentTarget; }
    get eventPhase() { return this[STATE].phase; }
    get defaultPrevented() { return this[STATE].defaultPrevented; }
    get timeStamp() { return this[STATE].timeStamp; }
    // Relative to the listener: a closed root is invisible from outside it, so
    // the path a listener on the document sees stops at the host.
    composedPath() {
      const state = this[STATE];
      const current = state.currentTarget;
      if (!current) return [...state.path];
      return state.path.filter((node) => visibleFrom(node, current));
    }
    stopPropagation() { this[STATE].propagationStopped = true; }
    stopImmediatePropagation() { this[STATE].propagationStopped = true; this[STATE].immediateStopped = true; }
    preventDefault() { if (this.cancelable && !this[STATE].passive) this[STATE].defaultPrevented = true; }
    get returnValue() { return !this[STATE].defaultPrevented; }
    set returnValue(value) { if (!value) this.preventDefault(); }
    initEvent(type, bubbles = false, cancelable = false) {
      const state = this[STATE];
      if (state.dispatching) return;
      state.type = String(type);
      state.bubbles = Boolean(bubbles);
      state.cancelable = Boolean(cancelable);
      state.defaultPrevented = false;
      state.initialized = true;
    }
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
  class WheelEvent extends MouseEvent {
    static DOM_DELTA_PIXEL = 0;
    static DOM_DELTA_LINE = 1;
    static DOM_DELTA_PAGE = 2;
    constructor(type, options = {}) { super(type, options); this.deltaX = Number(options.deltaX ?? 0); this.deltaY = Number(options.deltaY ?? 0); this.deltaZ = Number(options.deltaZ ?? 0); this.deltaMode = Number(options.deltaMode ?? 0); }
  }
  // There is no DataTransfer or Clipboard in a layout-free DOM, so the payload
  // is whatever the constructor was handed, and null by default.
  class DragEvent extends MouseEvent {
    constructor(type, options = {}) { super(type, options); this.dataTransfer = options.dataTransfer ?? null; }
  }
  class ClipboardEvent extends Event {
    constructor(type, options = {}) { super(type, options); this.clipboardData = options.clipboardData ?? null; }
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
      const opts = typeof options === "boolean" ? { capture: options } : options ?? {};
      // The signal dictionary member is not nullable. Resolve it before
      // returning for a null callback: Web IDL conversion is observable.
      const signal = opts.signal;
      const passive = Boolean(opts.passive);
      if (signal === null || (signal !== undefined && !(signal instanceof AbortSignal))) throw new TypeError("signal must be an AbortSignal");
      if (callback == null) return;
      if (typeof callback !== "function" && typeof callback !== "object") throw new TypeError("The listener must be a function or EventListener object");
      const capture = Boolean(opts.capture);
      const list = this[LISTENERS].get(String(type)) ?? [];
      if (list.some((listener) => listener.callback === callback && listener.capture === capture)) return;
      const listener = { callback, capture, once: Boolean(opts.once), passive, signal };
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
      if (!state.initialized) throw new DOMException("The event has not been initialized. Call initEvent() first.", "InvalidStateError");
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
        state.target = this; state.currentTarget = null; state.phase = Event.NONE; state.path = []; state.passive = false; state.dispatching = false;
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
        const hadEvent = Object.hasOwn(globalThis, "event");
        const previousEvent = globalThis.event;
        globalThis.event = event;
        try {
          if (typeof listener.callback === "function") listener.callback.call(target, event);
          else listener.callback.handleEvent.call(listener.callback, event);
        } finally {
          state.passive = false;
          if (hadEvent) globalThis.event = previousEvent;
          else delete globalThis.event;
        }
        if (state.immediateStopped) break;
      }
      const handler = !capture && target[`on${event.type}`];
      if (!state.immediateStopped && typeof handler === "function") handler.call(target, event);
    }
  }

  // `document.createEvent` takes an interface *name*, and the interfaces this
  // DOM has are the ones it creates. The HTML4-era aliases are refused by name
  // with the constructor to use instead, because a test reaching for
  // `"HTMLEvents"` is reaching for `new Event(…)` through a door that closed in
  // 2016 — and a message that says so is worth more than a tree it cannot use.
  const LEGACY_ALIASES = new Map(Object.entries({
    events: "Event",
    htmlevents: "Event",
    svgevents: "Event",
    uievents: "UIEvent",
    mouseevents: "MouseEvent",
    keyevents: "KeyboardEvent",
    mutationevents: "MutationEvent",
    customevents: "CustomEvent",
  }));

  const INTERFACES = new Map(
    Object.entries({ Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, InputEvent, FocusEvent, PointerEvent, WheelEvent, DragEvent, ClipboardEvent, SubmitEvent, ErrorEvent })
      .map(([name, constructor]) => [name.toLowerCase(), constructor]),
  );

  function createLegacy(interfaceName) {
    const wanted = String(interfaceName);
    const name = wanted.toLowerCase();
    const alias = LEGACY_ALIASES.get(name);
    if (alias) {
      throw new DOMException(
        `"${wanted}" is the HTML4 name for an event interface. Use new ${alias}(type, …), or createEvent("${alias}").`,
        "NotSupportedError",
      );
    }
    const Interface = INTERFACES.get(name);
    if (!Interface) {
      throw new DOMException(`"${wanted}" is not an event interface this DOM creates.`, "NotSupportedError");
    }
    const event = new Interface("");
    // Uninitialized until `initEvent`, so dispatching it before that is the
    // InvalidStateError the specification asks for rather than a "" event.
    event[STATE].initialized = false;
    return event;
  }

  // The window is the realm's global object, not an instance of anything this
  // module can construct, so it is made an event target in place: libraries
  // compare `event.currentTarget === window` and read `window` out of
  // `composedPath()`, and a separate instance standing in for it fails both.
  function asEventTarget(target) {
    Object.defineProperty(target, LISTENERS, { value: new Map() });
    // Bound, because unqualified `addEventListener(…)` in sloppy guest code
    // passes no receiver: a browser resolves that to the window, and an
    // unbound strict method would see `undefined` and throw.
    for (const name of ["addEventListener", "removeEventListener", "dispatchEvent", "_invoke"]) {
      Object.defineProperty(target, name, { value: EventTarget.prototype[name].bind(target), writable: true, configurable: true });
    }
    return target;
  }

  return { EventTarget, Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, InputEvent, FocusEvent, PointerEvent, WheelEvent, DragEvent, ClipboardEvent, SubmitEvent, ErrorEvent, PromiseRejectionEvent, asEventTarget, createLegacy };
}
