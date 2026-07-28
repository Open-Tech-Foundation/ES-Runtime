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

  // ---- User Timing --------------------------------------------------------
  // The entry buffer is in-process and unbounded by design: this runtime has no
  // navigation to clear it, so a long-lived process that marks in a loop must
  // clear its own entries — same as every other implementation.

  class PerformanceEntry {
    #name;
    #entryType;
    #startTime;
    #duration;
    constructor(key, name, entryType, startTime, duration) {
      if (key !== INTERNAL) throw new TypeError("Illegal constructor");
      this.#name = name;
      this.#entryType = entryType;
      this.#startTime = startTime;
      this.#duration = duration;
    }
    get name() {
      return this.#name;
    }
    get entryType() {
      return this.#entryType;
    }
    get startTime() {
      return this.#startTime;
    }
    get duration() {
      return this.#duration;
    }
    toJSON() {
      return {
        name: this.#name,
        entryType: this.#entryType,
        startTime: this.#startTime,
        duration: this.#duration,
      };
    }
  }

  class PerformanceMark extends PerformanceEntry {
    #detail;
    constructor(key, name, startTime, detail) {
      super(key, name, "mark", startTime, 0);
      this.#detail = detail ?? null;
    }
    get detail() {
      return this.#detail;
    }
  }

  class PerformanceMeasure extends PerformanceEntry {
    #detail;
    constructor(key, name, startTime, duration, detail) {
      super(key, name, "measure", startTime, duration);
      this.#detail = detail ?? null;
    }
    get detail() {
      return this.#detail;
    }
  }

  const entries = [];

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

    mark(name, options = {}) {
      const opts = options ?? {};
      const startTime = opts.startTime !== undefined ? Number(opts.startTime) : ops.now();
      if (startTime < 0) throw new TypeError("mark startTime cannot be negative");
      const entry = new PerformanceMark(INTERNAL, String(name), startTime, opts.detail);
      entries.push(entry);
      return entry;
    }

    // Resolves a measure endpoint: a mark name, a number, or absent.
    #resolveTime(value, fallback) {
      if (value === undefined) return fallback;
      if (typeof value === "number") return value;
      const name = String(value);
      // The most recent mark with that name wins, per the spec.
      for (let i = entries.length - 1; i >= 0; i--) {
        if (entries[i].entryType === "mark" && entries[i].name === name) {
          return entries[i].startTime;
        }
      }
      throw new DOMException(`No mark named "${name}" exists`, "SyntaxError");
    }

    measure(name, startOrOptions, endMark) {
      let start;
      let end;
      let detail = null;
      if (startOrOptions !== null && typeof startOrOptions === "object") {
        const opts = startOrOptions;
        detail = opts.detail ?? null;
        if (opts.duration !== undefined) {
          if (opts.start !== undefined && opts.end !== undefined) {
            throw new TypeError("measure cannot take start, end and duration together");
          }
          if (opts.start !== undefined) {
            start = this.#resolveTime(opts.start, 0);
            end = start + Number(opts.duration);
          } else {
            end = this.#resolveTime(opts.end, ops.now());
            start = end - Number(opts.duration);
          }
        } else {
          start = this.#resolveTime(opts.start, 0);
          end = this.#resolveTime(opts.end, ops.now());
        }
      } else {
        start = this.#resolveTime(startOrOptions, 0);
        end = this.#resolveTime(endMark, ops.now());
      }
      const entry = new PerformanceMeasure(
        INTERNAL,
        String(name),
        start,
        end - start,
        detail,
      );
      entries.push(entry);
      return entry;
    }

    getEntries() {
      return entries.slice();
    }
    getEntriesByType(type) {
      const t = String(type);
      return entries.filter((e) => e.entryType === t);
    }
    getEntriesByName(name, type) {
      const n = String(name);
      const t = type === undefined ? undefined : String(type);
      return entries.filter(
        (e) => e.name === n && (t === undefined || e.entryType === t),
      );
    }
    clearMarks(name) {
      clearEntries("mark", name);
    }
    clearMeasures(name) {
      clearEntries("measure", name);
    }

    toJSON() {
      return { timeOrigin };
    }
  }

  function clearEntries(entryType, name) {
    const n = name === undefined ? undefined : String(name);
    for (let i = entries.length - 1; i >= 0; i--) {
      const e = entries[i];
      if (e.entryType === entryType && (n === undefined || e.name === n)) {
        entries.splice(i, 1);
      }
    }
  }

  for (const Interface of [
    Performance,
    PerformanceEntry,
    PerformanceMark,
    PerformanceMeasure,
  ]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
  globalThis.performance = new Performance(INTERNAL);
})();
