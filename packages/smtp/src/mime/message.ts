/**
 * Building an RFC 5322 message from the fields a caller writes.
 *
 * The MIME tree is the smallest one the content needs — a text-only message is
 * a single part — and nests in the one order mail clients agree on:
 *
 * ```text
 * multipart/mixed            when there are attachments
 * └ multipart/related        when the HTML shows inline images (cid:)
 *   └ multipart/alternative  when there is both text and HTML
 *     ├ text/plain
 *     └ text/html
 * ```
 */

import { SmtpError, SmtpErrorCode } from "../errors.js";
import {
  type Address,
  type AddressInput,
  formatAddress,
  needsSmtpUtf8,
  parseAddress,
  parseAddresses,
} from "./address.js";
import {
  base64Lines,
  encodeWord,
  foldHeader,
  isSevenBit,
  needsEncoding,
  quotedPrintable,
} from "./encode.js";

export interface Attachment {
  /** The name the recipient sees. */
  filename?: string;
  content: string | Uint8Array | ArrayBuffer | Blob;
  /** Defaults to the Blob's type, else `application/octet-stream`. */
  contentType?: string;
  /**
   * Shows the attachment inline, for HTML that refers to it as `cid:<this>`.
   * An inline attachment is part of the message body rather than a download.
   */
  cid?: string;
}

export interface Message {
  from: AddressInput;
  to?: AddressInput | AddressInput[];
  cc?: AddressInput | AddressInput[];
  /** On the envelope only; never in a header. */
  bcc?: AddressInput | AddressInput[];
  replyTo?: AddressInput | AddressInput[];
  subject?: string;
  text?: string;
  html?: string;
  attachments?: Attachment[];
  /** Further headers, by name. The ones built from the fields above are refused here. */
  headers?: Record<string, string>;
  /** Defaults to `<uuid@sender's domain>`. */
  messageId?: string;
  /** Defaults to now. */
  date?: Date;
  /** Overrides the addresses the server is told, which otherwise come from the fields. */
  envelope?: { from: string; to: string[] };
}

export interface Built {
  /** The message, CRLF line endings, not yet dot-stuffed. */
  text: string;
  messageId: string;
  envelope: { from: string; to: string[] };
  /** A mailbox with a non-ASCII local part, which needs the server's `SMTPUTF8`. */
  smtpUtf8: boolean;
}

/** Headers built from the fields. Setting one through `headers` is refused. */
const RESERVED = new Set([
  "date",
  "from",
  "to",
  "cc",
  "bcc",
  "reply-to",
  "subject",
  "message-id",
  "mime-version",
  "content-type",
  "content-transfer-encoding",
  "content-disposition",
]);

export async function buildMessage(message: Message): Promise<Built> {
  const from = parseAddress(message.from);
  const to = parseAddresses(message.to);
  const cc = parseAddresses(message.cc);
  const bcc = parseAddresses(message.bcc);
  const replyTo = parseAddresses(message.replyTo);

  const envelope = message.envelope ?? {
    from: from.address,
    to: [...new Set([...to, ...cc, ...bcc].map((a) => a.address))],
  };
  for (const address of [envelope.from, ...envelope.to]) parseAddress(address);
  if (envelope.to.length === 0) invalid("the message has no recipients — set to, cc or bcc");

  const messageId = normalizeMessageId(
    message.messageId ??
      `${crypto.randomUUID()}@${from.address.slice(from.address.lastIndexOf("@") + 1)}`,
  );

  const headers: string[] = [
    `Date: ${formatDate(message.date ?? new Date())}`,
    foldHeader("From", formatAddress(from)),
  ];
  if (replyTo.length > 0) headers.push(addressHeader("Reply-To", replyTo));
  if (to.length > 0) headers.push(addressHeader("To", to));
  if (cc.length > 0) headers.push(addressHeader("Cc", cc));
  if (message.subject !== undefined) {
    checkValue("Subject", message.subject);
    headers.push(
      foldHeader(
        "Subject",
        needsEncoding(message.subject) ? encodeWord(message.subject) : message.subject,
      ),
    );
  }
  headers.push(`Message-ID: ${messageId}`, "MIME-Version: 1.0");
  for (const [name, value] of Object.entries(message.headers ?? {})) {
    if (!/^[!-9;-~]+$/.test(name)) invalid(`${JSON.stringify(name)} is not a header name`);
    if (RESERVED.has(name.toLowerCase())) {
      invalid(
        `the ${name} header is built from the message's fields — set those instead of headers["${name}"]`,
      );
    }
    checkValue(name, value);
    headers.push(foldHeader(name, needsEncoding(value) ? encodeWord(value) : value));
  }

  const body = await bodyPart(message);
  return {
    text: `${headers.join("\r\n")}\r\n${serialize(body)}`,
    messageId,
    envelope,
    smtpUtf8: [envelope.from, ...envelope.to].some(needsSmtpUtf8),
  };
}

// --- the MIME tree -------------------------------------------------------------

interface Part {
  headers: string[];
  /** A leaf's encoded body, or a multipart's children. */
  body: string | Part[];
}

async function bodyPart(message: Message): Promise<Part> {
  const attachments = message.attachments ?? [];
  const inline = attachments.filter((a) => a.cid !== undefined);
  const files = attachments.filter((a) => a.cid === undefined);

  const texts: Part[] = [];
  if (message.text !== undefined) texts.push(textPart("text/plain", message.text));
  if (message.html !== undefined) texts.push(textPart("text/html", message.html));
  if (texts.length === 0) texts.push(textPart("text/plain", ""));

  let part: Part = texts.length === 1 ? (texts[0] as Part) : multipart("alternative", texts);
  if (inline.length > 0) {
    part = multipart("related", [part, ...(await Promise.all(inline.map(attachmentPart)))]);
  }
  if (files.length > 0) {
    part = multipart("mixed", [part, ...(await Promise.all(files.map(attachmentPart)))]);
  }
  return part;
}

function textPart(type: string, text: string): Part {
  const sevenBit = isSevenBit(text);
  return {
    headers: [
      `Content-Type: ${type}; charset=utf-8`,
      `Content-Transfer-Encoding: ${sevenBit ? "7bit" : "quoted-printable"}`,
    ],
    body: sevenBit ? text.replace(/\r\n|\r|\n/g, "\r\n") : quotedPrintable(text),
  };
}

async function attachmentPart(attachment: Attachment): Promise<Part> {
  const bytes = await toBytes(attachment.content);
  const type =
    attachment.contentType ??
    (attachment.content instanceof Blob && attachment.content.type !== ""
      ? attachment.content.type
      : "application/octet-stream");
  checkValue("Content-Type", type);
  const disposition = attachment.cid === undefined ? "attachment" : "inline";
  const headers = [
    `Content-Type: ${type}${attachment.filename === undefined ? "" : `; name=${quotedParam(attachment.filename)}`}`,
    "Content-Transfer-Encoding: base64",
    `Content-Disposition: ${disposition}${attachment.filename === undefined ? "" : `; ${filenameParam(attachment.filename)}`}`,
  ];
  if (attachment.cid !== undefined) {
    checkValue("Content-ID", attachment.cid);
    headers.push(`Content-ID: <${attachment.cid.replace(/^<|>$/g, "")}>`);
  }
  return { headers, body: base64Lines(bytes) };
}

function multipart(subtype: string, children: Part[]): Part {
  return {
    headers: [`Content-Type: multipart/${subtype}; boundary="${boundary()}"`],
    body: children,
  };
}

function serialize(part: Part): string {
  const head = part.headers.join("\r\n");
  if (typeof part.body === "string") return `${head}\r\n\r\n${part.body}`;
  const marker = /boundary="([^"]+)"/.exec(part.headers[0] ?? "")?.[1] ?? "";
  const children = part.body.map((child) => `--${marker}\r\n${serialize(child)}`);
  return `${head}\r\n\r\n${children.join("\r\n")}\r\n--${marker}--`;
}

/**
 * A boundary no body can contain: base64 and quoted-printable never produce
 * `_` runs, and the UUID makes a collision with 7bit text a matter of chance
 * that is not worth a scan.
 */
function boundary(): string {
  return `----=_esrun_${crypto.randomUUID()}`;
}

// --- helpers -------------------------------------------------------------------

function addressHeader(name: string, addresses: Address[]): string {
  return foldHeader(name, addresses.map(formatAddress).join(", "));
}

/** A parameter value, quoted (RFC 2045 §5.1), or encoded-word if not ASCII. */
function quotedParam(value: string): string {
  checkValue("parameter", value);
  if (needsEncoding(value)) return `"${encodeWord(value)}"`;
  return `"${value.replace(/(["\\])/g, "\\$1")}"`;
}

/**
 * `filename=` for ASCII, and RFC 2231's `filename*=UTF-8''…` otherwise — the
 * form a non-ASCII parameter is actually specified to take.
 */
function filenameParam(filename: string): string {
  checkValue("filename", filename);
  if (!needsEncoding(filename)) return `filename=${quotedParam(filename)}`;
  const encoded = [...new TextEncoder().encode(filename)]
    .map((byte) =>
      /[A-Za-z0-9!#$&+.^_`|~-]/.test(String.fromCharCode(byte))
        ? String.fromCharCode(byte)
        : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`,
    )
    .join("");
  return `filename*=UTF-8''${encoded}`;
}

async function toBytes(content: Attachment["content"]): Promise<Uint8Array> {
  if (typeof content === "string") return new TextEncoder().encode(content);
  if (content instanceof Uint8Array) return content;
  if (content instanceof ArrayBuffer) return new Uint8Array(content);
  return new Uint8Array(await content.arrayBuffer());
}

function normalizeMessageId(id: string): string {
  checkValue("Message-ID", id);
  const bare = id.replace(/^<|>$/g, "");
  if (!/^[^\s<>@]+@[^\s<>@]+$/.test(bare)) invalid(`${JSON.stringify(id)} is not a Message-ID`);
  return `<${bare}>`;
}

const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** RFC 5322 §3.3, in UTC: `Sat, 26 Sep 2026 09:05:07 +0000`. */
export function formatDate(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${DAYS[date.getUTCDay()]}, ${date.getUTCDate()} ${MONTHS[date.getUTCMonth()]} ${date.getUTCFullYear()} ${pad(date.getUTCHours())}:${pad(date.getUTCMinutes())}:${pad(date.getUTCSeconds())} +0000`;
}

/**
 * A header value with a line break in it would end the header and start
 * another — the classic injection. It is refused, not stripped: silently
 * repairing an injection attempt would hide it from whoever should know.
 */
function checkValue(what: string, value: string): void {
  if (/[\r\n]/.test(value))
    invalid(`${what} contains a line break, which would start another header`);
}

function invalid(message: string): never {
  throw new SmtpError(message, { code: SmtpErrorCode.InvalidMessage });
}
