// console (SPEC §2.2). Guest output is formatted here and handed to the host
// `console` op, which forwards it to the injected Console sink. The inspector
// is a pragmatic util.inspect-lite: a top-level string passes through bare, but
// nested values are shown structurally — functions as `[Function: name]`,
// arrays/objects recursively (incl. function-valued and null-prototype/module
// namespace properties, which JSON would silently drop) — with a depth limit
// and circular-reference guard.
(() => {
  "use strict";
  const ops = globalThis.__ops;

  const fnToString = Function.prototype.toString;
  const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;
  const DEPTH = 4;

  function quote(s) {
    return (
      "'" +
      s
        .replace(/\\/g, "\\\\")
        .replace(/'/g, "\\'")
        .replace(/\n/g, "\\n")
        .replace(/\t/g, "\\t") +
      "'"
    );
  }

  function fnLabel(f) {
    let isClass = false;
    try {
      isClass = /^\s*class[\s{]/.test(fnToString.call(f));
    } catch {
      /* toString can throw for exotic callables; treat as function */
    }
    if (isClass) return f.name ? "[class " + f.name + "]" : "[class (anonymous)]";
    return f.name ? "[Function: " + f.name + "]" : "[Function (anonymous)]";
  }

  function entries(value, seen, depth) {
    return Object.keys(value).map((k) => {
      const key = IDENT.test(k) ? k : quote(k);
      return key + ": " + inspect(value[k], seen, depth);
    });
  }

  function inspect(value, seen, depth) {
    switch (typeof value) {
      case "string":
        return quote(value);
      case "number":
        return Object.is(value, -0) ? "-0" : String(value);
      case "boolean":
        return String(value);
      case "bigint":
        return value.toString() + "n";
      case "symbol":
        return value.toString();
      case "function":
        return fnLabel(value);
      case "undefined":
        return "undefined";
      case "object":
        break;
      default:
        return String(value);
    }

    if (value === null) return "null";
    if (seen.has(value)) return "[Circular]";

    if (value instanceof Error) return value.stack || value.name + ": " + value.message;
    if (value instanceof RegExp) return String(value);
    if (value instanceof Date) return isNaN(value) ? "Invalid Date" : value.toISOString();

    if (Array.isArray(value)) {
      if (depth < 0) return "[Array]";
      seen.add(value);
      const parts = value.map((v) => inspect(v, seen, depth - 1));
      seen.delete(value);
      return parts.length ? "[ " + parts.join(", ") + " ]" : "[]";
    }
    if (value instanceof Map) {
      if (depth < 0) return "[Map]";
      seen.add(value);
      const parts = [];
      for (const [k, v] of value)
        parts.push(inspect(k, seen, depth - 1) + " => " + inspect(v, seen, depth - 1));
      seen.delete(value);
      return "Map(" + value.size + ") {" + (parts.length ? " " + parts.join(", ") + " " : "") + "}";
    }
    if (value instanceof Set) {
      if (depth < 0) return "[Set]";
      seen.add(value);
      const parts = [];
      for (const v of value) parts.push(inspect(v, seen, depth - 1));
      seen.delete(value);
      return "Set(" + value.size + ") {" + (parts.length ? " " + parts.join(", ") + " " : "") + "}";
    }

    if (depth < 0) return "[Object]";
    // Constructor name as a prefix (Object / null-prototype get none).
    const proto = Object.getPrototypeOf(value);
    const ctor = proto && proto.constructor;
    const name = ctor && ctor.name;
    const prefix = name && name !== "Object" ? name + " " : proto === null ? "[Object: null prototype] " : "";

    seen.add(value);
    const parts = entries(value, seen, depth - 1);
    seen.delete(value);
    return parts.length ? prefix + "{ " + parts.join(", ") + " }" : prefix + "{}";
  }

  function format(args) {
    const seen = new WeakSet();
    // A lone top-level string prints bare (no quotes); everything else, and any
    // nested string, is inspected structurally.
    return args
      .map((a) => (typeof a === "string" ? a : inspect(a, seen, DEPTH)))
      .join(" ");
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
