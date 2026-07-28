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

  // name -> set of open channels. One agent, so every channel with the same
  // name is a peer; a channel never receives its own messages.
  const channels = new Map();

  class BroadcastChannel extends EventTarget {
    #name;
    #closed = false;
    #onmessage = null;
    #onmessageerror = null;

    constructor(name) {
      super();
      if (arguments.length < 1) {
        throw new TypeError("BroadcastChannel requires a name");
      }
      this.#name = String(name);
      let peers = channels.get(this.#name);
      if (!peers) {
        peers = new Set();
        channels.set(this.#name, peers);
      }
      peers.add(this);
    }

    get name() {
      return this.#name;
    }

    postMessage(message) {
      if (this.#closed) {
        throw new DOMException("The channel is closed.", "InvalidStateError");
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
