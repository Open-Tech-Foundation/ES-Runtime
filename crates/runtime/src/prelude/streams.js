// Streams (SPEC §2.8): ReadableStream (default), WritableStream, TransformStream,
// CountQueuingStrategy, ByteLengthQueuingStrategy — hand-written to the WHATWG
// spec's abstract operations (DECISIONS D19). Byte/BYOB streams are deferred
// (SPEC §7). Internal slots live on a module-private Symbol so guest code can't
// reach them; a later hardening pass may freeze the surface further.
(() => {
  "use strict";
  const S = Symbol("streamSlots");

  // ---- promise + queue helpers -------------------------------------------

  function defer() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => {
      resolve = res;
      reject = rej;
    });
    return { promise, resolve, reject };
  }
  const resolved = (v) => Promise.resolve(v);
  const rejected = (e) => {
    const p = Promise.reject(e);
    p.catch(() => {}); // avoid spurious unhandled-rejection noise
    return p;
  };

  function resetQueue(c) {
    c.queue = [];
    c.queueTotalSize = 0;
  }
  function enqueueValueWithSize(c, value, size) {
    if (!(size >= 0) || size === Infinity) {
      throw new RangeError("invalid chunk size");
    }
    c.queue.push({ value, size });
    c.queueTotalSize += size;
  }
  function dequeueValue(c) {
    const pair = c.queue.shift();
    c.queueTotalSize -= pair.size;
    if (c.queueTotalSize < 0) c.queueTotalSize = 0;
    return pair.value;
  }
  const peekQueueValue = (c) => c.queue[0].value;

  // ======================================================================
  // ReadableStream
  // ======================================================================

  class ReadableStream {
    constructor(underlyingSource = {}, strategy = {}) {
      const source = underlyingSource ?? {};
      this[S] = {
        state: "readable",
        storedError: undefined,
        controller: undefined,
        reader: undefined,
        disturbed: false,
      };
      if (source.type !== undefined) {
        // Only the default (non-byte) type is supported in this phase.
        throw new RangeError("byte streams are not yet supported");
      }
      const sizeAlgorithm = makeSizeAlgorithm(strategy.size);
      const highWaterMark = normalizeHWM(strategy.highWaterMark, 1);
      setUpDefaultControllerFromSource(this, source, highWaterMark, sizeAlgorithm);
    }

    get locked() {
      return this[S].reader !== undefined;
    }

    cancel(reason) {
      if (this[S].reader !== undefined && this[S].state === "readable") {
        // locked-by-reader is fine for stream.cancel only if not locked; spec
        // requires not locked.
      }
      if (this.locked) {
        return rejected(new TypeError("cannot cancel a locked stream"));
      }
      return readableStreamCancel(this, reason);
    }

    getReader(options = {}) {
      if (options && options.mode === "byob") {
        throw new TypeError("BYOB readers are not yet supported");
      }
      if (this.locked) throw new TypeError("stream is already locked");
      return new ReadableStreamDefaultReader(this);
    }

    tee() {
      return readableStreamTee(this);
    }

    pipeTo(destination, options = {}) {
      if (!(destination instanceof WritableStream)) {
        return rejected(new TypeError("pipeTo destination must be a WritableStream"));
      }
      if (this.locked || destination.locked) {
        return rejected(new TypeError("pipeTo requires unlocked streams"));
      }
      return pipeTo(this, destination, options ?? {});
    }

    pipeThrough(transform, options = {}) {
      const { writable, readable } = transform;
      if (this.locked || writable.locked) {
        throw new TypeError("pipeThrough requires unlocked streams");
      }
      pipeTo(this, writable, options ?? {}).catch(() => {});
      return readable;
    }
  }

  class ReadableStreamDefaultController {
    constructor(key) {
      if (key !== S) throw new TypeError("Illegal constructor");
    }
    get desiredSize() {
      return readableStreamDefaultControllerGetDesiredSize(this);
    }
    close() {
      if (!readableStreamDefaultControllerCanCloseOrEnqueue(this)) {
        throw new TypeError("cannot close");
      }
      readableStreamDefaultControllerClose(this);
    }
    enqueue(chunk) {
      if (!readableStreamDefaultControllerCanCloseOrEnqueue(this)) {
        throw new TypeError("cannot enqueue");
      }
      readableStreamDefaultControllerEnqueue(this, chunk);
    }
    error(e) {
      readableStreamDefaultControllerError(this, e);
    }
  }

  class ReadableStreamDefaultReader {
    constructor(stream) {
      this[S] = { stream: undefined, readRequests: [], closed: defer() };
      readableStreamReaderGenericInitialize(this, stream);
    }
    get closed() {
      return this[S].closed.promise;
    }
    read() {
      if (this[S].stream === undefined) {
        return rejected(new TypeError("reader has no stream"));
      }
      const d = defer();
      readableStreamDefaultReaderRead(this, {
        chunk: (c) => d.resolve({ value: c, done: false }),
        close: () => d.resolve({ value: undefined, done: true }),
        error: (e) => d.reject(e),
      });
      return d.promise;
    }
    cancel(reason) {
      if (this[S].stream === undefined) {
        return rejected(new TypeError("reader has no stream"));
      }
      return readableStreamCancel(this[S].stream, reason);
    }
    releaseLock() {
      if (this[S].stream === undefined) return;
      readableStreamReaderGenericRelease(this);
    }
  }

  // ---- ReadableStream abstract operations ---------------------------------

  function setUpDefaultControllerFromSource(stream, source, hwm, sizeAlgorithm) {
    const controller = new ReadableStreamDefaultController(S);
    const c = (controller[S] = {
      stream,
      queue: [],
      queueTotalSize: 0,
      started: false,
      closeRequested: false,
      pullAgain: false,
      pulling: false,
      strategyHWM: hwm,
      strategySizeAlgorithm: sizeAlgorithm,
      pullAlgorithm: () => resolved(undefined),
      cancelAlgorithm: () => resolved(undefined),
    });
    if (typeof source.pull === "function") {
      c.pullAlgorithm = () => resolved(source.pull(controller));
    }
    if (typeof source.cancel === "function") {
      c.cancelAlgorithm = (reason) => resolved(source.cancel(reason));
    }
    stream[S].controller = controller;

    const startResult =
      typeof source.start === "function" ? source.start(controller) : undefined;
    resolved(startResult).then(
      () => {
        c.started = true;
        readableStreamDefaultControllerCallPullIfNeeded(controller);
      },
      (r) => readableStreamDefaultControllerError(controller, r),
    );
  }

  function readableStreamDefaultControllerGetDesiredSize(controller) {
    const c = controller[S];
    const state = c.stream[S].state;
    if (state === "errored") return null;
    if (state === "closed") return 0;
    return c.strategyHWM - c.queueTotalSize;
  }
  function readableStreamDefaultControllerCanCloseOrEnqueue(controller) {
    const c = controller[S];
    return !c.closeRequested && c.stream[S].state === "readable";
  }
  function readableStreamDefaultControllerShouldCallPull(controller) {
    const c = controller[S];
    if (!readableStreamDefaultControllerCanCloseOrEnqueue(controller)) return false;
    if (!c.started) return false;
    if (c.stream[S].reader && c.stream[S].reader[S].readRequests.length > 0) {
      return true;
    }
    return readableStreamDefaultControllerGetDesiredSize(controller) > 0;
  }
  function readableStreamDefaultControllerCallPullIfNeeded(controller) {
    const c = controller[S];
    if (!readableStreamDefaultControllerShouldCallPull(controller)) return;
    if (c.pulling) {
      c.pullAgain = true;
      return;
    }
    c.pulling = true;
    c.pullAlgorithm().then(
      () => {
        c.pulling = false;
        if (c.pullAgain) {
          c.pullAgain = false;
          readableStreamDefaultControllerCallPullIfNeeded(controller);
        }
      },
      (e) => readableStreamDefaultControllerError(controller, e),
    );
  }
  function readableStreamDefaultControllerEnqueue(controller, chunk) {
    const c = controller[S];
    if (readableStreamHasReadRequests(c.stream)) {
      readableStreamFulfillReadRequest(c.stream, chunk, false);
    } else {
      let size;
      try {
        size = c.strategySizeAlgorithm(chunk);
      } catch (e) {
        readableStreamDefaultControllerError(controller, e);
        throw e;
      }
      try {
        enqueueValueWithSize(c, chunk, size);
      } catch (e) {
        readableStreamDefaultControllerError(controller, e);
        throw e;
      }
    }
    readableStreamDefaultControllerCallPullIfNeeded(controller);
  }
  function readableStreamDefaultControllerClose(controller) {
    const c = controller[S];
    if (!readableStreamDefaultControllerCanCloseOrEnqueue(controller)) return;
    c.closeRequested = true;
    if (c.queue.length === 0) {
      resetQueue(c);
      readableStreamClose(c.stream);
    }
  }
  function readableStreamDefaultControllerError(controller, e) {
    const c = controller[S];
    if (c.stream[S].state !== "readable") return;
    resetQueue(c);
    readableStreamError(c.stream, e);
  }
  function readableStreamDefaultControllerPull(controller, readRequest) {
    const c = controller[S];
    if (c.queue.length > 0) {
      const chunk = dequeueValue(c);
      if (c.closeRequested && c.queue.length === 0) {
        readableStreamClose(c.stream);
      } else {
        readableStreamDefaultControllerCallPullIfNeeded(controller);
      }
      readRequest.chunk(chunk);
    } else {
      readableStreamAddReadRequest(c.stream, readRequest);
      readableStreamDefaultControllerCallPullIfNeeded(controller);
    }
  }

  function readableStreamReaderGenericInitialize(reader, stream) {
    reader[S].stream = stream;
    stream[S].reader = reader;
    if (stream[S].state === "readable") {
      // pending closed promise
    } else if (stream[S].state === "closed") {
      reader[S].closed.resolve(undefined);
    } else {
      reader[S].closed.reject(stream[S].storedError);
      reader[S].closed.promise.catch(() => {});
    }
  }
  function readableStreamReaderGenericRelease(reader) {
    const stream = reader[S].stream;
    const err = new TypeError("reader released");
    if (stream[S].state === "readable") {
      reader[S].closed.reject(err);
    } else {
      reader[S].closed = defer();
      reader[S].closed.reject(err);
    }
    reader[S].closed.promise.catch(() => {});
    // error any pending reads
    for (const rr of reader[S].readRequests) rr.error(err);
    reader[S].readRequests = [];
    stream[S].reader = undefined;
    reader[S].stream = undefined;
  }
  function readableStreamDefaultReaderRead(reader, readRequest) {
    const stream = reader[S].stream;
    stream[S].disturbed = true;
    if (stream[S].state === "closed") {
      readRequest.close();
    } else if (stream[S].state === "errored") {
      readRequest.error(stream[S].storedError);
    } else {
      readableStreamDefaultControllerPull(stream[S].controller, readRequest);
    }
  }
  function readableStreamAddReadRequest(stream, readRequest) {
    stream[S].reader[S].readRequests.push(readRequest);
  }
  function readableStreamHasReadRequests(stream) {
    const r = stream[S].reader;
    return r !== undefined && r[S].readRequests.length > 0;
  }
  function readableStreamFulfillReadRequest(stream, chunk, done) {
    const rr = stream[S].reader[S].readRequests.shift();
    if (done) rr.close();
    else rr.chunk(chunk);
  }
  function readableStreamClose(stream) {
    stream[S].state = "closed";
    const reader = stream[S].reader;
    if (reader === undefined) return;
    reader[S].closed.resolve(undefined);
    for (const rr of reader[S].readRequests) rr.close();
    reader[S].readRequests = [];
  }
  function readableStreamError(stream, e) {
    stream[S].state = "errored";
    stream[S].storedError = e;
    const reader = stream[S].reader;
    if (reader === undefined) return;
    reader[S].closed.reject(e);
    reader[S].closed.promise.catch(() => {});
    for (const rr of reader[S].readRequests) rr.error(e);
    reader[S].readRequests = [];
  }
  function readableStreamCancel(stream, reason) {
    stream[S].disturbed = true;
    if (stream[S].state === "closed") return resolved(undefined);
    if (stream[S].state === "errored") return rejected(stream[S].storedError);
    readableStreamClose(stream);
    const c = stream[S].controller[S];
    resetQueue(c);
    return c.cancelAlgorithm(reason).then(() => undefined);
  }

  function readableStreamTee(stream) {
    const reader = new ReadableStreamDefaultReader(stream);
    let canceled1 = false;
    let canceled2 = false;
    let reason1;
    let reason2;
    const cancelDeferred = defer();
    let reading = false;
    let readAgain = false;

    let branch1;
    let branch2;

    function pull() {
      if (reading) {
        readAgain = true;
        return resolved(undefined);
      }
      reading = true;
      readableStreamDefaultReaderRead(reader, {
        chunk: (chunk) => {
          // Defer per spec so a re-entrant pull is observed via readAgain.
          queueMicrotask(() => {
            readAgain = false;
            if (!canceled1) {
              readableStreamDefaultControllerEnqueue(branch1[S].controller, chunk);
            }
            if (!canceled2) {
              readableStreamDefaultControllerEnqueue(branch2[S].controller, chunk);
            }
            reading = false;
            if (readAgain) pull();
          });
        },
        close: () => {
          reading = false;
          if (!canceled1) readableStreamDefaultControllerClose(branch1[S].controller);
          if (!canceled2) readableStreamDefaultControllerClose(branch2[S].controller);
          if (!canceled1 || !canceled2) cancelDeferred.resolve(undefined);
        },
        error: () => {
          reading = false;
        },
      });
      return resolved(undefined);
    }
    function cancel1(reason) {
      canceled1 = true;
      reason1 = reason;
      if (canceled2) finalize();
      return cancelDeferred.promise;
    }
    function cancel2(reason) {
      canceled2 = true;
      reason2 = reason;
      if (canceled1) finalize();
      return cancelDeferred.promise;
    }
    function finalize() {
      cancelDeferred.resolve(readableStreamCancel(stream, [reason1, reason2]));
    }

    branch1 = new ReadableStream({ pull, cancel: cancel1 });
    branch2 = new ReadableStream({ pull, cancel: cancel2 });
    reader[S].closed.promise.catch((e) => {
      readableStreamDefaultControllerError(branch1[S].controller, e);
      readableStreamDefaultControllerError(branch2[S].controller, e);
    });
    return [branch1, branch2];
  }

  // ---- queuing strategies -------------------------------------------------

  function makeSizeAlgorithm(size) {
    if (size === undefined) return () => 1;
    if (typeof size !== "function") throw new TypeError("size must be a function");
    return (chunk) => size(chunk);
  }
  function normalizeHWM(hwm, defaultValue) {
    if (hwm === undefined) return defaultValue;
    const n = Number(hwm);
    if (Number.isNaN(n) || n < 0) throw new RangeError("invalid highWaterMark");
    return n;
  }

  class CountQueuingStrategy {
    constructor(init) {
      this[S] = { highWaterMark: init.highWaterMark };
    }
    get highWaterMark() {
      return this[S].highWaterMark;
    }
    get size() {
      return () => 1;
    }
  }
  class ByteLengthQueuingStrategy {
    constructor(init) {
      this[S] = { highWaterMark: init.highWaterMark };
    }
    get highWaterMark() {
      return this[S].highWaterMark;
    }
    get size() {
      return (chunk) => chunk.byteLength;
    }
  }

  // ======================================================================
  // WritableStream
  // ======================================================================

  class WritableStream {
    constructor(underlyingSink = {}, strategy = {}) {
      const sink = underlyingSink ?? {};
      this[S] = {
        state: "writable",
        storedError: undefined,
        controller: undefined,
        writer: undefined,
        writeRequests: [],
        inFlightWriteRequest: undefined,
        closeRequest: undefined,
        inFlightCloseRequest: undefined,
        pendingAbortRequest: undefined,
        backpressure: false,
      };
      if (sink.type !== undefined) throw new RangeError("invalid sink type");
      const sizeAlgorithm = makeSizeAlgorithm(strategy.size);
      const highWaterMark = normalizeHWM(strategy.highWaterMark, 1);
      setUpWritableControllerFromSink(this, sink, highWaterMark, sizeAlgorithm);
    }

    get locked() {
      return this[S].writer !== undefined;
    }
    abort(reason) {
      if (this.locked) {
        return rejected(new TypeError("cannot abort a locked stream"));
      }
      return writableStreamAbort(this, reason);
    }
    close() {
      if (this.locked) {
        return rejected(new TypeError("cannot close a locked stream"));
      }
      if (writableStreamCloseQueuedOrInFlight(this)) {
        return rejected(new TypeError("stream already closing"));
      }
      return writableStreamClose(this);
    }
    getWriter() {
      if (this.locked) throw new TypeError("stream already locked");
      return new WritableStreamDefaultWriter(this);
    }
  }

  class WritableStreamDefaultController {
    constructor(key) {
      if (key !== S) throw new TypeError("Illegal constructor");
    }
    get signal() {
      return this[S].abortController.signal;
    }
    error(e) {
      if (this[S].stream[S].state === "writable") {
        writableStreamDefaultControllerError(this, e);
      }
    }
  }

  class WritableStreamDefaultWriter {
    constructor(stream) {
      this[S] = { stream: undefined, ready: defer(), closed: defer() };
      const slots = this[S];
      slots.stream = stream;
      stream[S].writer = this;
      const state = stream[S].state;
      if (state === "writable") {
        if (writableStreamCloseQueuedOrInFlight(stream) === false && stream[S].backpressure) {
          // ready stays pending
        } else {
          slots.ready.resolve(undefined);
        }
      } else if (state === "erroring") {
        slots.ready.reject(stream[S].storedError);
        slots.ready.promise.catch(() => {});
      } else if (state === "closed") {
        slots.ready.resolve(undefined);
        slots.closed.resolve(undefined);
      } else {
        const err = stream[S].storedError;
        slots.ready.reject(err);
        slots.ready.promise.catch(() => {});
        slots.closed.reject(err);
        slots.closed.promise.catch(() => {});
      }
    }

    get closed() {
      return this[S].closed.promise;
    }
    get desiredSize() {
      if (this[S].stream === undefined) throw new TypeError("no stream");
      return writableStreamDefaultControllerGetDesiredSize(this[S].stream[S].controller);
    }
    get ready() {
      return this[S].ready.promise;
    }
    write(chunk) {
      if (this[S].stream === undefined) {
        return rejected(new TypeError("no stream"));
      }
      return writableStreamDefaultWriterWrite(this, chunk);
    }
    close() {
      const stream = this[S].stream;
      if (stream === undefined) return rejected(new TypeError("no stream"));
      if (writableStreamCloseQueuedOrInFlight(stream)) {
        return rejected(new TypeError("already closing"));
      }
      return writableStreamClose(stream);
    }
    abort(reason) {
      if (this[S].stream === undefined) {
        return rejected(new TypeError("no stream"));
      }
      return writableStreamAbort(this[S].stream, reason);
    }
    releaseLock() {
      const stream = this[S].stream;
      if (stream === undefined) return;
      const err = new TypeError("writer released");
      writableStreamDefaultWriterEnsureReadyPromiseRejected(this, err);
      writableStreamDefaultWriterEnsureClosedPromiseRejected(this, err);
      stream[S].writer = undefined;
      this[S].stream = undefined;
    }
  }

  // ---- WritableStream abstract operations ---------------------------------

  function setUpWritableControllerFromSink(stream, sink, hwm, sizeAlgorithm) {
    const controller = new WritableStreamDefaultController(S);
    const c = (controller[S] = {
      stream,
      queue: [],
      queueTotalSize: 0,
      started: false,
      strategyHWM: hwm,
      strategySizeAlgorithm: sizeAlgorithm,
      abortController: new AbortController(),
      writeAlgorithm: () => resolved(undefined),
      closeAlgorithm: () => resolved(undefined),
      abortAlgorithm: () => resolved(undefined),
    });
    stream[S].controller = controller;
    if (typeof sink.write === "function") {
      c.writeAlgorithm = (chunk) => resolved(sink.write(chunk, controller));
    }
    if (typeof sink.close === "function") {
      c.closeAlgorithm = () => resolved(sink.close());
    }
    if (typeof sink.abort === "function") {
      c.abortAlgorithm = (reason) => resolved(sink.abort(reason));
    }
    writableStreamUpdateBackpressure(
      stream,
      writableStreamDefaultControllerGetBackpressure(controller),
    );

    const startResult =
      typeof sink.start === "function" ? sink.start(controller) : undefined;
    resolved(startResult).then(
      () => {
        c.started = true;
        writableStreamDefaultControllerAdvanceQueueIfNeeded(controller);
      },
      (r) => {
        c.started = true;
        writableStreamDealWithRejection(stream, r);
      },
    );
  }

  function writableStreamDefaultControllerGetDesiredSize(controller) {
    return controller[S].strategyHWM - controller[S].queueTotalSize;
  }
  function writableStreamDefaultControllerGetBackpressure(controller) {
    return writableStreamDefaultControllerGetDesiredSize(controller) <= 0;
  }
  function writableStreamDefaultControllerGetChunkSize(controller, chunk) {
    try {
      return controller[S].strategySizeAlgorithm(chunk);
    } catch (e) {
      if (controller[S].stream[S].state === "writable") {
        writableStreamDefaultControllerError(controller, e);
      }
      return 1;
    }
  }
  function writableStreamDefaultControllerWrite(controller, chunk, chunkSize) {
    const c = controller[S];
    try {
      enqueueValueWithSize(c, { chunk }, chunkSize);
    } catch (e) {
      if (c.stream[S].state === "writable") {
        writableStreamDefaultControllerError(controller, e);
      }
      return;
    }
    const stream = c.stream;
    if (!writableStreamCloseQueuedOrInFlight(stream) && stream[S].state === "writable") {
      writableStreamUpdateBackpressure(
        stream,
        writableStreamDefaultControllerGetBackpressure(controller),
      );
    }
    writableStreamDefaultControllerAdvanceQueueIfNeeded(controller);
  }
  function writableStreamDefaultControllerClose(controller) {
    enqueueValueWithSize(controller[S], "close", 0);
    writableStreamDefaultControllerAdvanceQueueIfNeeded(controller);
  }
  function writableStreamDefaultControllerAdvanceQueueIfNeeded(controller) {
    const c = controller[S];
    const stream = c.stream;
    if (!c.started) return;
    if (stream[S].inFlightWriteRequest !== undefined) return;
    if (stream[S].state === "erroring") {
      writableStreamFinishErroring(stream);
      return;
    }
    if (c.queue.length === 0) return;
    const value = peekQueueValue(c);
    if (value === "close") {
      writableStreamDefaultControllerProcessClose(controller);
    } else {
      writableStreamDefaultControllerProcessWrite(controller, value.chunk);
    }
  }
  function writableStreamDefaultControllerProcessWrite(controller, chunk) {
    const c = controller[S];
    const stream = c.stream;
    writableStreamMarkFirstWriteRequestInFlight(stream);
    c.writeAlgorithm(chunk).then(
      () => {
        writableStreamFinishInFlightWrite(stream);
        dequeueValue(c);
        if (!writableStreamCloseQueuedOrInFlight(stream) && stream[S].state === "writable") {
          writableStreamUpdateBackpressure(
            stream,
            writableStreamDefaultControllerGetBackpressure(controller),
          );
        }
        writableStreamDefaultControllerAdvanceQueueIfNeeded(controller);
      },
      (reason) => {
        if (stream[S].state === "writable") {
          // clear algorithms
        }
        writableStreamFinishInFlightWriteWithError(stream, reason);
      },
    );
  }
  function writableStreamDefaultControllerProcessClose(controller) {
    const c = controller[S];
    const stream = c.stream;
    writableStreamMarkCloseRequestInFlight(stream);
    dequeueValue(c);
    c.closeAlgorithm().then(
      () => writableStreamFinishInFlightClose(stream),
      (reason) => writableStreamFinishInFlightCloseWithError(stream, reason),
    );
  }
  function writableStreamDefaultControllerError(controller, error) {
    writableStreamStartErroring(controller[S].stream, error);
  }

  function writableStreamAddWriteRequest(stream) {
    const d = defer();
    stream[S].writeRequests.push(d);
    return d.promise;
  }
  function writableStreamCloseQueuedOrInFlight(stream) {
    return (
      stream[S].closeRequest !== undefined ||
      stream[S].inFlightCloseRequest !== undefined
    );
  }
  function writableStreamMarkFirstWriteRequestInFlight(stream) {
    stream[S].inFlightWriteRequest = stream[S].writeRequests.shift();
  }
  function writableStreamMarkCloseRequestInFlight(stream) {
    stream[S].inFlightCloseRequest = stream[S].closeRequest;
    stream[S].closeRequest = undefined;
  }
  function writableStreamFinishInFlightWrite(stream) {
    stream[S].inFlightWriteRequest.resolve(undefined);
    stream[S].inFlightWriteRequest = undefined;
  }
  function writableStreamFinishInFlightWriteWithError(stream, error) {
    stream[S].inFlightWriteRequest.reject(error);
    stream[S].inFlightWriteRequest = undefined;
    writableStreamDealWithRejection(stream, error);
  }
  function writableStreamFinishInFlightClose(stream) {
    stream[S].inFlightCloseRequest.resolve(undefined);
    stream[S].inFlightCloseRequest = undefined;
    if (stream[S].state === "erroring") {
      stream[S].storedError = undefined;
      if (stream[S].pendingAbortRequest !== undefined) {
        stream[S].pendingAbortRequest.deferred.resolve(undefined);
        stream[S].pendingAbortRequest = undefined;
      }
    }
    stream[S].state = "closed";
    const writer = stream[S].writer;
    if (writer !== undefined) writer[S].closed.resolve(undefined);
  }
  function writableStreamFinishInFlightCloseWithError(stream, error) {
    stream[S].inFlightCloseRequest.reject(error);
    stream[S].inFlightCloseRequest = undefined;
    if (stream[S].pendingAbortRequest !== undefined) {
      stream[S].pendingAbortRequest.deferred.reject(error);
      stream[S].pendingAbortRequest = undefined;
    }
    writableStreamDealWithRejection(stream, error);
  }
  function writableStreamDealWithRejection(stream, error) {
    if (stream[S].state === "writable") {
      writableStreamStartErroring(stream, error);
      return;
    }
    writableStreamFinishErroring(stream);
  }
  function writableStreamStartErroring(stream, reason) {
    const controller = stream[S].controller;
    stream[S].state = "erroring";
    stream[S].storedError = reason;
    const writer = stream[S].writer;
    if (writer !== undefined) {
      writableStreamDefaultWriterEnsureReadyPromiseRejected(writer, reason);
    }
    if (!writableStreamHasOperationMarkedInFlight(stream) && controller[S].started) {
      writableStreamFinishErroring(stream);
    }
  }
  function writableStreamFinishErroring(stream) {
    stream[S].state = "errored";
    const storedError = stream[S].storedError;
    for (const req of stream[S].writeRequests) req.reject(storedError);
    stream[S].writeRequests = [];
    const abortRequest = stream[S].pendingAbortRequest;
    if (abortRequest === undefined) {
      writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      return;
    }
    stream[S].pendingAbortRequest = undefined;
    if (abortRequest.wasAlreadyErroring) {
      abortRequest.deferred.reject(storedError);
      writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      return;
    }
    stream[S].controller[S].abortAlgorithm(abortRequest.reason).then(
      () => {
        abortRequest.deferred.resolve(undefined);
        writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      },
      (reason) => {
        abortRequest.deferred.reject(reason);
        writableStreamRejectCloseAndClosedPromiseIfNeeded(stream);
      },
    );
  }
  function writableStreamHasOperationMarkedInFlight(stream) {
    return (
      stream[S].inFlightWriteRequest !== undefined ||
      stream[S].inFlightCloseRequest !== undefined
    );
  }
  function writableStreamRejectCloseAndClosedPromiseIfNeeded(stream) {
    if (stream[S].closeRequest !== undefined) {
      stream[S].closeRequest.reject(stream[S].storedError);
      stream[S].closeRequest = undefined;
    }
    const writer = stream[S].writer;
    if (writer !== undefined) {
      writer[S].closed.reject(stream[S].storedError);
      writer[S].closed.promise.catch(() => {});
    }
  }
  function writableStreamUpdateBackpressure(stream, backpressure) {
    const writer = stream[S].writer;
    if (writer !== undefined && backpressure !== stream[S].backpressure) {
      if (backpressure) {
        writer[S].ready = defer();
      } else {
        writer[S].ready.resolve(undefined);
      }
    }
    stream[S].backpressure = backpressure;
  }
  function writableStreamAbort(stream, reason) {
    if (stream[S].state === "closed" || stream[S].state === "errored") {
      return resolved(undefined);
    }
    stream[S].controller[S].abortController.abort(reason);
    if (stream[S].pendingAbortRequest !== undefined) {
      return stream[S].pendingAbortRequest.deferred.promise;
    }
    let wasAlreadyErroring = false;
    if (stream[S].state === "erroring") {
      wasAlreadyErroring = true;
      reason = undefined;
    }
    const d = defer();
    stream[S].pendingAbortRequest = { deferred: d, reason, wasAlreadyErroring };
    if (!wasAlreadyErroring) writableStreamStartErroring(stream, reason);
    return d.promise;
  }
  function writableStreamClose(stream) {
    const d = defer();
    stream[S].closeRequest = d;
    const writer = stream[S].writer;
    if (writer !== undefined && stream[S].backpressure && stream[S].state === "writable") {
      writer[S].ready.resolve(undefined);
    }
    writableStreamDefaultControllerClose(stream[S].controller);
    return d.promise;
  }
  function writableStreamDefaultWriterWrite(writer, chunk) {
    const stream = writer[S].stream;
    const controller = stream[S].controller;
    const chunkSize = writableStreamDefaultControllerGetChunkSize(controller, chunk);
    if (stream !== writer[S].stream) {
      return rejected(new TypeError("writer mismatch"));
    }
    const state = stream[S].state;
    if (state === "errored") return rejected(stream[S].storedError);
    if (writableStreamCloseQueuedOrInFlight(stream) || state === "closed") {
      return rejected(new TypeError("stream is closing or closed"));
    }
    if (state === "erroring") return rejected(stream[S].storedError);
    const promise = writableStreamAddWriteRequest(stream);
    writableStreamDefaultControllerWrite(controller, chunk, chunkSize);
    return promise;
  }
  function writableStreamDefaultWriterEnsureReadyPromiseRejected(writer, error) {
    const ready = writer[S].ready;
    // If already resolved/pending, replace with a rejected one.
    writer[S].ready = defer();
    writer[S].ready.reject(error);
    writer[S].ready.promise.catch(() => {});
    void ready;
  }
  function writableStreamDefaultWriterEnsureClosedPromiseRejected(writer, error) {
    writer[S].closed = defer();
    writer[S].closed.reject(error);
    writer[S].closed.promise.catch(() => {});
  }

  // ======================================================================
  // TransformStream
  // ======================================================================

  class TransformStream {
    constructor(transformer = {}, writableStrategy = {}, readableStrategy = {}) {
      const t = transformer ?? {};
      if (t.readableType !== undefined || t.writableType !== undefined) {
        throw new RangeError("transformer types are not supported");
      }
      const slots = (this[S] = {
        controller: undefined,
        backpressure: undefined,
        backpressureChange: defer(),
        readable: undefined,
        writable: undefined,
      });

      const readableHWM = normalizeHWM(readableStrategy.highWaterMark, 0);
      const readableSize = makeSizeAlgorithm(readableStrategy.size);
      const writableHWM = normalizeHWM(writableStrategy.highWaterMark, 1);
      const writableSize = makeSizeAlgorithm(writableStrategy.size);

      const controller = new TransformStreamDefaultController(S);
      slots.controller = controller;
      controller[S] = {
        stream: this,
        transformAlgorithm: (chunk) => {
          try {
            transformStreamDefaultControllerEnqueue(controller, chunk);
            return resolved(undefined);
          } catch (e) {
            return rejected(e);
          }
        },
        flushAlgorithm: () => resolved(undefined),
      };
      if (typeof t.transform === "function") {
        controller[S].transformAlgorithm = (chunk) =>
          resolved(t.transform(chunk, controller));
      }
      if (typeof t.flush === "function") {
        controller[S].flushAlgorithm = () => resolved(t.flush(controller));
      }

      slots.writable = new WritableStream(
        {
          write: (chunk) => {
            const readableController = slots.readable[S].controller;
            void readableController;
            return controller[S].transformAlgorithm(chunk).then(undefined, (e) => {
              transformStreamError(this, e);
              throw e;
            });
          },
          close: () => {
            return controller[S].flushAlgorithm().then(
              () => {
                if (slots.readable[S].state === "readable") {
                  readableStreamDefaultControllerClose(slots.readable[S].controller);
                }
              },
              (e) => {
                transformStreamError(this, e);
                throw e;
              },
            );
          },
          abort: (reason) => {
            transformStreamError(this, reason);
            return resolved(undefined);
          },
        },
        { highWaterMark: writableHWM, size: writableSize },
      );

      slots.readable = new ReadableStream(
        {
          pull: () => {
            transformStreamSetBackpressure(this, false);
            return slots.backpressureChange.promise;
          },
          cancel: (reason) => {
            transformStreamError(this, reason);
            return resolved(undefined);
          },
        },
        { highWaterMark: readableHWM, size: readableSize },
      );

      slots.backpressure = undefined;
      transformStreamSetBackpressure(this, true);

      const startResult =
        typeof t.start === "function" ? t.start(controller) : undefined;
      resolved(startResult).catch((e) => transformStreamError(this, e));
    }

    get readable() {
      return this[S].readable;
    }
    get writable() {
      return this[S].writable;
    }
  }

  class TransformStreamDefaultController {
    constructor(key) {
      if (key !== S) throw new TypeError("Illegal constructor");
    }
    get desiredSize() {
      return readableStreamDefaultControllerGetDesiredSize(
        this[S].stream[S].readable[S].controller,
      );
    }
    enqueue(chunk) {
      transformStreamDefaultControllerEnqueue(this, chunk);
    }
    error(reason) {
      transformStreamError(this[S].stream, reason);
    }
    terminate() {
      const stream = this[S].stream;
      const readableController = stream[S].readable[S].controller;
      if (readableStreamDefaultControllerCanCloseOrEnqueue(readableController)) {
        readableStreamDefaultControllerClose(readableController);
      }
      transformStreamError(stream, new TypeError("stream terminated"));
    }
  }

  function transformStreamDefaultControllerEnqueue(controller, chunk) {
    const stream = controller[S].stream;
    const readableController = stream[S].readable[S].controller;
    if (!readableStreamDefaultControllerCanCloseOrEnqueue(readableController)) {
      throw new TypeError("readable side is not in a state that permits enqueue");
    }
    try {
      readableStreamDefaultControllerEnqueue(readableController, chunk);
    } catch (e) {
      transformStreamError(stream, e);
      throw stream[S].readable[S].storedError;
    }
    const backpressure = readableStreamDefaultControllerGetDesiredSize(readableController) <= 0;
    if (backpressure !== stream[S].backpressure) {
      transformStreamSetBackpressure(stream, true);
    }
  }
  function transformStreamError(stream, e) {
    readableStreamDefaultControllerError(stream[S].readable[S].controller, e);
    transformStreamErrorWritableAndUnblockWrite(stream, e);
  }
  function transformStreamErrorWritableAndUnblockWrite(stream, e) {
    const writableController = stream[S].writable[S].controller;
    if (stream[S].writable[S].state === "writable") {
      writableStreamDefaultControllerError(writableController, e);
    }
    transformStreamSetBackpressure(stream, false);
  }
  function transformStreamSetBackpressure(stream, backpressure) {
    if (stream[S].backpressure === backpressure) return;
    if (stream[S].backpressure !== undefined) {
      stream[S].backpressureChange.resolve(undefined);
    }
    stream[S].backpressureChange = defer();
    stream[S].backpressure = backpressure;
  }

  // ======================================================================
  // pipeTo / pipeThrough plumbing
  // ======================================================================

  function pipeTo(source, dest, options) {
    const preventClose = Boolean(options.preventClose);
    const preventAbort = Boolean(options.preventAbort);
    const preventCancel = Boolean(options.preventCancel);
    const signal = options.signal;

    const reader = source.getReader();
    const writer = dest.getWriter();
    source[S].disturbed = true;
    const done = defer();
    let shuttingDown = false;

    if (signal !== undefined) {
      if (signal.aborted) {
        abortAll(signal.reason);
      } else {
        signal.addEventListener("abort", () => abortAll(signal.reason), { once: true });
      }
    }

    function abortAll(reason) {
      const error = reason ?? new DOMException("pipe aborted", "AbortError");
      const actions = [];
      if (!preventAbort && dest[S].state === "writable") {
        actions.push(() => writableStreamAbort(dest, error));
      }
      if (!preventCancel && source[S].state === "readable") {
        actions.push(() => readableStreamCancel(source, error));
      }
      shutdownWithAction(() => Promise.all(actions.map((a) => a())), error);
    }

    function pump() {
      if (shuttingDown) return;
      writer[S].ready.promise.then(() => {
        if (shuttingDown) return;
        readableStreamDefaultReaderRead(reader, {
          chunk: (chunk) => {
            writableStreamDefaultWriterWrite(writer, chunk).catch(() => {});
            pump();
          },
          close: () => finishWithClose(),
          error: (e) => shutdown(e),
        });
      }, (e) => shutdown(e));
    }

    function finishWithClose() {
      if (shuttingDown) return;
      shuttingDown = true;
      if (!preventClose) {
        writableStreamDefaultWriterCloseLike(writer).then(
          () => finalize(),
          (e) => finalize(e),
        );
      } else {
        finalize();
      }
    }
    function shutdownWithAction(action, originalError) {
      if (shuttingDown) return;
      shuttingDown = true;
      action().then(
        () => finalize(originalError),
        (e) => finalize(e),
      );
    }
    function shutdown(error) {
      if (shuttingDown) return;
      shuttingDown = true;
      finalize(error);
    }
    function finalize(error) {
      writableStreamDefaultWriterReleaseLike(writer);
      readableStreamReaderGenericRelease(reader);
      if (error !== undefined) done.reject(error);
      else done.resolve(undefined);
    }

    pump();
    return done.promise;
  }

  function writableStreamDefaultWriterCloseLike(writer) {
    const stream = writer[S].stream;
    if (writableStreamCloseQueuedOrInFlight(stream)) {
      return rejected(new TypeError("already closing"));
    }
    return writableStreamClose(stream);
  }
  function writableStreamDefaultWriterReleaseLike(writer) {
    const stream = writer[S].stream;
    if (stream === undefined) return;
    const err = new TypeError("writer released");
    writableStreamDefaultWriterEnsureReadyPromiseRejected(writer, err);
    writableStreamDefaultWriterEnsureClosedPromiseRejected(writer, err);
    stream[S].writer = undefined;
    writer[S].stream = undefined;
  }

  globalThis.ReadableStream = ReadableStream;
  globalThis.ReadableStreamDefaultController = ReadableStreamDefaultController;
  globalThis.ReadableStreamDefaultReader = ReadableStreamDefaultReader;
  globalThis.WritableStream = WritableStream;
  globalThis.WritableStreamDefaultController = WritableStreamDefaultController;
  globalThis.WritableStreamDefaultWriter = WritableStreamDefaultWriter;
  globalThis.TransformStream = TransformStream;
  globalThis.TransformStreamDefaultController = TransformStreamDefaultController;
  globalThis.CountQueuingStrategy = CountQueuingStrategy;
  globalThis.ByteLengthQueuingStrategy = ByteLengthQueuingStrategy;
})();
