/**
 * DKIM signing (RFC 6376), with RSA-SHA256 and Ed25519-SHA256 (RFC 8463).
 *
 * A signature covers the body and the headers that say who a message is from
 * and what it is: a receiver fetches the public key from DNS at
 * `<selector>._domainkey.<domain>` and checks both. Canonicalization is
 * `relaxed/relaxed`, the form that survives the whitespace changes relays make
 * (refolded headers, trailing spaces), and the one every major sender uses.
 *
 * What is signed, and why:
 *
 * * **The headers a reader trusts**, each listed once more than it occurs —
 *   "oversigned". A verifier reads a name listed beyond the instances present
 *   as an empty header, so adding a second `From:` or `Subject:` after signing
 *   breaks the signature instead of changing what the reader sees.
 * * **The whole body.** `l=` (a signed prefix) is never written: it lets anyone
 *   append content that still verifies.
 *
 * The message is signed as it will be sent: line breaks normalised to CRLF, as
 * the transport's framing does, and before dot-stuffing, which the receiving
 * server undoes.
 */

import { SmtpError, SmtpErrorCode } from "./errors.js";
import { bytesToBase64 } from "./mime/encode.js";

export interface DkimOptions {
  /** The signing domain (`d=`), whose DNS publishes the public key. */
  domain: string;
  /** The selector (`s=`): the key is looked up at `<selector>._domainkey.<domain>`. */
  selector: string;
  /**
   * The private key: a PEM (`PRIVATE KEY`, or `RSA PRIVATE KEY`), or a
   * `CryptoKey` for `RSASSA-PKCS1-v1_5` with SHA-256 or for `Ed25519`. An RSA
   * key must be at least 1024 bits (RFC 8301); 2048 is what to generate.
   */
  privateKey: string | CryptoKey;
}

/**
 * Signed when present, each once more than it occurs. `List-Unsubscribe` and
 * `List-Unsubscribe-Post` are here because one-click unsubscribe (RFC 8058)
 * requires both to be covered by the signature.
 */
export const SIGNED_HEADERS = [
  "from",
  "sender",
  "reply-to",
  "to",
  "cc",
  "subject",
  "date",
  "message-id",
  "in-reply-to",
  "references",
  "mime-version",
  "content-type",
  "content-transfer-encoding",
  "list-unsubscribe",
  "list-unsubscribe-post",
] as const;

export interface DkimKey {
  algorithm: "rsa-sha256" | "ed25519-sha256";
  key: CryptoKey;
}

export interface Signer {
  domain: string;
  selector: string;
  key: DkimKey;
}

/** Checks the options and imports the key, once per transport. */
export async function prepareSigner(options: DkimOptions): Promise<Signer> {
  // Both end up inside a tag list and a DNS name: `;`, `=` or whitespace in
  // either would change what the header says.
  const label = /^[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?$/;
  const name = (value: unknown) =>
    typeof value === "string" && value.split(".").every((part) => label.test(part));
  if (!name(options.domain)) {
    dkimError(
      `dkim.domain ${JSON.stringify(options.domain)} is not a domain name — an internationalised one goes as its xn-- form`,
    );
  }
  if (!name(options.selector)) {
    dkimError(`dkim.selector ${JSON.stringify(options.selector)} is not a DNS label`);
  }
  return {
    domain: options.domain.toLowerCase(),
    selector: options.selector,
    key: await importKey(options.privateKey),
  };
}

/**
 * Returns `message` with a `DKIM-Signature` header prepended for each signer,
 * line breaks normalised to CRLF. Each signature is computed over the message
 * without the others, so each verifies on its own.
 */
export async function sign(
  message: string | Uint8Array,
  signers: Signer[],
  now: Date = new Date(),
): Promise<Uint8Array> {
  const text = crlf(toBinary(message));
  const { headers, body } = split(text);
  const bodyHash = bytesToBase64(await sha256(fromBinary(canonicalBody(body))));
  const signatures: string[] = [];
  for (const signer of signers) {
    signatures.push(await signature(signer, headers, bodyHash, now));
  }
  return fromBinary(signatures.map((s) => `${s}\r\n`).join("") + text);
}

async function signature(
  signer: Signer,
  fields: Field[],
  bodyHash: string,
  now: Date,
): Promise<string> {
  const names: string[] = [];
  for (const header of SIGNED_HEADERS) {
    const count = fields.filter((f) => f.name === header).length;
    if (count === 0) continue;
    for (let i = 0; i <= count; i++) names.push(header);
  }
  if (!names.includes("from")) dkimError("the message has no From header, which DKIM must sign");

  const tags = [
    "v=1",
    `a=${signer.key.algorithm}`,
    "c=relaxed/relaxed",
    `d=${signer.domain}`,
    `s=${signer.selector}`,
    `t=${Math.floor(now.getTime() / 1000)}`,
    `h=${names.join(":")}`,
    `bh=${bodyHash}`,
    "b=",
  ];
  const unsigned = foldTags(tags);
  const data = `${canonicalHeaders(selectHeaders(fields, names))}${canonicalHeader(unsigned)}`;
  const b = await signData(signer.key, fromBinary(data));
  return unsigned + foldValue(b, lastLineLength(unsigned));
}

// --- keys ----------------------------------------------------------------------

export async function importKey(privateKey: string | CryptoKey): Promise<DkimKey> {
  if (typeof privateKey !== "string") return checkCryptoKey(privateKey);
  const pem = /-----BEGIN ([A-Z0-9 ]+)-----([\s\S]*?)-----END \1-----/.exec(privateKey);
  if (pem === null) dkimError("dkim.privateKey is not a PEM private key");
  const [, label, content] = pem as unknown as [string, string, string];
  let der: Bytes;
  try {
    der = Uint8Array.from(atob(content.replace(/\s+/g, "")), (c) => c.charCodeAt(0));
  } catch {
    dkimError("dkim.privateKey's PEM body is not base64");
  }
  if (label === "RSA PRIVATE KEY") return importPkcs8(pkcs1ToPkcs8(der), "rsa-sha256");
  if (label === "PRIVATE KEY") return importPkcs8(der, algorithmOf(der));
  if (label === "ENCRYPTED PRIVATE KEY") {
    dkimError(
      "dkim.privateKey is encrypted — decrypt it (openssl pkey -in key.pem -out plain.pem)",
    );
  }
  dkimError(`a ${label} cannot sign DKIM — generate an RSA (2048-bit) or Ed25519 key`);
}

async function importPkcs8(der: Bytes, algorithm: DkimKey["algorithm"]): Promise<DkimKey> {
  const params =
    algorithm === "rsa-sha256"
      ? { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" }
      : { name: "Ed25519" };
  let key: CryptoKey;
  try {
    key = await crypto.subtle.importKey("pkcs8", der, params, false, ["sign"]);
  } catch (e) {
    throw new SmtpError(`dkim.privateKey could not be imported: ${(e as Error).message}`, {
      code: SmtpErrorCode.Dkim,
      cause: e,
    });
  }
  return checkCryptoKey(key);
}

function checkCryptoKey(key: CryptoKey): DkimKey {
  const algorithm = key.algorithm as {
    name: string;
    hash?: { name: string };
    modulusLength?: number;
  };
  if (key.type !== "private" || !key.usages.includes("sign")) {
    dkimError("dkim.privateKey must be a private key usable for sign");
  }
  if (algorithm.name === "Ed25519") return { algorithm: "ed25519-sha256", key };
  if (algorithm.name === "RSASSA-PKCS1-v1_5") {
    if (algorithm.hash?.name !== "SHA-256")
      dkimError("an RSA DKIM key must be imported for SHA-256");
    if ((algorithm.modulusLength ?? 0) < 1024) {
      dkimError(
        `a ${algorithm.modulusLength}-bit RSA key is too short — receivers ignore DKIM signatures under 1024 bits (RFC 8301); use 2048`,
      );
    }
    return { algorithm: "rsa-sha256", key };
  }
  dkimError(`a ${algorithm.name} key cannot sign DKIM — use RSASSA-PKCS1-v1_5 or Ed25519`);
}

/** The key type a PKCS#8 structure names, by its algorithm OID. */
function algorithmOf(der: Uint8Array): DkimKey["algorithm"] {
  const has = (oid: number[]) => {
    for (let i = 0; i + oid.length <= Math.min(der.length, 32); i++) {
      if (oid.every((byte, j) => der[i + j] === byte)) return true;
    }
    return false;
  };
  if (has([0x06, 0x03, 0x2b, 0x65, 0x70])) return "ed25519-sha256";
  if (has(RSA_OID)) return "rsa-sha256";
  dkimError("dkim.privateKey is neither an RSA nor an Ed25519 key");
}

/** rsaEncryption, 1.2.840.113549.1.1.1. */
const RSA_OID = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];

/**
 * `BEGIN RSA PRIVATE KEY` is PKCS#1, which WebCrypto does not import. It is the
 * same key inside PKCS#8's envelope: version 0, the rsaEncryption algorithm,
 * and the PKCS#1 structure as an octet string.
 */
export function pkcs1ToPkcs8(pkcs1: Uint8Array): Bytes {
  const algorithm = der(0x30, new Uint8Array([...RSA_OID, 0x05, 0x00]));
  return der(0x30, new Uint8Array([...[0x02, 0x01, 0x00], ...algorithm, ...der(0x04, pkcs1)]));
}

function der(tag: number, content: Uint8Array): Bytes {
  const n = content.length;
  const length =
    n < 0x80
      ? [n]
      : n < 0x100
        ? [0x81, n]
        : n < 0x10000
          ? [0x82, n >> 8, n & 0xff]
          : [0x83, n >> 16, (n >> 8) & 0xff, n & 0xff];
  return new Uint8Array([tag, ...length, ...content]);
}

export async function signData(key: DkimKey, data: Bytes): Promise<string> {
  // RFC 8463 §3: Ed25519 signs the SHA-256 hash of the data, not the data.
  const signed =
    key.algorithm === "rsa-sha256"
      ? await crypto.subtle.sign("RSASSA-PKCS1-v1_5", key.key, data)
      : await crypto.subtle.sign("Ed25519", key.key, await sha256(data));
  return bytesToBase64(new Uint8Array(signed));
}

async function sha256(data: Bytes): Promise<Bytes> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", data));
}

// --- canonicalization (RFC 6376 §3.4) -----------------------------------------------
//
// The message is handled as a binary string, one character per octet, so that a
// raw message's 8-bit octets are hashed exactly as they are sent.

export interface Field {
  /** Lowercased. */
  name: string;
  /** The whole field as written, folds included, without the final CRLF. */
  raw: string;
}

/** The header fields, in order, and the body after the empty line. */
export function split(text: string): { headers: Field[]; body: string } {
  const end = text.startsWith("\r\n") ? 0 : text.indexOf("\r\n\r\n");
  const head = end === -1 ? text : text.slice(0, end);
  const body = end === -1 ? "" : text.slice(end + (end === 0 ? 2 : 4));
  const headers: Field[] = [];
  for (const line of head === "" ? [] : head.split("\r\n")) {
    const last = headers[headers.length - 1];
    if ((line.startsWith(" ") || line.startsWith("\t")) && last !== undefined) {
      last.raw += `\r\n${line}`;
    } else {
      headers.push({ name: line.slice(0, line.indexOf(":")).trim().toLowerCase(), raw: line });
    }
  }
  return { headers, body };
}

/**
 * The field for each name in `h=`: a name listed again takes the next instance
 * up from the bottom (§5.4.2), and one listed past the last instance takes
 * none.
 */
export function selectHeaders(fields: Field[], names: string[]): Field[] {
  const used = new Map<string, number>();
  const selected: Field[] = [];
  for (const name of names) {
    const instances = fields.filter((f) => f.name === name);
    const n = used.get(name) ?? 0;
    used.set(name, n + 1);
    const field = instances[instances.length - 1 - n];
    if (field !== undefined) selected.push(field);
  }
  return selected;
}

function canonicalHeaders(fields: Field[]): string {
  return fields.map((f) => `${canonicalHeader(f.raw)}\r\n`).join("");
}

/** Relaxed: lowercase name, unfolded, whitespace runs as one space, none at the ends of the value. */
export function canonicalHeader(raw: string): string {
  const colon = raw.indexOf(":");
  const name = raw.slice(0, colon).trim().toLowerCase();
  const value = raw
    .slice(colon + 1)
    .replace(/\r\n/g, "")
    .replace(/[ \t]+/g, " ")
    .trim();
  return `${name}:${value}`;
}

/** Relaxed: whitespace runs as one space, none at line ends, no empty lines at the end. */
export function canonicalBody(body: string): string {
  const lines = body.split("\r\n").map((line) => line.replace(/[ \t]+/g, " ").replace(/ $/, ""));
  while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines.length === 0 ? "" : `${lines.join("\r\n")}\r\n`;
}

// --- the header as written ------------------------------------------------------------

/** `DKIM-Signature:` and the tags, folded after a `;` to keep lines near 78. */
function foldTags(tags: string[]): string {
  let out = "DKIM-Signature:";
  let line = out.length;
  tags.forEach((tag, i) => {
    const piece = i === tags.length - 1 ? tag : `${tag};`;
    if (line + 1 + piece.length > 78 && line > "DKIM-Signature:".length) {
      out += `\r\n ${piece}`;
      line = 1 + piece.length;
    } else {
      out += ` ${piece}`;
      line += 1 + piece.length;
    }
  });
  return out;
}

/** The `b=` value, broken into folded lines. A verifier ignores whitespace in it. */
function foldValue(value: string, used: number): string {
  let out = "";
  let rest = value;
  let room = Math.max(8, 78 - used);
  while (rest.length > room) {
    out += `${rest.slice(0, room)}\r\n `;
    rest = rest.slice(room);
    room = 77;
  }
  return out + rest;
}

function lastLineLength(text: string): number {
  return text.length - (text.lastIndexOf("\r\n") + 2);
}

// --- octets ---------------------------------------------------------------------------

/** Bytes WebCrypto accepts: backed by an `ArrayBuffer`, never a shared one. */
type Bytes = Uint8Array<ArrayBuffer>;

function toBinary(message: string | Uint8Array): string {
  const bytes = typeof message === "string" ? new TextEncoder().encode(message) : message;
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return binary;
}

function fromBinary(text: string): Bytes {
  return Uint8Array.from(text, (c) => c.charCodeAt(0));
}

/** As the transport frames it: a bare CR or LF is a CRLF. */
function crlf(text: string): string {
  return text.replace(/\r\n|\r|\n/g, "\r\n");
}

function dkimError(message: string): never {
  throw new SmtpError(message, { code: SmtpErrorCode.Dkim });
}
