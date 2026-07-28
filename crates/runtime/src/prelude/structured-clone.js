// structuredClone (SPEC §2.1) — pure-JS deep clone of the standard cloneable
// types, with cycle handling. Functions and symbols throw DataCloneError, as the
// spec requires. Transferables and a few exotic host types are not supported
// (documented in SPEC §7); a V8 ValueSerializer-based path is a later refinement.
(() => {
  "use strict";
  const BYTES = __internal.bytes;

  const TYPED_ARRAYS = [
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    Float32Array,
    Float64Array,
    BigInt64Array,
    BigUint64Array,
  ];

  function cannotClone() {
    return new DOMException(
      "The object could not be cloned.",
      "DataCloneError",
    );
  }

  // The error types the spec lists as serializable. Anything else that is an
  // Error clones as a plain Error, as the spec requires.
  const ERROR_TYPES = {
    Error,
    EvalError,
    RangeError,
    ReferenceError,
    SyntaxError,
    TypeError,
    URIError,
  };

  function clone(value, seen, transferred) {
    if (value === null) return null;
    const type = typeof value;
    if (type === "function" || type === "symbol") throw cannotClone();
    if (type !== "object") return value; // string/number/boolean/bigint/undefined

    if (seen.has(value)) return seen.get(value);

    // Boxed primitives.
    if (value instanceof Boolean) return new Boolean(value.valueOf());
    if (value instanceof Number) return new Number(value.valueOf());
    if (value instanceof String) return new String(value.valueOf());

    if (value instanceof Date) return new Date(value.getTime());
    if (value instanceof RegExp) return new RegExp(value.source, value.flags);

    if (value instanceof ArrayBuffer) {
      // A transferred buffer moves into the clone rather than being copied.
      const moved = transferred.get(value);
      return moved !== undefined ? moved : value.slice(0);
    }
    // A view whose buffer was transferred is reading a detached buffer; the
    // spec makes that a DataCloneError rather than a TypeError from the copy.
    if (value instanceof DataView) {
      if (transferred.has(value.buffer)) throw cannotClone();
      return new DataView(
        value.buffer.slice(0),
        value.byteOffset,
        value.byteLength,
      );
    }
    for (const TA of TYPED_ARRAYS) {
      if (value instanceof TA) {
        if (transferred.has(value.buffer)) throw cannotClone();
        return new TA(value);
      }
    }

    // Blob and File are serializable; both are cloned by value.
    if (globalThis.Blob && value instanceof Blob) {
      if (globalThis.File && value instanceof File) {
        return new File([value[BYTES]()], value.name, {
          type: value.type,
          lastModified: value.lastModified,
        });
      }
      return new Blob([value[BYTES]()], { type: value.type });
    }

    if (Array.isArray(value)) {
      const out = new Array(value.length);
      seen.set(value, out);
      for (let i = 0; i < value.length; i++) {
        if (i in value) out[i] = clone(value[i], seen, transferred);
      }
      return out;
    }

    if (value instanceof Map) {
      const out = new Map();
      seen.set(value, out);
      for (const [k, v] of value) out.set(clone(k, seen, transferred), clone(v, seen, transferred));
      return out;
    }

    if (value instanceof Set) {
      const out = new Set();
      seen.set(value, out);
      for (const v of value) out.add(clone(v, seen, transferred));
      return out;
    }

    if (value instanceof Error) {
      // DOMException carries its `.name` as data, so it must be reconstructed
      // through the two-argument constructor rather than by class.
      let out;
      if (globalThis.DOMException && value instanceof DOMException) {
        out = new DOMException(value.message, value.name);
      } else {
        const Ctor = ERROR_TYPES[value.name] ?? Error;
        out = new Ctor(value.message);
      }
      seen.set(value, out);
      // `cause` is part of the serialization; `stack` is not specified but is
      // what makes a cloned error useful, and every engine carries it over.
      if ("cause" in value) {
        out.cause = clone(value.cause, seen, transferred);
      }
      if (typeof value.stack === "string") out.stack = value.stack;
      return out;
    }

    // Plain objects (and null-prototype objects). Reject exotic platform objects
    // we cannot faithfully clone.
    const proto = Object.getPrototypeOf(value);
    if (proto !== Object.prototype && proto !== null) throw cannotClone();

    const out = Object.create(proto);
    seen.set(value, out);
    for (const key of Reflect.ownKeys(value)) {
      const desc = Object.getOwnPropertyDescriptor(value, key);
      if (desc.enumerable) out[key] = clone(value[key], seen, transferred);
    }
    return out;
  }

  globalThis.structuredClone = (value, options) => {
    // Transfer first: each listed ArrayBuffer is detached and its contents move
    // into the clone, so the original is left empty (ES2024 `transfer()` is what
    // actually detaches — there is no way to do it from JS otherwise).
    const transferred = new Map();
    const list = options && options.transfer ? [...options.transfer] : [];
    for (const item of list) {
      if (!(item instanceof ArrayBuffer) || typeof item.transfer !== "function") {
        throw new DOMException(
          "Only ArrayBuffer objects can be transferred.",
          "DataCloneError",
        );
      }
      if (transferred.has(item)) continue; // listed twice: transfer once
      transferred.set(item, item.transfer());
    }
    return clone(value, new Map(), transferred);
  };
})();
