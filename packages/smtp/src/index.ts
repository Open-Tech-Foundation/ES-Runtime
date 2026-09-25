/**
 * `@opentf/esrun-smtp` — an SMTP client for esrun, in JavaScript over
 * `runtime:net` (DECISIONS D136).
 *
 * ```js
 * import { createTransport } from "@opentf/esrun-smtp";
 *
 * const mail = createTransport({ host: "smtp.example.com", user: "app", password });
 * await mail.send({
 *   from: "App <app@example.com>",
 *   to: "ada@example.com",
 *   subject: "Welcome",
 *   text: "Hello, Ada.",
 * });
 * ```
 */

export type { ConnectionOptions, Rejected, Security, SendResult } from "./connection.js";
export { type Reply, SmtpError, SmtpErrorCode } from "./errors.js";
export type { Address, AddressInput } from "./mime/address.js";
export {
  type Attachment,
  type BlobLike,
  type Built,
  buildMessage,
  type Message,
} from "./mime/message.js";
export {
  createTransport,
  fromUrl,
  type Sent,
  Transport,
  type TransportOptions,
} from "./transport.js";
