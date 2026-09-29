// A type test for `runtime:http`'s cookies. Compiled by `tsc -p .`, never run.

import { Cookie, CookieMap, serve } from "runtime:http";

serve((req) => {
  const sid: string | null = req.cookies.get("sid");
  req.cookies.set("visited", "yes", { httpOnly: true, sameSite: "Strict" });
  req.cookies.delete("__Host-old", { secure: true });
  // @ts-expect-error — a cookie value is a string.
  req.cookies.set("n", 1);
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
