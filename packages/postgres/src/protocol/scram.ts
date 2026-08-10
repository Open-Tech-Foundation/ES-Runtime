/**
 * SCRAM-SHA-256 (RFC 5802 / RFC 7677) — the authentication PostgreSQL has
 * defaulted to since version 14.
 *
 * Entirely WebCrypto: PBKDF2 and HMAC-SHA-256 are native ops in this runtime,
 * so the expensive half (four thousand PBKDF2 iterations, by default) runs at
 * native speed rather than in a JavaScript loop.
 */

const ENCODER = new TextEncoder();

export interface ScramSession {
  /** The `client-first-message` to send. */
  initial: string;
  /** Given the server's first message, produce the client's final one. */
  final(serverFirst: string): Promise<{ message: string; verify(serverFinal: string): void }>;
}

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

function unbase64(text: string): Uint8Array {
  const binary = atob(text);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

/** `a,b,c` → `{ a: …, b: … }`, the shape every SCRAM message has. */
function attributes(message: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const part of message.split(",")) {
    const eq = part.indexOf("=");
    if (eq > 0) out[part.slice(0, eq)] = part.slice(eq + 1);
  }
  return out;
}

async function hmac(key: Uint8Array, data: string | Uint8Array): Promise<Uint8Array> {
  const imported = await crypto.subtle.importKey(
    "raw",
    key as BufferSource,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const bytes = typeof data === "string" ? ENCODER.encode(data) : data;
  return new Uint8Array(await crypto.subtle.sign("HMAC", imported, bytes as BufferSource));
}

async function sha256(data: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", data as BufferSource));
}

function xor(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length);
  for (let i = 0; i < a.length; i++) out[i] = a[i]! ^ b[i]!;
  return out;
}

/**
 * The password, normalized.
 *
 * SASLprep (RFC 4013) is what the specification asks for. This applies the part
 * that matters in practice — Unicode NFKC — and leaves the prohibited-character
 * and bidirectional checks out. An ASCII password, which is nearly all of them,
 * is unaffected either way; a non-ASCII one now agrees with the server for the
 * common case instead of failing outright. The gap is documented rather than
 * papered over.
 */
function normalize(password: string): string {
  return password.normalize("NFKC");
}

/**
 * Begins a SCRAM exchange for `password`.
 *
 * `options` exists for the tests: RFC 7677's published vectors fix the nonce and
 * carry a username, and an algorithm that can only be run with a random nonce
 * can only be checked against itself. Neither is used in production —
 * PostgreSQL takes the username from the startup packet and ignores the one in
 * the SCRAM message, which is why the default is empty.
 */
export function scram(
  password: string,
  options: { nonce?: string; username?: string } = {},
): ScramSession {
  const clientNonce = options.nonce ?? base64(crypto.getRandomValues(new Uint8Array(18)));
  // `n,,` is the GS2 header: no channel binding, no authorization identity.
  const bare = `n=${options.username ?? ""},r=${clientNonce}`;
  const initial = `n,,${bare}`;

  return {
    initial,
    async final(serverFirst: string) {
      const fields = attributes(serverFirst);
      const nonce = fields["r"];
      const salt = fields["s"];
      const iterations = Number(fields["i"]);
      if (!nonce || !salt || !Number.isInteger(iterations) || iterations < 1) {
        throw new Error(`the server's SCRAM challenge is malformed: ${serverFirst}`);
      }
      if (!nonce.startsWith(clientNonce)) {
        // The server must echo our nonce. One that does not is not the server
        // we started talking to.
        throw new Error("the server's SCRAM nonce does not extend the client's");
      }

      const key = await crypto.subtle.importKey(
        "raw",
        ENCODER.encode(normalize(password)) as BufferSource,
        "PBKDF2",
        false,
        ["deriveBits"],
      );
      const saltedPassword = new Uint8Array(
        await crypto.subtle.deriveBits(
          {
            name: "PBKDF2",
            hash: "SHA-256",
            salt: unbase64(salt) as BufferSource,
            iterations,
          },
          key,
          256,
        ),
      );

      // `biws` is base64("n,,") — the GS2 header again, as the channel-binding
      // attribute of a client that offered no binding.
      const withoutProof = `c=biws,r=${nonce}`;
      const authMessage = `${bare},${serverFirst},${withoutProof}`;

      const clientKey = await hmac(saltedPassword, "Client Key");
      const storedKey = await sha256(clientKey);
      const clientSignature = await hmac(storedKey, authMessage);
      const proof = xor(clientKey, clientSignature);

      const serverKey = await hmac(saltedPassword, "Server Key");
      const expected = base64(await hmac(serverKey, authMessage));

      return {
        message: `${withoutProof},p=${base64(proof)}`,
        verify(serverFinal: string) {
          const signature = attributes(serverFinal)["v"];
          // Mutual authentication: without this the client has proved itself to
          // the server and learned nothing about who it is talking to.
          if (signature !== expected) {
            throw new Error("the server failed to prove it knows the password");
          }
        },
      };
    },
  };
}
