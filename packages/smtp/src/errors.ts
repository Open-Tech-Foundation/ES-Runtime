/**
 * What can go wrong sending mail, in a vocabulary an application can branch on.
 *
 * SMTP already says the one thing a sender most needs to know: a `4xx` reply is
 * transient (try again later — greylisting, a rate limit, a full queue) and a
 * `5xx` is permanent (the address does not exist, the content was refused).
 * `permanent` carries that, so a retry loop does not need to parse reply codes,
 * and the server's reply is kept verbatim for everything else.
 */

/** Stable codes. The server's own text is on {@link SmtpError.reply}. */
export const SmtpErrorCode = {
  /** The connection could not be made, or was lost. */
  Connection: "ERR_SMTP_CONNECTION",
  /** TLS could not be negotiated, or the certificate did not verify. */
  Tls: "ERR_SMTP_TLS",
  /** The server refused the credentials, or offers no login we support. */
  Auth: "ERR_SMTP_AUTH",
  /**
   * A login was about to be sent over a connection that is not encrypted, and
   * `allowPlaintextAuth` was not set. Nothing was sent.
   */
  PlaintextAuth: "ERR_SMTP_PLAINTEXT_AUTH",
  /** The server refused the sender (`MAIL FROM`). */
  Sender: "ERR_SMTP_SENDER",
  /** The server refused every recipient (`RCPT TO`). */
  Recipients: "ERR_SMTP_RECIPIENTS",
  /** The server refused the message itself, after `DATA`. */
  Message: "ERR_SMTP_MESSAGE",
  /** The message is larger than the server's advertised `SIZE`. */
  TooLarge: "ERR_SMTP_TOO_LARGE",
  /** The message needs something the server does not offer (`STARTTLS`, `SMTPUTF8`, `8BITMIME`). */
  Unsupported: "ERR_SMTP_UNSUPPORTED",
  /** The message could not be built: a header with a line break, no recipient. */
  InvalidMessage: "ERR_SMTP_INVALID_MESSAGE",
  /** No reply came within the timeout. */
  Timeout: "ERR_SMTP_TIMEOUT",
  /** The server's reply was not SMTP. */
  Protocol: "ERR_SMTP_PROTOCOL",
  /** The transport was closed. */
  Closed: "ERR_SMTP_CLOSED",
} as const;

export type SmtpErrorCode = (typeof SmtpErrorCode)[keyof typeof SmtpErrorCode];

/** One SMTP reply: `250-first line` … `250 last line`. */
export interface Reply {
  /** The three-digit code: `250`, `354`, `550`. */
  code: number;
  /** The RFC 3463 enhanced status code, when the server sent one: `"5.1.1"`. */
  enhanced: string | null;
  /** Every line's text, without the code and separator. */
  lines: string[];
}

export interface SmtpErrorOptions {
  code: SmtpErrorCode;
  /** Defaults from the reply (5xx permanent, 4xx not), else from the code. */
  permanent?: boolean;
  reply?: Reply;
  /** The command the reply answered: `"RCPT TO"`, `"DATA"`. */
  command?: string;
  cause?: unknown;
}

/** Codes that no retry will change. The rest are worth trying again. */
const PERMANENT: ReadonlySet<SmtpErrorCode> = new Set<SmtpErrorCode>([
  SmtpErrorCode.Auth,
  SmtpErrorCode.PlaintextAuth,
  SmtpErrorCode.TooLarge,
  SmtpErrorCode.Unsupported,
  SmtpErrorCode.InvalidMessage,
  SmtpErrorCode.Tls,
]);

export class SmtpError extends Error {
  override name = "SmtpError";
  readonly code: SmtpErrorCode;
  /** `true` when trying again will not help. */
  readonly permanent: boolean;
  /** The server's reply, when there was one. */
  readonly reply: Reply | null;
  /** The command that reply answered. */
  readonly command: string | null;

  constructor(message: string, options: SmtpErrorOptions) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.code = options.code;
    this.reply = options.reply ?? null;
    this.command = options.command ?? null;
    this.permanent =
      options.permanent ??
      (this.reply !== null ? this.reply.code >= 500 : PERMANENT.has(options.code));
  }
}

/** The error a refusing reply becomes. */
export function replyError(code: SmtpErrorCode, command: string, reply: Reply): SmtpError {
  const text = reply.lines.join(" ");
  const enhanced = reply.enhanced === null ? "" : ` ${reply.enhanced}`;
  return new SmtpError(
    `${command}: the server replied ${reply.code}${enhanced} ${text}`.trimEnd(),
    {
      code,
      reply,
      command,
    },
  );
}
