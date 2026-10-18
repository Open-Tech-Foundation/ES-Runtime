/**
 * What can go wrong talking to a broker, in a vocabulary an application can
 * branch on.
 *
 * AMQP splits its own failures in two, and the split is the thing a caller most
 * needs: a **channel** exception (a queue that does not exist, a precondition
 * that failed) kills the channel and leaves the connection standing, while a
 * **connection** exception kills everything. `soft` carries that, so a caller
 * does not have to read the reply code to know what is still usable.
 *
 * The broker's own numbers are kept verbatim on `replyCode` and `replyText`:
 * they are the part that says *which* refusal it was, and no vocabulary here
 * can be as precise as the broker's own text.
 */

/** Stable codes. The broker's own text is on {@link RabbitError.replyText}. */
export const RabbitErrorCode = {
  /** The bytes were not AMQP — this client's or the broker's framing disagreeing. */
  Protocol: "ERR_RABBITMQ_PROTOCOL",
  /** The socket could not be opened, or was lost, or a frame could not be read. */
  Connection: "ERR_RABBITMQ_CONNECTION",
  /** The connection was closed, by this client or the broker. */
  Closed: "ERR_RABBITMQ_CLOSED",
  /** Something did not answer within the time allowed. */
  Timeout: "ERR_RABBITMQ_TIMEOUT",
  /** The handshake failed: a protocol version or a tuning value we cannot agree to. */
  Handshake: "ERR_RABBITMQ_HANDSHAKE",
  /** The broker refused the credentials or the virtual host. */
  Auth: "ERR_RABBITMQ_AUTH",
  /** A channel was used in a way the protocol does not allow. */
  Channel: "ERR_RABBITMQ_CHANNEL",
  /** The broker refused a method; `soft` says whether only its channel died. */
  Server: "ERR_RABBITMQ_SERVER",
  /** The broker would not accept a published message, in publisher-confirm mode. */
  Nack: "ERR_RABBITMQ_NACK",
  /** Something this client does not implement was asked for. */
  Unsupported: "ERR_RABBITMQ_UNSUPPORTED",
} as const;

export type RabbitErrorCode = (typeof RabbitErrorCode)[keyof typeof RabbitErrorCode];

/**
 * The reply codes that are **channel** exceptions rather than connection ones
 * (AMQP 0-9-1 §1.5.2.3). Everything else the broker can say ends the
 * connection.
 */
const SOFT_REPLIES: ReadonlySet<number> = new Set([311, 313, 403, 404, 405, 406]);

export interface RabbitErrorOptions {
  code: RabbitErrorCode;
  /** The broker's reply code, for a method it refused. */
  replyCode?: number;
  /** The broker's reply text, verbatim. */
  replyText?: string;
  /**
   * Whether only a channel died. Defaults from `replyCode`: the four hundred
   * series above are channel exceptions, and everything else is a connection
   * exception.
   */
  soft?: boolean;
  /** The class and method of the method that provoked it, as the broker names them. */
  failing?: { classId: number; methodId: number };
  cause?: unknown;
}

export class RabbitError extends Error {
  override name = "RabbitError";
  readonly code: RabbitErrorCode;
  readonly replyCode: number | null;
  readonly replyText: string | null;
  readonly soft: boolean;
  readonly failing: { classId: number; methodId: number } | null;

  constructor(message: string, options: RabbitErrorOptions) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.code = options.code;
    this.replyCode = options.replyCode ?? null;
    this.replyText = options.replyText ?? null;
    this.soft =
      options.soft ?? (this.replyCode === null ? false : SOFT_REPLIES.has(this.replyCode));
    this.failing = options.failing ?? null;
  }
}

/**
 * The failure the wire codec raises when the bytes are not AMQP.
 *
 * A `ProtocolError` is always this client's or the broker's framing code
 * disagreeing, never a broker's refusal, and it is a `RabbitError` like any
 * other so a caller can catch one thing. Its code is
 * {@link RabbitErrorCode.Protocol}.
 */
export class ProtocolError extends RabbitError {
  constructor(message: string) {
    super(message, { code: RabbitErrorCode.Protocol });
    this.name = "ProtocolError";
  }
}
