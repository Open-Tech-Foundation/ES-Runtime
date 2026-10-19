/**
 * The client: publish, subscribe, and request/reply over one connection.
 *
 * Everything the connection answered on the protocol's behalf — a server `PING`,
 * the `PONG` ordering, a reconnect and its resubscriptions — has already happened
 * by the time a {@link NatsMessage} arrives here. What is left is what an
 * application asked for:
 *
 * - **Publish**, which core NATS makes fire-and-forget: the call resolves once
 *   the bytes are with the socket, and there is no acknowledgement to await. The
 *   one refusal is a message larger than the server's advertised maximum, which
 *   is caught here before it is written.
 * - **Subscribe**, which registers a subject filter and hands back an async
 *   iterable of {@link NatsMessage}. The server routes; a `MSG` names the
 *   subscription by the id this client allocated, so delivery is exact and needs
 *   no wildcard matching on this side.
 * - **Request/reply**, which is the request/reply convention and nothing more:
 *   subscribe to a fresh inbox, publish with it as the reply subject, and read
 *   the first message back. When the server speaks headers and no responder
 *   exists, it answers with a `503` status instead of silence, which becomes a
 *   clear {@link NatsErrorCode.NoResponders} rather than a timeout.
 *
 * A lost connection is recovered by the connection layer, subscriptions and all;
 * this layer only has to route by subscription id and to end its iterables when
 * the connection is finally over.
 */

import {
  Connection,
  type ConnectionHandlers,
  type ConnectionSettings,
  type IncomingMessage,
} from "./connection.js";
import { NatsError, NatsErrorCode } from "./errors.js";
import type { HeaderStatus } from "./protocol/headers.js";
import { createInbox } from "./protocol/subject.js";
import type { Transport } from "./transport.js";

/** How long a request waits for a reply when nothing else is asked, in ms. */
export const DEFAULT_REQUEST_TIMEOUT: number = 1_000;

/** A message delivered to a subscriber. */
export class NatsMessage {
  /** The subject it was published to. */
  readonly subject: string;
  /** The subscription it was routed to. */
  readonly sid: number;
  /** The reply subject the publisher named, if any. */
  readonly reply: string | null;
  /** The payload, as bytes. */
  readonly payload: Uint8Array;
  /** The header block's values, each name holding every value it carried. */
  readonly headers: Record<string, string[]>;
  /** A `NATS/1.0` status, when the server attached one. */
  readonly status: HeaderStatus | null;

  constructor(message: IncomingMessage) {
    this.subject = message.subject;
    this.sid = message.sid;
    this.reply = message.reply;
    this.payload = message.payload;
    this.headers = message.headers;
    this.status = message.status;
  }

  /** The payload decoded as text. */
  text(encoding: string = "utf-8"): string {
    return new TextDecoder(encoding).decode(this.payload);
  }

  /** The payload parsed as JSON. */
  json(): unknown {
    return JSON.parse(this.text());
  }

  /** The first value of a header, matched case-insensitively, or `null`. */
  header(name: string): string | null {
    const wanted = name.toLowerCase();
    for (const [key, values] of Object.entries(this.headers)) {
      if (key.toLowerCase() === wanted) return values[0] ?? null;
    }
    return null;
  }

  /** Whether this message carries a status at or above 300 — a refusal. */
  get isError(): boolean {
    return this.status !== null && this.status.code >= 300;
  }
}

/** What to publish with. */
export interface PublishOptions {
  /** A subject replies should go to — set by {@link NatsClient.request}. */
  reply?: string;
  /** A header block. Requires a server that advertised header support. */
  headers?: Record<string, string | readonly string[]>;
  /** A `NATS/1.0` status, for a responder answering with one. */
  status?: HeaderStatus | number;
}

/** What to subscribe with. */
export interface SubscribeOptions {
  /** Join a queue group: one member of the group gets each message. */
  queue?: string;
}

/** What a request is sent with. */
export interface RequestOptions {
  /** Milliseconds to wait for a reply. Defaults to the client's `timeout`, or 1000. */
  timeout?: number;
  /** A request header block. */
  headers?: Record<string, string | readonly string[]>;
  /** The inbox to use. A fresh one is generated when omitted. */
  reply?: string;
}

/** Any extra connection settings, plus a default request timeout. */
export interface NatsClientOptions extends ConnectionSettings {
  /** The default timeout for a request, in ms. Defaults to 1000. */
  timeout?: number;
}

/** A live subscription: an async iterable of messages, until unsubscribed. */
export class Subscription implements AsyncIterable<NatsMessage> {
  /** The subject it was made with. */
  readonly subject: string;
  /** The queue group it joined, if any. */
  readonly queue: string | null;
  #sid: number;
  #onRelease: () => void;
  #queue: NatsMessage[] = [];
  #waiters: Array<(result: IteratorResult<NatsMessage>) => void> = [];
  #closed = false;
  #error: NatsError | null = null;

  /** @internal */
  constructor(subject: string, queue: string | null, sid: number, onRelease: () => void) {
    this.subject = subject;
    this.queue = queue;
    this.#sid = sid;
    this.#onRelease = onRelease;
  }

  /** The server-side subscription id this client allocated. */
  get sid(): number {
    return this.#sid;
  }

  /** Whether the subscription is over. */
  get closed(): boolean {
    return this.#closed;
  }

  /** @internal */
  offer(message: NatsMessage): void {
    if (this.#closed) return;
    const waiter = this.#waiters.shift();
    if (waiter !== undefined) waiter({ value: message, done: false });
    else this.#queue.push(message);
  }

  /** @internal */
  finish(error: NatsError | null = null): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#error = error;
    for (const waiter of this.#waiters.splice(0)) waiter({ value: undefined, done: true });
  }

  /** The next message, or `done` once the subscription ends. */
  next(): Promise<IteratorResult<NatsMessage>> {
    const message = this.#queue.shift();
    if (message !== undefined) return Promise.resolve({ value: message, done: false });
    if (this.#closed) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  /** The next message, rejecting if none arrives within `timeout` ms. */
  nextMessage(timeout: number = DEFAULT_REQUEST_TIMEOUT): Promise<NatsMessage> {
    const message = this.#queue.shift();
    if (message !== undefined) return Promise.resolve(message);
    if (this.#closed) return Promise.reject(this.#closedError());
    return new Promise<NatsMessage>((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout>;
      const waiter = (result: IteratorResult<NatsMessage>) => {
        clearTimeout(timer);
        if (result.done) reject(this.#closedError());
        else resolve(result.value);
      };
      timer = setTimeout(() => {
        // Remove the waiter only if it is still queued. A delivery may have taken
        // it already, and leaving it behind would let the next message resolve
        // this abandoned waiter — which is to say, lose that message.
        if (this.#removeWaiter(waiter)) {
          reject(new NatsError("no message arrived in time", { code: NatsErrorCode.Timeout }));
        }
      }, timeout);
      this.#waiters.push(waiter);
    });
  }

  #removeWaiter(waiter: (result: IteratorResult<NatsMessage>) => void): boolean {
    const at = this.#waiters.indexOf(waiter);
    if (at < 0) return false;
    this.#waiters.splice(at, 1);
    return true;
  }

  #closedError(): NatsError {
    return (
      this.#error ?? new NatsError("the subscription is closed", { code: NatsErrorCode.Closed })
    );
  }

  /** Unsubscribes at the server and ends the iterable. */
  unsubscribe(): void {
    if (this.#closed) return;
    this.#onRelease();
    this.finish();
  }

  [Symbol.asyncIterator](): AsyncIterator<NatsMessage> {
    return this;
  }
}

export class NatsClient {
  #connection: Connection;
  #options: NatsClientOptions;
  #subscriptions = new Map<number, Subscription>();
  #closed = false;

  private constructor(connection: Connection, options: NatsClientOptions) {
    this.#connection = connection;
    this.#options = options;
  }

  /** Connects to `url` and completes the handshake. */
  static async connect(url: string, options: NatsClientOptions = {}): Promise<NatsClient> {
    const holder: { client: NatsClient | null } = { client: null };
    const connection = await Connection.open(url, options, NatsClient.#handlers(holder));
    const client = new NatsClient(connection, options);
    holder.client = client;
    return client;
  }

  /**
   * Connects over an already-established transport.
   *
   * The seam a test stands on: the client's routing and request/reply run the
   * same whether the bytes come from a socket or a scripted server in memory.
   */
  static async over(transport: Transport, options: NatsClientOptions = {}): Promise<NatsClient> {
    const holder: { client: NatsClient | null } = { client: null };
    const connection = await Connection.over(transport, options, NatsClient.#handlers(holder));
    const client = new NatsClient(connection, options);
    holder.client = client;
    return client;
  }

  static #handlers(holder: { client: NatsClient | null }): ConnectionHandlers {
    return {
      onMessage: (message) => {
        const client = holder.client;
        if (client !== null) client.#onMessage(message);
      },
      onDisconnect: () => {},
      onReconnect: () => {},
      onClosed: (error) => {
        const client = holder.client;
        if (client !== null) client.#onClosed(error);
      },
    };
  }

  /** The connection underneath, for a caller that wants to drive the protocol. */
  get connection(): Connection {
    return this.#connection;
  }

  /** Whether the connection is up (and not mid-reconnect). */
  get connected(): boolean {
    return this.#connection.connected;
  }

  /** Whether the client is closed. */
  get closed(): boolean {
    return this.#closed;
  }

  /** The failure that ended the connection, or `null` while it stands. */
  get failure(): NatsError | null {
    return this.#connection.failure;
  }

  /** The server's version string, when it advertised one. */
  get serverVersion(): string | null {
    return this.#connection.serverVersion;
  }

  /** The largest body the server accepts, in bytes. */
  get maxPayload(): number {
    return this.#connection.maxPayload;
  }

  /** Publishes a message, resolving once its bytes are written. */
  publish(
    subject: string,
    payload: string | Uint8Array,
    options: PublishOptions = {},
  ): Promise<void> {
    this.#assertOpen();
    return this.#connection.publish(subject, payload, options);
  }

  /** Registers a subject filter and returns the messages it matches. */
  subscribe(subject: string, options: SubscribeOptions = {}): Subscription {
    this.#assertOpen();
    const sid = this.#connection.subscribe(subject, options.queue);
    const subscription = new Subscription(subject, options.queue ?? null, sid, () => {
      this.#connection.unsubscribe(sid);
      this.#subscriptions.delete(sid);
    });
    this.#subscriptions.set(sid, subscription);
    return subscription;
  }

  /**
   * Publishes to `subject` and returns the first reply.
   *
   * A fresh inbox is subscribed, used as the reply subject, and dropped once a
   * reply — or a no-responder status — arrives. The default timeout is short,
   * because a request without a responder should fail rather than hold a caller
   * for as long as a subscription might live.
   */
  async request(
    subject: string,
    payload: string | Uint8Array,
    options: RequestOptions = {},
  ): Promise<NatsMessage> {
    this.#assertOpen();
    const inbox = options.reply ?? createInbox();
    const subscription = this.subscribe(inbox);
    try {
      await this.publish(subject, payload, {
        reply: inbox,
        ...(options.headers === undefined ? {} : { headers: options.headers }),
      });
      const message = await subscription.nextMessage(options.timeout ?? this.#requestTimeout());
      if (message.status !== null && (message.status.code === 503 || message.status.code === 404)) {
        throw new NatsError("no responders are available for the request", {
          code: NatsErrorCode.NoResponders,
        });
      }
      return message;
    } finally {
      subscription.unsubscribe();
    }
  }

  /** Sends a `PING` and resolves when its `PONG` arrives. */
  flush(timeout?: number): Promise<void> {
    this.#assertOpen();
    return this.#connection.flush(timeout);
  }

  /** Closes the connection, ending every subscription. */
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#finish(new NatsError("the client was closed", { code: NatsErrorCode.Closed }));
    await this.#connection.close();
  }

  // -- handlers -------------------------------------------------------------

  #onMessage(message: IncomingMessage): void {
    const subscription = this.#subscriptions.get(message.sid);
    if (subscription === undefined) return;
    subscription.offer(new NatsMessage(message));
  }

  #onClosed(error: NatsError | null): void {
    this.#finish(
      error ?? new NatsError("the connection was closed", { code: NatsErrorCode.Closed }),
    );
  }

  // -- helpers --------------------------------------------------------------

  #requestTimeout(): number {
    return this.#options.timeout ?? DEFAULT_REQUEST_TIMEOUT;
  }

  #assertOpen(): void {
    if (this.#closed) throw new NatsError("the client is closed", { code: NatsErrorCode.Closed });
    if (this.#connection.failure !== null) throw this.#connection.failure;
  }

  #finish(error: NatsError): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const subscription of this.#subscriptions.values()) subscription.finish(error);
    this.#subscriptions.clear();
  }
}
