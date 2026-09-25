/**
 * The encodings a message needs so that every octet on the wire is 7-bit and
 * every line is short (RFC 2045, 2047, 5322).
 *
 * A message built here never relies on `8BITMIME` or `SMTPUTF8` for its
 * content: text goes as quoted-printable, files as base64, and a header with
 * anything outside ASCII as encoded-words. That is what makes it deliverable
 * through any relay, including the ones that still strip the high bit.
 */

/** Whether text holds anything but printable ASCII, tab and space. */
export function needsEncoding(text: string): boolean {
  return /[^\x20-\x7e\t]/.test(text);
}

/**
 * RFC 2047 B-encoded words for a header: `=?UTF-8?B?…?=`.
 *
 * Each word stays within 75 characters (§2) and is cut on a character
 * boundary, never inside a UTF-8 sequence — a decoder handles each word on its
 * own, so a split character would turn into two replacement characters.
 */
export function encodeWord(text: string): string {
  const words: string[] = [];
  // 75 - "=?UTF-8?B?".length - "?=".length = 63 base64 characters = 45 bytes.
  const maxBytes = 45;
  let chunk = "";
  let chunkBytes = 0;
  const encoder = new TextEncoder();
  for (const char of text) {
    const size = encoder.encode(char).length;
    if (chunkBytes + size > maxBytes && chunk !== "") {
      words.push(word(chunk));
      chunk = "";
      chunkBytes = 0;
    }
    chunk += char;
    chunkBytes += size;
  }
  if (chunk !== "") words.push(word(chunk));
  return words.join(" ");
}

function word(text: string): string {
  return `=?UTF-8?B?${bytesToBase64(new TextEncoder().encode(text))}?=`;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary);
}

/** Base64 in 76-character lines (RFC 2045 §6.8). */
export function base64Lines(bytes: Uint8Array): string {
  const encoded = bytesToBase64(bytes);
  const lines: string[] = [];
  for (let i = 0; i < encoded.length; i += 76) lines.push(encoded.slice(i, i + 76));
  return lines.join("\r\n");
}

/**
 * Quoted-printable (RFC 2045 §6.7) of a text's UTF-8, with CRLF line breaks
 * kept as line breaks and soft breaks (`=` at the end) keeping every encoded
 * line within 76 characters.
 */
export function quotedPrintable(text: string): string {
  const out: string[] = [];
  for (const line of text.split(/\r\n|\r|\n/)) {
    const bytes = new TextEncoder().encode(line);
    let encoded = "";
    let length = 0;
    for (let i = 0; i < bytes.length; i++) {
      const byte = bytes[i] as number;
      const last = i === bytes.length - 1;
      // Printable ASCII except `=` goes as itself; so does a space or tab that
      // is not at the end of a line, where a transport may strip it.
      const literal =
        (byte >= 33 && byte <= 126 && byte !== 61) || ((byte === 32 || byte === 9) && !last);
      const piece = literal
        ? String.fromCharCode(byte)
        : `=${byte.toString(16).toUpperCase().padStart(2, "0")}`;
      if (length + piece.length > 75) {
        encoded += "=\r\n";
        length = 0;
      }
      encoded += piece;
      length += piece.length;
    }
    out.push(encoded);
  }
  return out.join("\r\n");
}

/** Whether text can go as 7bit: ASCII, and no line past 998 octets. */
export function isSevenBit(text: string): boolean {
  if (needsEncoding(text.replace(/\r\n|\r|\n/g, ""))) return false;
  return text.split(/\r\n|\r|\n/).every((line) => line.length <= 998);
}

/**
 * A header, folded at whitespace so that lines stay near 78 characters
 * (RFC 5322 §2.2.3). A single token longer than that is left long — it cannot
 * be folded without changing it — and is still within the 998 hard limit,
 * since encoded-words are short by construction.
 */
export function foldHeader(name: string, value: string): string {
  const tokens = value.split(" ");
  let line = `${name}:`;
  const lines: string[] = [];
  for (const token of tokens) {
    if (line.length + 1 + token.length > 78 && line.length > name.length + 1) {
      lines.push(line);
      line = ` ${token}`;
    } else {
      line += ` ${token}`;
    }
  }
  lines.push(line);
  return lines.join("\r\n");
}
