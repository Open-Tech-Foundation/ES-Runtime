// runtime:hashing — digests, checksums, MACs and password hashing (DECISIONS D57).
//
// `crypto.subtle` is the WebCrypto standard and stays exactly that. This module
// is the rest of what a server hashes for, and needs no capability: hashing
// reads nothing and reaches nothing, so a runtime granted no authority at all
// can still import and use every function here.
//
//   * The digests WebCrypto has no name for — SHA-3, BLAKE3, MD5, RIPEMD-160 —
//     alongside the SHA-2 family it does, under one API. `subtle`'s names work
//     too: "SHA-256" and "sha256" are the same algorithm.
//   * Incrementally, not only one-shot. `subtle.digest` takes the whole input
//     at once, so hashing a 4 GB upload means holding 4 GB. A `Hasher` holds a
//     few hundred bytes of state instead.
//   * Encoded output. `hash("sha256", x, "hex")` returns the string, because
//     the alternative is the byte-by-byte loop every codebase writes once and
//     then copies forever.
//   * Checksums — xxHash, CRC-32, CRC-32C — for cache keys, ETags and shard
//     selection, where a cryptographic hash costs ten times as much to answer a
//     question nobody is attacking.
//   * Passwords, where `subtle` offers only PBKDF2. Argon2id by default,
//     bcrypt and scrypt for what already exists in your database.
//
// One asymmetry worth knowing: `password.hash()` needs a random salt, so it
// draws one from `crypto.getRandomValues` and therefore needs the Entropy
// capability. `password.verify()` needs no randomness — the salt is inside the
// stored string — so a service that only checks passwords needs nothing.

const ops = globalThis.__ops;

// Defaults per algorithm, following the OWASP Password Storage Cheat Sheet.
// They live here, in the open, rather than in the host: raising them is a
// decision to make deliberately, and a hash written under the old ones keeps
// verifying (its parameters travel inside the stored string).
const PASSWORD_DEFAULTS = {
  argon2id: { memoryCost: 19456, timeCost: 2, parallelism: 1 },
  argon2i: { memoryCost: 19456, timeCost: 3, parallelism: 1 },
  argon2d: { memoryCost: 19456, timeCost: 2, parallelism: 1 },
  bcrypt: { cost: 12 },
  scrypt: { cost: 17, blockSize: 8, parallelism: 1 },
};

// bcrypt's salt is a fixed-width field of the `$2b$` string, not a
// variable-length one, so it is 16 bytes and cannot be anything else.
const SALT_BYTES = 16;

// A string is passed through untouched: the host reads its UTF-8 bytes
// directly, which is one copy fewer than encoding it here first.
function toInput(data, what) {
  if (typeof data === "string" || data instanceof Uint8Array) return data;
  if (ArrayBuffer.isView(data)) return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  throw new TypeError(`${what} must be a string, ArrayBuffer, or ArrayBufferView`);
}

function checkAlgorithm(algorithm) {
  if (typeof algorithm !== "string") throw new TypeError("an algorithm name must be a string");
  return algorithm;
}

// A hasher that is dropped without ever being digested — a request handler that
// throws halfway through one — would otherwise leave its native state alive for
// the life of the isolate. `digest()` frees it; this frees the rest. The held
// value is the id alone, never the hasher, since a registry that held the
// object would keep it alive and the callback would never run.
const abandoned = new FinalizationRegistry((id) => ops.hash_free(id));

/**
 * The digest of `data`, in one call.
 *
 * `encoding` is "hex", "base64" or "base64url" for a string; omit it for a
 * Uint8Array.
 */
export function hash(algorithm, data, encoding) {
  return ops.hash_digest(checkAlgorithm(algorithm), toInput(data, "data"), encoding);
}

/**
 * A hash computed across many chunks — for anything too large to hold, or that
 * arrives over time.
 *
 *   const h = new Hasher("sha256");
 *   for await (const chunk of file.stream()) h.update(chunk);
 *   h.digest("hex");
 *
 * `digest()` ends the hasher: the host state is released, and calling either
 * method again throws rather than silently starting a second hash.
 */
export class Hasher {
  #id;
  #algorithm;

  constructor(algorithm) {
    this.#algorithm = checkAlgorithm(algorithm);
    this.#id = ops.hash_new(this.#algorithm);
    abandoned.register(this, this.#id, this);
  }

  /** The algorithm this hasher was created for. */
  get algorithm() {
    return this.#algorithm;
  }

  /** Adds `data` to the hash. Returns the hasher, so calls chain. */
  update(data) {
    ops.hash_update(this.#id, toInput(data, "data"));
    return this;
  }

  /** The digest of everything added so far. Ends the hasher. */
  digest(encoding) {
    const out = ops.hash_finish(this.#id, encoding);
    abandoned.unregister(this);
    this.#id = 0;
    return out;
  }
}

/**
 * The digest of a stream, read to the end.
 *
 * The reason `Hasher` exists, spelled as the one line it is usually wanted in:
 * `await hashStream("sha256", request.body, "hex")`.
 */
export async function hashStream(algorithm, stream, encoding) {
  if (!stream || typeof stream.getReader !== "function") {
    throw new TypeError("hashStream expects a ReadableStream");
  }
  const hasher = new Hasher(algorithm);
  const reader = stream.getReader();
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      hasher.update(value);
    }
  } finally {
    reader.releaseLock();
  }
  return hasher.digest(encoding);
}

/**
 * HMAC (RFC 2104) over any cryptographic hash here — including SHA-3 and, for
 * legacy protocols, MD5, neither of which `crypto.subtle` will name.
 *
 * Synchronous, and one call rather than `importKey` then `sign`. For WebCrypto
 * interoperability — a `CryptoKey` you already hold, a JWK — use
 * `crypto.subtle` instead; this is the same construction, not a different one.
 */
export function hmac(algorithm, key, data, encoding) {
  return ops.hash_hmac(
    checkAlgorithm(algorithm),
    toInput(key, "key"),
    toInput(data, "data"),
    encoding,
  );
}

/**
 * Constant-time comparison, for anything an attacker can submit repeatedly: a
 * webhook signature, an API token, a MAC.
 *
 * `===` on the hex strings leaks how much of the prefix was right, one request
 * at a time. This does not — though the *lengths* are compared first and in
 * ordinary time, since a digest's length is fixed by its algorithm and public
 * already.
 */
export function timingSafeEqual(a, b) {
  return ops.hash_equal(toInput(a, "a"), toInput(b, "b"));
}

function resolveOptions(options) {
  const { algorithm = "argon2id", ...rest } = options ?? {};
  const defaults = PASSWORD_DEFAULTS[algorithm];
  if (!defaults) {
    const known = Object.keys(PASSWORD_DEFAULTS).join(", ");
    throw new TypeError(`unknown password algorithm '${algorithm}' (expected one of: ${known})`);
  }
  return { algorithm, params: { ...defaults, ...rest } };
}

// The parameters a stored hash was written with, read back out of it. PHC
// strings (`$argon2id$v=19$m=19456,t=2,p=1$…`) carry them as a comma-separated
// field; bcrypt (`$2b$12$…`) carries its one parameter as the second field.
function storedParams(stored) {
  if (typeof stored !== "string") throw new TypeError("a stored password hash must be a string");
  if (stored.startsWith("$2")) {
    const cost = Number.parseInt(stored.split("$")[2], 10);
    return { algorithm: "bcrypt", params: { cost } };
  }
  const fields = stored.split("$");
  const algorithm = fields[1];
  // Every `k=v` field, merged: the version field (`v=19`) is one of them and is
  // simply read alongside the rest, which is why this does not have to know
  // whether a given algorithm writes one.
  const params = {};
  for (const field of fields) {
    if (!/^[a-z]+=\d/.test(field)) continue;
    for (const pair of field.split(",")) {
      const [key, value] = pair.split("=");
      if (key && value !== undefined) params[key] = Number.parseInt(value, 10);
    }
  }
  if (algorithm === "scrypt") {
    return { algorithm, params: { cost: params.ln, blockSize: params.r, parallelism: params.p } };
  }
  return {
    algorithm,
    params: { memoryCost: params.m, timeCost: params.t, parallelism: params.p },
  };
}

/**
 * Password hashing: Argon2id by default, bcrypt and scrypt for hashes that
 * already exist.
 *
 * These are slow on purpose — that is the entire mechanism — and they are slow
 * on the thread that calls them. `hash()` at the default cost takes on the
 * order of 50ms and blocks the isolate for all of it, so a login endpoint under
 * load wants a queue in front of it, not a hundred concurrent calls.
 */
export const password = {
  /**
   * Hashes `input`, returning the string to store: algorithm, parameters, salt
   * and digest together, so nothing else has to be kept beside it.
   *
   * The salt comes from `crypto.getRandomValues`, so this needs the Entropy
   * capability. Pass `salt` to supply your own — for reproducing a specific
   * hash in a test, and for nothing else.
   */
  async hash(input, options) {
    const { algorithm, params } = resolveOptions(options);
    const salt = params.salt
      ? toInput(params.salt, "salt")
      : crypto.getRandomValues(new Uint8Array(SALT_BYTES));
    delete params.salt;
    return ops.hash_password(algorithm, toInput(input, "password"), salt, params);
  },

  /**
   * Whether `input` is the password `stored` was made from.
   *
   * The algorithm and parameters are read from `stored`, so a hash written
   * years ago under weaker settings still verifies — which is what makes it
   * possible to raise the settings at all. Needs no capability.
   */
  async verify(input, stored) {
    if (typeof stored !== "string") throw new TypeError("a stored password hash must be a string");
    return ops.hash_password_verify(toInput(input, "password"), stored);
  },

  /**
   * Whether `stored` was written with weaker settings than `options` asks for.
   *
   * The companion to `verify()`: a correct login is the one moment you hold the
   * plaintext, and so the only moment an old hash can be replaced.
   *
   *   if (await password.verify(input, user.hash)) {
   *     if (password.needsRehash(user.hash)) user.hash = await password.hash(input);
   *   }
   */
  needsRehash(stored, options) {
    const wanted = resolveOptions(options);
    const actual = storedParams(stored);
    if (actual.algorithm !== wanted.algorithm) return true;
    for (const [key, value] of Object.entries(wanted.params)) {
      if (key === "salt") continue;
      if (!(actual.params[key] >= value)) return true;
    }
    return false;
  },
};

// ---- TOTP (D164) -----------------------------------------------------------
//
// RFC 6238 codes over RFC 4226's HOTP, the `otpauth://` URI authenticator apps
// scan (Google's Key URI Format), and backup codes. `verify` returns the step
// it matched so the caller can refuse it next time: RFC 6238 §5.2 says a code
// MUST NOT be accepted twice, and that needs storage the runtime does not have.

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
// Crockford's base32 alphabet: no I, L, O or U, so a code read aloud or copied
// from paper is not misread.
const RECOVERY = "0123456789abcdefghjkmnpqrstvwxyz";
const MIN_TOTP_SECRET = 16; // RFC 4226 R6: at least 128 bits
const MAX_TOTP_WINDOW = 10;
const TOTP_ALGORITHMS = { SHA1: "sha1", SHA256: "sha256", SHA512: "sha512" };
const TOTP_OPTIONS = new Set(["algorithm", "digits", "period", "time"]);

function base32Encode(bytes) {
  let out = "";
  let bits = 0;
  let value = 0;
  for (const byte of bytes) {
    value = (value << 8) | byte;
    bits += 8;
    while (bits >= 5) {
      out += BASE32[(value >>> (bits - 5)) & 31];
      bits -= 5;
    }
  }
  if (bits > 0) out += BASE32[(value << (5 - bits)) & 31];
  return out;
}

// Read as typed: any case, spaces and `=` padding ignored.
function base32Decode(text) {
  const clean = text.replace(/[\s=]/g, "").toUpperCase();
  const out = [];
  let bits = 0;
  let value = 0;
  for (const char of clean) {
    const index = BASE32.indexOf(char);
    if (index === -1) throw new TypeError(`the TOTP secret is not base32: unexpected "${char}"`);
    value = ((value << 5) | index) & 0xffff;
    bits += 5;
    if (bits >= 8) {
      out.push((value >>> (bits - 8)) & 0xff);
      bits -= 8;
    }
  }
  return new Uint8Array(out);
}

function totpKey(secret) {
  const key =
    typeof secret === "string" ? base32Decode(secret) : toInput(secret, "the TOTP secret");
  if (key.byteLength < MIN_TOTP_SECRET) {
    throw new TypeError(
      `a TOTP secret must be at least ${MIN_TOTP_SECRET} bytes (128 bits); this one is ${key.byteLength}`,
    );
  }
  return key;
}

function totpOptions(options, allowed, fn) {
  if (options === null || typeof options !== "object") {
    throw new TypeError(`totp.${fn} options must be an object`);
  }
  for (const key of Object.keys(options)) {
    if (!allowed.has(key)) throw new TypeError(`unknown totp.${fn} option "${key}"`);
  }
  const name = String(options.algorithm ?? "SHA1").toUpperCase().replace("-", "");
  const algorithm = TOTP_ALGORITHMS[name];
  if (!algorithm) throw new TypeError('algorithm must be "SHA1", "SHA256" or "SHA512"');
  const digits = options.digits ?? 6;
  if (![6, 7, 8].includes(digits)) throw new TypeError("digits must be 6, 7 or 8");
  const period = options.period ?? 30;
  if (!Number.isInteger(period) || period <= 0) {
    throw new TypeError("period must be a positive whole number of seconds");
  }
  const time = options.time ?? Date.now();
  const ms = time instanceof Date ? time.getTime() : time;
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) {
    throw new TypeError("time must be a Date or milliseconds since the epoch");
  }
  return { name, algorithm, digits, period, step: Math.floor(ms / 1000 / period) };
}

// RFC 4226 §5.3: HMAC over the counter, dynamic truncation, then the digits.
function hotp(key, counter, algorithm, digits) {
  const message = new Uint8Array(8);
  const high = Math.floor(counter / 2 ** 32);
  const low = counter >>> 0;
  new DataView(message.buffer).setUint32(0, high);
  new DataView(message.buffer).setUint32(4, low);
  const mac = ops.hash_hmac(algorithm, key, message);
  const offset = mac[mac.length - 1] & 0x0f;
  const binary =
    ((mac[offset] & 0x7f) << 24) |
    (mac[offset + 1] << 16) |
    (mac[offset + 2] << 8) |
    mac[offset + 3];
  return String(binary % 10 ** digits).padStart(digits, "0");
}

/**
 * Time-based one-time passwords for two-factor sign-in (D164).
 */
export const totp = {
  /** A new base32 secret of 20 random bytes (160 bits). Needs Entropy. */
  generateSecret() {
    return base32Encode(crypto.getRandomValues(new Uint8Array(20)));
  },

  /**
   * The `otpauth://totp/…` URI an authenticator app reads from a QR code.
   * Google Authenticator ignores `algorithm`, `digits` and `period`, so keep
   * the defaults for it.
   */
  uri(options) {
    if (options === null || typeof options !== "object") {
      throw new TypeError("totp.uri takes { secret, issuer, account }");
    }
    const { secret, issuer, account, ...rest } = options;
    const { name, digits, period } = totpOptions(
      rest,
      new Set(["algorithm", "digits", "period"]),
      "uri",
    );
    for (const [what, value] of [["issuer", issuer], ["account", account]]) {
      if (typeof value !== "string" || value === "") throw new TypeError(`${what} is required`);
      if (value.includes(":")) throw new TypeError(`${what} must not contain ":"`);
    }
    const encoded = base32Encode(totpKey(secret));
    const label = `${encodeURIComponent(issuer)}:${encodeURIComponent(account)}`;
    return (
      `otpauth://totp/${label}?secret=${encoded}&issuer=${encodeURIComponent(issuer)}` +
      `&algorithm=${name}&digits=${digits}&period=${period}`
    );
  },

  /** The code for `time` (default now). */
  generate(secret, options = {}) {
    const { algorithm, digits, step } = totpOptions(options, TOTP_OPTIONS, "generate");
    return hotp(totpKey(secret), step, algorithm, digits);
  },

  /**
   * The time step `code` belongs to, within `window` steps of `time`, or
   * `null`. A step at or before `after` is refused: store the step this
   * returns and pass it as `after` next time, so a code works only once.
   * A malformed code is `null`, never a throw: it is request input.
   */
  verify(code, secret, options = {}) {
    const { window = 1, after, ...rest } = options;
    if (!Number.isInteger(window) || window < 0 || window > MAX_TOTP_WINDOW) {
      throw new TypeError(`window must be a whole number from 0 to ${MAX_TOTP_WINDOW}`);
    }
    if (after !== undefined && after !== null && !Number.isInteger(after)) {
      throw new TypeError("after must be the step a previous verify returned");
    }
    const { algorithm, digits, step } = totpOptions(rest, TOTP_OPTIONS, "verify");
    const key = totpKey(secret);
    if (typeof code !== "string") return null;
    const given = code.replace(/\s/g, "");
    if (given.length !== digits || !/^\d+$/.test(given)) return null;
    // Every step in the window is computed and compared, so the time taken
    // does not say which one matched.
    let matched = null;
    for (let s = step - window; s <= step + window; s++) {
      if (s < 0) continue;
      if (ops.hash_equal(hotp(key, s, algorithm, digits), given)) matched = s;
    }
    if (matched === null || (after != null && matched <= after)) return null;
    return matched;
  },

  /**
   * One-time recovery codes, `xxxxx-xxxxx` (50 bits each). Store them with
   * `password.hash` and check with `password.verify`. Needs Entropy.
   */
  backupCodes(count = 10) {
    if (!Number.isInteger(count) || count < 1 || count > 100) {
      throw new TypeError("count must be a whole number from 1 to 100");
    }
    const codes = [];
    for (let i = 0; i < count; i++) {
      const chars = Array.from(crypto.getRandomValues(new Uint8Array(10)), (b) => RECOVERY[b & 31]);
      codes.push(`${chars.slice(0, 5).join("")}-${chars.slice(5).join("")}`);
    }
    return codes;
  },
};

export default { hash, Hasher, hashStream, hmac, timingSafeEqual, password, totp };
