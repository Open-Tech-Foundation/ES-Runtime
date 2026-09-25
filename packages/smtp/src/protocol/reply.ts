/**
 * Reading SMTP replies off a byte stream (RFC 5321 §4.2).
 *
 * A reply is one or more lines, each a three-digit code and then `-` (more
 * lines follow) or a space (this is the last). The code is the same on every
 * line. A server that implements RFC 2034 puts an enhanced status code —
 * `5.1.1` — at the start of each line's text; it is lifted out once, because it
 * is the part of the reply a program can act on.
 */

import { type Reply, SmtpError, SmtpErrorCode } from "../errors.js";

/** A reply line longer than this is not a server speaking SMTP. */
const MAX_LINE = 64 * 1024;

const ENHANCED = /^([245]\.\d{1,3}\.\d{1,3})(?:\s+|$)/;

/** Parses one reply from its lines. Exported for the unit tests. */
export function parseReply(lines: string[]): Reply {
  let code = -1;
  const texts: string[] = [];
  let enhanced: string | null = null;
  for (const [index, line] of lines.entries()) {
    const match = /^(\d{3})([ -]|$)(.*)$/.exec(line);
    if (match === null) {
      throw new SmtpError(
        `the server sent a line that is not an SMTP reply: ${JSON.stringify(line.slice(0, 80))}`,
        {
          code: SmtpErrorCode.Protocol,
        },
      );
    }
    const lineCode = Number(match[1]);
    if (code === -1) code = lineCode;
    else if (lineCode !== code) {
      throw new SmtpError(`the server changed reply code mid-reply (${code}, then ${lineCode})`, {
        code: SmtpErrorCode.Protocol,
      });
    }
    let text = match[3] ?? "";
    const status = ENHANCED.exec(text);
    // Only a status whose class matches the reply's is one: `250 2.0.0 OK` is,
    // while `250 5.4 GHz` is a line that happens to start with digits.
    if (status !== null && status[1]?.[0] === String(code)[0]) {
      if (index === 0) enhanced = status[1] ?? null;
      text = text.slice(status[0].length);
    }
    texts.push(text);
  }
  return { code, enhanced, lines: texts };
}

/** Splits a stream into CRLF lines and groups them into replies. */
export class ReplyReader {
  #reader: ReadableStreamDefaultReader<Uint8Array>;
  #decoder = new TextDecoder();
  #buffer = "";
  #ended = false;

  constructor(readable: ReadableStream<Uint8Array>) {
    this.#reader = readable.getReader();
  }

  /**
   * The next complete reply, or a timeout.
   *
   * A timeout leaves the stream mid-conversation — the reply may still arrive,
   * and would then be read as the answer to the next command — so a connection
   * that timed out is not reused. The caller closes it.
   */
  async read(timeout: number): Promise<Reply> {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expired = new Promise<never>((_, reject) => {
      timer = setTimeout(
        () =>
          reject(
            new SmtpError(`no reply from the server within ${timeout} ms`, {
              code: SmtpErrorCode.Timeout,
            }),
          ),
        timeout,
      );
    });
    try {
      return await Promise.race([this.#reply(), expired]);
    } finally {
      clearTimeout(timer);
    }
  }

  async #reply(): Promise<Reply> {
    const lines: string[] = [];
    for (;;) {
      const line = await this.#line();
      lines.push(line);
      // `250-…` continues; `250 …` or a bare `250` ends.
      if (line.length < 4 || line[3] !== "-") return parseReply(lines);
    }
  }

  async #line(): Promise<string> {
    for (;;) {
      const end = this.#buffer.indexOf("\n");
      if (end !== -1) {
        const line = this.#buffer.slice(0, end).replace(/\r$/, "");
        this.#buffer = this.#buffer.slice(end + 1);
        return line;
      }
      if (this.#buffer.length > MAX_LINE) {
        throw new SmtpError("the server sent a reply line longer than 64 KiB", {
          code: SmtpErrorCode.Protocol,
        });
      }
      if (this.#ended) {
        throw new SmtpError("the server closed the connection", { code: SmtpErrorCode.Connection });
      }
      const { value, done } = await this.#reader.read();
      if (done) {
        this.#ended = true;
        this.#buffer += this.#decoder.decode();
      } else {
        this.#buffer += this.#decoder.decode(value, { stream: true });
      }
    }
  }

  /**
   * Hands the stream back — before `STARTTLS` upgrades the socket underneath
   * it. Anything already buffered at that point would be plaintext the server
   * sent after agreeing to encrypt, which RFC 3207 §6 says must be discarded
   * and which here is refused, since it can only be an injection.
   */
  release(): void {
    if (this.#buffer.length > 0) {
      throw new SmtpError("the server sent data after agreeing to STARTTLS", {
        code: SmtpErrorCode.Protocol,
      });
    }
    this.#reader.releaseLock();
  }
}
