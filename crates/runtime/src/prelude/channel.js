// MessageChannel / MessagePort / BroadcastChannel (HTML messaging).
//
// This runtime has one agent: there are no workers and no second realm, so
// "the other side" is always in this isolate. That makes delivery a queued task
// rather than a cross-thread hop, but the observable contract is the same —
// messages are structured-cloned (a mutation after `postMessage` is not seen by
// the receiver), delivered asynchronously in order, and a port buffers until it
// is started.
//
// Not supported: transferring a MessagePort itself. `structuredClone` has no
// way to move one, so a port in the transfer list is a DataCloneError. With a
// single agent there is nothing to transfer it *to*.
(() => {
  "use strict";
  const INTERNAL = Symbol("MessagePort.construct");
  const DELIVER = Symbol("deliver");
  const ENTANGLE = Symbol("entangle");

  // Extracts the transfer list from either overload: postMessage(msg, [t]) or
  // postMessage(msg, { transfer: [t] }).
  function transferList(options) {
    if (Array.isArray(options)) return options;
    if (options && typeof options === "object" && Array.isArray(options.transfer)) {
      return options.transfer;
    }
    return [];
  }

  class MessagePort extends EventTarget {
    #peer = null;
    #started = false;
    #closed = false;
    #queue = [];
    #onmessage = null;
    #onmessageerror = null;

    constructor(key) {
      super();
      if (key !== INTERNAL) throw new TypeError("Illegal constructor");
    }

    [ENTANGLE](peer) {
      this.#peer = peer;
    }

    // Called on the *receiving* port, one task after postMessage.
    [DELIVER](data) {
      if (this.#closed) return;
      if (!this.#started) {
        // A port that has not been started buffers, in order, until it is.
        this.#queue.push(data);
        return;
      }
      this.dispatchEvent(new MessageEvent("message", { data }));
    }

    postMessage(message, options) {
      if (this.#closed || this.#peer === null) return; // no peer: silently dropped
      // Cloning happens now, synchronously, so a later mutation of `message`
      // is not visible to the receiver — and a non-cloneable value throws here,
      // at the call site, rather than in a detached task.
      const data = structuredClone(message, { transfer: transferList(options) });
      const peer = this.#peer;
      setTimeout(() => peer[DELIVER](data), 0);
    }

    start() {
      if (this.#started || this.#closed) return;
      this.#started = true;
      const queued = this.#queue;
      this.#queue = [];
      for (const data of queued) {
        this.dispatchEvent(new MessageEvent("message", { data }));
      }
    }

    close() {
      this.#closed = true;
      this.#queue = [];
      // Closing one end disentangles the pair; the peer's sends now go nowhere.
      const peer = this.#peer;
      this.#peer = null;
      if (peer) peer[ENTANGLE](null);
    }

    get onmessage() {
      return this.#onmessage;
    }
    set onmessage(handler) {
      if (this.#onmessage) this.removeEventListener("message", this.#onmessage);
      this.#onmessage = typeof handler === "function" ? handler : null;
      if (this.#onmessage) this.addEventListener("message", this.#onmessage);
      // Assigning onmessage implicitly starts the port, per the spec — which is
      // why the addEventListener form needs an explicit start() and this does
      // not.
      this.start();
    }

    get onmessageerror() {
      return this.#onmessageerror;
    }
    set onmessageerror(handler) {
      if (this.#onmessageerror) {
        this.removeEventListener("messageerror", this.#onmessageerror);
      }
      this.#onmessageerror = typeof handler === "function" ? handler : null;
      if (this.#onmessageerror) {
        this.addEventListener("messageerror", this.#onmessageerror);
      }
    }
  }

  class MessageChannel {
    #port1;
    #port2;
    constructor() {
      this.#port1 = new MessagePort(INTERNAL);
      this.#port2 = new MessagePort(INTERNAL);
      this.#port1[ENTANGLE](this.#port2);
      this.#port2[ENTANGLE](this.#port1);
    }
    get port1() {
      return this.#port1;
    }
    get port2() {
      return this.#port2;
    }
  }

  // ---- BroadcastChannel ----------------------------------------------------

  // The spec scopes a BroadcastChannel to the **agent cluster**: every channel
  // of the same name, in this agent and every other. Two deliveries in one:
  //
  //   * with a BroadcastHub installed, the host is the broker and reaches every
  //     agent — including this one, so same-agent peers arrive that way too and
  //     there is a single path;
  //   * with no hub — an embedder that installed none, which is also one that
  //     has no workers — the map below keeps the agent-local behaviour this
  //     interface always had.
  //
  // Asked on first use rather than now, because "now" is snapshot-build time —
  // one blob is restored into every agent, and the builder isolate has no hub
  // at all. Baking the answer in would leave every launch agent-local.
  let hostedCache = null;
  function hosted() {
    if (hostedCache === null) hostedCache = __ops.broadcast_available();
    return hostedCache;
  }

  // name -> set of open channels, for the no-hub case.
  const channels = new Map();

  class BroadcastChannel extends EventTarget {
    #name;
    #closed = false;
    // Hub subscription id, once `#subscribe` resolves; null before that and in
    // the no-hub case.
    #id = null;
    #pending = [];
    #onmessage = null;
    #onmessageerror = null;

    constructor(name) {
      super();
      if (arguments.length < 1) {
        throw new TypeError("BroadcastChannel requires a name");
      }
      this.#name = String(name);
      if (hosted()) {
        this.#subscribe();
        return;
      }
      let peers = channels.get(this.#name);
      if (!peers) {
        peers = new Set();
        channels.set(this.#name, peers);
      }
      peers.add(this);
    }

    // Subscribe, then pump. The pump is one outstanding async op at a time, on
    // the ordinary tick contract — the same shape as the worker and WebSocket
    // pumps, so an open channel keeps its agent alive exactly as a browser tab
    // stays reachable while one is open.
    async #subscribe() {
      try {
        this.#id = await __ops.broadcast_subscribe(this.#name);
      } catch {
        return;
      }
      // Anything posted before the subscription resolved goes out now, in order.
      for (const bytes of this.#pending) __ops.broadcast_publish(this.#id, bytes);
      this.#pending = [];
      if (this.#closed) {
        __ops.broadcast_close(this.#id);
        return;
      }
      for (;;) {
        let bytes;
        try {
          bytes = await __ops.broadcast_recv(this.#id);
        } catch {
          return;
        }
        if (bytes === null || bytes === undefined || this.#closed) return;
        let data;
        try {
          data = __structuredDeserialize(bytes);
        } catch {
          this.dispatchEvent(new MessageEvent("messageerror"));
          continue;
        }
        this.dispatchEvent(new MessageEvent("message", { data }));
      }
    }

    get name() {
      return this.#name;
    }

    postMessage(message) {
      if (this.#closed) {
        throw new DOMException("The channel is closed.", "InvalidStateError");
      }
      if (hosted()) {
        // Serialized here, at post time, so a later mutation of `message` is
        // not seen by any receiver — the same guarantee the local path gets
        // from cloning eagerly below.
        const bytes = __structuredSerialize(message);
        if (this.#id === null) this.#pending.push(bytes);
        else __ops.broadcast_publish(this.#id, bytes);
        return;
      }
      const data = structuredClone(message);
      const peers = channels.get(this.#name);
      if (!peers) return;
      // Snapshot: a listener may open or close channels while we deliver.
      for (const peer of [...peers]) {
        if (peer === this || peer.#closed) continue;
        setTimeout(() => {
          if (peer.#closed) return;
          peer.dispatchEvent(new MessageEvent("message", { data }));
        }, 0);
      }
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      if (hosted()) {
        this.#pending = [];
        // Before the subscription resolves there is no id to close; `#subscribe`
        // sees `#closed` and closes it the moment there is one.
        if (this.#id !== null) __ops.broadcast_close(this.#id);
        return;
      }
      const peers = channels.get(this.#name);
      if (peers) {
        peers.delete(this);
        if (peers.size === 0) channels.delete(this.#name);
      }
    }

    get onmessage() {
      return this.#onmessage;
    }
    set onmessage(handler) {
      if (this.#onmessage) this.removeEventListener("message", this.#onmessage);
      this.#onmessage = typeof handler === "function" ? handler : null;
      if (this.#onmessage) this.addEventListener("message", this.#onmessage);
    }

    get onmessageerror() {
      return this.#onmessageerror;
    }
    set onmessageerror(handler) {
      if (this.#onmessageerror) {
        this.removeEventListener("messageerror", this.#onmessageerror);
      }
      this.#onmessageerror = typeof handler === "function" ? handler : null;
      if (this.#onmessageerror) {
        this.addEventListener("messageerror", this.#onmessageerror);
      }
    }
  }

  for (const Interface of [MessageChannel, MessagePort, BroadcastChannel]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
