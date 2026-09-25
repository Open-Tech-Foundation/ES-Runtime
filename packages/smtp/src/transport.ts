/**
 * What an application holds: a transport to one server, with a pool of
 * sessions behind it.
 *
 * A TLS handshake and a login are most of what sending one message costs, so
 * sessions are reused — up to `maxMessages` each, since servers cap how much
 * one session may carry — and closed after `idleTimeout` unused. The idle timer
 * does not hold the process open: a program that has sent its mail and has
 * nothing else to do exits.
 */

import { unrefTimer } from "runtime:process";
import {
  type ConnectionOptions,
  type Security,
  type SendResult,
  SmtpConnection,
} from "./connection.js";
import { SmtpError, SmtpErrorCode } from "./errors.js";
import { needsSmtpUtf8, parseAddress } from "./mime/address.js";
import { type Built, buildMessage, type Message } from "./mime/message.js";
import { frame } from "./protocol/data.js";

export interface TransportOptions {
  host: string;
  /** Defaults by `security`: 465 for `"tls"`, 587 for `"starttls"`, 25 for `"none"`. */
  port?: number;
  /**
   * `"tls"` encrypts from the first byte (port 465, which RFC 8314 recommends);
   * `"starttls"` upgrades a plaintext connection and **requires** the server to
   * offer it; `"none"` sends in plaintext. Defaults to `"tls"` on port 465 and
   * `"starttls"` otherwise.
   */
  security?: Security;
  user?: string;
  password?: string;
  /** An OAuth 2.0 access token, for XOAUTH2 (Gmail, Microsoft 365). */
  accessToken?: string;
  /** Permits a login over a connection that is not encrypted: a local relay, a test server. */
  allowPlaintextAuth?: boolean;
  /** Extra trust anchors (PEM), for a private certificate authority. */
  ca?: string | Uint8Array;
  /** The name the server's certificate must carry, when it is not `host`. */
  servername?: string;
  /** What this client announces itself as in `EHLO`. Defaults to `localhost`. */
  name?: string;
  /** Milliseconds to wait for each reply. Defaults to 60 000. */
  timeout?: number;
  /** Milliseconds to wait for the reply to the end of a message. Defaults to 600 000 (RFC 5321 §4.5.3.2.6). */
  dataTimeout?: number;
  /** Sessions open at once. Defaults to 2. */
  maxConnections?: number;
  /** Messages per session before it is closed and replaced. Defaults to 100. */
  maxMessages?: number;
  /** Milliseconds an unused session is kept. Defaults to 30 000. */
  idleTimeout?: number;
}

export interface Sent extends SendResult {
  messageId: string;
}

interface Idle {
  connection: SmtpConnection;
  timer: ReturnType<typeof setTimeout>;
}

export class Transport {
  readonly #options: ConnectionOptions;
  readonly #maxConnections: number;
  readonly #maxMessages: number;
  readonly #idleTimeout: number;
  #idle: Idle[] = [];
  #open = 0;
  #waiting: Array<(connection: SmtpConnection | Error) => void> = [];
  #closed = false;

  constructor(options: TransportOptions | string) {
    const o = typeof options === "string" ? fromUrl(options) : options;
    const security = o.security ?? (o.port === 465 ? "tls" : "starttls");
    const port = o.port ?? (security === "tls" ? 465 : security === "starttls" ? 587 : 25);
    if (o.user === undefined && (o.password !== undefined || o.accessToken !== undefined)) {
      throw new SmtpError("a password or accessToken needs a user", { code: SmtpErrorCode.Auth });
    }
    this.#options = {
      host: o.host,
      port,
      security,
      name: o.name ?? "localhost",
      allowPlaintextAuth: o.allowPlaintextAuth ?? false,
      timeout: o.timeout ?? 60_000,
      dataTimeout: o.dataTimeout ?? 600_000,
      ...(o.ca === undefined ? {} : { ca: o.ca }),
      ...(o.servername === undefined ? {} : { servername: o.servername }),
      ...(o.user === undefined
        ? {}
        : {
            auth: {
              user: o.user,
              ...(o.password === undefined ? {} : { password: o.password }),
              ...(o.accessToken === undefined ? {} : { accessToken: o.accessToken }),
            },
          }),
    };
    this.#maxConnections = Math.max(1, o.maxConnections ?? 2);
    this.#maxMessages = Math.max(1, o.maxMessages ?? 100);
    this.#idleTimeout = o.idleTimeout ?? 30_000;
  }

  /** Builds `message` and sends it. */
  async send(message: Message): Promise<Sent> {
    const built = await buildMessage(message);
    return { ...(await this.#deliver(built)), messageId: built.messageId };
  }

  /**
   * Sends a message built elsewhere, as it is: `envelope` says who it is from
   * and to, since a raw message's headers are not read. Lines are still
   * normalised to CRLF, dot-stuffed and checked for length.
   */
  async sendRaw(
    envelope: { from: string; to: string[] },
    message: string | Uint8Array,
  ): Promise<SendResult> {
    if (envelope.to.length === 0) {
      throw new SmtpError("the envelope has no recipients", { code: SmtpErrorCode.InvalidMessage });
    }
    for (const address of [envelope.from, ...envelope.to]) parseAddress(address);
    const smtpUtf8 = [envelope.from, ...envelope.to].some(needsSmtpUtf8);
    return this.#deliver({ text: "", messageId: "", envelope, smtpUtf8 }, message);
  }

  /** Opens a session — connect, TLS, login — and returns it to the pool: proof the settings work. */
  async verify(): Promise<void> {
    const connection = await this.#acquire();
    this.#release(connection);
  }

  /** Closes every session. Sends waiting for one fail with `ERR_SMTP_CLOSED`. */
  async close(): Promise<void> {
    this.#closed = true;
    for (const waiter of this.#waiting.splice(0)) {
      waiter(new SmtpError("the transport was closed", { code: SmtpErrorCode.Closed }));
    }
    const idle = this.#idle.splice(0);
    for (const { timer } of idle) clearTimeout(timer);
    await Promise.all(idle.map(({ connection }) => connection.close()));
  }

  async #deliver(built: Built, raw?: string | Uint8Array): Promise<SendResult> {
    const framed = frame(raw ?? built.text);
    // A pooled session the server has since dropped fails on its first
    // command. That is not the message's fault, so it is tried once more on a
    // fresh session — and only then, and only for a lost connection.
    for (let attempt = 0; ; attempt++) {
      const connection = await this.#acquire();
      const reused = connection.sent > 0 || attempt > 0;
      try {
        const result = await connection.sendMail(built.envelope, framed, built.smtpUtf8);
        this.#release(connection);
        return result;
      } catch (e) {
        this.#release(connection);
        const lost = e instanceof SmtpError && e.code === SmtpErrorCode.Connection;
        if (lost && reused && attempt === 0) continue;
        throw e;
      }
    }
  }

  async #acquire(): Promise<SmtpConnection> {
    if (this.#closed)
      throw new SmtpError("the transport was closed", { code: SmtpErrorCode.Closed });
    const idle = this.#idle.pop();
    if (idle !== undefined) {
      clearTimeout(idle.timer);
      return idle.connection;
    }
    if (this.#open < this.#maxConnections) {
      this.#open++;
      try {
        return await SmtpConnection.open(this.#options);
      } catch (e) {
        this.#open--;
        this.#wake();
        throw e;
      }
    }
    const next = await new Promise<SmtpConnection | Error>((resolve) =>
      this.#waiting.push(resolve),
    );
    if (next instanceof Error) throw next;
    return next;
  }

  #release(connection: SmtpConnection): void {
    if (connection.broken || connection.sent >= this.#maxMessages || this.#closed) {
      this.#open--;
      void connection.close();
      this.#wake();
      return;
    }
    const waiter = this.#waiting.shift();
    if (waiter !== undefined) {
      waiter(connection);
      return;
    }
    const timer = setTimeout(() => {
      this.#idle = this.#idle.filter((entry) => entry.connection !== connection);
      this.#open--;
      void connection.close();
    }, this.#idleTimeout);
    unrefTimer(timer as unknown as number);
    this.#idle.push({ connection, timer });
  }

  /** A session slot freed up: the next waiter opens one of its own. */
  #wake(): void {
    const waiter = this.#waiting.shift();
    if (waiter === undefined) return;
    this.#acquire().then(waiter, waiter);
  }
}

/**
 * `smtp://user:pass@host:587`, `smtps://user:pass@host` (implicit TLS, port
 * 465). Query parameters: `security` (`tls`, `starttls`, `none`), `name`,
 * `allowPlaintextAuth`. Credentials are percent-decoded.
 */
export function fromUrl(url: string): TransportOptions {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    throw new SmtpError(`${JSON.stringify(url)} is not an smtp: or smtps: URL`, {
      code: SmtpErrorCode.Connection,
    });
  }
  if (parsed.protocol !== "smtp:" && parsed.protocol !== "smtps:") {
    throw new SmtpError(`${parsed.protocol} is not smtp: or smtps:`, {
      code: SmtpErrorCode.Connection,
    });
  }
  const query = parsed.searchParams;
  const security = (query.get("security") ?? (parsed.protocol === "smtps:" ? "tls" : undefined)) as
    | Security
    | undefined;
  if (security !== undefined && !["tls", "starttls", "none"].includes(security)) {
    throw new SmtpError(`security=${security} is not tls, starttls or none`, {
      code: SmtpErrorCode.Connection,
    });
  }
  return {
    host: parsed.hostname.replace(/^\[|\]$/g, ""),
    ...(parsed.port === "" ? {} : { port: Number(parsed.port) }),
    ...(security === undefined ? {} : { security }),
    ...(parsed.username === "" ? {} : { user: decodeURIComponent(parsed.username) }),
    ...(parsed.password === "" ? {} : { password: decodeURIComponent(parsed.password) }),
    ...(query.has("name") ? { name: query.get("name") as string } : {}),
    ...(query.get("allowPlaintextAuth") === "true" ? { allowPlaintextAuth: true } : {}),
  };
}

export function createTransport(options: TransportOptions | string): Transport {
  return new Transport(options);
}
