// JSONC and JSON5 (DECISIONS D191). Each mirrors the global `JSON`: `parse`
// is an engine builtin that builds values straight from the text
// (crates/engine/src/json.rs), with `reviver` applied here as `JSON.parse`
// applies it; `JSONC.stringify` is `JSON.stringify`, since a comment cannot
// come from a value; `JSON5.stringify` writes JSON5 as the `json5` reference
// package does. `edit` changes one value in the text and keeps every other
// byte: the host finds the place and the file's style, the dialect's
// `stringify` writes the value in that style, and the host splices it in.

type Dialect = "jsonc" | "json5";
type Reviver = (this: any, key: string, value: any) => any;
type Replacer = ((this: any, key: string, value: any) => any) | readonly (string | number)[] | null;

const g = globalThis as unknown as {
  __json_parse: (text: string, dialect: Dialect) => unknown;
  __json_validate: (text: string, dialect: Dialect) => true | string;
};
// Captured now: the globals are hidden, not locked (harden.js).
const jsonParse = g.__json_parse;
const jsonValidate = g.__json_validate;
const { json_edit_plan, json_edit } = (
  g as unknown as { __ops: Record<string, (...a: any[]) => any> }
).__ops;

export interface ValidateOptions {
  detailed?: boolean;
}

// The parser reads Rust text, which cannot hold a lone surrogate, so a raw one
// is written as the escape that means the same code unit in a string. Outside
// a string either form is refused. (In JSON5 a backslash before a raw lone
// surrogate escapes the backslash instead, the one input this reads
// differently; JSONC refuses both.)
const LONE_SURROGATE = /\p{Surrogate}/gu;
function source(text: unknown): string {
  const s = String(text);
  return s.isWellFormed()
    ? s
    : s.replace(LONE_SURROGATE, (c) => `\\u${c.charCodeAt(0).toString(16)}`);
}

function define(target: object, key: string, value: unknown): void {
  Reflect.defineProperty(target, key, {
    value,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}

// ECMA-262's InternalizeJSONProperty: the reviver sees each value after its
// children, and `undefined` removes it.
function internalize(holder: any, name: string, reviver: Reviver): unknown {
  const value = holder[name];
  if (typeof value === "object" && value !== null) {
    const keys = Array.isArray(value)
      ? Array.from(
          { length: Math.min(Math.max(Math.trunc(value.length) || 0, 0), Number.MAX_SAFE_INTEGER) },
          (_, i) => String(i),
        )
      : Object.keys(value);
    for (const key of keys) {
      const replaced = internalize(value, key, reviver);
      if (replaced === undefined) Reflect.deleteProperty(value, key);
      else define(value, key, replaced);
    }
  }
  return Reflect.apply(reviver, holder, [name, value]);
}

function parseWith(dialect: Dialect, text: unknown, reviver?: Reviver): any {
  const value = jsonParse(source(text), dialect);
  if (typeof reviver !== "function") return value;
  const root = {};
  define(root, "", value);
  return internalize(root, "", reviver);
}

function validateWith(dialect: Dialect, text: unknown, options: ValidateOptions = {}) {
  const result = jsonValidate(source(text), dialect);
  if (result === true) return options.detailed ? { valid: true } : true;
  return options.detailed ? { valid: false, error: result } : false;
}

// ---- JSON5.stringify ---------------------------------------------------------

export interface JSON5StringifyOptions {
  replacer?: Replacer;
  space?: string | number | null;
  /** The quote for every string; by default each string takes whichever needs fewer escapes. */
  quote?: "'" | '"';
}

const ESCAPES: Record<string, string> = {
  "\\": "\\\\",
  "\b": "\\b",
  "\f": "\\f",
  "\n": "\\n",
  "\r": "\\r",
  "\t": "\\t",
  "\v": "\\v",
  "\0": "\\0",
  "\u2028": "\\u2028",
  "\u2029": "\\u2029",
};

// Unquoted only when ASCII: every JSON5 reader agrees on these, where readers
// built on different Unicode versions disagree about the rest.
const IDENTIFIER = /^[A-Za-z_$][\w$]*$/;

function stringifyJSON5(
  value: unknown,
  replacerOrOptions?: Replacer | JSON5StringifyOptions,
  spaceArg?: string | number | null,
  // Not public: `edit` turns them off for a file that has none.
  trailingCommas = true,
): string | undefined {
  let replacer: unknown = replacerOrOptions;
  let space: unknown = spaceArg;
  let quote: string | undefined;
  if (
    replacerOrOptions != null &&
    typeof replacerOrOptions === "object" &&
    !Array.isArray(replacerOrOptions)
  ) {
    const options = replacerOrOptions as JSON5StringifyOptions;
    replacer = options.replacer;
    space = options.space;
    quote = options.quote;
    if (quote !== undefined && quote !== "'" && quote !== '"') {
      throw new TypeError(`JSON5.stringify: quote must be "'" or '"'`);
    }
  }

  // The replacer and gap, exactly as JSON.stringify reads them.
  let replacerFn: ((this: any, key: string, value: any) => any) | undefined;
  let allowList: string[] | undefined;
  if (typeof replacer === "function") {
    replacerFn = replacer as typeof replacerFn;
  } else if (Array.isArray(replacer)) {
    allowList = [];
    const length = Math.min(Math.max(Math.trunc(replacer.length) || 0, 0), Number.MAX_SAFE_INTEGER);
    for (let i = 0; i < length; i++) {
      const v = replacer[i];
      let item: string | undefined;
      if (typeof v === "string") item = v;
      else if (typeof v === "number" || v instanceof String || v instanceof Number)
        item = String(v);
      if (item !== undefined && !allowList.includes(item)) allowList.push(item);
    }
  }
  if (space instanceof Number) space = Number(space);
  else if (space instanceof String) space = String(space);
  let gap = "";
  if (typeof space === "number")
    gap = " ".repeat(Math.min(10, Math.max(0, Math.trunc(space) || 0)));
  else if (typeof space === "string") gap = space.slice(0, 10);

  const stack: object[] = [];
  let indent = "";

  const quoteString = (text: string): string => {
    let singles = 0;
    let doubles = 0;
    let out = "";
    for (let i = 0; i < text.length; i++) {
      const c = text[i];
      const code = text.charCodeAt(i);
      if (c === "'") singles++;
      else if (c === '"') doubles++;
      if (c === "\0" && text.charCodeAt(i + 1) >= 0x30 && text.charCodeAt(i + 1) <= 0x39) {
        out += "\\x00";
      } else if (ESCAPES[c] !== undefined) {
        out += ESCAPES[c];
      } else if (code < 0x20) {
        out += `\\x${code.toString(16).padStart(2, "0")}`;
      } else if (code >= 0xd800 && code <= 0xdfff) {
        // A pair is written as is; a lone surrogate as its escape, as
        // JSON.stringify does, so the output is well-formed.
        const next = text.charCodeAt(i + 1);
        if (code <= 0xdbff && next >= 0xdc00 && next <= 0xdfff) {
          out += c + text[i + 1];
          i++;
        } else {
          out += `\\u${code.toString(16)}`;
        }
      } else {
        out += c;
      }
    }
    const q = quote ?? (singles <= doubles ? "'" : '"');
    return q + out.replaceAll(q, `\\${q}`) + q;
  };

  const key = (name: string): string => (IDENTIFIER.test(name) ? name : quoteString(name));

  const property = (holder: any, name: string): string | undefined => {
    let v = holder[name];
    if (v !== null && (typeof v === "object" || typeof v === "bigint")) {
      const toJSON5 = v.toJSON5;
      if (typeof toJSON5 === "function") {
        v = Reflect.apply(toJSON5, v, [name]);
      } else {
        const toJSON = v.toJSON;
        if (typeof toJSON === "function") v = Reflect.apply(toJSON, v, [name]);
      }
    }
    if (replacerFn) v = Reflect.apply(replacerFn, holder, [name, v]);
    if (v instanceof Number) v = Number(v);
    else if (v instanceof String) v = String(v);
    else if (v instanceof Boolean) v = Boolean.prototype.valueOf.call(v);
    else if (
      typeof v === "object" &&
      v !== null &&
      Object.prototype.toString.call(v) === "[object BigInt]"
    ) {
      v = BigInt.prototype.valueOf.call(v);
    }
    if (v === null) return "null";
    if (v === true) return "true";
    if (v === false) return "false";
    if (typeof v === "string") return quoteString(v);
    if (typeof v === "number") return String(v);
    if (typeof v === "bigint") throw new TypeError("JSON5.stringify: a BigInt has no JSON5 form");
    if (typeof v === "object" && typeof v !== "function")
      return Array.isArray(v) ? array(v) : object(v);
    return undefined;
  };

  const enter = (v: object) => {
    if (stack.includes(v)) throw new TypeError("JSON5.stringify: the value is circular");
    stack.push(v);
  };

  const close = (open: string, parts: string[], end: string, outer: string): string => {
    if (parts.length === 0) return open + end;
    if (gap === "") return open + parts.join(",") + end;
    // Indented, each member ends with a comma: adding one is a one-line diff.
    const last = trailingCommas ? "," : "";
    return `${open}\n${indent}${parts.join(`,\n${indent}`)}${last}\n${outer}${end}`;
  };

  const object = (v: any): string => {
    enter(v);
    const outer = indent;
    indent += gap;
    const parts: string[] = [];
    for (const name of allowList ?? Object.keys(v)) {
      const written = property(v, name);
      if (written !== undefined) parts.push(`${key(name)}:${gap ? " " : ""}${written}`);
    }
    const out = close("{", parts, "}", outer);
    stack.pop();
    indent = outer;
    return out;
  };

  const array = (v: any): string => {
    enter(v);
    const outer = indent;
    indent += gap;
    const parts: string[] = [];
    const length = Math.min(Math.max(Math.trunc(v.length) || 0, 0), Number.MAX_SAFE_INTEGER);
    for (let i = 0; i < length; i++) parts.push(property(v, String(i)) ?? "null");
    const out = close("[", parts, "]", outer);
    stack.pop();
    indent = outer;
    return out;
  };

  const root = {};
  define(root, "", value);
  return property(root, "");
}

// ---- edit ------------------------------------------------------------------

/** Keys and array indices, from the top. */
export type JSONPath = readonly (string | number)[];

function editWith(dialect: Dialect, text: unknown, path: unknown, value: unknown): string {
  const name = dialect === "jsonc" ? "JSONC" : "JSON5";
  const s = String(text);
  // The edit returns the text with every other byte as it was, which the
  // escaping `parse` does for a lone surrogate would break.
  if (!s.isWellFormed())
    throw new TypeError(`${name}.edit: the text has a lone surrogate, which no file holds`);
  if (!Array.isArray(path))
    throw new TypeError(`${name}.edit: the path is an array of keys and indices`);
  const steps = Array.from(path);
  for (const step of steps) {
    if (typeof step !== "string" && !(Number.isSafeInteger(step) && step >= 0)) {
      throw new TypeError(
        `${name}.edit: each step of the path is a string key or a non-negative integer index`,
      );
    }
  }
  if (value === undefined) return json_edit(dialect, s, steps, undefined);
  const plan: { indent: string; quote?: "'" | '"'; trailingCommas: boolean } = json_edit_plan(
    dialect,
    s,
    steps,
  );
  const written =
    dialect === "jsonc"
      ? JSON.stringify(value, null, plan.indent)
      : stringifyJSON5(
          value,
          { space: plan.indent, quote: plan.quote },
          undefined,
          plan.trailingCommas,
        );
  if (written === undefined) throw new TypeError(`${name}.edit: the value has no ${name} form`);
  return json_edit(dialect, s, steps, written);
}

export const JSONC = {
  /** Parses JSON with comments and trailing commas, as `JSON.parse` would. */
  parse: (text: string, reviver?: Reviver) => parseWith("jsonc", text, reviver),
  /** Standard JSON: what `JSON.stringify` writes. */
  stringify: (value: unknown, replacer?: Replacer, space?: string | number | null) =>
    JSON.stringify(value, replacer as any, space as any),
  validate: (text: string, options?: ValidateOptions) => validateWith("jsonc", text, options),
  /** The text with the value at `path` set to `value`, or removed when it is `undefined`; every other byte is kept. */
  edit: (text: string, path: JSONPath, value: unknown) => editWith("jsonc", text, path, value),
};

export const JSON5 = {
  /** Parses JSON5 1.0.0, as `JSON.parse` would parse the JSON it shares. */
  parse: (text: string, reviver?: Reviver) => parseWith("json5", text, reviver),
  stringify: stringifyJSON5,
  validate: (text: string, options?: ValidateOptions) => validateWith("json5", text, options),
  /** The text with the value at `path` set to `value`, or removed when it is `undefined`; every other byte is kept. */
  edit: (text: string, path: JSONPath, value: unknown) => editWith("json5", text, path, value),
};
