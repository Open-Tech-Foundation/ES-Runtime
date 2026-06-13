// performance (SPEC §2.11), backed by the Clock provider. `now()` is monotonic
// milliseconds since the clock's epoch (≈ ms since construction) with
// sub-millisecond (µs) precision; `timeOrigin` is the wall-clock time at
// construction.
(() => {
  "use strict";
  const ops = globalThis.__ops;
  const timeOrigin = ops.time_origin();

  globalThis.performance = Object.freeze({
    now() {
      return ops.now();
    },
    get timeOrigin() {
      return timeOrigin;
    },
    toJSON() {
      return { timeOrigin };
    },
  });
})();
