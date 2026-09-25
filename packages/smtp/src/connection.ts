/**
 * One SMTP session over `runtime:net` (RFC 5321).
 *
 * Opening one is the whole handshake — greeting, `EHLO`, `STARTTLS` and `EHLO`
 * again, login — so a connection that exists is one that can send. Sending is
 * one mail transaction: `MAIL FROM`, a `RCPT TO` per recipient, `DATA`, the
 * message. With `PIPELINING` (RFC 2920) those commands go out in one write and
 * their replies are read in order, which turns a round trip per recipient into
 * one for the lot.
 */

import { connect as netConnect } from "runtime:net";
import { type Reply, replyError, SmtpError, SmtpErrorCode } from "./errors.js";
import { base64, type Credentials, choose, plain, xoauth2 } from "./protocol/auth.js";
import type { Framed } from "./protocol/data.js";
import { ReplyReader } from "./protocol/reply.js";

type Socket = ReturnType<typeof netConnect>;

export type Security = "tls" | "starttls" | "none";

export interface ConnectionOptions {
  host: string;
  port: number;
  security: Security;
  /** Extra trust anchors (PEM), for a server whose certificate a private CA signed. */
  ca?: string | Uint8Array;
  /** The name the certificate is checked against, when it is not `host`. */
  servername?: string;
  /** What `EHLO` announces this client as. */
  name: string;
  auth?: Credentials;
  allowPlaintextAuth: boolean;
  /** For each reply, in milliseconds. */
  timeout: number;
  /** For the reply to the end of the message, which a server may spend a while scanning. */
  dataTimeout: number;
}

export interface Rejected {
  address: string;
  reply: Reply;
}

export interface SendResult {
  accepted: string[];
  rejected: Rejected[];
  /** The server's reply to the message: usually its queue id. */
  response: string;
}

export class SmtpConnection {
  #socket: Socket;
  #reader: ReplyReader;
  #writer: WritableStreamDefaultWriter<Uint8Array>;
  #options: ConnectionOptions;
  #encoder = new TextEncoder();

  /** `EHLO` extensions by name, each with its parameters: `SIZE` → `["35882577"]`. */
  extensions: Map<string, string[]> = new Map();
  /** Whether the session is encrypted. */
  encrypted = false;
  /** Set when the session can no longer be trusted to be in step: a timeout, a lost socket. */
  broken = false;
  /** Messages sent over this session. */
  sent = 0;

  private constructor(socket: Socket, options: ConnectionOptions) {
    this.#socket = socket;
    this.#options = options;
    this.#reader = new ReplyReader(socket.readable);
    this.#writer = socket.writable.getWriter();
  }

  static async open(options: ConnectionOptions): Promise<SmtpConnection> {
    const tlsOptions = {
      ...(options.ca === undefined ? {} : { ca: options.ca }),
      ...(options.servername === undefined ? {} : { sni: options.servername }),
    };
    const secureTransport =
      options.security === "tls" ? "on" : options.security === "starttls" ? "starttls" : "off";
    const socket = netConnect(
      { hostname: options.host, port: options.port },
      { secureTransport, ...tlsOptions },
    );
    try {
      await socket.opened;
    } catch (cause) {
      throw socketError(options, cause, options.security === "tls");
    }
    const connection = new SmtpConnection(socket, options);
    connection.encrypted = options.security === "tls";
    try {
      await connection.#handshake();
    } catch (e) {
      await connection.close();
      throw e;
    }
    return connection;
  }

  async #handshake(): Promise<void> {
    const greeting = await this.#read(this.#options.timeout);
    if (greeting.code !== 220) throw replyError(SmtpErrorCode.Connection, "greeting", greeting);
    await this.#ehlo();

    if (this.#options.security === "starttls") {
      if (!this.extensions.has("STARTTLS")) {
        throw new SmtpError(
          `${this.#options.host} does not offer STARTTLS, so the session would be sent in plaintext — connect on its TLS port (security: "tls"), or set security: "none" if that is intended`,
          { code: SmtpErrorCode.Unsupported },
        );
      }
      await this.#command("STARTTLS", [220], SmtpErrorCode.Tls);
      this.#reader.release();
      this.#writer.releaseLock();
      this.#socket = this.#socket.startTls();
      try {
        await this.#socket.opened;
      } catch (cause) {
        throw socketError(this.#options, cause, true);
      }
      this.#reader = new ReplyReader(this.#socket.readable);
      this.#writer = this.#socket.writable.getWriter();
      this.encrypted = true;
      // RFC 3207 §4.2: everything learned before the upgrade is discarded,
      // including the extension list, which an attacker could have edited.
      await this.#ehlo();
    }

    if (this.#options.auth !== undefined) await this.#login(this.#options.auth);
  }

  async #ehlo(): Promise<void> {
    const reply = await this.#send(`EHLO ${this.#options.name}`, this.#options.timeout);
    this.extensions.clear();
    if (reply.code === 250) {
      // The first line is the server's greeting; each further line one extension.
      for (const line of reply.lines.slice(1)) {
        const [name, ...params] = line.trim().split(/\s+/);
        if (name) this.extensions.set(name.toUpperCase(), params);
      }
      return;
    }
    // A server too old for ESMTP answers EHLO with 500/502; HELO it is, with
    // no extensions.
    if (reply.code >= 500) {
      await this.#command(`HELO ${this.#options.name}`, [250], SmtpErrorCode.Connection);
      return;
    }
    throw replyError(SmtpErrorCode.Connection, "EHLO", reply);
  }

  async #login(credentials: Credentials): Promise<void> {
    if (!this.encrypted && !this.#options.allowPlaintextAuth) {
      throw new SmtpError(
        "refusing to send a login over a connection that is not encrypted — use TLS, or set allowPlaintextAuth for a trusted local relay",
        { code: SmtpErrorCode.PlaintextAuth },
      );
    }
    const offered = (this.extensions.get("AUTH") ?? []).map((m) => m.toUpperCase());
    const mechanism = choose(offered, credentials);
    if (mechanism === null) {
      throw new SmtpError(
        offered.length === 0
          ? `${this.#options.host} offers no login`
          : `${this.#options.host} offers ${offered.join(", ")}, and none of them can use the credentials given (PLAIN or LOGIN need a password, XOAUTH2 an accessToken)`,
        { code: SmtpErrorCode.Auth },
      );
    }
    const password = credentials.password ?? "";
    switch (mechanism) {
      case "PLAIN":
        await this.#command(
          `AUTH PLAIN ${plain(credentials.user, password)}`,
          [235],
          SmtpErrorCode.Auth,
          "AUTH PLAIN",
        );
        return;
      case "LOGIN":
        await this.#command("AUTH LOGIN", [334], SmtpErrorCode.Auth);
        await this.#command(base64(credentials.user), [334], SmtpErrorCode.Auth, "AUTH LOGIN");
        await this.#command(base64(password), [235], SmtpErrorCode.Auth, "AUTH LOGIN");
        return;
      case "XOAUTH2": {
        const reply = await this.#send(
          `AUTH XOAUTH2 ${xoauth2(credentials.user, credentials.accessToken ?? "")}`,
          this.#options.timeout,
        );
        if (reply.code === 235) return;
        // A refused token is a 334 carrying the reason as base64 JSON; the
        // exchange ends with an empty line, answered by the final failure.
        const final = reply.code === 334 ? await this.#send("", this.#options.timeout) : reply;
        throw replyError(SmtpErrorCode.Auth, "AUTH XOAUTH2", final);
      }
    }
  }

  /** One mail transaction. The connection is ready for the next when it returns or throws, unless {@link broken}. */
  async sendMail(
    envelope: { from: string; to: string[] },
    framed: Framed,
    smtpUtf8: boolean,
  ): Promise<SendResult> {
    const limit = Number(this.extensions.get("SIZE")?.[0] ?? 0);
    if (limit > 0 && framed.size > limit) {
      throw new SmtpError(
        `the message is ${framed.size} octets and the server accepts at most ${limit}`,
        {
          code: SmtpErrorCode.TooLarge,
        },
      );
    }
    if (smtpUtf8 && !this.extensions.has("SMTPUTF8")) {
      throw new SmtpError(
        "an address has a non-ASCII local part, which needs the server's SMTPUTF8, and it does not offer it",
        {
          code: SmtpErrorCode.Unsupported,
        },
      );
    }
    const eightBitOk = this.extensions.has("8BITMIME") || this.extensions.has("SMTPUTF8");
    if (framed.eightBit && !eightBitOk) {
      throw new SmtpError(
        "the message has 8-bit content and the server offers neither 8BITMIME nor SMTPUTF8 — encode it as 7-bit",
        {
          code: SmtpErrorCode.Unsupported,
        },
      );
    }

    let params = "";
    if (limit > 0 || this.extensions.has("SIZE")) params += ` SIZE=${framed.size}`;
    if (framed.eightBit && this.extensions.has("8BITMIME")) params += " BODY=8BITMIME";
    if (smtpUtf8) params += " SMTPUTF8";
    const commands = [
      `MAIL FROM:<${envelope.from}>${params}`,
      ...envelope.to.map((address) => `RCPT TO:<${address}>`),
      "DATA",
    ];

    const replies: Reply[] = [];
    if (this.extensions.has("PIPELINING")) {
      await this.#write(`${commands.join("\r\n")}\r\n`);
      for (let i = 0; i < commands.length; i++)
        replies.push(await this.#read(this.#options.timeout));
    } else {
      // One at a time, and stop at a refused sender: the server would refuse
      // every RCPT after it, and DATA too.
      for (const [i, command] of commands.entries()) {
        const reply = await this.#send(command, this.#options.timeout);
        replies.push(reply);
        if (i === 0 && reply.code !== 250) break;
        if (command === "DATA") break;
        if (
          i === commands.length - 2 &&
          !replies.slice(1).some((r) => r.code === 250 || r.code === 251)
        ) {
          break; // every recipient refused: no DATA
        }
      }
    }

    const mail = replies[0] as Reply;
    if (mail.code !== 250) {
      await this.#abort(replies);
      throw replyError(SmtpErrorCode.Sender, "MAIL FROM", mail);
    }
    const accepted: string[] = [];
    const rejected: Rejected[] = [];
    for (const [i, address] of envelope.to.entries()) {
      const reply = replies[i + 1];
      if (reply === undefined) break;
      if (reply.code === 250 || reply.code === 251) accepted.push(address);
      else rejected.push({ address, reply });
    }
    if (accepted.length === 0) {
      await this.#abort(replies);
      const first = rejected[0];
      throw first === undefined
        ? new SmtpError("the server accepted no recipient", { code: SmtpErrorCode.Recipients })
        : Object.assign(replyError(SmtpErrorCode.Recipients, "RCPT TO", first.reply), { rejected });
    }
    const data = replies[commands.length - 1];
    if (data === undefined || data.code !== 354) {
      await this.#abort(replies);
      throw data === undefined
        ? new SmtpError("the server did not answer DATA", { code: SmtpErrorCode.Protocol })
        : replyError(SmtpErrorCode.Message, "DATA", data);
    }

    await this.#writeBytes(framed.bytes);
    const final = await this.#read(this.#options.dataTimeout);
    if (final.code !== 250) throw replyError(SmtpErrorCode.Message, "end of data", final);
    this.sent++;
    return { accepted, rejected, response: final.lines.join(" ") };
  }

  /**
   * Ends a transaction that failed part-way, so the session can take the next.
   *
   * A pipelined `DATA` may have been answered `354` even though the
   * transaction is failing — the server is then reading message text, and an
   * `RSET` would become part of it. An empty message ends that first.
   */
  async #abort(replies: Reply[]): Promise<void> {
    try {
      if (replies.at(-1)?.code === 354) await this.#send(".", this.#options.timeout);
      await this.#command("RSET", [250], SmtpErrorCode.Protocol);
    } catch {
      this.broken = true;
    }
  }

  /** Checks the session is still alive, as a pool does before reusing it. */
  async noop(): Promise<void> {
    await this.#command("NOOP", [250], SmtpErrorCode.Connection);
  }

  /** `QUIT`, politely and briefly, then the socket. Never throws. */
  async close(): Promise<void> {
    if (!this.broken) {
      try {
        await this.#send("QUIT", Math.min(this.#options.timeout, 2000));
      } catch {
        /* the socket is going either way */
      }
    }
    this.broken = true;
    try {
      this.#writer.releaseLock();
    } catch {
      /* already released */
    }
    await this.#socket.close().catch(() => {});
  }

  async #command(
    line: string,
    expect: number[],
    code: SmtpErrorCode,
    name?: string,
  ): Promise<Reply> {
    const reply = await this.#send(line, this.#options.timeout);
    // The command's own name, not the line: a login line carries a secret.
    if (!expect.includes(reply.code))
      throw replyError(code, name ?? line.split(" ")[0] ?? line, reply);
    return reply;
  }

  async #send(line: string, timeout: number): Promise<Reply> {
    await this.#write(`${line}\r\n`);
    return this.#read(timeout);
  }

  async #read(timeout: number): Promise<Reply> {
    try {
      return await this.#reader.read(timeout);
    } catch (e) {
      this.broken = true;
      throw e;
    }
  }

  async #write(text: string): Promise<void> {
    await this.#writeBytes(this.#encoder.encode(text));
  }

  async #writeBytes(bytes: Uint8Array): Promise<void> {
    try {
      await this.#writer.write(bytes);
    } catch (cause) {
      this.broken = true;
      throw new SmtpError("the connection to the server was lost", {
        code: SmtpErrorCode.Connection,
        cause,
      });
    }
  }
}

function socketError(options: ConnectionOptions, cause: unknown, tls: boolean): SmtpError {
  const reason = cause instanceof Error ? cause.message : String(cause);
  const isTls = tls && /tls|certificate|handshake/i.test(reason);
  return new SmtpError(
    isTls
      ? `TLS with ${options.host}:${options.port} failed: ${reason}. A server whose certificate a private authority signed needs that authority as ca`
      : `cannot connect to ${options.host}:${options.port}: ${reason}`,
    { code: isTls ? SmtpErrorCode.Tls : SmtpErrorCode.Connection, cause },
  );
}
