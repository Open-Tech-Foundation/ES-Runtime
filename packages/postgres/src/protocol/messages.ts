/**
 * Writing the wire.
 *
 * Every frontend message is `tag(1) length(4) body`, with the length counting
 * itself — which is what `ByteWriter`'s `beginLength`/`endLength` were put in
 * `runtime:db` for. The startup packet and `SSLRequest` are the two without a
 * tag, because they are sent before the server knows what protocol it speaks.
 */
import { ByteWriter } from "runtime:db";

const ENCODER = new TextEncoder();

/** Frontend message tags. */
export const F = {
  Bind: 0x42, // 'B'
  Close: 0x43, // 'C'
  Describe: 0x44, // 'D'
  Execute: 0x45, // 'E'
  Parse: 0x50, // 'P'
  Query: 0x51, // 'Q'
  Sync: 0x53, // 'S'
  Terminate: 0x58, // 'X'
  PasswordMessage: 0x70, // 'p' — also SASLInitialResponse and SASLResponse
} as const;

/** Backend message tags. */
export const B = {
  Authentication: 0x52, // 'R'
  BackendKeyData: 0x4b, // 'K'
  BindComplete: 0x32, // '2'
  CloseComplete: 0x33, // '3'
  CommandComplete: 0x43, // 'C'
  DataRow: 0x44, // 'D'
  EmptyQueryResponse: 0x49, // 'I'
  ErrorResponse: 0x45, // 'E'
  NoData: 0x6e, // 'n'
  NoticeResponse: 0x4e, // 'N'
  NotificationResponse: 0x41, // 'A'
  ParameterDescription: 0x74, // 't'
  ParameterStatus: 0x53, // 'S'
  ParseComplete: 0x31, // '1'
  PortalSuspended: 0x73, // 's'
  ReadyForQuery: 0x5a, // 'Z'
  RowDescription: 0x54, // 'T'
} as const;

/** Authentication sub-codes (the int32 after the `R` tag). */
export const AUTH = {
  Ok: 0,
  CleartextPassword: 3,
  MD5Password: 5,
  SASL: 10,
  SASLContinue: 11,
  SASLFinal: 12,
} as const;

function cstring(w: ByteWriter, text: string): void {
  w.bytes(ENCODER.encode(text)).u8(0);
}

function tagged(tag: number, body: (w: ByteWriter) => void): Uint8Array {
  const w = new ByteWriter(128);
  w.u8(tag);
  const at = w.beginLength();
  body(w);
  w.endLength(at);
  return w.finish();
}

/**
 * `SSLRequest` — a length and a magic number, no tag. The server answers with a
 * single byte rather than a message, because there is no agreed framing yet.
 */
export function sslRequest(): Uint8Array {
  const w = new ByteWriter(8);
  w.i32(8).i32(80877103);
  return w.finish();
}

/**
 * `CancelRequest` — a length, a magic number, and the backend's identity.
 *
 * Sent on a **new** connection, not the one running the query: the connection
 * running it is busy reading the answer, which is the thing being cancelled.
 * The server closes this connection without replying, so there is nothing to
 * wait for and no way to learn from here whether it worked.
 */
export function cancelRequest(processId: number, secretKey: number): Uint8Array {
  const w = new ByteWriter(16);
  w.i32(16).i32(80877102).i32(processId).i32(secretKey);
  return w.finish();
}

/** The startup packet: protocol 3.0 and the connection parameters. */
export function startup(params: Record<string, string>): Uint8Array {
  const w = new ByteWriter(256);
  const at = w.beginLength();
  w.i32(196608); // 3.0
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === "") continue;
    cstring(w, key);
    cstring(w, value);
  }
  w.u8(0);
  w.endLength(at);
  return w.finish();
}

export function password(text: string): Uint8Array {
  return tagged(F.PasswordMessage, (w) => cstring(w, text));
}

export function saslInitialResponse(mechanism: string, initial: string): Uint8Array {
  return tagged(F.PasswordMessage, (w) => {
    cstring(w, mechanism);
    const bytes = ENCODER.encode(initial);
    w.i32(bytes.length).bytes(bytes);
  });
}

export function saslResponse(final: string): Uint8Array {
  return tagged(F.PasswordMessage, (w) => w.bytes(ENCODER.encode(final)));
}

/**
 * The simple query protocol: one string, run as written.
 *
 * The extended protocol cannot carry more than one statement — a prepared
 * statement is one statement by definition — so this is the only way to run a
 * script. It takes no parameters, which is exactly why it can: nothing has to
 * be prepared.
 */
export function simpleQuery(sql: string): Uint8Array {
  return tagged(F.Query, (w) => cstring(w, sql));
}

export function parse(name: string, sql: string): Uint8Array {
  return tagged(F.Parse, (w) => {
    cstring(w, name);
    cstring(w, sql);
    // No parameter type hints: the server infers them from the statement, which
    // it does better than we would.
    w.i16(0);
  });
}

/**
 * `Bind`, with a result format per column.
 *
 * `formats` is `0` (text) or `1` (binary) for each column, and an empty list
 * means text throughout. The formats have to be chosen **here**, before the
 * server has said what the columns are — which is why the statement's shape is
 * learned once with `Describe` and kept.
 */
export function bind(
  portal: string,
  statement: string,
  params: (Uint8Array | null)[],
  formats: readonly number[] = [],
): Uint8Array {
  return tagged(F.Bind, (w) => {
    cstring(w, portal);
    cstring(w, statement);
    w.i16(0); // every parameter in text format
    w.i16(params.length);
    for (const value of params) {
      if (value === null) {
        w.i32(-1);
      } else {
        w.i32(value.length).bytes(value);
      }
    }
    w.i16(formats.length);
    for (const format of formats) w.i16(format);
  });
}

/**
 * `Describe` for a prepared statement, which answers with the parameter types
 * *and* the row shape — before anything is bound.
 *
 * That ordering is the whole reason this exists: `Bind` carries the result
 * formats, so the column types must be known before it is sent, and only a
 * statement-level describe can say them that early.
 */
export function describeStatement(name: string): Uint8Array {
  return tagged(F.Describe, (w) => {
    w.u8(0x53); // 'S'
    cstring(w, name);
  });
}

export function describePortal(name: string): Uint8Array {
  return tagged(F.Describe, (w) => {
    w.u8(0x50); // 'P'
    cstring(w, name);
  });
}

export function execute(portal: string, maxRows = 0): Uint8Array {
  return tagged(F.Execute, (w) => {
    cstring(w, portal);
    w.i32(maxRows);
  });
}

/** `Close` for a prepared statement — releases its name and its plan. */
export function closeStatement(name: string): Uint8Array {
  return tagged(F.Close, (w) => {
    w.u8(0x53); // 'S'
    cstring(w, name);
  });
}

export function sync(): Uint8Array {
  return tagged(F.Sync, () => {});
}

export function terminate(): Uint8Array {
  return tagged(F.Terminate, () => {});
}

/** Concatenates messages so a whole exchange leaves in one write. */
export function concat(parts: Uint8Array[]): Uint8Array {
  let total = 0;
  for (const part of parts) total += part.length;
  const out = new Uint8Array(total);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}
