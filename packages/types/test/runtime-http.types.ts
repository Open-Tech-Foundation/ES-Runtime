// A type test for `runtime:http`'s cookies. Compiled by `tsc -p .`, never run.

import { Cookie, CookieMap, CSRF, serve, Webhook, WebhookVerificationError } from "runtime:http";
import { env } from "runtime:process";

serve(async (req) => {
  const sid: string | null = req.cookies.get("sid");
  req.cookies.set("visited", "yes", { httpOnly: true, sameSite: "Strict" });
  req.cookies.delete("__Host-old", { secure: true });
  // @ts-expect-error — a cookie value is a string.
  req.cookies.set("n", 1);
  // Signed and sealed (D161): secrets newest first, strings, bytes or Secrets.
  const secrets = [env.COOKIE_KEY, new Uint8Array(32), "k".repeat(32)];
  req.cookies.setSealed("session", "{}", { secrets, maxAge: 3600, httpOnly: true });
  const session: string | null = req.cookies.getSealed("session", { secrets });
  req.cookies.setSigned("uid", "42", { secrets });
  const uid: string | null = req.cookies.getSigned("uid", { secrets });
  // @ts-expect-error — reading needs the secrets.
  req.cookies.getSealed("session");
  void [session, uid];
  // CSRF (D162): the token from the form; the session ID from the session.
  const token: string = CSRF.generate(secrets, { sessionId: "s1", expiresIn: 3_600_000 });
  const ok: boolean = CSRF.verify(token, { secret: env.COOKIE_KEY, sessionId: "s1", request: req });
  // @ts-expect-error — a token must be bound to a session.
  CSRF.generate("k".repeat(32), {});
  // @ts-expect-error — only base64url is produced.
  CSRF.generate("k".repeat(32), { sessionId: "s1", encoding: "hex" });
  void ok;
  // Webhooks (D163): verify the raw body against the request's headers.
  const hook = new Webhook([env.WEBHOOK_SECRET, "whsec_old"]);
  try {
    const event = hook.verify(await req.text(), req.headers, { tolerance: 60 });
    void event;
  } catch (e) {
    if (e instanceof WebhookVerificationError) return new Response(e.message, { status: 400 });
  }
  const signature: string = hook.sign("msg_1", new Date(), new Uint8Array(2));
  const { secretKey, publicKey } = Webhook.generateKeyPair();
  new Webhook(new Uint8Array(32), { format: "raw" });
  // @ts-expect-error — the timestamp is a Date, as in the reference library.
  hook.sign("msg_1", 1614265330, "x");
  void [signature, secretKey, publicKey, Webhook.generateSecret()];
  return new Response(sid);
});

const cookie = Cookie.parse("sid=abc; Path=/; HttpOnly");
const header: string = cookie.serialize();
const expired: boolean = new Cookie("a", "b", { maxAge: 0 }).isExpired();
const map = new CookieMap({ a: "1" });
for (const [name, value] of map) {
  void [name satisfies string, value satisfies string];
}
const headers: string[] = map.toSetCookieHeaders();
// @ts-expect-error — sameSite is one of three values.
new Cookie("a", "b", { sameSite: "sometimes" });
void [header, expired, headers];
