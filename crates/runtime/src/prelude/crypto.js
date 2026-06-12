// WebCrypto (SPEC §2.10): crypto.getRandomValues, crypto.randomUUID, and
// crypto.subtle (digest, HMAC, AES-GCM/CBC/CTR, HKDF/PBKDF2 derivation,
// ECDSA/ECDH over P-256/P-384/P-521, and RSA — RSASSA-PKCS1-v1_5/RSA-PSS/
// RSA-OAEP). Crypto runs in vetted Rust ops (RustCrypto, D9); this layer is the
// JS surface + key bookkeeping. Asymmetric keys cross the op boundary as
// PKCS#8 (private) / SEC1 points or SPKI (public); JWK is assembled here.
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
  const AES_ALGS = new Set(["AES-GCM", "AES-CBC", "AES-CTR"]);
  const KDF_ALGS = new Set(["HKDF", "PBKDF2"]);
  const EC_ALGS = new Set(["ECDSA", "ECDH"]);
  const RSA_ALGS = new Set(["RSASSA-PKCS1-v1_5", "RSA-PSS", "RSA-OAEP"]);
  const HASH_BITS = { "SHA-1": 160, "SHA-256": 256, "SHA-384": 384, "SHA-512": 512 };
  // Field byte length (and so the JWK coordinate width) per named curve.
  const EC_FIELD = { "P-256": 32, "P-384": 48, "P-521": 66 };

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

  // ---- EC / JWK helpers ---------------------------------------------------

  function b64uToBytes(s) {
    let t = s.replace(/-/g, "+").replace(/_/g, "/");
    while (t.length % 4) t += "=";
    const bin = atob(t);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  function bytesToB64u(bytes) {
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    let bin = "";
    for (let i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i]);
    return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  }
  // SEC1 uncompressed point (0x04 || X || Y) from raw coordinate bytes.
  function sec1FromXY(x, y) {
    const out = new Uint8Array(1 + x.length + y.length);
    out[0] = 0x04;
    out.set(x, 1);
    out.set(y, 1 + x.length);
    return out;
  }
  function ecCurve(algo) {
    const curve = algo.namedCurve;
    if (!EC_FIELD[curve]) {
      throw new DOMException(`unsupported named curve: ${curve}`, "NotSupportedError");
    }
    return curve;
  }

  function importEcKey(format, keyData, algo, extractable, usages) {
    const curve = ecCurve(algo);
    const alg = { name: algo.name, namedCurve: curve };
    if (format === "raw") {
      // `raw` is public-only: an uncompressed SEC1 point.
      return new CryptoKey("public", extractable, alg, usages, toBytes(keyData).slice());
    }
    if (format === "spki") {
      return new CryptoKey("public", extractable, alg, usages, ops.ec_import_spki(curve, toBytes(keyData)));
    }
    if (format === "pkcs8") {
      return new CryptoKey("private", extractable, alg, usages, ops.ec_import_pkcs8(curve, toBytes(keyData)));
    }
    if (format === "jwk") {
      const jwk = keyData;
      if (jwk.kty !== "EC") throw new DOMException("JWK kty must be EC", "DataError");
      if (jwk.crv !== curve) throw new DOMException("JWK crv does not match algorithm", "DataError");
      if (jwk.d != null) {
        const pkcs8 = ops.ec_pkcs8_from_scalar(curve, b64uToBytes(jwk.d));
        return new CryptoKey("private", extractable, alg, usages, pkcs8);
      }
      if (jwk.x == null || jwk.y == null) {
        throw new DOMException("JWK is missing coordinates", "DataError");
      }
      const sec1 = sec1FromXY(b64uToBytes(jwk.x), b64uToBytes(jwk.y));
      return new CryptoKey("public", extractable, alg, usages, sec1);
    }
    throw new DOMException(`unsupported import format: ${format}`, "NotSupportedError");
  }

  function exportEcKey(format, key) {
    if (!key.extractable) {
      throw new DOMException("key is not extractable", "InvalidAccessError");
    }
    const curve = key.algorithm.namedCurve;
    const f = EC_FIELD[curve];
    if (format === "raw") {
      if (key.type !== "public") throw new DOMException("raw export is public-only", "InvalidAccessError");
      return asArrayBuffer(key[KEY].slice());
    }
    if (format === "spki") {
      if (key.type !== "public") throw new DOMException("spki export is public-only", "InvalidAccessError");
      return asArrayBuffer(ops.ec_export_spki(curve, key[KEY]));
    }
    if (format === "pkcs8") {
      if (key.type !== "private") throw new DOMException("pkcs8 export is private-only", "InvalidAccessError");
      return asArrayBuffer(key[KEY].slice());
    }
    if (format === "jwk") {
      let sec1 = key[KEY];
      let d;
      if (key.type === "private") {
        sec1 = ops.ec_public_point(curve, key[KEY]);
        d = bytesToB64u(ops.ec_private_scalar(curve, key[KEY]));
      }
      const jwk = {
        kty: "EC",
        crv: curve,
        x: bytesToB64u(sec1.subarray(1, 1 + f)),
        y: bytesToB64u(sec1.subarray(1 + f, 1 + 2 * f)),
        key_ops: [...key.usages],
        ext: key.extractable,
      };
      if (d != null) jwk.d = d;
      return jwk;
    }
    throw new DOMException(`unsupported export format: ${format}`, "NotSupportedError");
  }

  // ---- RSA helpers --------------------------------------------------------

  // Parse the host's length-prefixed framing (u32 BE length + bytes, repeated).
  function unframe(bytes) {
    const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
    const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    const parts = [];
    let off = 0;
    while (off < u8.length) {
      const len = dv.getUint32(off);
      off += 4;
      parts.push(u8.subarray(off, off + len));
      off += len;
    }
    return parts;
  }

  function rsaAlgorithm(name, hashNm, n, e) {
    return {
      name,
      hash: { name: hashNm },
      modulusLength: n.length * 8,
      publicExponent: new Uint8Array(e),
    };
  }

  function importRsaKey(format, keyData, algo, extractable, usages) {
    const hashNm = hashName(algo.hash);
    if (format === "spki") {
      const spki = ops.rsa_import_spki(toBytes(keyData));
      const [n, e] = unframe(ops.rsa_jwk_public_params(spki));
      return new CryptoKey("public", extractable, rsaAlgorithm(algo.name, hashNm, n, e), usages, spki);
    }
    if (format === "pkcs8") {
      const pkcs8 = ops.rsa_import_pkcs8(toBytes(keyData));
      const [n, e] = unframe(ops.rsa_jwk_private_params(pkcs8));
      return new CryptoKey("private", extractable, rsaAlgorithm(algo.name, hashNm, n, e), usages, pkcs8);
    }
    if (format === "jwk") {
      const jwk = keyData;
      if (jwk.kty !== "RSA") throw new DOMException("JWK kty must be RSA", "DataError");
      if (jwk.n == null || jwk.e == null) throw new DOMException("JWK missing n/e", "DataError");
      const n = b64uToBytes(jwk.n);
      const e = b64uToBytes(jwk.e);
      if (jwk.d != null) {
        if (jwk.p == null || jwk.q == null) {
          throw new DOMException("RSA private JWK requires p and q", "DataError");
        }
        const pkcs8 = ops.rsa_pkcs8_from_jwk(n, e, b64uToBytes(jwk.d), b64uToBytes(jwk.p), b64uToBytes(jwk.q));
        return new CryptoKey("private", extractable, rsaAlgorithm(algo.name, hashNm, n, e), usages, pkcs8);
      }
      const spki = ops.rsa_spki_from_jwk(n, e);
      return new CryptoKey("public", extractable, rsaAlgorithm(algo.name, hashNm, n, e), usages, spki);
    }
    throw new DOMException(`unsupported import format: ${format}`, "NotSupportedError");
  }

  function exportRsaKey(format, key) {
    if (!key.extractable) {
      throw new DOMException("key is not extractable", "InvalidAccessError");
    }
    if (format === "spki") {
      if (key.type !== "public") throw new DOMException("spki export is public-only", "InvalidAccessError");
      return asArrayBuffer(key[KEY].slice());
    }
    if (format === "pkcs8") {
      if (key.type !== "private") throw new DOMException("pkcs8 export is private-only", "InvalidAccessError");
      return asArrayBuffer(key[KEY].slice());
    }
    if (format === "jwk") {
      const priv = key.type === "private";
      const parts = priv
        ? unframe(ops.rsa_jwk_private_params(key[KEY]))
        : unframe(ops.rsa_jwk_public_params(key[KEY]));
      const jwk = {
        kty: "RSA",
        n: bytesToB64u(parts[0]),
        e: bytesToB64u(parts[1]),
        key_ops: [...key.usages],
        ext: key.extractable,
      };
      if (priv) {
        const [, , d, p, q, dp, dq, qi] = parts;
        jwk.d = bytesToB64u(d);
        jwk.p = bytesToB64u(p);
        jwk.q = bytesToB64u(q);
        jwk.dp = bytesToB64u(dp);
        jwk.dq = bytesToB64u(dq);
        jwk.qi = bytesToB64u(qi);
      }
      return jwk;
    }
    throw new DOMException(`unsupported export format: ${format}`, "NotSupportedError");
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
        const lengthBits = algo.length ?? HASH_BITS[hash];
        const material = ops.random_bytes(Math.ceil(lengthBits / 8));
        return new CryptoKey("secret", extractable, { name: "HMAC", hash: { name: hash }, length: lengthBits }, usages, material);
      }
      if (AES_ALGS.has(algo.name)) {
        const allowed = algo.name === "AES-GCM" ? [128, 256] : [128, 192, 256];
        if (!allowed.includes(algo.length)) {
          throw new DOMException(
            `${algo.name} key length must be ${allowed.join(" or ")}`,
            "OperationError",
          );
        }
        const material = ops.random_bytes(algo.length / 8);
        return new CryptoKey("secret", extractable, { name: algo.name, length: algo.length }, usages, material);
      }
      if (EC_ALGS.has(algo.name)) {
        const curve = ecCurve(algo);
        const alg = { name: algo.name, namedCurve: curve };
        const pkcs8 = ops.ec_generate_pkcs8(curve);
        const sec1 = ops.ec_public_point(curve, pkcs8);
        // Split the requested usages between the two keys: ECDSA →
        // sign(private)/verify(public); ECDH → derive*(private), none public.
        const isEcdsa = algo.name === "ECDSA";
        const privOps = isEcdsa ? ["sign"] : ["deriveBits", "deriveKey"];
        const privUsages = usages.filter((u) => privOps.includes(u));
        const pubUsages = isEcdsa ? usages.filter((u) => u === "verify") : [];
        return {
          privateKey: new CryptoKey("private", extractable, alg, privUsages, pkcs8),
          publicKey: new CryptoKey("public", true, alg, pubUsages, sec1),
        };
      }
      if (RSA_ALGS.has(algo.name)) {
        const exp = toBytes(algo.publicExponent);
        const pkcs8 = ops.rsa_generate_pkcs8(algo.modulusLength, exp);
        const spki = ops.rsa_public_spki(pkcs8);
        const alg = {
          name: algo.name,
          hash: { name: hashName(algo.hash) },
          modulusLength: algo.modulusLength,
          publicExponent: new Uint8Array(exp),
        };
        // OAEP keys do encrypt/decrypt(+wrap); SSA/PSS keys sign/verify.
        const isOaep = algo.name === "RSA-OAEP";
        const privOps = isOaep ? ["decrypt", "unwrapKey"] : ["sign"];
        const pubOps = isOaep ? ["encrypt", "wrapKey"] : ["verify"];
        return {
          privateKey: new CryptoKey("private", extractable, alg, usages.filter((u) => privOps.includes(u)), pkcs8),
          publicKey: new CryptoKey("public", true, alg, usages.filter((u) => pubOps.includes(u)), spki),
        };
      }
      throw new DOMException(`unsupported algorithm: ${algo.name}`, "NotSupportedError");
    },

    async importKey(format, keyData, algorithm, extractable, usages) {
      const algo = normalizeAlgorithm(algorithm);
      if (EC_ALGS.has(algo.name)) {
        return importEcKey(format, keyData, algo, extractable, usages);
      }
      if (RSA_ALGS.has(algo.name)) {
        return importRsaKey(format, keyData, algo, extractable, usages);
      }
      if (format !== "raw") {
        throw new DOMException(`unsupported import format: ${format}`, "NotSupportedError");
      }
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
      if (AES_ALGS.has(algo.name)) {
        const bits = material.length * 8;
        if (bits !== 128 && bits !== 192 && bits !== 256) {
          throw new DOMException(`invalid ${algo.name} key length`, "DataError");
        }
        return new CryptoKey("secret", extractable, { name: algo.name, length: bits }, usages, material);
      }
      if (KDF_ALGS.has(algo.name)) {
        // HKDF/PBKDF2 base keys carry the raw IKM/password and are never
        // extractable (per spec); the derivation parameters are supplied at
        // derive time, so the key's algorithm is just its name.
        if (extractable) {
          throw new DOMException(`${algo.name} keys must be non-extractable`, "SyntaxError");
        }
        return new CryptoKey("secret", false, { name: algo.name }, usages, material);
      }
      throw new DOMException(`unsupported algorithm: ${algo.name}`, "NotSupportedError");
    },

    async exportKey(format, key) {
      if (EC_ALGS.has(key.algorithm.name)) {
        return exportEcKey(format, key);
      }
      if (RSA_ALGS.has(key.algorithm.name)) {
        return exportRsaKey(format, key);
      }
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
      if (algo.name === "ECDSA") {
        return asArrayBuffer(
          ops.ecdsa_sign(key.algorithm.namedCurve, hashName(algo.hash), key[KEY], toBytes(data)),
        );
      }
      if (algo.name === "RSASSA-PKCS1-v1_5") {
        return asArrayBuffer(ops.rsa_pkcs1v15_sign(key.algorithm.hash.name, key[KEY], toBytes(data)));
      }
      if (algo.name === "RSA-PSS") {
        return asArrayBuffer(
          ops.rsa_pss_sign(key.algorithm.hash.name, algo.saltLength, key[KEY], toBytes(data)),
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
      if (algo.name === "ECDSA") {
        return ops.ecdsa_verify(
          key.algorithm.namedCurve,
          hashName(algo.hash),
          key[KEY],
          toBytes(signature),
          toBytes(data),
        );
      }
      if (algo.name === "RSASSA-PKCS1-v1_5") {
        return ops.rsa_pkcs1v15_verify(key.algorithm.hash.name, key[KEY], toBytes(signature), toBytes(data));
      }
      if (algo.name === "RSA-PSS") {
        return ops.rsa_pss_verify(
          key.algorithm.hash.name,
          algo.saltLength,
          key[KEY],
          toBytes(signature),
          toBytes(data),
        );
      }
      throw new DOMException(`unsupported verify algorithm: ${algo.name}`, "NotSupportedError");
    },

    async encrypt(algorithm, key, data) {
      const algo = normalizeAlgorithm(algorithm);
      switch (algo.name) {
        case "AES-GCM": {
          const iv = toBytes(algo.iv);
          const aad = algo.additionalData ? toBytes(algo.additionalData) : new Uint8Array(0);
          return asArrayBuffer(ops.subtle_aes_gcm_encrypt(key[KEY], iv, toBytes(data), aad));
        }
        case "AES-CBC":
          return asArrayBuffer(ops.subtle_aes_cbc_encrypt(key[KEY], toBytes(algo.iv), toBytes(data)));
        case "AES-CTR":
          return asArrayBuffer(
            ops.subtle_aes_ctr(key[KEY], toBytes(algo.counter), algo.length, toBytes(data)),
          );
        case "RSA-OAEP": {
          const label = algo.label ? toBytes(algo.label) : new Uint8Array(0);
          return asArrayBuffer(ops.rsa_oaep_encrypt(key.algorithm.hash.name, label, key[KEY], toBytes(data)));
        }
      }
      throw new DOMException(`unsupported encrypt algorithm: ${algo.name}`, "NotSupportedError");
    },

    async decrypt(algorithm, key, data) {
      const algo = normalizeAlgorithm(algorithm);
      switch (algo.name) {
        case "AES-GCM": {
          const iv = toBytes(algo.iv);
          const aad = algo.additionalData ? toBytes(algo.additionalData) : new Uint8Array(0);
          return asArrayBuffer(ops.subtle_aes_gcm_decrypt(key[KEY], iv, toBytes(data), aad));
        }
        case "AES-CBC":
          return asArrayBuffer(ops.subtle_aes_cbc_decrypt(key[KEY], toBytes(algo.iv), toBytes(data)));
        case "AES-CTR":
          // CTR is symmetric — the same op decrypts.
          return asArrayBuffer(
            ops.subtle_aes_ctr(key[KEY], toBytes(algo.counter), algo.length, toBytes(data)),
          );
        case "RSA-OAEP": {
          const label = algo.label ? toBytes(algo.label) : new Uint8Array(0);
          return asArrayBuffer(ops.rsa_oaep_decrypt(key.algorithm.hash.name, label, key[KEY], toBytes(data)));
        }
      }
      throw new DOMException(`unsupported decrypt algorithm: ${algo.name}`, "NotSupportedError");
    },

    async deriveBits(algorithm, baseKey, length) {
      const algo = normalizeAlgorithm(algorithm);
      if (algo.name === "ECDH") {
        // The full shared secret is the agreed X coordinate (field width).
        // A null length returns all of it; otherwise take the leading bits.
        const curve = baseKey.algorithm.namedCurve;
        const shared = ops.ecdh_derive(curve, baseKey[KEY], algo.public[KEY]);
        if (length == null) return asArrayBuffer(shared);
        if (length % 8 !== 0) {
          throw new DOMException("ECDH length must be a multiple of 8", "OperationError");
        }
        const bytes = length / 8;
        if (bytes > shared.length) {
          throw new DOMException("requested ECDH length exceeds the shared secret", "OperationError");
        }
        return asArrayBuffer(shared.subarray(0, bytes));
      }
      if (length == null || length % 8 !== 0) {
        throw new DOMException("deriveBits length must be a non-null multiple of 8", "OperationError");
      }
      const lengthBytes = length / 8;
      if (algo.name === "HKDF") {
        const info = algo.info ? toBytes(algo.info) : new Uint8Array(0);
        return asArrayBuffer(
          ops.subtle_hkdf(hashName(algo.hash), baseKey[KEY], toBytes(algo.salt), info, lengthBytes),
        );
      }
      if (algo.name === "PBKDF2") {
        return asArrayBuffer(
          ops.subtle_pbkdf2(
            hashName(algo.hash),
            baseKey[KEY],
            toBytes(algo.salt),
            algo.iterations,
            lengthBytes,
          ),
        );
      }
      throw new DOMException(`unsupported derive algorithm: ${algo.name}`, "NotSupportedError");
    },

    async deriveKey(algorithm, baseKey, derivedKeyAlgorithm, extractable, usages) {
      const dka = normalizeAlgorithm(derivedKeyAlgorithm);
      let bits;
      if (AES_ALGS.has(dka.name)) {
        bits = dka.length;
        if (bits !== 128 && bits !== 192 && bits !== 256) {
          throw new DOMException(`invalid derived ${dka.name} key length`, "OperationError");
        }
      } else if (dka.name === "HMAC") {
        bits = dka.length ?? HASH_BITS[hashName(dka.hash)];
      } else {
        throw new DOMException(`cannot derive ${dka.name} keys`, "NotSupportedError");
      }
      const derived = await subtle.deriveBits(algorithm, baseKey, bits);
      return subtle.importKey("raw", derived, dka, extractable, usages);
    },
  };

  globalThis.CryptoKey = CryptoKey;
  globalThis.crypto = Object.freeze({
    getRandomValues,
    randomUUID,
    subtle: Object.freeze(subtle),
  });
})();
