// WebCrypto (SPEC §2.10): crypto.getRandomValues, crypto.randomUUID, and
// crypto.subtle (digest, HMAC, AES-GCM). Crypto runs in vetted Rust ops
// (RustCrypto, D9); this layer is the JS surface + key bookkeeping.
// ECDSA/ECDH/RSA are staged for a follow-up (SPEC §7).
(() => {
  "use strict";
  const ops = globalThis.__ops;

  const INTEGER_VIEWS = [
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    BigInt64Array,
    BigUint64Array,
  ];

  function getRandomValues(view) {
    if (!ArrayBuffer.isView(view) || !INTEGER_VIEWS.some((T) => view instanceof T)) {
      throw new TypeError("getRandomValues expects an integer TypedArray");
    }
    if (view.byteLength > 65536) {
      throw new DOMException("requested too many random bytes", "QuotaExceededError");
    }
    const bytes = ops.random_bytes(view.byteLength);
    new Uint8Array(view.buffer, view.byteOffset, view.byteLength).set(bytes);
    return view;
  }

  const HEX = [];
  for (let i = 0; i < 256; i++) HEX.push((i + 0x100).toString(16).slice(1));
  function randomUUID() {
    const b = ops.random_bytes(16);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant
    return (
      HEX[b[0]] + HEX[b[1]] + HEX[b[2]] + HEX[b[3]] + "-" +
      HEX[b[4]] + HEX[b[5]] + "-" +
      HEX[b[6]] + HEX[b[7]] + "-" +
      HEX[b[8]] + HEX[b[9]] + "-" +
      HEX[b[10]] + HEX[b[11]] + HEX[b[12]] + HEX[b[13]] + HEX[b[14]] + HEX[b[15]]
    );
  }

  // ---- subtle helpers -----------------------------------------------------

  const KEY = Symbol("cryptoKeyMaterial");

  function toBytes(data) {
    if (data instanceof Uint8Array) return data;
    if (data instanceof ArrayBuffer) return new Uint8Array(data);
    if (ArrayBuffer.isView(data)) {
      return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    }
    throw new TypeError("expected a BufferSource");
  }
  function asArrayBuffer(u8) {
    return u8.buffer.slice(u8.byteOffset, u8.byteOffset + u8.byteLength);
  }
  function normalizeAlgorithm(algorithm) {
    return typeof algorithm === "string" ? { name: algorithm } : algorithm;
  }
  function hashName(h) {
    return typeof h === "string" ? h : h.name;
  }

  class CryptoKey {
    #type;
    #extractable;
    #algorithm;
    #usages;
    constructor(type, extractable, algorithm, usages, material) {
      this.#type = type;
      this.#extractable = extractable;
      this.#algorithm = algorithm;
      this.#usages = usages;
      this[KEY] = material; // Uint8Array (raw key)
    }
    get type() {
      return this.#type;
    }
    get extractable() {
      return this.#extractable;
    }
    get algorithm() {
      return this.#algorithm;
    }
    get usages() {
      return this.#usages;
    }
  }

  const subtle = {
    async digest(algorithm, data) {
      const name = normalizeAlgorithm(algorithm).name;
      return asArrayBuffer(ops.subtle_digest(name, toBytes(data)));
    },

    async generateKey(algorithm, extractable, usages) {
      const algo = normalizeAlgorithm(algorithm);
      if (algo.name === "HMAC") {
        const hash = hashName(algo.hash);
        const lengthBits =
          algo.length ??
          { "SHA-1": 160, "SHA-256": 256, "SHA-384": 384, "SHA-512": 512 }[hash];
        const material = ops.random_bytes(Math.ceil(lengthBits / 8));
        return new CryptoKey("secret", extractable, { name: "HMAC", hash: { name: hash }, length: lengthBits }, usages, material);
      }
      if (algo.name === "AES-GCM") {
        if (algo.length !== 128 && algo.length !== 256) {
          throw new DOMException("AES-GCM key length must be 128 or 256", "OperationError");
        }
        const material = ops.random_bytes(algo.length / 8);
        return new CryptoKey("secret", extractable, { name: "AES-GCM", length: algo.length }, usages, material);
      }
      throw new DOMException(`unsupported algorithm: ${algo.name}`, "NotSupportedError");
    },

    async importKey(format, keyData, algorithm, extractable, usages) {
      if (format !== "raw") {
        throw new DOMException(`unsupported import format: ${format}`, "NotSupportedError");
      }
      const algo = normalizeAlgorithm(algorithm);
      const material = toBytes(keyData).slice();
      if (algo.name === "HMAC") {
        return new CryptoKey(
          "secret",
          extractable,
          { name: "HMAC", hash: { name: hashName(algo.hash) }, length: material.length * 8 },
          usages,
          material,
        );
      }
      if (algo.name === "AES-GCM") {
        return new CryptoKey("secret", extractable, { name: "AES-GCM", length: material.length * 8 }, usages, material);
      }
      throw new DOMException(`unsupported algorithm: ${algo.name}`, "NotSupportedError");
    },

    async exportKey(format, key) {
      if (format !== "raw") {
        throw new DOMException(`unsupported export format: ${format}`, "NotSupportedError");
      }
      if (!key.extractable) {
        throw new DOMException("key is not extractable", "InvalidAccessError");
      }
      return asArrayBuffer(key[KEY].slice());
    },

    async sign(algorithm, key, data) {
      const algo = normalizeAlgorithm(algorithm);
      if (algo.name === "HMAC") {
        return asArrayBuffer(
          ops.subtle_hmac_sign(key.algorithm.hash.name, key[KEY], toBytes(data)),
        );
      }
      throw new DOMException(`unsupported sign algorithm: ${algo.name}`, "NotSupportedError");
    },

    async verify(algorithm, key, signature, data) {
      const algo = normalizeAlgorithm(algorithm);
      if (algo.name === "HMAC") {
        return ops.subtle_hmac_verify(
          key.algorithm.hash.name,
          key[KEY],
          toBytes(signature),
          toBytes(data),
        );
      }
      throw new DOMException(`unsupported verify algorithm: ${algo.name}`, "NotSupportedError");
    },

    async encrypt(algorithm, key, data) {
      const algo = normalizeAlgorithm(algorithm);
      if (algo.name === "AES-GCM") {
        const iv = toBytes(algo.iv);
        const aad = algo.additionalData ? toBytes(algo.additionalData) : new Uint8Array(0);
        return asArrayBuffer(ops.subtle_aes_gcm_encrypt(key[KEY], iv, toBytes(data), aad));
      }
      throw new DOMException(`unsupported encrypt algorithm: ${algo.name}`, "NotSupportedError");
    },

    async decrypt(algorithm, key, data) {
      const algo = normalizeAlgorithm(algorithm);
      if (algo.name === "AES-GCM") {
        const iv = toBytes(algo.iv);
        const aad = algo.additionalData ? toBytes(algo.additionalData) : new Uint8Array(0);
        return asArrayBuffer(ops.subtle_aes_gcm_decrypt(key[KEY], iv, toBytes(data), aad));
      }
      throw new DOMException(`unsupported decrypt algorithm: ${algo.name}`, "NotSupportedError");
    },
  };

  globalThis.CryptoKey = CryptoKey;
  globalThis.crypto = Object.freeze({
    getRandomValues,
    randomUUID,
    subtle: Object.freeze(subtle),
  });
})();
