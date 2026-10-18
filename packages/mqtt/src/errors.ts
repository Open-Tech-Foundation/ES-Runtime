/**
 * What can go wrong talking to a broker, in a vocabulary an application can
 * branch on.
 *
 * MQTT 5.0 answers almost every request with a reason code rather than closing
 * the connection, so the interesting distinction is *where* a refusal landed:
 * a CONNECT that was refused (the credentials or the client identifier — nothing
 * is usable afterwards), a SUBSCRIBE or UNSUBSCRIBE the broker refused for one
 * topic filter, a PUBLISH it refused for one message, or a DISCONNECT it sent
 * carrying an error. Each gets its own code, and the broker's own number is kept
 * verbatim on `reasonCode` with its name on `reasonName`, because no vocabulary
 * here is as precise as the specification's own words.
 */

/** Stable codes. The broker's own text stays on `reasonCode`/`reasonName`. */
export const MqttErrorCode = {
  /** The bytes were not MQTT — this client's or the broker's framing disagreeing. */
  Protocol: "ERR_MQTT_PROTOCOL",
  /** The socket could not be opened, or was lost, or a packet could not be read. */
  Connection: "ERR_MQTT_CONNECTION",
  /** The connection was closed, by this client or the broker. */
  Closed: "ERR_MQTT_CLOSED",
  /** Something did not answer within the time allowed. */
  Timeout: "ERR_MQTT_TIMEOUT",
  /** The handshake failed, before or instead of a CONNACK. */
  Handshake: "ERR_MQTT_HANDSHAKE",
  /** The broker refused the credentials or the client identifier. */
  Auth: "ERR_MQTT_AUTH",
  /** The broker refused the CONNECT itself (CONNACK reason code at or above 0x80). */
  Refused: "ERR_MQTT_REFUSED",
  /** The broker refused one topic filter, on SUBACK or UNSUBACK. */
  Subscription: "ERR_MQTT_SUBSCRIPTION",
  /** The broker refused one published message (PUBACK/PUBREC reason at or above 0x80). */
  Publish: "ERR_MQTT_PUBLISH",
  /** The broker sent a DISCONNECT carrying an error, or violated the protocol. */
  Server: "ERR_MQTT_SERVER",
  /** Something this client does not implement was asked for. */
  Unsupported: "ERR_MQTT_UNSUPPORTED",
} as const;

export type MqttErrorCode = (typeof MqttErrorCode)[keyof typeof MqttErrorCode];

export interface MqttErrorOptions {
  code: MqttErrorCode;
  /** The MQTT 5.0 reason code the broker sent, when there was one. */
  reasonCode?: number;
  /** Its name, as the specification spells it (`Not authorized`). */
  reasonName?: string;
  cause?: unknown;
}

export class MqttError extends Error {
  override name = "MqttError";
  readonly code: MqttErrorCode;
  readonly reasonCode: number | null;
  readonly reasonName: string | null;

  constructor(message: string, options: MqttErrorOptions) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.code = options.code;
    this.reasonCode = options.reasonCode ?? null;
    this.reasonName = options.reasonName ?? null;
  }
}

/**
 * The failure the wire codec raises when the bytes are not MQTT.
 *
 * A `ProtocolError` is always this client's or the broker's framing code
 * disagreeing, never a broker's refusal, and it is an `MqttError` like any
 * other so a caller can catch one thing. Its code is
 * {@link MqttErrorCode.Protocol}.
 */
export class ProtocolError extends MqttError {
  constructor(message: string) {
    super(message, { code: MqttErrorCode.Protocol });
    this.name = "ProtocolError";
  }
}
