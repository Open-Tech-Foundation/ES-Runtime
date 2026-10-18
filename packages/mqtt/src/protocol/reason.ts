/**
 * MQTT 5.0 reason codes, and the rule that tells a success from a refusal.
 *
 * MQTT 5.0 replaced the older protocol's handful of return codes with one
 * byte-wide vocabulary shared by CONNACK, PUBACK, PUBREC, PUBREL, SUBACK,
 * UNSUBACK, DISCONNECT and AUTH. Anything below 0x80 is successful (and, on a
 * SUBACK, carries the granted QoS); anything at or above is a refusal, which is
 * the whole test {@link isErrorReason} makes. The names here are the
 * specification's own, kept so an error carries the broker's words rather than
 * a number a reader has to look up.
 *
 * MQTT 3.1.1 has no reason codes — a CONNACK there carries one of a smaller set
 * of return codes, named by {@link connack311Name} — which is why the fallback
 * has its own vocabulary rather than pretending the 5.0 one fits.
 */

/** Whether a reason code is a refusal (0x80 and above). */
export function isErrorReason(code: number): boolean {
  return code >= 0x80;
}

const NAMES: ReadonlyMap<number, string> = new Map([
  [0x00, "Success"],
  [0x01, "Granted QoS 1"],
  [0x02, "Granted QoS 2"],
  [0x04, "Disconnect with Will Message"],
  [0x10, "No matching subscribers"],
  [0x11, "No subscription existed"],
  [0x18, "Continue authentication"],
  [0x19, "Re-authenticate"],
  [0x80, "Unspecified error"],
  [0x81, "Malformed Packet"],
  [0x82, "Protocol Error"],
  [0x83, "Implementation specific error"],
  [0x84, "Unsupported Protocol Version"],
  [0x85, "Client Identifier not valid"],
  [0x86, "Bad User Name or Password"],
  [0x87, "Not authorized"],
  [0x88, "Server unavailable"],
  [0x89, "Server busy"],
  [0x8a, "Banned"],
  [0x8b, "Server shutting down"],
  [0x8c, "Bad authentication method"],
  [0x8d, "Keep Alive timeout"],
  [0x8e, "Session taken over"],
  [0x8f, "Topic Filter invalid"],
  [0x90, "Topic Name invalid"],
  [0x91, "Packet Identifier in use"],
  [0x92, "Packet Identifier not found"],
  [0x93, "Receive Maximum exceeded"],
  [0x94, "Topic Alias invalid"],
  [0x95, "Packet too large"],
  [0x96, "Message rate too high"],
  [0x97, "Quota exceeded"],
  [0x98, "Administrative action"],
  [0x99, "Payload format invalid"],
  [0x9a, "Retain not supported"],
  [0x9b, "QoS not supported"],
  [0x9c, "Use another server"],
  [0x9d, "Server moved"],
  [0x9e, "Shared Subscriptions not supported"],
  [0x9f, "Connection rate exceeded"],
  [0xa0, "Maximum connect time"],
  [0xa1, "Subscription Identifiers not supported"],
  [0xa2, "Wildcard Subscriptions not supported"],
]);

/** The specification's name for a 5.0 reason code, or `null` if it is unknown. */
export function reasonName(code: number): string | null {
  return NAMES.get(code) ?? null;
}

const CONNACK_311_NAMES = [
  "Connection Accepted",
  "Unacceptable Protocol Version",
  "Identifier Rejected",
  "Server Unavailable",
  "Bad User Name or Password",
  "Not authorized",
] as const;

/**
 * The name of an MQTT 3.1.1 CONNACK return code (0–5).
 *
 * A 3.1.1 broker answers a CONNECT with a small return code rather than a 5.0
 * reason code; its own names are the older ones and are all we have to report.
 */
export function connack311Name(code: number): string | null {
  return CONNACK_311_NAMES[code] ?? null;
}
