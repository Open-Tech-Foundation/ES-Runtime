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
  // Whether `close()` has been called, and whether the port has already been
  // transferred away. Either makes it detached, and the structured-clone codec
  // refuses to transfer a detached object.
  const PORT_CLOSED = Symbol("MessagePort closed");
  const PORT_DETACHED = Symbol("MessagePort detached");
  const DELIVER_BROADCAST = Symbol("BroadcastChannel deliver");

  // Asked on first use, not now: "now" is snapshot-build time, and one blob is
  // restored into every agent. See the same note on BroadcastChannel below.
  let hostedPortsCache = null;
  function hostedPorts() {
    if (hostedPortsCache === null) hostedPortsCache = __ops.port_available();
    return hostedPortsCache;
  }

  // Extracts the transfer list from either overload: postMessage(msg, [t]) or
  // postMessage(msg, { transfer: [t] }).
  // Unwraps the `[message, ports]` pair a `postMessage` puts on the wire; the
  // ports become `event.ports`.
  function receive(bytes) {
    const [data, ports] = __structuredDeserialize(bytes);
    return { data, ports };
  }

  // Every message event the platform delivers is trusted; one a script builds
  // and dispatches itself is not.
  function fire(target, type, init) {
    target.dispatchEvent(new MessageEvent(type, init)[__internal.trustEvent]());
  }

  // What an event-handler IDL attribute stores. WebIDL's `EventHandler` is
  // `[LegacyTreatNonObjectAsNull]`: anything that is not an object becomes
  // null, and an object that is not callable is *kept* — the getter returns
  // what was assigned — but is never invoked. Only functions become listeners.
  function eventHandler(value) {
    if (typeof value === "function") return value;
    return value !== null && typeof value === "object" ? value : null;
  }

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

    get [PORT_CLOSED]() {
      return this.#closed;
    }

    get [PORT_DETACHED]() {
      return this.#detached;
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
        let message;
        try {
          message = receive(bytes);
        } catch {
          fire(this, "messageerror", {});
          continue;
        }
        fire(this, "message", message);
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
      fire(this, "message", { data });
    }

    postMessage(message, options) {
      if (arguments.length < 1) {
        throw new TypeError("MessagePort.postMessage requires a message");
      }
      // "If transfer contains this port, throw a DataCloneError." Sending a
      // port down itself would leave nothing to receive it.
      if (transferList(options).includes(this)) {
        throw new DOMException(
          "A MessagePort cannot be transferred through itself.",
          "DataCloneError",
        );
      }
      if (this.#closed || this.#detached) return;
      if (this.#id !== null) {
        // Serialized now, synchronously, so a later mutation of `message` is
        // not visible to the receiver — and a non-cloneable value throws here,
        // at the call site, rather than in a detached task. The op is
        // synchronous too, which is what keeps successive posts in order.
        const bytes = __internal.transfer.serializeMessage(message, transferList(options));
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
      for (const data of queued) fire(this, "message", { data });
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
      if (typeof this.#onmessage === "function") {
        this.removeEventListener("message", this.#onmessage);
      }
      this.#onmessage = eventHandler(handler);
      if (typeof this.#onmessage === "function") {
        this.addEventListener("message", this.#onmessage);
      }
      // Assigning onmessage implicitly starts the port, per the spec — which is
      // why the addEventListener form needs an explicit start() and this does
      // not.
      this.start();
    }

    get onmessageerror() {
      return this.#onmessageerror;
    }
    set onmessageerror(handler) {
      if (typeof this.#onmessageerror === "function") {
        this.removeEventListener("messageerror", this.#onmessageerror);
      }
      this.#onmessageerror = eventHandler(handler);
      if (typeof this.#onmessageerror === "function") {
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

  // Hosted delivery is one stream for the whole agent, not one per channel:
  // every destination of a post is delivered before any destination of the
  // next, which is the order a single event-loop task queue gives and what the
  // spec means by "in port creation order". A receive per channel would hand
  // back whichever op happened to settle first.
  //
  // subscription id -> the BroadcastChannel it belongs to.
  const subscribed = new Map();
  let pumping = false;

  async function pumpBroadcasts() {
    if (pumping) return;
    pumping = true;
    try {
      for (;;) {
        let event;
        try {
          event = await __ops.broadcast_recv_next();
        } catch {
          return;
        }
        // Null once this agent holds no open channel; a later one restarts the
        // pump.
        if (event === null || event === undefined) return;
        const channel = subscribed.get(event.id);
        if (channel === undefined) continue;
        channel[DELIVER_BROADCAST](event.data);
      }
    } finally {
      pumping = false;
    }
  }

  class BroadcastChannel extends EventTarget {
    #name;
    #closed = false;
    // Hub subscription id; null in the no-hub case.
    #id = null;
    #onmessage = null;
    #onmessageerror = null;

    constructor(name) {
      super();
      if (arguments.length < 1) {
        throw new TypeError("BroadcastChannel requires a name");
      }
      this.#name = String(name);
      if (hosted()) {
        // Synchronously, as the constructor is: a channel that subscribed a
        // turn later would miss what the next line posts, and the spec has no
        // such window.
        try {
          this.#id = __ops.broadcast_subscribe(this.#name);
        } catch {
          return;
        }
        subscribed.set(this.#id, this);
        pumpBroadcasts();
        return;
      }
      let peers = channels.get(this.#name);
      if (!peers) {
        peers = new Set();
        channels.set(this.#name, peers);
      }
      peers.add(this);
    }

    // Called by the agent's single broadcast pump, in delivery order.
    [DELIVER_BROADCAST](bytes) {
      if (this.#closed) return;
      let data;
      try {
        data = __structuredDeserialize(bytes);
      } catch {
        fire(this, "messageerror", {});
        return;
      }
      fire(this, "message", { data });
    }

    get name() {
      return this.#name;
    }

    postMessage(message) {
      if (arguments.length < 1) {
        throw new TypeError("BroadcastChannel.postMessage requires a message");
      }
      if (this.#closed) {
        throw new DOMException("The channel is closed.", "InvalidStateError");
      }
      if (hosted()) {
        // Serialized here, at post time, so a later mutation of `message` is
        // not seen by any receiver — the same guarantee the local path gets
        // from cloning eagerly below.
        const bytes = __structuredSerialize(message);
        if (this.#id !== null) __ops.broadcast_publish(this.#id, bytes);
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
          fire(peer, "message", { data });
        }, 0);
      }
    }

    close() {
      if (this.#closed) return;
      this.#closed = true;
      if (hosted()) {
        if (this.#id !== null) {
          subscribed.delete(this.#id);
          __ops.broadcast_close(this.#id);
        }
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
      if (typeof this.#onmessage === "function") {
        this.removeEventListener("message", this.#onmessage);
      }
      this.#onmessage = eventHandler(handler);
      if (typeof this.#onmessage === "function") {
        this.addEventListener("message", this.#onmessage);
      }
    }

    get onmessageerror() {
      return this.#onmessageerror;
    }
    set onmessageerror(handler) {
      if (typeof this.#onmessageerror === "function") {
        this.removeEventListener("messageerror", this.#onmessageerror);
      }
      this.#onmessageerror = eventHandler(handler);
      if (typeof this.#onmessageerror === "function") {
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
      // A detached port is a husk: `close()` detaches it, and so does having
      // been transferred already. Either way the queue belongs to someone else
      // now, and a detached object cannot be transferred.
      if (port[PORT_CLOSED] || port[PORT_DETACHED]) {
        throw new DOMException(
          "A detached MessagePort cannot be transferred.",
          "DataCloneError",
        );
      }
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

  // Transferable streams are carried by a port pair, and live in their own
  // fragment (they need ReadableStream, which loads later). They cannot reach
  // `INTERNAL`, so the two operations they need are published here.
  Object.assign(__internal.ports, {
    available: hostedPorts,
    create: () => __ops.port_create(),
    adopt: (id) => new MessagePort(INTERNAL, id),
    idOf: (port) => port[PORT_ID],
  });

  for (const Interface of [MessageChannel, MessagePort, BroadcastChannel]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
