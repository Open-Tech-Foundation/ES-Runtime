/**
 * The message body as it crosses the wire after `DATA` (RFC 5321 §4.5.2).
 *
 * Three rules, all the transport's to enforce so that no message — built here
 * or handed over raw — can be framed wrongly:
 *
 * * **Lines end in CRLF.** A bare LF or CR is normalised to CRLF; a message is
 *   text, and a lone line break in it is not one the protocol has.
 * * **A line starting with `.` gets another** (dot-stuffing), because
 *   `<CRLF>.<CRLF>` ends the data and a message containing that line would
 *   otherwise end early — and whatever followed would be read as commands.
 * * **No line exceeds 998 octets.** A server may truncate or refuse longer ones;
 *   refusing here says which message was at fault.
 */

import { SmtpError, SmtpErrorCode } from "../errors.js";

/** RFC 5321 §4.5.3.1.6: 1000 octets including the CRLF. */
export const MAX_LINE_OCTETS = 998;

const CR = 0x0d;
const LF = 0x0a;
const DOT = 0x2e;

export interface Framed {
  /** The bytes to send after the `354`, terminator included. */
  bytes: Uint8Array;
  /** The message's own size in octets (CRLF-normalised, before stuffing), for `SIZE`. */
  size: number;
  /** Whether any octet is above 127, which needs `8BITMIME` or `SMTPUTF8`. */
  eightBit: boolean;
}

export function frame(message: string | Uint8Array): Framed {
  const input = typeof message === "string" ? new TextEncoder().encode(message) : message;
  // Worst case every line gains a dot and every break a CR, plus the terminator.
  const out = new Uint8Array(input.length * 2 + 5);
  let o = 0;
  let size = 0;
  let lineLength = 0;
  let lineStart = true;
  let eightBit = false;
  let lineNumber = 1;

  const endLine = () => {
    out[o++] = CR;
    out[o++] = LF;
    size += 2;
    lineStart = true;
    lineLength = 0;
    lineNumber++;
  };

  for (let i = 0; i < input.length; i++) {
    const byte = input[i] as number;
    if (byte === CR) {
      // CRLF, or a bare CR: either way one line break.
      if (input[i + 1] === LF) i++;
      endLine();
      continue;
    }
    if (byte === LF) {
      endLine();
      continue;
    }
    if (lineStart && byte === DOT) out[o++] = DOT;
    lineStart = false;
    if (byte > 127) eightBit = true;
    out[o++] = byte;
    size++;
    if (++lineLength > MAX_LINE_OCTETS) {
      throw new SmtpError(
        `line ${lineNumber} of the message is longer than ${MAX_LINE_OCTETS} octets, which SMTP does not allow — encode the body (quoted-printable or base64) rather than sending it raw`,
        { code: SmtpErrorCode.InvalidMessage },
      );
    }
  }
  // The terminator is `CRLF . CRLF`; a message that did not end its last line
  // gets the line ending first, so the dot starts a line of its own.
  if (!lineStart) endLine();
  out[o++] = DOT;
  out[o++] = CR;
  out[o++] = LF;
  return { bytes: out.subarray(0, o), size, eightBit };
}
