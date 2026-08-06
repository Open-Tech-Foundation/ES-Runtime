// MessageChannel / MessagePort / BroadcastChannel (HTML messaging).
//
// A port's queue lives in the host, not in this isolate, which is what lets a
// port be **transferred** to another agent: what travels in a `postMessage` is
// the port's id, and whichever agent holds it is the one its peer's messages
// reach. Everything already in flight stays queued where it was.
//
// With no PortHub installed — an embedder that has no workers either, so
// nowhere to transfer a port *to* — ports fall back to the agent-local pair
// this file used to implement, and transferring one is a DataCloneError. The
// observable contract is the same either way: messages are structured-cloned
// (a mutation after `postMessage` is not seen by the receiver), delivered
// asynchronously and in order, and a port buffers until it is started.
(() => {
  "use strict";
  const INTERNAL = Symbol("MessagePort.construct");
  const DELIVER = Symbol("deliver");
  const ENTANGLE = Symbol("entangle");
  const DETACH = __internal.portDetach;
  const PORT_ID = Symbol("MessagePort id");

  // Asked on first use, not now: "now" is snapshot-build time, and one blob is
  // restored into every agent. See the same note on BroadcastChannel below.
  let hostedPortsCache = null;
  function hostedPorts() {
    if (hostedPortsCache === null) hostedPortsCache = __ops.port_available();
    return hostedPortsCache;
  }

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
    // Host-backed: the port's queue id. `null` in the agent-local fallback.
    #id = null;
    #peer = null;
    #started = false;
    #closed = false;
    // Transferred away: this object is a husk, and the agent that received the
    // id is the one the queue now belongs to.
    #detached = false;
    #pumping = false;
    #queue = [];
    #onmessage = null;
    #onmessageerror = null;

    constructor(key, id = null) {
      super();
      if (key !== INTERNAL) throw new TypeError("Illegal constructor");
      this.#id = id;
    }

    get [PORT_ID]() {
      return this.#id;
    }

    // Called by structured-clone.js once this port has been serialized into a
    // transfer. Stopping the host-side read matters: an outstanding `recv`
    // holding a message would swallow it, and the agent receiving the port must
    // find everything that was already in flight.
    [DETACH]() {
      this.#detached = true;
      if (this.#id !== null) __ops.port_detach(this.#id);
    }

    // One outstanding op at a time, on the ordinary tick contract — the same
    // shape as the worker and WebSocket pumps, so a started port keeps its
    // agent alive exactly while it could still receive something.
    async #pump() {
      if (this.#pumping) return;
      this.#pumping = true;
      for (;;) {
        if (this.#closed || this.#detached) return;
        let bytes;
        try {
          bytes = await __ops.port_recv(this.#id);
        } catch {
          return;
        }
        if (bytes === null || bytes === undefined) return;
        if (this.#closed || this.#detached) return;
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
      if (this.#closed || this.#detached) return;
      if (this.#id !== null) {
        // Serialized now, synchronously, so a later mutation of `message` is
        // not visible to the receiver — and a non-cloneable value throws here,
        // at the call site, rather than in a detached task. The op is
        // synchronous too, which is what keeps successive posts in order.
        const bytes = __internal.transfer.serialize(message, transferList(options));
        __ops.port_post(this.#id, bytes);
        return;
      }
      if (this.#peer === null) return; // no peer: silently dropped
      const data = structuredClone(message, { transfer: transferList(options) });
      const peer = this.#peer;
      setTimeout(() => peer[DELIVER](data), 0);
    }

    start() {
      if (this.#started || this.#closed) return;
      this.#started = true;
      if (this.#id !== null) {
        this.#pump();
        return;
      }
      const queued = this.#queue;
      this.#queue = [];
      for (const data of queued) {
        this.dispatchEvent(new MessageEvent("message", { data }));
      }
    }

    close() {
      this.#closed = true;
      this.#queue = [];
      if (this.#id !== null) {
        __ops.port_close(this.#id);
        return;
      }
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
      if (hostedPorts()) {
        // Synchronous, as the constructor is: allocating two queues has nothing
        // to await, and `new MessageChannel().port1` must exist immediately.
        const [a, b] = __ops.port_create();
        this.#port1 = new MessagePort(INTERNAL, a);
        this.#port2 = new MessagePort(INTERNAL, b);
        return;
      }
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

  // Structured clone: a port is a host object, and the *only* one that may not
  // be cloned. The spec allows a port to be transferred and refuses to copy it —
  // two ends of a channel cannot become three — and by the time the codec sees
  // the object, being named in the transfer list is the only difference.
  Object.defineProperty(MessagePort.prototype, __internal.hostClone, {
    value: "MessagePort",
  });
  __internal.hostCodecs.set("MessagePort", {
    write(port) {
      if (!__internal.transferringPorts.has(port)) {
        throw new DOMException(
          "A MessagePort can only be transferred, not cloned.",
          "DataCloneError",
        );
      }
      const id = port[PORT_ID];
      if (id === null) {
        // The agent-local fallback: nowhere to transfer it to, which is the
        // same situation this interface was in before workers existed.
        throw new DOMException(
          "A MessagePort could not be transferred.",
          "DataCloneError",
        );
      }
      return __internal.hostCodec.pack({ id });
    },
    read(bytes) {
      const { header } = __internal.hostCodec.unpack(bytes);
      // Arrives unstarted, as the spec requires: whatever was queued for it
      // waits until the receiver calls `start()` or assigns `onmessage`.
      return new MessagePort(INTERNAL, header.id);
    },
  });

  for (const Interface of [MessageChannel, MessagePort, BroadcastChannel]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
