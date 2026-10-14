declare module "runtime:hashing" {
  /**
   * Anything that can be hashed. A string is hashed as its UTF-8 bytes — the
   * same bytes `TextEncoder` would have produced.
   */
  export type HashInput = string | ArrayBuffer | ArrayBufferView;

  /**
   * How a digest is returned. Omit it (or pass `"bytes"`) for a `Uint8Array`;
   * the string forms are encoded in the host rather than at the call site.
   */
  export type HashEncoding = "bytes" | "hex" | "base64" | "base64url";

  /**
   * Every algorithm `hash()` and `Hasher` accept.
   *
   * The first group is cryptographic; `xxhash64`/`xxhash3`/`crc32`/`crc32c` are
   * not, and are refused by {@link hmac}. WebCrypto's spellings (`"SHA-256"`)
   * are accepted too, as is any casing.
   */
  export type HashAlgorithm =
    | "sha1"
    | "sha256"
    | "sha384"
    | "sha512"
    | "sha3-224"
    | "sha3-256"
    | "sha3-384"
    | "sha3-512"
    | "blake3"
    | "md5"
    | "ripemd160"
    | "xxhash64"
    | "xxhash3"
    | "crc32"
    | "crc32c"
    | (string & {});

  /** The digest of `data` as a `Uint8Array`. */
  export function hash(algorithm: HashAlgorithm, data: HashInput): Uint8Array;
  /** The digest of `data`, encoded as a string. */
  export function hash(
    algorithm: HashAlgorithm,
    data: HashInput,
    encoding: "hex" | "base64" | "base64url",
  ): string;
  export function hash(
    algorithm: HashAlgorithm,
    data: HashInput,
    encoding?: HashEncoding,
  ): Uint8Array | string;

  /**
   * A hash computed across many chunks — for anything too large to hold, or
   * that arrives over time.
   *
   * ```ts
   * const h = new Hasher("sha256");
   * for await (const chunk of file.stream()) h.update(chunk);
   * h.digest("hex");
   * ```
   *
   * `digest()` ends the hasher: the host state is released, and calling either
   * method again throws rather than silently starting a second hash.
   */
  export class Hasher {
    constructor(algorithm: HashAlgorithm);
    /** The algorithm this hasher was created for. */
    readonly algorithm: string;
    /** Adds `data` to the hash. Returns the hasher, so calls chain. */
    update(data: HashInput): this;
    /** The digest of everything added so far, as bytes. Ends the hasher. */
    digest(): Uint8Array;
    /** The digest of everything added so far, encoded. Ends the hasher. */
    digest(encoding: "hex" | "base64" | "base64url"): string;
    digest(encoding?: HashEncoding): Uint8Array | string;
  }

  /**
   * The digest of a stream, read to the end.
   *
   * ```ts
   * await hashStream("sha256", request.body, "hex");
   * ```
   */
  export function hashStream(
    algorithm: HashAlgorithm,
    stream: ReadableStream<Uint8Array>,
  ): Promise<Uint8Array>;
  export function hashStream(
    algorithm: HashAlgorithm,
    stream: ReadableStream<Uint8Array>,
    encoding: "hex" | "base64" | "base64url",
  ): Promise<string>;
  export function hashStream(
    algorithm: HashAlgorithm,
    stream: ReadableStream<Uint8Array>,
    encoding?: HashEncoding,
  ): Promise<Uint8Array | string>;

  /**
   * HMAC (RFC 2104), synchronous and in one call.
   *
   * Needs a cryptographic hash: the checksums are refused. For a `CryptoKey` or
   * a JWK you already hold, use `crypto.subtle` — it is the same construction.
   */
  export function hmac(
    algorithm: HashAlgorithm,
    key: HashInput,
    data: HashInput,
  ): Uint8Array;
  export function hmac(
    algorithm: HashAlgorithm,
    key: HashInput,
    data: HashInput,
    encoding: "hex" | "base64" | "base64url",
  ): string;
  export function hmac(
    algorithm: HashAlgorithm,
    key: HashInput,
    data: HashInput,
    encoding?: HashEncoding,
  ): Uint8Array | string;

  /**
   * Constant-time comparison, for anything an attacker can submit repeatedly:
   * a webhook signature, an API token, a MAC.
   *
   * The lengths are compared first and in ordinary time, since a digest's
   * length is fixed by its algorithm and public already.
   */
  export function timingSafeEqual(a: HashInput, b: HashInput): boolean;

  /** The password algorithms. Argon2id is the default. */
  export type PasswordAlgorithm =
    | "argon2id"
    | "argon2i"
    | "argon2d"
    | "bcrypt"
    | "scrypt";

  export interface PasswordOptions {
    /** Default `"argon2id"`. */
    algorithm?: PasswordAlgorithm;
    /** argon2 only: memory in KiB. Default `19456`. */
    memoryCost?: number;
    /** argon2 only: passes. Default `2` (`3` for argon2i). */
    timeCost?: number;
    /** argon2 and scrypt: lanes. Default `1`. */
    parallelism?: number;
    /** bcrypt: log₂ rounds (default `12`). scrypt: log₂ N (default `17`). */
    cost?: number;
    /** scrypt only: block size r. Default `8`. */
    blockSize?: number;
    /**
     * The salt, if you are supplying it. Defaults to 16 fresh random bytes,
     * which is what you want everywhere except reproducing a specific hash in
     * a test. bcrypt's salt is exactly 16 bytes.
     */
    salt?: HashInput;
  }

  /**
   * Password hashing: Argon2id by default, bcrypt and scrypt for hashes that
   * already exist.
   *
   * These are slow on purpose, and slow on the thread that calls them — a login
   * endpoint under load wants a queue in front of it, not a hundred concurrent
   * calls.
   */
  export const password: {
    /**
     * Hashes `input`, returning the string to store: algorithm, parameters,
     * salt and digest together, so nothing else has to be kept beside it.
     *
     * Draws a random salt, and so needs the `Entropy` capability.
     */
    hash(input: HashInput, options?: PasswordOptions): Promise<string>;

    /**
     * Whether `input` is the password `stored` was made from.
     *
     * The algorithm and parameters are read from `stored`, so a hash written
     * under weaker settings still verifies. Needs no capability.
     */
    verify(input: HashInput, stored: string): Promise<boolean>;

    /**
     * Whether `stored` was written with weaker settings than `options` asks
     * for — the companion to `verify()`, since a correct login is the only
     * moment an old hash can be replaced.
     */
    needsRehash(stored: string, options?: PasswordOptions): boolean;
  };

  /** Options shared by the TOTP functions (D164). The defaults are the RFC's. */
  export interface TotpOptions {
    /** Default `"SHA1"`. Google Authenticator supports only SHA-1. */
    algorithm?: "SHA1" | "SHA256" | "SHA512";
    /** Default 6. */
    digits?: 6 | 7 | 8;
    /** Seconds per code. Default 30. */
    period?: number;
    /** The moment to generate or check for. Default now. */
    time?: Date | number;
  }

  /**
   * Time-based one-time passwords (RFC 6238) for two-factor sign-in (D164).
   * A secret is base32, as authenticator apps expect, or bytes; at least 16
   * bytes.
   */
  export const totp: {
    /** A new base32 secret of 20 random bytes. Needs Entropy. */
    generateSecret(): string;
    /** The `otpauth://totp/…` URI for a QR code. */
    uri(options: {
      secret: string | BufferSource;
      issuer: string;
      account: string;
      algorithm?: TotpOptions["algorithm"];
      digits?: TotpOptions["digits"];
      period?: number;
    }): string;
    /** The code for `options.time`, default now. */
    generate(secret: string | BufferSource, options?: TotpOptions): string;
    /**
     * The time step `code` matched, within `window` steps (default 1, at most
     * 10), or `null`. Store the step and pass it as `after` next time, so a
     * code works only once. A malformed code is `null`.
     */
    verify(
      code: unknown,
      secret: string | BufferSource,
      options?: TotpOptions & { window?: number; after?: number | null },
    ): number | null;
    /**
     * One-time recovery codes, `xxxxx-xxxxx`; default 10. Store them with
     * `password.hash`. Needs Entropy.
     */
    backupCodes(count?: number): string[];
  };

  const hashing: {
    hash: typeof hash;
    Hasher: typeof Hasher;
    hashStream: typeof hashStream;
    hmac: typeof hmac;
    timingSafeEqual: typeof timingSafeEqual;
    password: typeof password;
    totp: typeof totp;
  };
  export default hashing;
}
