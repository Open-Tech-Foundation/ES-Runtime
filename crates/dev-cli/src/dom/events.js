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

// "Retarget A against B": the nearest thing in A's line that B is allowed to
// see. A node inside a shadow tree that B cannot reach is reported as that
// tree's host, which is what keeps a closed tree's internals out of a listener
// that sits outside it.
function retarget(a, b) {
  let current = a;
  for (let guard = 0; guard < 64; guard += 1) {
    const root = rootOf(current);
    if (!root?.host) return current;
    if (reaches(b, root)) return current;
    current = root.host;
  }
  return current;
}

// Whether a node's own tree is a shadow tree, which is what decides whether the
// targets are wiped after a dispatch.
function inShadowTree(node) {
  const root = rootOf(node);
  return Boolean(root?.host);
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

// An exception inside a listener is reported, not thrown at the dispatcher. The
// runtime's `reportError` is the platform's own path: it dispatches an
// `ErrorEvent` on the global and falls back to the console when nothing claims
// it.
function reportUncaught(error) {
  if (typeof globalThis.reportError === "function") globalThis.reportError(error);
  else globalThis.console?.error?.(error);
}

// Web IDL constants: on the interface and its prototype, so `node.ELEMENT_NODE`
// reads as `Node.ELEMENT_NODE` does, and neither writable nor configurable.
function defineConstants(Interface, names) {
  for (const name of names) {
    const value = Interface[name];
    for (const target of [Interface, Interface.prototype]) {
      Object.defineProperty(target, name, { value, writable: false, enumerable: true, configurable: false });
    }
  }
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
      Object.defineProperty(this, "isTrusted", { get: isTrusted, configurable: true, enumerable: true });
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
    // Legacy, and still specified: it is the stop-propagation flag under an
    // older name, and a compatibility layer asking whether propagation was
    // stopped reads it rather than calling the modern method.
    get cancelBubble() { return this[STATE].propagationStopped; }
    set cancelBubble(value) { if (value) this[STATE].propagationStopped = true; }
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
  // `EventModifierInit`, shared by mouse and keyboard events: the four modifier
  // flags every filter checks — false, never undefined, when not given — and
  // the rest, which only `getModifierState` reads.
  const MODIFIER = Symbol("modifiers");
  const MODIFIER_KEYS = ["AltGraph", "CapsLock", "Fn", "FnLock", "Hyper", "NumLock", "ScrollLock", "Super", "Symbol", "SymbolLock"];
  function initModifiers(event, options) {
    event.ctrlKey = Boolean(options.ctrlKey); event.shiftKey = Boolean(options.shiftKey);
    event.altKey = Boolean(options.altKey); event.metaKey = Boolean(options.metaKey);
    const held = new Set();
    for (const key of MODIFIER_KEYS) if (options[`modifier${key}`]) held.add(key);
    Object.defineProperty(event, MODIFIER, { value: held });
  }
  function getModifierState(key) {
    switch (String(key)) {
      case "Control": return this.ctrlKey;
      case "Shift": return this.shiftKey;
      case "Alt": return this.altKey;
      case "Meta": return this.metaKey;
      default: return this[MODIFIER].has(String(key));
    }
  }

  class MouseEvent extends UIEvent {
    constructor(type, options = {}) {
      super(type, options);
      this.screenX = Number(options.screenX ?? 0); this.screenY = Number(options.screenY ?? 0);
      this.clientX = Number(options.clientX ?? 0); this.clientY = Number(options.clientY ?? 0);
      initModifiers(this, options);
      this.button = Number(options.button ?? 0); this.buttons = Number(options.buttons ?? 0); this.relatedTarget = options.relatedTarget ?? null;
    }
    getModifierState(key) { return getModifierState.call(this, key); }
  }
  class KeyboardEvent extends UIEvent {
    static DOM_KEY_LOCATION_STANDARD = 0;
    static DOM_KEY_LOCATION_LEFT = 1;
    static DOM_KEY_LOCATION_RIGHT = 2;
    static DOM_KEY_LOCATION_NUMPAD = 3;
    constructor(type, options = {}) {
      super(type, options);
      this.key = String(options.key ?? ""); this.code = String(options.code ?? "");
      this.location = Number(options.location ?? 0) >>> 0;
      initModifiers(this, options);
      this.repeat = Boolean(options.repeat); this.isComposing = Boolean(options.isComposing);
      // The legacy codes are still in Chrome's dictionary, and still read.
      this.charCode = Number(options.charCode ?? 0) >>> 0; this.keyCode = Number(options.keyCode ?? 0) >>> 0;
    }
    getModifierState(key) { return getModifierState.call(this, key); }
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
  // `CommandEvent` is what a `<button command=… commandfor=…>` dispatches at the
  // element it names, and what a component listens for instead of wiring a
  // click handler to an id.
  class CommandEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      this.source = options.source ?? null;
      this.command = String(options.command ?? "");
    }
  }

  // What a popover, a dialog and a `<details>` fire either side of opening or
  // closing: `beforetoggle` and `toggle` both carry the two states.
  class ToggleEvent extends Event {
    constructor(type, options = {}) {
      super(type, options);
      this.oldState = String(options.oldState ?? "");
      this.newState = String(options.newState ?? "");
      this.source = options.source ?? null;
    }
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
      // The related target is retargeted against this target before anything is
      // dispatched, and when the two turn out to be the same node the dispatch
      // does not happen at all: an event between two nodes of one closed tree is
      // not an event anybody outside it can see.
      // `relatedTarget` is an own property of the events that have one, not part
      // of the shared state, so it is read and written where it lives.
      const declared = "relatedTarget" in event ? event.relatedTarget : null;
      const related = declared === null || declared === undefined ? declared : retarget(declared, this);
      if (related === this && this !== declared) {
        state.dispatching = false;
        return !state.defaultPrevented;
      }
      if (declared !== null && declared !== undefined) event.relatedTarget = related;
      // Whether the targets are wiped afterwards, which they are when either of
      // them lives in a shadow tree — the nodes are not the listener's to keep.
      const clearTargets = inShadowTree(this) || inShadowTree(related);
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
        state.target = clearTargets ? null : this;
        if (clearTargets && "relatedTarget" in event) event.relatedTarget = null;
        state.currentTarget = null; state.phase = Event.NONE; state.path = []; state.passive = false; state.dispatching = false;
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
        } catch (error) {
          // "Report the exception", which a browser does by raising it as an
          // uncaught error on the global — not by handing it to whoever called
          // `dispatchEvent`. The rest of the listeners still run, and a test
          // that asserts on `window.onerror` sees what it is written for.
          reportUncaught(error);
        } finally {
          state.passive = false;
          if (hadEvent) globalThis.event = previousEvent;
          else delete globalThis.event;
        }
        if (state.immediateStopped) break;
      }
      const handler = !capture && target[`on${event.type}`];
      if (!state.immediateStopped && typeof handler === "function") {
        try {
          handler.call(target, event);
        } catch (error) {
          reportUncaught(error);
        }
      }
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
    Object.entries({ Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, InputEvent, FocusEvent, PointerEvent, WheelEvent, DragEvent, ClipboardEvent, CommandEvent, SubmitEvent, ErrorEvent })
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

  defineConstants(Event, ["NONE", "CAPTURING_PHASE", "AT_TARGET", "BUBBLING_PHASE"]);
  defineConstants(KeyboardEvent, ["DOM_KEY_LOCATION_STANDARD", "DOM_KEY_LOCATION_LEFT", "DOM_KEY_LOCATION_RIGHT", "DOM_KEY_LOCATION_NUMPAD"]);
  defineConstants(WheelEvent, ["DOM_DELTA_PIXEL", "DOM_DELTA_LINE", "DOM_DELTA_PAGE"]);

  return { EventTarget, Event, CustomEvent, UIEvent, MouseEvent, KeyboardEvent, InputEvent, FocusEvent, PointerEvent, WheelEvent, DragEvent, ClipboardEvent, CommandEvent, ToggleEvent, SubmitEvent, ErrorEvent, PromiseRejectionEvent, asEventTarget, createLegacy };
}
