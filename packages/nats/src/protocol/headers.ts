/**
 * The `NATS/1.0` header block.
 *
 * Headers are not part of the core protocol. They arrived with `HMSG`/`HPUB`
 * and a `headers` flag in `INFO`, and they carry one thing the payload cannot:
 * a **status line**. A request published with no responder is answered by the
 * server itself with `NATS/1.0 503` and an empty body, which is how a caller
 * learns "nobody is listening" instead of waiting out a timeout.
 *
 * The block is a first line that must read `NATS/1.0` (optionally followed by a
 * status code and a description), then `Name: Value` lines, then a blank line.
 * Names are case-insensitive on the wire and may repeat; this codec keeps every
 * value and folds case only on lookup. A name or value carrying CR or LF would
 * let a caller inject a second header or end the block early, so both are
 * refused here rather than written.
 */

import { ProtocolError } from "./errors.js";

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

/** The status a server attaches to a header block, or `null` for none. */
export interface HeaderStatus {
  code: number;
  description: string | null;
}

/** What `encodeHeaders` accepts: a name to one value, or a name to many. */
export type HeaderInit = Record<string, string | readonly string[]>;

/** A decoded header block: every value kept, and the status if there was one. */
export interface DecodedHeaders {
  headers: Record<string, string[]>;
  status: HeaderStatus | null;
}

function checkLinePart(part: string, what: string): void {
  if (part.includes("\r") || part.includes("\n")) {
    throw new ProtocolError(`a header ${what} must not contain CR or LF: ${JSON.stringify(part)}`);
  }
}

/** Encodes a header block, with an optional status line, as the bytes `HPUB` carries. */
export function encodeHeaders(values: HeaderInit = {}, status?: HeaderStatus | number): Uint8Array {
  const lines: string[] = ["NATS/1.0"];
  if (status !== undefined) {
    const code = typeof status === "number" ? status : status.code;
    const description = typeof status === "number" ? null : status.description;
    if (!Number.isInteger(code) || code < 0) {
      throw new ProtocolError(`a header status code must be a non-negative integer`);
    }
    if (description !== null) checkLinePart(description, "status description");
    lines[0] = description === null ? `NATS/1.0 ${code}` : `NATS/1.0 ${code} ${description}`;
  }
  for (const [name, value] of Object.entries(values)) {
    checkLinePart(name, "name");
    if (name.length === 0) throw new ProtocolError("a header name must not be empty");
    for (const one of typeof value === "string" ? [value] : value) {
      checkLinePart(one, "value");
      lines.push(`${name}: ${one}`);
    }
  }
  return ENCODER.encode(`${lines.join("\r\n")}\r\n\r\n`);
}

/** Decodes the header bytes an `HMSG` carried back into names, values and a status. */
export function decodeHeaders(bytes: Uint8Array): DecodedHeaders {
  const text = DECODER.decode(bytes);
  const lines = text.split("\r\n");
  const first = lines[0] ?? "";
  if (!first.startsWith("NATS/1.0")) {
    throw new ProtocolError("a header block must begin with `NATS/1.0`");
  }
  let status: HeaderStatus | null = null;
  const rest = first.slice("NATS/1.0".length).trim();
  if (rest.length > 0) {
    const match = /^(\d+)\s*(.*)$/.exec(rest);
    if (match === null) {
      throw new ProtocolError(`a header status line is malformed: ${JSON.stringify(first)}`);
    }
    status = { code: Number(match[1]), description: match[2] === "" ? null : match[2]! };
  }
  const headers: Record<string, string[]> = {};
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i]!;
    if (line.length === 0) break;
    const colon = line.indexOf(":");
    if (colon < 0) continue;
    const name = line.slice(0, colon);
    const value = line.slice(colon + 1).replace(/^ /, "");
    (headers[name] ??= []).push(value);
  }
  return { headers, status };
}
