// performance (SPEC §2.11), backed by the Clock provider. `now()` is monotonic
// milliseconds since the clock's epoch (≈ ms since construction) with
// sub-millisecond (µs) precision; `timeOrigin` is the wall-clock time at
// construction.
//
// A real class rather than an object literal, so `performance` is an instance
// of a branded `Performance` interface with its members on the prototype —
// which is what WebIDL describes and what inspectors and type checks expect.
(() => {
  "use strict";
  const ops = globalThis.__ops;
  const timeOrigin = ops.time_origin();
  // Guards the constructor: `performance` is exposed as an instance, and
  // `new Performance()` is not something a script may do.
  const INTERNAL = Symbol("Performance.construct");

  class Performance {
    constructor(key) {
      if (key !== INTERNAL) throw new TypeError("Illegal constructor");
    }
    now() {
      return ops.now();
    }
    get timeOrigin() {
      return timeOrigin;
    }
    toJSON() {
      return { timeOrigin };
    }
  }
  Object.defineProperty(Performance.prototype, Symbol.toStringTag, {
    value: "Performance",
    configurable: true,
  });

  globalThis.Performance = Performance;
  globalThis.performance = new Performance(INTERNAL);
})();
