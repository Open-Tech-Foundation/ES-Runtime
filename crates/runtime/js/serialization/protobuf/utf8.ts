// Inlined UTF-8 codec — no TextEncoder/TextDecoder host calls. Measured ~2x
// faster than the host TextDecoder binding in esrun for many short strings,
// which dominate protobuf payloads.

export function utf8Read(buf: Uint8Array, start: number, end: number): string {
  const len = end - start;
  if (len < 1) return "";
  let parts: string[] | null = null;
  const chunk: number[] = [];
  let i = 0;
  let t: number;
  let p = start;
  while (p < end) {
    t = buf[p++]!;
    if (t < 128) {
      chunk[i++] = t;
    } else if (t > 191 && t < 224) {
      chunk[i++] = ((t & 31) << 6) | (buf[p++]! & 63);
    } else if (t > 239 && t < 365) {
      t = (((t & 7) << 18) | ((buf[p++]! & 63) << 12) | ((buf[p++]! & 63) << 6) | (buf[p++]! & 63)) - 0x10000;
      chunk[i++] = 0xd800 + (t >> 10);
      chunk[i++] = 0xdc00 + (t & 1023);
    } else {
      chunk[i++] = ((t & 15) << 12) | ((buf[p++]! & 63) << 6) | (buf[p++]! & 63);
    }
    if (i > 8191) {
      (parts || (parts = [])).push(String.fromCharCode.apply(String, chunk));
      i = 0;
    }
  }
  if (parts) {
    if (i) parts.push(String.fromCharCode.apply(String, chunk.slice(0, i)));
    return parts.join("");
  }
  return String.fromCharCode.apply(String, chunk.slice(0, i));
}

/** UTF-8 byte length of `str`. */
export function utf8Length(str: string): number {
  let len = 0;
  for (let i = 0; i < str.length; i++) {
    const c = str.charCodeAt(i);
    if (c < 128) len += 1;
    else if (c < 2048) len += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length && (str.charCodeAt(i + 1) & 0xfc00) === 0xdc00) {
      // surrogate pair → 4 bytes
      i++;
      len += 4;
    } else len += 3;
  }
  return len;
}

/** Writes the UTF-8 of `str` into `buf` at `offset`; returns the next offset. */
export function utf8Write(str: string, buf: Uint8Array, offset: number): number {
  let p = offset;
  for (let i = 0; i < str.length; i++) {
    let c = str.charCodeAt(i);
    if (c < 128) {
      buf[p++] = c;
    } else if (c < 2048) {
      buf[p++] = (c >> 6) | 192;
      buf[p++] = (c & 63) | 128;
    } else if (c >= 0xd800 && c <= 0xdbff && i + 1 < str.length && (str.charCodeAt(i + 1) & 0xfc00) === 0xdc00) {
      c = 0x10000 + ((c & 0x3ff) << 10) + (str.charCodeAt(++i) & 0x3ff);
      buf[p++] = (c >> 18) | 240;
      buf[p++] = ((c >> 12) & 63) | 128;
      buf[p++] = ((c >> 6) & 63) | 128;
      buf[p++] = (c & 63) | 128;
    } else {
      buf[p++] = (c >> 12) | 224;
      buf[p++] = ((c >> 6) & 63) | 128;
      buf[p++] = (c & 63) | 128;
    }
  }
  return p;
}
