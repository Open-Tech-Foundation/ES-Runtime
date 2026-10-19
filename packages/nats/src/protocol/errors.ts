/**
 * The failure the wire codec raises when the bytes are not NATS.
 *
 * Defined on the package's error type so a caller catches one thing: a
 * `ProtocolError` is this client's or the server's framing code disagreeing,
 * never a server's `-ERR`.
 */
export { ProtocolError } from "../errors.js";
