/**
 * The three logins: SASL PLAIN (RFC 4616), LOGIN (not standardised, offered by
 * nearly every server) and XOAUTH2 (Google's, used by Gmail and Microsoft 365).
 *
 * All three send the secret itself, base64-encoded rather than protected, which
 * is why the connection refuses to run any of them over plaintext unless told
 * to ([`allowPlaintextAuth`](../connection.ts)). CRAM-MD5 is not offered: it
 * protects the password from an eavesdropper only by storing it recoverably on
 * the server, and inside TLS it gains nothing over PLAIN (D136).
 */

/** Base64 of a string's UTF-8 bytes. `btoa` alone takes Latin-1. */
export function base64(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

/** A string from base64 of UTF-8, as a `334` challenge carries it. */
export function unbase64(encoded: string): string {
  const binary = atob(encoded.trim());
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

/** PLAIN's single message: an empty authorization identity, the user, the password. */
export function plain(user: string, password: string): string {
  return base64(`\0${user}\0${password}`);
}

/** XOAUTH2's single message. */
export function xoauth2(user: string, accessToken: string): string {
  return base64(`user=${user}\x01auth=Bearer ${accessToken}\x01\x01`);
}

export type Mechanism = "PLAIN" | "LOGIN" | "XOAUTH2";

export interface Credentials {
  user: string;
  /** For PLAIN and LOGIN. */
  password?: string;
  /** For XOAUTH2. Obtaining and refreshing it is the OAuth provider's business. */
  accessToken?: string;
}

/**
 * Which mechanism to use, from what the server offered and what the
 * credentials hold. An access token means XOAUTH2 and nothing else — falling
 * back to a password the caller did not give would be a different login.
 */
export function choose(offered: readonly string[], credentials: Credentials): Mechanism | null {
  const has = (name: string) => offered.includes(name);
  if (credentials.accessToken !== undefined) return has("XOAUTH2") ? "XOAUTH2" : null;
  if (has("PLAIN")) return "PLAIN";
  if (has("LOGIN")) return "LOGIN";
  return null;
}
