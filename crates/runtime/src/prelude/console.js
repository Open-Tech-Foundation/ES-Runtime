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
  // Objects carrying this global-registry marker (e.g. runtime:process Secrets)
  // render as "[redacted]" so secret values never leak into console output.
  const REDACTED_MARK = Symbol.for("runtime.secret.redacted");

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

    // Redaction marker (secret values) — checked before any structural walk so
    // a Secret never has its contents inspected.
    try {
      if (value[REDACTED_MARK] === true) return "[redacted]";
    } catch {
      /* exotic getters may throw; fall through to normal formatting */
    }

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

  // ---- Format specifiers ---------------------------------------------------
  //
  // The Console Standard's Formatter: when the first argument is a string
  // containing `%` directives, the following arguments are consumed to fill
  // them, and whatever is left over is appended as usual. `%c` (CSS) is
  // recognised and its argument discarded — there is no styling to apply to a
  // provider sink, but silently printing the CSS would be worse.

  function applyFormat(template, args) {
    const seen = new WeakSet();
    let next = 0;
    const out = template.replace(/%([sdifoOjc%])/g, (match, spec) => {
      if (spec === "%") return "%";
      if (next >= args.length) return match; // no argument left: leave it verbatim
      const value = args[next++];
      switch (spec) {
        case "s":
          return typeof value === "string" ? value : inspect(value, seen, DEPTH);
        case "d":
        case "i":
          return typeof value === "bigint"
            ? value.toString() + "n"
            : typeof value === "symbol"
              ? "NaN"
              : String(Math.trunc(Number(value)));
        case "f":
          return typeof value === "symbol" ? "NaN" : String(Number(value));
        case "j":
          try {
            return JSON.stringify(value);
          } catch {
            return "[Circular]";
          }
        case "c":
          return ""; // styling: consumed, not printed
        default: // o, O
          return inspect(value, seen, DEPTH);
      }
    });
    const rest = args.slice(next);
    return rest.length ? out + " " + formatValues(rest) : out;
  }

  function formatValues(args) {
    const seen = new WeakSet();
    // A lone top-level string prints bare (no quotes); everything else, and any
    // nested string, is inspected structurally.
    return args
      .map((a) => (typeof a === "string" ? a : inspect(a, seen, DEPTH)))
      .join(" ");
  }

  function format(args) {
    if (args.length > 1 && typeof args[0] === "string" && /%[sdifoOjc%]/.test(args[0])) {
      return applyFormat(args[0], args.slice(1));
    }
    return formatValues(args);
  }

  // ---- Grouping ------------------------------------------------------------
  //
  // A group indents everything printed until it is closed. The indent is
  // applied per line so a multi-line value (a stack trace, an inspected object)
  // stays aligned.

  let indent = "";

  function write(level, message) {
    ops.console(level, indent ? indent + message.replaceAll("\n", "\n" + indent) : message);
  }

  function emit(level) {
    return (...args) => write(level, format(args));
  }

  const debug = emit("debug");
  const info = emit("info");
  const log = emit("log");
  const warn = emit("warn");
  const error = emit("error");

  // ---- Counting and timing -------------------------------------------------

  const counts = new Map();
  const timers = new Map();

  const labelOf = (label) => (label === undefined ? "default" : String(label));
  const elapsed = (started) => `${(performance.now() - started).toFixed(3)}ms`;

  // ---- table ---------------------------------------------------------------
  //
  // Rendered rather than dumped: a table of rows is the one console output that
  // is genuinely hard to read as a nested inspection, which is why the method
  // exists at all.

  const isTabular = (v) => v !== null && (typeof v === "object" || typeof v === "function");
  // Marks the column holding rows that are primitives rather than records.
  const VALUES = Symbol("table values column");

  function renderTable(data, columns) {
    if (!isTabular(data)) return format([data]);

    // Rows come from an array's indices or an object's keys; each row's own
    // keys become columns, with a "Values" column for rows that are primitives.
    const rowKeys = Object.keys(data);
    const headers = [];
    const cells = [];
    let hasValues = false;

    for (const key of rowKeys) {
      const row = data[key];
      const rendered = {};
      if (isTabular(row)) {
        for (const column of Object.keys(row)) {
          if (columns !== undefined && !columns.includes(column)) continue;
          if (!headers.includes(column)) headers.push(column);
          rendered[column] = inspect(row[column], new WeakSet(), 1);
        }
      } else {
        hasValues = true;
        rendered[VALUES] = inspect(row, new WeakSet(), 1);
      }
      cells.push(rendered);
    }

    const indexHeader = Array.isArray(data) ? "(index)" : "(key)";
    const allHeaders = [indexHeader, ...headers, ...(hasValues ? ["Values"] : [])];
    const rows = cells.map((rendered, i) => [
      rowKeys[i],
      ...headers.map((h) => rendered[h] ?? ""),
      ...(hasValues ? [rendered[VALUES] ?? ""] : []),
    ]);

    const widths = allHeaders.map((h, i) =>
      Math.max(h.length, ...rows.map((r) => r[i].length), 0),
    );
    const line = (left, mid, right) =>
      left + widths.map((w) => "─".repeat(w + 2)).join(mid) + right;
    const row = (values) =>
      "│" + values.map((v, i) => " " + v.padEnd(widths[i]) + " ").join("│") + "│";

    return [
      line("┌", "┬", "┐"),
      row(allHeaders),
      line("├", "┼", "┤"),
      ...rows.map(row),
      line("└", "┴", "┘"),
    ].join("\n");
  }

  globalThis.console = {
    debug,
    info,
    log,
    warn,
    error,
    dir: log,
    // No markup to pretty-print in a server runtime, so this is `log` — which is
    // what the standard says to do when the value is not a node.
    dirxml: log,

    trace: (...args) => {
      // The stack is the point of `trace`; the frames below this one are the
      // caller's, so the first (this function) is dropped.
      const stack = new Error().stack || "";
      const frames = stack.split("\n").slice(2).join("\n");
      const label = args.length ? ": " + format(args) : "";
      write("error", "Trace" + label + (frames ? "\n" + frames : ""));
    },

    group: (...args) => {
      if (args.length) log(...args);
      indent += "  ";
    },
    groupCollapsed: (...args) => {
      // Nothing here can collapse, so an open group is the honest rendering.
      if (args.length) log(...args);
      indent += "  ";
    },
    groupEnd: () => {
      indent = indent.slice(0, -2);
    },

    table: (data, columns) => {
      write("log", renderTable(data, columns));
    },

    assert: (condition, ...args) => {
      if (!condition) {
        error("Assertion failed" + (args.length ? ": " + format(args) : ""));
      }
    },

    clear: () => {
      // The sink is a provider, not a terminal: there is nothing to clear, and
      // the standard says to do nothing when the console is not clearable. The
      // group nesting is reset, which is the part that *is* console state.
      indent = "";
    },

    count: (label) => {
      const key = labelOf(label);
      const n = (counts.get(key) ?? 0) + 1;
      counts.set(key, n);
      log(`${key}: ${n}`);
    },
    countReset: (label) => {
      counts.delete(labelOf(label));
    },

    time: (label) => {
      const key = labelOf(label);
      if (timers.has(key)) {
        warn(`Timer '${key}' already exists`);
        return;
      }
      timers.set(key, performance.now());
    },
    timeLog: (label, ...args) => {
      const key = labelOf(label);
      const started = timers.get(key);
      if (started === undefined) {
        warn(`Timer '${key}' does not exist`);
        return;
      }
      log(`${key}: ${elapsed(started)}` + (args.length ? " " + format(args) : ""));
    },
    timeEnd: (label) => {
      const key = labelOf(label);
      const started = timers.get(key);
      if (started === undefined) {
        warn(`Timer '${key}' does not exist`);
        return;
      }
      timers.delete(key);
      log(`${key}: ${elapsed(started)}`);
    },
  };
})();
