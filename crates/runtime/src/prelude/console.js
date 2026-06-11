// console (SPEC §2.2). Guest output is formatted here and handed to the host
// `console` op, which forwards it to the injected Console sink. Formatting is
// intentionally simple (not full util.inspect): strings pass through, objects
// are JSON where possible, everything else uses String().
(() => {
  "use strict";
  const ops = globalThis.__ops;

  function inspect(value, seen) {
    switch (typeof value) {
      case "string":
        return value;
      case "bigint":
        return value.toString() + "n";
      case "symbol":
        return value.toString();
      case "function":
        return "[Function: " + (value.name || "anonymous") + "]";
      case "undefined":
        return "undefined";
      case "object": {
        if (value === null) return "null";
        if (seen.has(value)) return "[Circular]";
        seen.add(value);
        try {
          return JSON.stringify(value, (_k, v) =>
            typeof v === "bigint" ? v.toString() + "n" : v,
          ) ?? String(value);
        } catch {
          return String(value);
        } finally {
          seen.delete(value);
        }
      }
      default:
        return String(value);
    }
  }

  function format(args) {
    const seen = new WeakSet();
    return args.map((a) => inspect(a, seen)).join(" ");
  }

  function emit(level) {
    return (...args) => ops.console(level, format(args));
  }

  const debug = emit("debug");
  const info = emit("info");
  const log = emit("log");
  const warn = emit("warn");
  const error = emit("error");

  globalThis.console = {
    debug,
    info,
    log,
    warn,
    error,
    trace: error,
    dir: log,
    // group/table are minimal (SPEC §7 deferral): routed to log.
    group: log,
    groupCollapsed: log,
    groupEnd: () => {},
    table: log,
    assert: (condition, ...args) => {
      if (!condition) {
        error("Assertion failed" + (args.length ? ": " + format(args) : ""));
      }
    },
    count: () => {},
    time: () => {},
    timeEnd: () => {},
  };
})();
