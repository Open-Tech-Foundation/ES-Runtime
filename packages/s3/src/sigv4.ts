/**
 * AWS Signature Version 4, as S3 uses it.
 *
 * Pure: a request description and credentials in, the headers or query
 * parameters that sign it out. Nothing here touches the network or the clock —
 * the caller passes the time, so the official test suite can pin it.
 *
 * Two places S3 differs from the generic algorithm, both handled by the caller
 * through `canonicalUri`: S3 encodes the path once (other services encode it
 * twice) and never normalizes it, so `a//b` and `a/./b` are distinct keys.
 */
import { hash, hmac } from "runtime:hashing";

/** Credentials as the signer needs them: plain strings. */
export interface SigningCredentials {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken?: string | undefined;
}

export interface SigningRequest {
  method: string;
  /** The path exactly as it will be sent, already percent-encoded. */
  canonicalUri: string;
  /** Query parameters, decoded. Order does not matter; they are sorted here. */
  query: ReadonlyArray<readonly [string, string]>;
  /** Headers to sign, including `host`. Names in any case; repeats are joined. */
  headers: ReadonlyArray<readonly [string, string]>;
}

export interface SigningScope {
  region: string;
  service: string;
  date: Date;
}

/** The hex SHA-256 of nothing: the payload hash of a request without a body. */
export const EMPTY_SHA256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/** The payload hash S3 accepts when the body is not hashed (presigned URLs). */
export const UNSIGNED_PAYLOAD = "UNSIGNED-PAYLOAD";

const ALGORITHM = "AWS4-HMAC-SHA256";

/**
 * RFC 3986 percent-encoding of a UTF-8 string: everything but the unreserved
 * characters `A-Z a-z 0-9 - _ . ~` is encoded, with upper-case hex.
 * `encodeURIComponent` leaves `!'()*` alone, which SigV4 does not.
 */
export function uriEncode(value: string): string {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

/** {@link uriEncode}, leaving `/` as the separator it is in a path. */
export function uriEncodePath(path: string): string {
  return path.split("/").map(uriEncode).join("/");
}

/** `20150830T123600Z`. */
export function amzDate(date: Date): string {
  return date
    .toISOString()
    .replace(/[-:]/g, "")
    .replace(/\.\d{3}/, "");
}

function canonicalQuery(query: SigningRequest["query"]): string {
  return query
    .map(([k, v]) => [uriEncode(k), uriEncode(v)] as const)
    .sort(([ak, av], [bk, bv]) => (ak < bk ? -1 : ak > bk ? 1 : av < bv ? -1 : av > bv ? 1 : 0))
    .map(([k, v]) => `${k}=${v}`)
    .join("&");
}

/** Lower-cased names, values trimmed with inner runs of spaces collapsed, repeats joined by `,`. */
function canonicalHeaders(headers: SigningRequest["headers"]): { block: string; signed: string } {
  const byName = new Map<string, string[]>();
  for (const [name, value] of headers) {
    const key = name.trim().toLowerCase();
    const clean = value.replace(/\s+/g, " ").trim();
    const list = byName.get(key);
    if (list) list.push(clean);
    else byName.set(key, [clean]);
  }
  const names = [...byName.keys()].sort();
  return {
    block: names.map((n) => `${n}:${byName.get(n)!.join(",")}\n`).join(""),
    signed: names.join(";"),
  };
}

function scopeOf(scope: SigningScope): string {
  return `${amzDate(scope.date).slice(0, 8)}/${scope.region}/${scope.service}/aws4_request`;
}

/**
 * Derives signing keys, remembering the last one: it changes once a day per
 * region and service, and every request of a client would otherwise pay four
 * HMACs for it.
 */
export class SigningKeys {
  #last: { id: string; key: Uint8Array } | undefined;

  get(secretAccessKey: string, scope: SigningScope): Uint8Array {
    const day = amzDate(scope.date).slice(0, 8);
    const id = `${day}\0${scope.region}\0${scope.service}\0${secretAccessKey}`;
    if (this.#last?.id === id) return this.#last.key;
    const kDate = hmac("sha256", `AWS4${secretAccessKey}`, day);
    const kRegion = hmac("sha256", kDate, scope.region);
    const kService = hmac("sha256", kRegion, scope.service);
    const key = hmac("sha256", kService, "aws4_request");
    this.#last = { id, key };
    return key;
  }
}

const sharedKeys = new SigningKeys();

export interface Signed {
  canonicalRequest: string;
  stringToSign: string;
  signature: string;
}

function sign(
  request: SigningRequest,
  query: SigningRequest["query"],
  headers: SigningRequest["headers"],
  payloadHash: string,
  credentials: SigningCredentials,
  scope: SigningScope,
  keys: SigningKeys,
): Signed & { signedHeaders: string } {
  const { block, signed } = canonicalHeaders(headers);
  const canonicalRequest = [
    request.method.toUpperCase(),
    request.canonicalUri,
    canonicalQuery(query),
    block,
    signed,
    payloadHash,
  ].join("\n");
  const stringToSign = [
    ALGORITHM,
    amzDate(scope.date),
    scopeOf(scope),
    hash("sha256", canonicalRequest, "hex"),
  ].join("\n");
  const signature = hmac(
    "sha256",
    keys.get(credentials.secretAccessKey, scope),
    stringToSign,
    "hex",
  );
  return { canonicalRequest, stringToSign, signature, signedHeaders: signed };
}

export interface HeaderSigningOptions {
  /** Hex SHA-256 of the body, or {@link UNSIGNED_PAYLOAD}. */
  payloadHash: string;
  /** Send and sign `x-amz-content-sha256`, as S3 requires. */
  contentSha256Header: boolean;
  keys?: SigningKeys;
}

/**
 * Signs `request` in the `Authorization` header. Returns the headers to add to
 * the request — `x-amz-date`, `x-amz-security-token` when there is a session
 * token, `x-amz-content-sha256` when asked, and `authorization` — along with
 * the intermediate strings, which the tests compare against the suite.
 */
export function signHeaders(
  request: SigningRequest,
  credentials: SigningCredentials,
  scope: SigningScope,
  options: HeaderSigningOptions,
): Signed & { headers: Array<[string, string]> } {
  const added: Array<[string, string]> = [["x-amz-date", amzDate(scope.date)]];
  if (credentials.sessionToken) added.push(["x-amz-security-token", credentials.sessionToken]);
  if (options.contentSha256Header) added.push(["x-amz-content-sha256", options.payloadHash]);
  const result = sign(
    request,
    request.query,
    [...request.headers, ...added],
    options.payloadHash,
    credentials,
    scope,
    options.keys ?? sharedKeys,
  );
  added.push([
    "authorization",
    `${ALGORITHM} Credential=${credentials.accessKeyId}/${scopeOf(scope)}, SignedHeaders=${result.signedHeaders}, Signature=${result.signature}`,
  ]);
  return { ...result, headers: added };
}

export interface QuerySigningOptions {
  /** Seconds the URL stays valid. */
  expiresIn: number;
  /** Defaults to {@link UNSIGNED_PAYLOAD}: a presigned URL's body is not known in advance. */
  payloadHash?: string;
  keys?: SigningKeys;
}

/**
 * Signs `request` in its query string — a presigned URL. Returns the full
 * query (the request's own parameters plus the `X-Amz-*` ones, signature last)
 * as decoded pairs, ready to append to a `URL`.
 */
export function signQuery(
  request: SigningRequest,
  credentials: SigningCredentials,
  scope: SigningScope,
  options: QuerySigningOptions,
): Signed & { query: Array<[string, string]> } {
  const { signed } = canonicalHeaders(request.headers);
  const query: Array<[string, string]> = [
    ...request.query.map(([k, v]) => [k, v] as [string, string]),
    ["X-Amz-Algorithm", ALGORITHM],
    ["X-Amz-Credential", `${credentials.accessKeyId}/${scopeOf(scope)}`],
    ["X-Amz-Date", amzDate(scope.date)],
    ["X-Amz-Expires", String(options.expiresIn)],
  ];
  if (credentials.sessionToken) query.push(["X-Amz-Security-Token", credentials.sessionToken]);
  query.push(["X-Amz-SignedHeaders", signed]);
  const result = sign(
    request,
    query,
    request.headers,
    options.payloadHash ?? UNSIGNED_PAYLOAD,
    credentials,
    scope,
    options.keys ?? sharedKeys,
  );
  query.push(["X-Amz-Signature", result.signature]);
  return { ...result, query };
}

/** A query string from decoded pairs, encoded exactly as {@link signQuery} signed it. */
export function encodeQuery(query: ReadonlyArray<readonly [string, string]>): string {
  return query.map(([k, v]) => `${uriEncode(k)}=${uriEncode(v)}`).join("&");
}
