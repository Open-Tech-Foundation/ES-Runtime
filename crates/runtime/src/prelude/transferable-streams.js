// Transferable streams (Streams Standard §9): ReadableStream, WritableStream
// and TransformStream in a transfer list.
//
// A stream is not serialized — its *chunks* are, one at a time, as they flow.
// Transferring one sets up a `MessageChannel` and pipes across it: the sending
// agent keeps one port and drives it, the receiving agent gets the other and
// builds a stream over it. So the two halves below are the spec's
// SetUpCrossRealmTransformWritable and SetUpCrossRealmTransformReadable, and
// transferring a readable is "make a writable here and pipe into it", while
// transferring a writable is the mirror image.
//
// The wire protocol is the spec's, in both directions over the one port:
//
//   chunk-ward   { type: "chunk", value } | { type: "close" } | { type: "error", value }
//   back-ward    { type: "pull" }         | { type: "error", value }
//
// `pull` is the whole of backpressure: the reading side asks for the next
// chunk, and the writing side's `write()` does not settle until it does. Without
// it a fast producer would serialize its entire output into the port's queue,
// which is precisely what a stream exists to avoid.
(() => {
  "use strict";
  const PORTS = __internal.ports;
  const CODECS = __internal.hostCodecs;
  const TRANSFERRING = __internal.transferringStreams;

  function cannotTransfer(what) {
    return new DOMException(
      `A ${what} could not be transferred.`,
      "DataCloneError",
    );
  }

  // ---- the writing half -----------------------------------------------------

  // A WritableStream whose writes go over `port`. Used on the *receiving* side
  // of a transferred WritableStream, and on the *sending* side of a transferred
  // ReadableStream.
  function crossRealmWritable(port) {
    let backpressure = null;
    let releaseBackpressure = () => {};
    const nextTurn = () => {
      backpressure = new Promise((resolve) => {
        releaseBackpressure = resolve;
      });
    };
    nextTurn();

    let errored = null;
    port.onmessage = (event) => {
      const message = event.data;
      if (message.type === "pull") {
        releaseBackpressure();
        nextTurn();
      } else if (message.type === "error") {
        // The reader went away or cancelled; fail the next write rather than
        // going on producing for nobody.
        errored = message.value;
        releaseBackpressure();
      }
    };

    return new WritableStream({
      async write(chunk) {
        // Wait for the reader to ask before handing over the next chunk.
        await backpressure;
        if (errored !== null) throw errored;
        // A chunk that cannot be structured-cloned fails this write, which is
        // where the author can see it — not silently on the far side.
        port.postMessage({ type: "chunk", value: chunk });
      },
      close() {
        port.postMessage({ type: "close" });
        port.close();
      },
      abort(reason) {
        port.postMessage({ type: "error", value: reason });
        port.close();
      },
    });
  }

  // ---- the reading half -----------------------------------------------------

  // A ReadableStream fed from `port`. The mirror of the above.
  function crossRealmReadable(port) {
    return new ReadableStream({
      start(controller) {
        port.onmessage = (event) => {
          const message = event.data;
          if (message.type === "chunk") {
            controller.enqueue(message.value);
          } else if (message.type === "close") {
            // `close` on an already-closed controller throws; a producer that
            // closed twice is not this stream's problem to report.
            try {
              controller.close();
            } catch {
              /* already closed */
            }
            port.close();
          } else if (message.type === "error") {
            controller.error(message.value);
            port.close();
          }
        };
      },
      pull() {
        // One `pull` per chunk wanted: this is the backpressure signal the
        // writing half waits on.
        port.postMessage({ type: "pull" });
      },
      cancel(reason) {
        port.postMessage({ type: "error", value: reason });
        port.close();
      },
    });
  }

  // ---- the codecs -----------------------------------------------------------

  // Both halves of a fresh channel: one kept and driven here, one handed over.
  function channel() {
    const [mine, theirs] = PORTS.create();
    return { mine: PORTS.adopt(mine), theirs };
  }

  function guard(stream, what) {
    if (!TRANSFERRING.has(stream)) {
      // Like a MessagePort, a stream may be transferred and may not be cloned:
      // there is one source of its chunks, and copying the object would not
      // copy that.
      throw new DOMException(
        `A ${what} can only be transferred, not cloned.`,
        "DataCloneError",
      );
    }
    if (!PORTS.available()) throw cannotTransfer(what);
  }

  CODECS.set("ReadableStream", {
    write(stream) {
      guard(stream, "ReadableStream");
      if (stream.locked) throw cannotTransfer("locked ReadableStream");
      const { mine, theirs } = channel();
      // Piping is what locks the original, which is exactly the spec's
      // outcome: a transferred stream is no longer usable where it came from.
      stream.pipeTo(crossRealmWritable(mine)).catch(() => {
        // The failure has already been signalled over the port; swallowing it
        // here only stops an unhandled rejection about a stream the author no
        // longer holds.
      });
      return __internal.hostCodec.pack({ readable: theirs });
    },
    read(bytes) {
      const { header } = __internal.hostCodec.unpack(bytes);
      return crossRealmReadable(PORTS.adopt(header.readable));
    },
  });

  CODECS.set("WritableStream", {
    write(stream) {
      guard(stream, "WritableStream");
      if (stream.locked) throw cannotTransfer("locked WritableStream");
      const { mine, theirs } = channel();
      crossRealmReadable(mine).pipeTo(stream).catch(() => {});
      return __internal.hostCodec.pack({ writable: theirs });
    },
    read(bytes) {
      const { header } = __internal.hostCodec.unpack(bytes);
      return crossRealmWritable(PORTS.adopt(header.writable));
    },
  });

  // A TransformStream is its two ends, so it transfers as both — the receiver
  // gets a plain object with the same shape rather than a reconstructed
  // TransformStream, which is what the spec's own serialization amounts to.
  CODECS.set("TransformStream", {
    write(stream) {
      guard(stream, "TransformStream");
      const readable = channel();
      const writable = channel();
      stream.readable.pipeTo(crossRealmWritable(readable.mine)).catch(() => {});
      crossRealmReadable(writable.mine).pipeTo(stream.writable).catch(() => {});
      return __internal.hostCodec.pack({
        readable: readable.theirs,
        writable: writable.theirs,
      });
    },
    read(bytes) {
      const { header } = __internal.hostCodec.unpack(bytes);
      return {
        readable: crossRealmReadable(PORTS.adopt(header.readable)),
        writable: crossRealmWritable(PORTS.adopt(header.writable)),
      };
    },
  });

  for (const [Interface, tag] of [
    [ReadableStream, "ReadableStream"],
    [WritableStream, "WritableStream"],
    [TransformStream, "TransformStream"],
  ]) {
    Object.defineProperty(Interface.prototype, __internal.hostClone, {
      value: tag,
    });
  }
})();
