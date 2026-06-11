// atob / btoa (SPEC §2.3) — base64 over Latin-1 strings.
(() => {
  "use strict";
  const ALPHABET =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  const LOOKUP = new Int16Array(128).fill(-1);
  for (let i = 0; i < ALPHABET.length; i++) {
    LOOKUP[ALPHABET.charCodeAt(i)] = i;
  }

  globalThis.btoa = (data) => {
    const str = String(data);
    let out = "";
    for (let i = 0; i < str.length; i += 3) {
      const c0 = str.charCodeAt(i);
      const c1 = i + 1 < str.length ? str.charCodeAt(i + 1) : 0;
      const c2 = i + 2 < str.length ? str.charCodeAt(i + 2) : 0;
      if (c0 > 0xff || c1 > 0xff || c2 > 0xff) {
        throw new DOMException(
          "The string to be encoded contains characters outside of the Latin1 range.",
          "InvalidCharacterError",
        );
      }
      const triple = (c0 << 16) | (c1 << 8) | c2;
      out += ALPHABET[(triple >> 18) & 0x3f];
      out += ALPHABET[(triple >> 12) & 0x3f];
      out += i + 1 < str.length ? ALPHABET[(triple >> 6) & 0x3f] : "=";
      out += i + 2 < str.length ? ALPHABET[triple & 0x3f] : "=";
    }
    return out;
  };

  globalThis.atob = (data) => {
    // Strip ASCII whitespace, which the spec ignores.
    let str = String(data).replace(/[\t\n\f\r ]/g, "");
    if (str.length % 4 === 1) {
      throw new DOMException(
        "The string to be decoded is not correctly encoded.",
        "InvalidCharacterError",
      );
    }
    str = str.replace(/=+$/, "");
    let out = "";
    let bits = 0;
    let acc = 0;
    for (let i = 0; i < str.length; i++) {
      const v = LOOKUP[str.charCodeAt(i) & 0x7f] ?? -1;
      if (v < 0 || str.charCodeAt(i) > 127) {
        throw new DOMException(
          "The string to be decoded is not correctly encoded.",
          "InvalidCharacterError",
        );
      }
      acc = (acc << 6) | v;
      bits += 6;
      if (bits >= 8) {
        bits -= 8;
        out += String.fromCharCode((acc >> bits) & 0xff);
      }
    }
    return out;
  };
})();
