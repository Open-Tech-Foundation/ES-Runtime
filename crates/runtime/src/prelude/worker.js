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
    // The messaging form: transferred objects travel with the message, so a
    // port named only in the transfer list reaches the other side as
    // `event.ports` rather than being detached into nothing.
    return __internal.transfer.serializeMessage(message, list);
  }

  // ---- failures -------------------------------------------------------------
  //
  // A worker's failure crosses a thread boundary in pieces, so the `error` its
  // parent sees is necessarily a new object rather than the one that was
  // thrown. Rebuilding it is worth the trouble anyway: the parent of a worker
  // is usually a supervisor, and a supervisor branches on the error's class
  // before it does anything else with it — `err instanceof RangeError` decides
  // "never retry", where a formatted string decided nothing.
  //
  // Only the standard classes can be restored: a class the worker declared for
  // itself does not exist in this realm. Those become an `Error` carrying the
  // right `name`, which is the discriminator that survives anyway — a
  // `DOMException` is told apart by `"AbortError"`, not by its constructor.
  const ERROR_CLASSES = {
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    URIError,
  };

  // A live JS error, in the same shape one that crossed a thread arrives in —
  // so both routes into `#fail` are one path. `error` rides along here because
  // this one never left the realm: there is no need to rebuild what is already
  // the object that was thrown.
  function describe(error) {
    const isObject = error !== null && typeof error === "object";
    return {
      name: isObject && error.name !== undefined ? String(error.name) : "",
      message:
        isObject && error.message !== undefined ? String(error.message) : String(error),
      stack: isObject && typeof error.stack === "string" ? error.stack : "",
      error,
    };
  }

  // A spawn that never got off the ground, worded for whoever wrote the
  // `new Worker()`.
  //
  // Only one refusal needs the help, and it is the one people hit first:
  // starting a worker means reading its entry module, and reading a module
  // needs `imports` — so `--deny-all --allow-workers` alone is refused, naming
  // a permission the author never mentioned. Node and Deno both require a file
  // read here too; Deno is the one that says which flag fixes it, and this is
  // that sentence.
  //
  // Context only. The capability gate in the host is the whole enforcement and
  // has already refused by the time this runs — nothing here decides anything,
  // and the error's `name` and `code` are left exactly as they arrived so
  // programmatic handling is unaffected.
  function startupFailure(error, url) {
    const failure = describe(error);
    if (error?.code !== "ERR_CAPABILITY_DENIED") return failure;
    const permission = /permission "([^"]+)"/.exec(failure.message)?.[1];
    if (permission === undefined) return failure;
    // The permission is named once, in the sentence that says what to do about
    // it; what the original adds is the capability behind it, so the trailing
    // repeat of the name comes off.
    const denial = failure.message.replace(/ \(permission "[^"]+"\)$/, "");
    return {
      ...failure,
      message:
        `cannot start a worker from ${url}: reading its module needs the ` +
        `"${permission}" permission — add --allow-${permission} (${denial})`,
    };
  }

  function rebuildError(failure) {
    const Class = Object.hasOwn(ERROR_CLASSES, failure.name)
      ? ERROR_CLASSES[failure.name]
      : Error;
    const error = new Class(failure.message);
    if (failure.name && failure.name !== error.name) error.name = failure.name;
    // The worker's stack, not the pump's: the frames that matter are the ones
    // in the agent that failed, and this realm's are noise.
    if (failure.stack) error.stack = failure.stack;
    return error;
  }

  // `new Worker(url, { env })`: what the worker's `runtime:process` `env`
  // reports.
  //
  //   omitted / "inherit"  the host environment, exactly as today — which the
  //                        worker still needs the `env` capability to read, and
  //                        which the deployment's `--allow-env=<names>` still
  //                        narrows
  //   { …  }               precisely these variables, and no capability needed
  //                        to read them: a parent can only pass values it could
  //                        already read, so this attenuates rather than grants.
  //                        `{}` is a worker with no environment at all
  //
  // Deliberately not Node's `SHARE_ENV`: a shared, mutable environment is an
  // undeclared side channel between agents, and this runtime already has a
  // declared one in `postMessage`.
  function workerEnv(value) {
    if (value === undefined || value === "inherit") return null;
    if (value === null || typeof value !== "object") {
      throw new TypeError(
        'Worker option "env" must be "inherit" or an object of variables, ' +
          `not ${JSON.stringify(value)}`,
      );
    }
    return Object.entries(value).map(([name, entry]) => {
      // A `Secret` from the parent's own `env` carries its real value behind a
      // registered symbol. Passing one on means passing the value — anything
      // else would silently hand the worker the string "[redacted]" — and the
      // worker re-masks it by the same key convention on arrival.
      const secret = entry === null || typeof entry !== "object"
        ? undefined
        : entry[SECRET_VALUE];
      return [String(name), String(secret ?? entry)];
    });
  }

  // The denial vocabulary, from the host rather than transcribed here — the
  // same list `--deny-<name>` and `permissions.has()` take. Read once: it is
  // fixed for the build.
  const PERMISSION_NAMES = Object.freeze(__ops.permission_names());

  // `new Worker(url, { permissions })`: what the worker may do.
  //
  //   omitted    nothing. A worker starts confined and is granted explicitly
  //   [ … ]      exactly these, still bounded by what this agent holds
  //   "inherit"  everything this agent holds
  //
  // An unknown name **throws** rather than being skipped. Dropping it silently
  // fails closed, which sounds harmless right up until the worker takes the
  // degraded path forever and the denial surfaces three layers from the typo —
  // the same reason `permissions.has()` refuses to answer `false` for a name it
  // does not know.
  //
  // `"inherit"` expands to the whole vocabulary rather than being a flag of its
  // own: the host intersects whatever is asked for with this agent's own set,
  // so asking for everything *is* asking for the parent's set, and there is no
  // second path through which a spawn could widen anything.
  function workerPermissions(value) {
    if (value === undefined || value === null) return [];
    if (value === "inherit") return [...PERMISSION_NAMES];
    if (!Array.isArray(value)) {
      throw new TypeError(
        'Worker option "permissions" must be "inherit" or an array of permission ' +
          `names, not ${JSON.stringify(value)}`,
      );
    }
    return value.map((entry) => {
      const name = String(entry);
      if (!PERMISSION_NAMES.includes(name)) {
        throw new TypeError(
          `unknown Worker permission ${JSON.stringify(name)} — expected one of: ` +
            PERMISSION_NAMES.join(", "),
        );
      }
      return name;
    });
  }

  // `new Worker(url, { memory })`: the worker's heap ceiling, **in megabytes**
  // — the unit Node's `resourceLimits.maxOldGenerationSizeMb` uses, and the one
  // a deployment writes anyway.
  //
  // Omitted means "as much as this agent has". It only ever narrows: a worker
  // that could raise its own ceiling above its parent's would make the parent's
  // no limit at all, the same reason `permissions` cannot widen. The host
  // enforces that, since only it knows what this agent's ceiling actually is.
  function workerMemory(value) {
    if (value === undefined || value === null) return 0;
    const mb = Number(value);
    if (!Number.isInteger(mb) || mb <= 0) {
      throw new TypeError(
        'Worker option "memory" must be a positive whole number of megabytes, ' +
          `not ${JSON.stringify(value)}`,
      );
    }
    return mb;
  }

  // Every event the platform delivers is trusted; one a script builds and
  // dispatches itself is not.
  const fired = (event) => event[__internal.trustEvent]();

  // A single-handler slot (`onmessage = fn`) over an EventTarget: assigning
  // twice replaces rather than accumulates, and the listener list is separate.
  // WebIDL's `EventHandler` is `[LegacyTreatNonObjectAsNull]`: a non-object
  // becomes null, and a non-callable *object* is kept — the getter returns what
  // was assigned — but is never invoked. Only a function becomes a listener.
  function handlerSlot(target, type) {
    let current = null;
    return {
      get: () => current,
      set(value) {
        if (typeof current === "function") target.removeEventListener(type, current);
        current =
          typeof value === "function" || (value !== null && typeof value === "object")
            ? value
            : null;
        if (typeof current === "function") target.addEventListener(type, current);
      },
    };
  }

  // `runtime:process` wraps a secret-looking env value in a `Secret`, whose
  // real value sits behind this registered symbol. Registered precisely so the
  // two can meet without the prelude importing the module.
  const SECRET_VALUE = Symbol.for("runtime.secret.value");

  const scope = __ops.worker_scope_info();

  // ---- the parent's half: `Worker` ------------------------------------------

  class Worker extends EventTarget {
    #id = null;
    #ready;
    #terminated = false;
    // Whether this worker is a reason for the process to stay up. `true` from
    // the moment it starts, as in Node; `unref()` gives that up. Tracked here
    // rather than counted host-side per call so the count can never drift: the
    // host holds one number, and this flag is what says whether *this* worker
    // is one of them.
    #referenced = false;
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

      // Validated here rather than in `#start`: a malformed option is a bad
      // argument, and a bad argument throws from the call that made it. Only a
      // worker that *fails to start* reports asynchronously, through `onerror`.
      const env = workerEnv(opts.env);
      const memory = workerMemory(opts.memory);
      const permissions = workerPermissions(opts.permissions);

      // Referenced from the moment it exists, as in Node: a worker is a reason
      // for the process to stay up until it ends or is told otherwise. Released
      // exactly once, wherever this worker stops being live.
      this.#setReferenced(true);

      Object.defineProperty(this, "onmessage", handlerSlot(this, "message"));
      Object.defineProperty(this, "onmessageerror", handlerSlot(this, "messageerror"));
      Object.defineProperty(this, "onerror", handlerSlot(this, "error"));

      this.#ready = this.#start(String(url), opts, env, memory, permissions);
    }

    async #start(url, opts, env, memory, permissions) {
      const name = opts.name === undefined ? "" : String(opts.name);

      try {
        const absolute = new URL(url, __ops.worker_base()).href;
        const { specifier, source } = await __ops.worker_read_entry(absolute);
        this.#id = await __ops.worker_spawn(
          specifier,
          source,
          name,
          permissions,
          env,
          memory,
        );
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
        queueMicrotask(() => this.#fail(startupFailure(e, url)));
        this.#terminated = true;
        this.#setReferenced(false);
        this.#pending = [];
        return null;
      }

      // Flushed **synchronously**, and that is the whole point: nothing may run
      // between `#id` becoming observable and the queue being empty, or a
      // `postMessage` from a microtask would take the direct path below and
      // overtake messages posted before the spawn resolved. Awaiting each post
      // here yielded exactly that window, and reordered the ordinary
      // `new Worker(u); w.postMessage(x)` against anything posted from a `.then`.
      //
      // Un-awaited is not fire-and-forget of the ordering: pending async ops are
      // polled in the order they were registered, which is what already keeps
      // every steady-state `postMessage` in order.
      const pending = this.#pending;
      this.#pending = [];
      for (const bytes of pending) __ops.worker_post(this.#id, bytes);
      this.#pump();
      return this.#id;
    }

    // Push events arrive as pulls: the receive pump is one outstanding async op
    // at a time, riding the ordinary tick contract, so a `Worker` adds no loop
    // of its own (the same shape as the WebSocket pump, D29).
    async #pump() {
      // Every way out of this loop is the worker ceasing to be live — drained,
      // closed, terminated, or the receive itself failing — so releasing here
      // covers all four in one place rather than four.
      try {
        await this.#receive();
      } finally {
        this.#setReferenced(false);
      }
    }

    async #receive() {
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
          let ports;
          try {
            [data, ports] = __structuredDeserialize(event.data);
          } catch {
            // The payload arrived but could not be rebuilt here — exactly what
            // `messageerror` is for.
            this.dispatchEvent(fired(new MessageEvent("messageerror")));
            continue;
          }
          this.dispatchEvent(fired(new MessageEvent("message", { data, ports })));
        } else if (event.type === "error") {
          this.#fail(event.data);
        } else if (event.type === "close") {
          return;
        }
      }
    }

    #fail(failure) {
      // Cancelable, because `preventDefault()` is how a parent says it has
      // taken responsibility — the same contract as `unhandledrejection` and
      // the global `error` event. Without it the report below would go out even
      // for a failure the guest handled.
      const event = new ErrorEvent("error", {
        message: failure.message,
        filename: failure.filename,
        lineno: failure.lineno,
        colno: failure.colno,
        // `error` when the failure never left this realm (a spawn that was
        // refused); rebuilt when it crossed a thread and could not.
        error: failure.error ?? rebuildError(failure),
        cancelable: true,
      });
      const claimed = !this.dispatchEvent(event);
      if (!claimed) {
        // Nobody took responsibility, so it goes to the console rather than
        // being lost — but *only* to the console.
        //
        // Deliberately not `reportError`: that dispatches the failure as this
        // agent's own uncaught error, and an agent's own uncaught error ends it.
        // A worker that has merely *heard* about its child's failure has not
        // failed — its state is intact, and it may well be the thing that
        // restarts the child. Escalating would mean a single leaf failure took
        // down every ancestor that had not attached an `onerror`, which is a
        // blast radius nobody asked for.
        //
        // The stack when there is one: it opens with the same `name: message`
        // the event carried, and the frames below say where in the worker to
        // look.
        globalThis.console.error(failure.stack || failure.message);
      }
    }

    // The one place the host counter moves, so a double `unref()` or an
    // `unref()` after termination cannot unbalance it.
    #setReferenced(on) {
      if (this.#referenced === on) return;
      this.#referenced = on;
      __ops.worker_ref(on ? 1 : -1);
    }

    /**
     * Stop this worker from keeping the process alive. It carries on running
     * and still delivers messages — the only thing given up is being a reason
     * not to exit.
     *
     * For a pool: idle workers waiting for the next job would otherwise hold
     * the process open forever, which is exactly the shape `unref` exists for.
     * Node and Bun both have this; Deno has neither.
     */
    unref() {
      this.#setReferenced(false);
    }

    /** Undoes {@link unref}. A worker starts referenced, so this only matters
     * after an `unref()`. */
    ref() {
      this.#setReferenced(true);
    }

    /**
     * How many messages have been posted to this worker and not yet taken by
     * it.
     *
     * The only backpressure signal there is: `postMessage` never refuses a
     * message — HTML does not permit it to fail for queue depth, and Node, Deno
     * and Bun all queue without limit — so a producer that outruns its worker
     * grows memory unless it chooses to pace itself. This is what it paces
     * against, the way a socket's `bufferedAmount` works.
     *
     * ```js
     * for (const job of jobs) {
     *   w.postMessage(job);
     *   if (w.queued > 1000) await drain();
     * }
     * ```
     *
     * Messages queued before the worker has started count too: they are held
     * here rather than in the host, but they are just as much backlog.
     */
    get queued() {
      if (this.#id === null) return this.#pending.length;
      return this.#terminated ? 0 : __ops.worker_queued(this.#id);
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
      this.#setReferenced(false);
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

  // ---- WorkerLocation -------------------------------------------------------
  //
  // The worker's own script URL, read-only. A server runtime has no document to
  // locate, but a worker does have a location in the spec's sense — the module
  // it was started from — and code that resolves a sibling file with
  // `new URL("./data.bin", location)` is doing something meaningful here, not
  // borrowing a browser idiom. Deno exposes it for the same reason; the driver
  // agent still has none, since that scope has no script URL that is *the*
  // script.
  function makeLocation(href) {
    const url = new URL(href);
    class WorkerLocation {
      constructor() {
        throw new TypeError("Illegal constructor");
      }
      toString() {
        return url.href;
      }
    }
    for (const part of [
      "href",
      "origin",
      "protocol",
      "host",
      "hostname",
      "port",
      "pathname",
      "search",
      "hash",
    ]) {
      // Accessors with no setter: `location.href = "…"` is a navigation, and
      // there is nothing here to navigate.
      Object.defineProperty(WorkerLocation.prototype, part, {
        get: () => url[part],
        enumerable: true,
        configurable: true,
      });
    }
    Object.defineProperty(WorkerLocation.prototype, Symbol.toStringTag, {
      value: "WorkerLocation",
      configurable: true,
    });
    return { WorkerLocation, location: Object.create(WorkerLocation.prototype) };
  }

  // ---- the scope interfaces -------------------------------------------------
  //
  // `WorkerGlobalScope` and `DedicatedWorkerGlobalScope` exist so that the two
  // scopes can be *told apart*: `self instanceof DedicatedWorkerGlobalScope` is
  // how the platform says "am I in a worker", and WPT's own helpers, Deno and
  // real worker code all use exactly that. Without them the members were still
  // there and the question was unanswerable, which is worse than either.
  //
  // The members move onto the prototypes rather than staying own properties of
  // the global, because that is what makes `self` a readonly attribute of an
  // interface rather than a variable a script can overwrite.
  //
  // The global object cannot be a real instance — it has no private fields to
  // brand — so these prototypes hold no per-instance state and close over what
  // they need. Reaching them through the chain is what `instanceof` checks, and
  // that is the observable half.
  // A worker's `navigator` is a **WorkerNavigator**, and `Navigator` is not
  // exposed in a worker at all — one interface per scope is how the two are
  // told apart, and WPT checks both halves.
  //
  // Built from `Navigator.prototype`'s own descriptors rather than written out
  // again, so the two interfaces cannot drift; the members read no `this`, so
  // they work on this receiver. `navigator.js` cannot do this itself: it is
  // baked into the snapshot, where which scope this will be is not yet known.
  function makeNavigator() {
    class WorkerNavigator {
      constructor() {
        throw new TypeError("Illegal constructor");
      }
    }
    for (const [member, descriptor] of Object.entries(
      Object.getOwnPropertyDescriptors(Object.getPrototypeOf(globalThis.navigator)),
    )) {
      if (member === "constructor") continue;
      Object.defineProperty(WorkerNavigator.prototype, member, descriptor);
    }
    Object.defineProperty(WorkerNavigator.prototype, Symbol.toStringTag, {
      value: "WorkerNavigator",
      configurable: true,
    });
    return {
      WorkerNavigator,
      navigator: Object.create(WorkerNavigator.prototype),
    };
  }

  function installWorkerScope(info) {
    const { WorkerNavigator, navigator } = makeNavigator();
    const { WorkerLocation, location } = makeLocation(info.url);

    class WorkerGlobalScope extends EventTarget {
      constructor() {
        throw new TypeError("Illegal constructor");
      }
      get self() {
        return globalThis;
      }
      get location() {
        return location;
      }
      get navigator() {
        return navigator;
      }
    }

    class DedicatedWorkerGlobalScope extends WorkerGlobalScope {
      constructor() {
        throw new TypeError("Illegal constructor");
      }
      get name() {
        return info.name;
      }
      postMessage(message, options) {
        __ops.worker_self_post(serialize(message, options));
      }
      /**
       * How many messages this worker has sent to its parent and the parent has
       * not yet taken — the mirror of `worker.queued`, for a worker producing
       * results faster than its parent consumes them.
       */
      get queued() {
        return __ops.worker_self_queued();
      }
      close() {
        __ops.worker_self_close();
      }
    }

    for (const [Interface, tag] of [
      [WorkerGlobalScope, "WorkerGlobalScope"],
      [DedicatedWorkerGlobalScope, "DedicatedWorkerGlobalScope"],
    ]) {
      Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
        value: tag,
        configurable: true,
      });
    }

    globalThis.WorkerGlobalScope = WorkerGlobalScope;
    globalThis.DedicatedWorkerGlobalScope = DedicatedWorkerGlobalScope;
    globalThis.WorkerNavigator = WorkerNavigator;
    globalThis.WorkerLocation = WorkerLocation;
    // `Navigator` is the window's interface and is not exposed in a worker;
    // `navigator` here is the WorkerNavigator built above.
    delete globalThis.Navigator;
    Object.defineProperty(globalThis, "navigator", {
      value: navigator,
      writable: false,
      enumerable: true,
      configurable: true,
    });

    // `self` is an own, writable property on every agent (globals.js). In a
    // worker the interface's readonly accessor takes over, so the own one has
    // to go or it would keep shadowing it — and `self = 1` would keep sticking.
    delete globalThis.self;

    Object.setPrototypeOf(globalThis, DedicatedWorkerGlobalScope.prototype);

    Object.defineProperty(globalThis, "onmessage", handlerSlot(globalThis, "message"));
    Object.defineProperty(
      globalThis,
      "onmessageerror",
      handlerSlot(globalThis, "messageerror"),
    );

    // A failure nothing in this worker claimed. `reportError` has already
    // dispatched the `error` event and found no listener willing to take
    // responsibility, so it belongs to the parent: reported there, and fatal
    // here — the exception escaped every handler the author wrote, so what the
    // agent's state is from now on is anybody's guess.
    //
    // This is the route the host cannot see on its own. An exception thrown by
    // an event listener never escapes to the engine at all: `dispatchEvent`
    // catches it so that one bad listener does not cancel the rest of the
    // dispatch, and hands it to `reportError`. A throw inside `onmessage` — the
    // way a worker most commonly fails — takes exactly that path.
    //
    // Taken apart here rather than passed across: an `Error` handed to an op
    // arrives as a marshaled plain object with no `stack` and no class, so what
    // travels is the pieces the parent rebuilds one from. The host reads the
    // location out of the stack, the same way it does for a failure it saw
    // itself.
    __internal.failure.unclaimed = (error) => {
      const failure = describe(error);
      __ops.worker_self_fail(failure.name, failure.message, failure.stack);
      return true;
    };

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
        let ports;
        try {
          [data, ports] = __structuredDeserialize(bytes);
        } catch {
          globalThis.dispatchEvent(fired(new MessageEvent("messageerror")));
          continue;
        }
        globalThis.dispatchEvent(fired(new MessageEvent("message", { data, ports })));
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
