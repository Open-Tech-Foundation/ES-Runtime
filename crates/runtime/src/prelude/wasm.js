// WebAssembly — the JS API (https://webassembly.github.io/spec/js-api/).
//
// V8 supplies the `WebAssembly` namespace itself (Module, Instance, Memory,
// Table, Global, the error types, and the sync/async compile entry points), so
// this fragment does not reimplement any of it. It adds the two things an
// embedder must provide:
//
//   1. Loop bookkeeping. V8 runs async compilation on its own threads and
//      reports completion as a foreground task. The host drains that queue each
//      tick, but it has no way to know a compile is outstanding, so the driver
//      would be free to exit — or park forever — while one is still in flight.
//      The wrappers below tell the host when a compile enters and leaves flight
//      via `__wasm_pending`.
//
//   2. `compileStreaming` / `instantiateStreaming`, which are defined by the
//      *fetch* integration and so are absent from bare V8.
(() => {
  "use strict";

  const WebAssembly = globalThis.WebAssembly;
  if (!WebAssembly) return;

  const pending = globalThis.__wasm_pending;

  // Marks `promise` as in-flight host work until it settles. `finally` keeps the
  // original settlement — value, rejection, and unhandled-rejection reporting —
  // observable to the caller, who receives the derived promise.
  const track = (promise) =>
    promise.finally(() => {
      pending(-1);
    });

  const inFlight = (promise) => {
    pending(1);
    return track(promise);
  };

  // Re-expose a native function under its own name/arity so the wrapper is not
  // observably different from what it replaces.
  const wrap = (name, arity, impl) => {
    Object.defineProperty(impl, "name", { value: name, configurable: true });
    Object.defineProperty(impl, "length", { value: arity, configurable: true });
    Object.defineProperty(WebAssembly, name, {
      value: impl,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  const compile = WebAssembly.compile;
  wrap("compile", 1, function (...args) {
    return inFlight(Reflect.apply(compile, WebAssembly, args));
  });

  const instantiate = WebAssembly.instantiate;
  wrap("instantiate", 1, function (...args) {
    return inFlight(Reflect.apply(instantiate, WebAssembly, args));
  });

  // Resolves the `source` argument of the streaming entry points to the module
  // bytes. Per the JS API, `source` is a Response or a promise for one; the
  // response must be ok and its MIME type essence exactly `application/wasm`.
  //
  // The bytes are buffered and handed to the non-streaming compiler rather than
  // fed to V8 incrementally. That is observably equivalent — only the peak
  // memory and time-to-first-byte differ — and true streaming compilation is a
  // later optimization, not a semantic gap.
  const bytesFrom = async (source) => {
    const response = await source;
    if (!(response instanceof globalThis.Response)) {
      throw new TypeError(
        "WebAssembly streaming source must be a Response or a Promise for one",
      );
    }
    const mime = (response.headers.get("content-type") || "")
      .split(";")[0]
      .trim()
      .toLowerCase();
    if (mime !== "application/wasm") {
      throw new TypeError(
        `WebAssembly streaming expects a Content-Type of application/wasm, got ${
          mime || "none"
        }`,
      );
    }
    if (!response.ok) {
      throw new TypeError(
        `WebAssembly streaming source returned HTTP ${response.status}`,
      );
    }
    return response.arrayBuffer();
  };

  wrap("compileStreaming", 1, function (source) {
    pending(1);
    return track(bytesFrom(source).then((bytes) => compile.call(WebAssembly, bytes)));
  });

  wrap("instantiateStreaming", 1, function (source, imports) {
    pending(1);
    return track(
      bytesFrom(source).then((bytes) =>
        instantiate.call(WebAssembly, bytes, imports),
      ),
    );
  });
})();
