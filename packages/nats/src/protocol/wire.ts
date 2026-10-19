/**
 * The NATS wire protocol: what the server says, and what the client says back.
 *
 * Core NATS is a text protocol with one binary shape. Control is line-based —
 * `INFO`, `PING`, `PONG`, `+OK`, `-ERR` — and a message is a `MSG` or `HMSG`
 * line naming its subject, its subscription id and a byte count, followed by
 * exactly that many payload bytes and a `\r\n`. A socket delivers those bytes in
 * whatever chunks it likes, so the parser here is incremental: it consumes whole
 * commands off the front of a buffer and hands back the unconsumed remainder,
 * which is the difference between "a command" and "half a command so far". It
 * never decodes a payload as text — only the line is ASCII, the body is opaque.
 *
 * `INFO` carries the server's JSON: its limits (`max_payload`), its identity,
 * and whether it speaks headers, which is what decides whether this client can
 * offer no-responders. `-ERR` is the server's only refusal, and it usually ends
 * the connection, so it is surfaced with the server's own words kept verbatim.
 */

import { ProtocolError } from "./errors.js";

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

const CR = 0x0d;
const LF = 0x0a;

/** The core protocol version this client speaks. */
export const NATS_PROTOCOL: number = 1;
/** The `lang` this client reports to the server. */
export const CLIENT_LANG: string = "esrun";
/** The server's payload ceiling when `INFO` does not name one. */
export const DEFAULT_MAX_PAYLOAD: number = 1024 * 1024;

/** What the server told us about itself in `INFO`. */
export interface ServerInfo {
  serverId: string | null;
  serverName: string | null;
  version: string | null;
  /** The largest message body the server accepts, in bytes. */
  maxPayload: number;
  clientId: number | null;
  authRequired: boolean;
  tlsRequired: boolean;
  tlsAvailable: boolean;
  /** Whether the server speaks `HMSG`/`HPUB` and no-responder statuses. */
  headers: boolean;
  /** Cluster peers to try, as `host:port`, in the order advertised. */
  connectUrls: string[];
  connectUrlsTls: string[];
  /** The nkey challenge, when the server wants one. Present but unimplemented. */
  nonce: string | null;
  /** The JSON as received, for anything this type does not name. */
  raw: Record<string, unknown>;
}

/** The fields of the `CONNECT` JSON this client sets. Undefined ones are omitted. */
export interface ConnectOptions {
  verbose: boolean;
  pedantic: boolean;
  tls_required: boolean;
  name?: string;
  lang: string;
  protocol: number;
  echo: boolean;
  headers: boolean;
  no_responders: boolean;
  user?: string;
  pass?: string;
  auth_token?: string;
}

/** One command off the wire. */
export type ServerCommand =
  | { kind: "info"; info: ServerInfo }
  | {
      kind: "msg";
      subject: string;
      sid: number;
      reply: string | null;
      payload: Uint8Array;
      headers: Uint8Array | null;
    }
  | { kind: "ping" }
  | { kind: "pong" }
  | { kind: "ok" }
  | { kind: "err"; message: string };

function concatAll(parts: Uint8Array[]): Uint8Array {
  let length = 0;
  for (const part of parts) length += part.length;
  const out = new Uint8Array(length);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

function line(text: string): Uint8Array {
  return ENCODER.encode(`${text}\r\n`);
}

/** Encodes the `CONNECT` command that opens a session. */
export function encodeConnect(options: ConnectOptions): Uint8Array {
  return line(`CONNECT ${JSON.stringify(options)}`);
}

/** Encodes a `PUB` with its payload. */
export function encodePub(subject: string, reply: string | null, payload: Uint8Array): Uint8Array {
  const head =
    reply === null
      ? `PUB ${subject} ${payload.length}`
      : `PUB ${subject} ${reply} ${payload.length}`;
  return concatAll([line(head), payload, CONCAT_CRLF]);
}

/** Encodes an `HPUB` with its header block and payload. */
export function encodeHpub(
  subject: string,
  reply: string | null,
  headers: Uint8Array,
  payload: Uint8Array,
): Uint8Array {
  const total = headers.length + payload.length;
  const head =
    reply === null
      ? `HPUB ${subject} ${headers.length} ${total}`
      : `HPUB ${subject} ${reply} ${headers.length} ${total}`;
  return concatAll([line(head), headers, payload, CONCAT_CRLF]);
}

/** Encodes a `SUB` for a subject (and optional queue group) under a subscription id. */
export function encodeSub(subject: string, queue: string | undefined, sid: number): Uint8Array {
  return queue === undefined
    ? line(`SUB ${subject} ${sid}`)
    : line(`SUB ${subject} ${queue} ${sid}`);
}

/** Encodes an `UNSUB`, optionally auto-removing the subscription after `max` messages. */
export function encodeUnsub(sid: number, max?: number): Uint8Array {
  return max === undefined ? line(`UNSUB ${sid}`) : line(`UNSUB ${sid} ${max}`);
}

/** The `PING` command. */
export function encodePing(): Uint8Array {
  return PING;
}

/** The `PONG` command. */
export function encodePong(): Uint8Array {
  return PONG;
}

const PING = line("PING");
const PONG = line("PONG");
const CONCAT_CRLF = new Uint8Array([CR, LF]);

function asString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function asBool(value: unknown): boolean {
  return value === true;
}

function asInt(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

/** Parses the JSON body of an `INFO` line into {@link ServerInfo}. */
export function parseInfo(json: string): ServerInfo {
  let value: unknown;
  try {
    value = JSON.parse(json);
  } catch {
    throw new ProtocolError(`the server's INFO was not JSON: ${JSON.stringify(json)}`);
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new ProtocolError("the server's INFO was not a JSON object");
  }
  const raw = value as Record<string, unknown>;
  const urls = Array.isArray(raw.connect_urls)
    ? raw.connect_urls.filter((url): url is string => typeof url === "string")
    : [];
  const tlsUrls = Array.isArray(raw.connect_urls_tls)
    ? raw.connect_urls_tls.filter((url): url is string => typeof url === "string")
    : [];
  return {
    serverId: asString(raw.server_id),
    serverName: asString(raw.server_name),
    version: asString(raw.version),
    maxPayload: asInt(raw.max_payload) ?? DEFAULT_MAX_PAYLOAD,
    clientId: asInt(raw.client_id),
    authRequired: asBool(raw.auth_required),
    tlsRequired: asBool(raw.tls_required),
    tlsAvailable: asBool(raw.tls_available),
    headers: asBool(raw.headers),
    connectUrls: urls,
    connectUrlsTls: tlsUrls,
    nonce: asString(raw.nonce),
    raw,
  };
}

function indexOfCrlf(bytes: Uint8Array, from: number): number {
  for (let i = from; i + 1 < bytes.length; i++) {
    if (bytes[i] === CR && bytes[i + 1] === LF) return i;
  }
  return -1;
}

function count(token: string, what: string): number {
  const value = Number(token);
  if (!Number.isInteger(value) || value < 0) {
    throw new ProtocolError(`a ${what} in a protocol line was not a non-negative integer`);
  }
  return value;
}

function errMessage(lineText: string): string {
  let rest = lineText.slice("-ERR".length).trim();
  if (rest.length >= 2 && rest.startsWith("'") && rest.endsWith("'")) {
    rest = rest.slice(1, -1);
  }
  return rest;
}

function controlCommand(lineText: string): ServerCommand {
  if (lineText === "PING") return { kind: "ping" };
  if (lineText === "PONG") return { kind: "pong" };
  if (lineText === "+OK") return { kind: "ok" };
  if (lineText.startsWith("INFO ")) return { kind: "info", info: parseInfo(lineText.slice(5)) };
  if (lineText === "INFO") return { kind: "info", info: parseInfo("{}") };
  if (lineText.startsWith("-ERR")) return { kind: "err", message: errMessage(lineText) };
  throw new ProtocolError(`the server sent a line this client does not know: ${lineText}`);
}

/**
 * Consumes whole commands off the front of `buffer`.
 *
 * Returns the commands it could read and the bytes after them — a partial
 * trailing command stays in `rest` for the next chunk, which is what makes this
 * safe against arbitrary segmentation. A message header whose payload has not
 * arrived is left whole, not half-decoded.
 */
export function drain(buffer: Uint8Array): { commands: ServerCommand[]; rest: Uint8Array } {
  const commands: ServerCommand[] = [];
  let offset = 0;
  for (;;) {
    const cr = indexOfCrlf(buffer, offset);
    if (cr < 0) break;
    const lineText = DECODER.decode(buffer.subarray(offset, cr));
    const body = cr + 2;
    if (lineText.startsWith("MSG ") || lineText.startsWith("HMSG ")) {
      const header = lineText.startsWith("HMSG ");
      const fields = lineText.split(" ");
      const expected = header ? [5, 6] : [4, 5];
      if (!expected.includes(fields.length)) {
        throw new ProtocolError(`a ${header ? "HMSG" : "MSG"} line had ${fields.length} fields`);
      }
      const subject = fields[1]!;
      const sid = count(fields[2]!, "subscription id");
      let reply: string | null = null;
      let headerBytes = 0;
      let totalBytes: number;
      if (header) {
        if (fields.length === 6) {
          reply = fields[3]!;
          headerBytes = count(fields[4]!, "header byte count");
          totalBytes = count(fields[5]!, "total byte count");
        } else {
          headerBytes = count(fields[3]!, "header byte count");
          totalBytes = count(fields[4]!, "total byte count");
        }
        if (headerBytes > totalBytes) {
          throw new ProtocolError("an HMSG line had more header bytes than total bytes");
        }
      } else {
        if (fields.length === 5) {
          reply = fields[3]!;
          totalBytes = count(fields[4]!, "byte count");
        } else {
          totalBytes = count(fields[3]!, "byte count");
        }
      }
      const end = body + totalBytes;
      if (buffer.length < end + 2) break;
      if (buffer[end] !== CR || buffer[end + 1] !== LF) {
        throw new ProtocolError("a message body was not terminated by CRLF");
      }
      commands.push({
        kind: "msg",
        subject,
        sid,
        reply,
        headers: header ? buffer.subarray(body, body + headerBytes) : null,
        payload: buffer.subarray(body + headerBytes, end),
      });
      offset = end + 2;
    } else {
      commands.push(controlCommand(lineText));
      offset = body;
    }
  }
  return { commands, rest: buffer.subarray(offset) };
}
