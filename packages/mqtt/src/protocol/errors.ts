/**
 * The failure the wire codec raises when the bytes are not MQTT.
 *
 * Defined on the package's error type so a caller catches one thing: a
 * `ProtocolError` is this client's or the broker's framing code disagreeing,
 * never a broker's refusal.
 */
export { ProtocolError } from "../errors.js";
