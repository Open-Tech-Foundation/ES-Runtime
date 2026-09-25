/**
 * The two authentication plugins every MySQL server since 5.7 offers.
 *
 * Neither sends the password. Each proves knowledge of it against the 20-byte
 * nonce ("scramble") the server sent in its greeting, so a recorded exchange
 * cannot be replayed against the next connection.
 */

const ENCODER = new TextEncoder();

async function digest(algorithm: "SHA-1" | "SHA-256", ...parts: Uint8Array[]): Promise<Uint8Array> {
  let length = 0;
  for (const part of parts) length += part.length;
  const joined = new Uint8Array(length);
  let at = 0;
  for (const part of parts) {
    joined.set(part, at);
    at += part.length;
  }
  return new Uint8Array(await crypto.subtle.digest(algorithm, joined));
}

function xor(a: Uint8Array, b: Uint8Array): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(a.length);
  for (let i = 0; i < a.length; i++) out[i] = a[i]! ^ b[i % b.length]!;
  return out;
}

/**
 * `mysql_native_password`: `SHA1(pw) XOR SHA1(scramble + SHA1(SHA1(pw)))`.
 *
 * The server stores `SHA1(SHA1(pw))`, so it can check this without ever having
 * held the password. An empty password sends an empty response.
 */
export async function nativePassword(password: string, scramble: Uint8Array): Promise<Uint8Array> {
  if (password === "") return new Uint8Array(0);
  const stage1 = await digest("SHA-1", ENCODER.encode(password));
  const stage2 = await digest("SHA-1", stage1);
  return xor(stage1, await digest("SHA-1", scramble, stage2));
}

/**
 * `caching_sha2_password`'s fast path:
 * `SHA256(pw) XOR SHA256(SHA256(SHA256(pw)) + scramble)`.
 *
 * Enough on its own when the server has this account's hash cached. When it
 * has not — the first login after a restart, or after `FLUSH PRIVILEGES` — it
 * asks for the password itself, which is {@link fullAuthentication}.
 */
export async function cachingSha2(password: string, scramble: Uint8Array): Promise<Uint8Array> {
  if (password === "") return new Uint8Array(0);
  const stage1 = await digest("SHA-256", ENCODER.encode(password));
  const stage2 = await digest("SHA-256", stage1);
  return xor(stage1, await digest("SHA-256", stage2, scramble));
}

/** The password as the server's full authentication wants it: NUL-terminated. */
export function passwordBytes(password: string): Uint8Array {
  const bytes = ENCODER.encode(password);
  const out = new Uint8Array(bytes.length + 1);
  out.set(bytes);
  return out;
}

/**
 * The password for `caching_sha2_password`'s full authentication over a
 * connection that is **not** encrypted: XORed with the scramble and encrypted
 * to the server's RSA public key with OAEP padding.
 *
 * Over TLS the password goes as it is, because the channel already protects
 * it. Over plaintext this is what keeps it from crossing the wire readable —
 * and the key it is encrypted to is one the server just sent, so a server that
 * is not who it claims to be gets the password anyway. That is the same trade
 * every MySQL client makes here; the fix for it is TLS, not a better cipher.
 */
export async function encryptPassword(
  password: string,
  scramble: Uint8Array,
  publicKeyPem: string,
): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey(
    "spki",
    pemToDer(publicKeyPem),
    { name: "RSA-OAEP", hash: "SHA-1" },
    false,
    ["encrypt"],
  );
  const plain = xor(passwordBytes(password), scramble);
  return new Uint8Array(await crypto.subtle.encrypt({ name: "RSA-OAEP" }, key, plain));
}

function pemToDer(pem: string): Uint8Array<ArrayBuffer> {
  const body = pem
    .replace(/-----BEGIN [^-]+-----/, "")
    .replace(/-----END [^-]+-----/, "")
    .replace(/\s+/g, "");
  const binary = atob(body);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}
