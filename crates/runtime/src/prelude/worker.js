// Worker and DedicatedWorkerGlobalScope (HTML §8.2).
//
// Two halves in one fragment, because they are two views of one channel: the
// `Worker` object a parent holds, and the global scope a worker *is*.
//
// Which half installs is decided per launch by `__ops.worker_scope_info()` —
// null on the agent driving the process, an object inside a worker. That is the
// spec's own way of telling the two apart, and the reason there is no
// `isMainThread`: a worker is recognised by the shape of its global scope, in
// HTML as in Deno and Bun. (`isMainThread` is a Node-ism, from a design that
// has no worker global scope at all.)
//
// Messages cross as bytes. The object graph is flattened by
// `__structuredSerialize` *before* the op, because an op argument is a
// marshaled host value and could not carry a Map, a cycle or a class instance —
// see the engine's `serialize` module.
(() => {
  "use strict";

  // ---- shared ---------------------------------------------------------------

  // Transfer semantics live in structured-clone.js, shared with
  // `structuredClone` and `MessagePort.postMessage` — one reading of the spec
  // rather than three. Both overloads are spelled in the wild:
  // postMessage(msg, [t]) and postMessage(msg, { transfer: [t] }).
  function serialize(message, options) {
    const list = Array.isArray(options)
      ? options
      : options && typeof options === "object" && Array.isArray(options.transfer)
        ? options.transfer
        : [];
    return __internal.transfer.serialize(message, list);
  }

  // A single-handler slot (`onmessage = fn`) over an EventTarget: assigning
  // twice replaces rather than accumulates, and the listener list is separate.
  function handlerSlot(target, type) {
    let current = null;
    return {
      get: () => current,
      set(fn) {
        if (current) target.removeEventListener(type, current);
        current = typeof fn === "function" ? fn : null;
        if (current) target.addEventListener(type, current);
      },
    };
  }

  const scope = __ops.worker_scope_info();

  // ---- the parent's half: `Worker` ------------------------------------------

  class Worker extends EventTarget {
    #id = null;
    #ready;
    #terminated = false;
    // Messages posted before the spawn resolves are queued, not dropped:
    // `new Worker(u); w.postMessage(x)` is the ordinary way to write this, and
    // the spec has no window in which that message is lost.
    #pending = [];

    constructor(url, options = {}) {
      super();
      if (arguments.length < 1) {
        throw new TypeError("Worker requires a script URL");
      }
      const opts = options ?? {};
      // Classic workers would need classic-script evaluation, which this
      // runtime does not do at all — every input is a module (SPEC §8), the
      // same reason `require` is absent. Deno refuses them for the same reason.
      if (opts.type !== undefined && opts.type !== "module") {
        throw new TypeError(
          `Worker type ${JSON.stringify(String(opts.type))} is not supported; ` +
            'this runtime evaluates every script as a module, so only "module" workers exist',
        );
      }

      Object.defineProperty(this, "onmessage", handlerSlot(this, "message"));
      Object.defineProperty(this, "onmessageerror", handlerSlot(this, "messageerror"));
      Object.defineProperty(this, "onerror", handlerSlot(this, "error"));

      this.#ready = this.#start(String(url), opts);
    }

    async #start(url, opts) {
      const permissions = Array.isArray(opts.permissions)
        ? opts.permissions.map(String)
        : [];
      const name = opts.name === undefined ? "" : String(opts.name);

      try {
        const absolute = new URL(url, __ops.worker_base()).href;
        const { specifier, source } = await __ops.worker_read_entry(absolute);
        this.#id = await __ops.worker_spawn(specifier, source, name, permissions);
      } catch (e) {
        // A worker that cannot start reports through `onerror`, asynchronously
        // — `new Worker()` is not allowed to throw for a script that fails to
        // fetch, and a synchronous throw would also be unobservable, since
        // there is no handler attached yet.
        //
        // Deliberately *not* rethrown. `#ready` is internal, so a rejection
        // here has no one to catch it and would surface as an unhandled
        // rejection on top of the `error` event — reporting one failure twice,
        // the second time as something the guest cannot act on.
        queueMicrotask(() => this.#fail(e && e.message ? e.message : String(e)));
        this.#terminated = true;
        this.#pending = [];
        return null;
      }

      for (const bytes of this.#pending) await __ops.worker_post(this.#id, bytes);
      this.#pending = [];
      this.#pump();
      return this.#id;
    }

    // Push events arrive as pulls: the receive pump is one outstanding async op
    // at a time, riding the ordinary tick contract, so a `Worker` adds no loop
    // of its own (the same shape as the WebSocket pump, D29).
    async #pump() {
      for (;;) {
        if (this.#terminated || this.#id === null) return;
        let event;
        try {
          event = await __ops.worker_recv(this.#id);
        } catch {
          return;
        }
        if (event === null || event === undefined) return;

        if (event.type === "message") {
          let data;
          try {
            data = __structuredDeserialize(event.data);
          } catch {
            // The payload arrived but could not be rebuilt here — exactly what
            // `messageerror` is for.
            this.dispatchEvent(new MessageEvent("messageerror"));
            continue;
          }
          this.dispatchEvent(new MessageEvent("message", { data }));
        } else if (event.type === "error") {
          this.#fail(event.data);
        } else if (event.type === "close") {
          return;
        }
      }
    }

    #fail(message) {
      // Cancelable, because `preventDefault()` is how a parent says it has
      // taken responsibility — the same contract as `unhandledrejection` and
      // the global `error` event. Without it the report below would go out even
      // for a failure the guest handled.
      const event = new ErrorEvent("error", {
        message: String(message),
        cancelable: true,
      });
      const claimed = !this.dispatchEvent(event);
      if (!claimed) {
        // Nobody took responsibility. Report it the way an uncaught error on
        // this agent would be, rather than letting a worker fail in silence.
        reportError(new Error(String(message)));
      }
    }

    postMessage(message, options) {
      const bytes = serialize(message, options);
      if (this.#terminated) return;
      if (this.#id === null) {
        this.#pending.push(bytes);
        return;
      }
      __ops.worker_post(this.#id, bytes);
    }

    terminate() {
      this.#terminated = true;
      this.#pending = [];
      // Idempotent, and safe before the spawn has resolved: the `then` runs
      // once there is an id to terminate.
      if (this.#id !== null) {
        __ops.worker_terminate(this.#id);
      } else {
        // Still starting. `#ready` settles with the id, or with null if the
        // spawn failed — in which case there is nothing left to terminate.
        this.#ready.then(
          (id) => {
            if (id !== null) __ops.worker_terminate(id);
          },
          () => {},
        );
      }
    }
  }

  // ---- the worker's half: DedicatedWorkerGlobalScope -------------------------

  function installWorkerScope(info) {
    Object.defineProperty(globalThis, "name", {
      value: info.name,
      writable: false,
      enumerable: true,
      configurable: true,
    });

    Object.defineProperty(globalThis, "postMessage", {
      value(message, options) {
        __ops.worker_self_post(serialize(message, options));
      },
      writable: true,
      enumerable: true,
      configurable: true,
    });

    Object.defineProperty(globalThis, "close", {
      value() {
        __ops.worker_self_close();
      },
      writable: true,
      enumerable: true,
      configurable: true,
    });

    Object.defineProperty(globalThis, "onmessage", handlerSlot(globalThis, "message"));
    Object.defineProperty(
      globalThis,
      "onmessageerror",
      handlerSlot(globalThis, "messageerror"),
    );

    // The worker's own pump. Pending for as long as the parent may send, which
    // is what keeps a worker with an `onmessage` alive after its module has
    // finished evaluating — the op is outstanding work, so the loop does not
    // reach quiescence.
    (async () => {
      for (;;) {
        let bytes;
        try {
          bytes = await __ops.worker_self_recv();
        } catch {
          return;
        }
        if (bytes === null || bytes === undefined) return;

        let data;
        try {
          data = __structuredDeserialize(bytes);
        } catch {
          globalThis.dispatchEvent(new MessageEvent("messageerror"));
          continue;
        }
        globalThis.dispatchEvent(new MessageEvent("message", { data }));
      }
    })();
  }

  // `Worker` goes on **every** agent, not only the one driving the process: a
  // dedicated worker may start its own, and the spec says so. What a worker
  // adds is its global scope on top — the two are not alternatives.
  //
  // Nesting stays bounded by the capability chain rather than by hiding the
  // constructor: a worker can only spawn if it was granted `workers`, and can
  // only pass on what it holds, so a chain narrows and never widens.
  Object.defineProperty(Worker.prototype, Symbol.toStringTag, {
    value: "Worker",
    configurable: true,
  });
  globalThis.Worker = Worker;

  if (scope !== null && scope !== undefined) installWorkerScope(scope);
})();
