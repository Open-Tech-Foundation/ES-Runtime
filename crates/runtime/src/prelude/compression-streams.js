// CompressionStream / DecompressionStream (Compression Streams — part of the
// WinterTC Minimum Common API). Each stream is a TransformStream over a
// stateful native flate2 context: `compression_write` feeds a chunk and
// returns whatever output the codec produced, `compression_finish` flushes the
// tail (erroring on truncated input — the spec's flush-time check), and the
// transformer's cancel hook frees the context on abort/cancel, where flush
// never runs. Formats: "brotli", "gzip", "deflate" (zlib), "deflate-raw".
(() => {
  "use strict";

  const ops = globalThis.__ops;
  const FORMATS = ["brotli", "deflate", "deflate-raw", "gzip"];

  function toBytes(chunk, iface) {
    if (ArrayBuffer.isView(chunk)) {
      return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    }
    if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
    throw new TypeError(
      `Failed to execute 'transform' on '${iface}': chunk must be a BufferSource`,
    );
  }

  function makeTransform(iface, format, decompress) {
    if (format === undefined) {
      throw new TypeError(
        `Failed to construct '${iface}': 1 argument required, but only 0 present.`,
      );
    }
    format = `${format}`;
    if (!FORMATS.includes(format)) {
      throw new TypeError(
        `Failed to construct '${iface}': Unsupported compression format: '${format}'`,
      );
    }
    const id = ops.compression_new(format, decompress);
    let finished = false;
    return new TransformStream({
      transform(chunk, controller) {
        const out = ops.compression_write(id, toBytes(chunk, iface));
        if (out.length > 0) controller.enqueue(out);
      },
      flush(controller) {
        finished = true;
        const out = ops.compression_finish(id);
        if (out.length > 0) controller.enqueue(out);
      },
      cancel() {
        if (!finished) {
          finished = true;
          ops.compression_free(id);
        }
      },
    });
  }

  class CompressionStream {
    #transform;
    constructor(format) {
      this.#transform = makeTransform("CompressionStream", format, false);
    }
    get readable() {
      return this.#transform.readable;
    }
    get writable() {
      return this.#transform.writable;
    }
  }

  class DecompressionStream {
    #transform;
    constructor(format) {
      this.#transform = makeTransform("DecompressionStream", format, true);
    }
    get readable() {
      return this.#transform.readable;
    }
    get writable() {
      return this.#transform.writable;
    }
  }

  globalThis.CompressionStream = CompressionStream;
  globalThis.DecompressionStream = DecompressionStream;
})();
