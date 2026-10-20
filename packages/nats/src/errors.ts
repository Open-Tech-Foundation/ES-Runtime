/**
 * What can go wrong talking to a NATS server, in a vocabulary an application
 * can branch on.
 *
 * The core NATS protocol answers almost nothing: a publish is fire-and-forget,
 * a subscription carries no acknowledgement, and the server's only reply to a
 * bad request is `-ERR`, which usually ends the connection. So the interesting
 * distinctions here are narrower than a request/response protocol's: the socket
 * failed or was lost, the handshake was refused, the server rejected the
 * credentials, the server named an error, or the client was asked for something
 * it does not implement. A request that finds no responder is its own code,
 * because with no-responders support the server answers it explicitly rather
 * than leaving the caller to a timeout.
 */

/** Stable codes. The server's own `-ERR` text stays on the message. */
export const NatsErrorCode = {
  /** The bytes were not NATS — this client's or the server's framing disagreeing. */
  Protocol: "ERR_NATS_PROTOCOL",
  /** The socket could not be opened, or was lost, or a command could not be read. */
  Connection: "ERR_NATS_CONNECTION",
  /** The connection was closed, by this client or the server. */
  Closed: "ERR_NATS_CLOSED",
  /** Something did not answer within the time allowed. */
  Timeout: "ERR_NATS_TIMEOUT",
  /** The handshake failed, before or instead of the INFO/CONNECT exchange. */
  Handshake: "ERR_NATS_HANDSHAKE",
  /** The server refused the credentials (a `-ERR` authorization violation). */
  Auth: "ERR_NATS_AUTH",
  /** The server sent a `-ERR`, or refused a command. */
  Server: "ERR_NATS_SERVER",
  /** A request found no responder, and the server said so. */
  NoResponders: "ERR_NATS_NO_RESPONDERS",
  /** A subscription's pending limits were full, so messages were dropped. */
  SlowConsumer: "ERR_NATS_SLOW_CONSUMER",
  /** A message is larger than the server's advertised maximum payload. */
  Payload: "ERR_NATS_PAYLOAD",
  /** Something this client does not implement was asked for. */
  Unsupported: "ERR_NATS_UNSUPPORTED",
} as const;

export type NatsErrorCode = (typeof NatsErrorCode)[keyof typeof NatsErrorCode];

export interface NatsErrorOptions {
  code: NatsErrorCode;
  /** The server's `-ERR` text, when there was one. */
  serverError?: string;
  cause?: unknown;
}

export class NatsError extends Error {
  override name = "NatsError";
  readonly code: NatsErrorCode;
  readonly serverError: string | null;

  constructor(message: string, options: NatsErrorOptions) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.code = options.code;
    this.serverError = options.serverError ?? null;
  }
}

/**
 * The failure the wire codec raises when the bytes are not NATS.
 *
 * A `ProtocolError` is always this client's or the server's framing code
 * disagreeing, never a server's `-ERR`, and it is a `NatsError` like any other
 * so a caller can catch one thing. Its code is {@link NatsErrorCode.Protocol}.
 */
export class ProtocolError extends NatsError {
  constructor(message: string) {
    super(message, { code: NatsErrorCode.Protocol });
    this.name = "ProtocolError";
  }
}
