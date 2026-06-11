// structuredClone (SPEC §2.1) — pure-JS deep clone of the standard cloneable
// types, with cycle handling. Functions and symbols throw DataCloneError, as the
// spec requires. Transferables and a few exotic host types are not supported
// (documented in SPEC §7); a V8 ValueSerializer-based path is a later refinement.
(() => {
  "use strict";

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

  function clone(value, seen) {
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

    if (value instanceof ArrayBuffer) return value.slice(0);
    if (value instanceof DataView) {
      return new DataView(
        value.buffer.slice(0),
        value.byteOffset,
        value.byteLength,
      );
    }
    for (const TA of TYPED_ARRAYS) {
      if (value instanceof TA) return new TA(value);
    }

    if (Array.isArray(value)) {
      const out = new Array(value.length);
      seen.set(value, out);
      for (let i = 0; i < value.length; i++) {
        if (i in value) out[i] = clone(value[i], seen);
      }
      return out;
    }

    if (value instanceof Map) {
      const out = new Map();
      seen.set(value, out);
      for (const [k, v] of value) out.set(clone(k, seen), clone(v, seen));
      return out;
    }

    if (value instanceof Set) {
      const out = new Set();
      seen.set(value, out);
      for (const v of value) out.add(clone(v, seen));
      return out;
    }

    if (value instanceof Error) {
      const out = new value.constructor(value.message);
      seen.set(value, out);
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
      if (desc.enumerable) out[key] = clone(value[key], seen);
    }
    return out;
  }

  globalThis.structuredClone = (value) => clone(value, new Map());
})();
