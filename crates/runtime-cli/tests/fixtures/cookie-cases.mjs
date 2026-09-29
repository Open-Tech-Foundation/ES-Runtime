import { Cookie, CookieMap } from "runtime:http";
// Cases for `Cookie` and `CookieMap`. The expected output beside it
// (`cookie-cases.expected`) was recorded from Bun 1.4.2 running this same script
// with `Bun.Cookie`/`Bun.CookieMap`, and differs only where DECISIONS D144 says
// it does: the strict-output refusals, decoding in `Cookie.parse`, a malformed
// `%` kept raw, `sameSite` in any case, and an integer `maxAge`.
const Bun = { Cookie, CookieMap };
const t = (label, f) => { try { console.log(label, "=>", JSON.stringify(f())); } catch (e) { console.log(label, "THROWS", e.name, e.message); } };
t("default serialize", () => new Bun.Cookie("a", "b").serialize());
t("value with space;", () => new Bun.Cookie("a", "x y;z").serialize());
t("unicode value", () => new Bun.Cookie("a", "é").serialize());
t("bad name", () => new Bun.Cookie("a b", "v").serialize());
t("empty name", () => new Bun.Cookie("", "v").serialize());
t("host prefix no secure", () => new Bun.Cookie("__Host-x", "v", { domain: "a.com" }).serialize());
t("maxAge -1", () => new Bun.Cookie("a", "b", { maxAge: -1 }).serialize());
t("all attrs", () => new Bun.Cookie("a", "b", { domain: "ex.com", path: "/p", expires: new Date(0), maxAge: 60, secure: true, httpOnly: true, sameSite: "strict", partitioned: true }).serialize());
t("parse set-cookie", () => Bun.Cookie.parse("sid=abc%20d; Path=/x; HttpOnly; SameSite=None; Secure; Max-Age=5").toJSON());
t("map dupes", () => new Bun.CookieMap("a=1; a=2; b=%20x; c").toJSON());
t("map junk", () => new Bun.CookieMap("=x; ;d=\"q\"; e=1=2").toJSON());
t("map set/delete headers", () => { const m = new Bun.CookieMap("a=1"); m.set("s", "v"); m.delete("a"); return m.toSetCookieHeaders(); });
t("partitioned no secure", () => new Bun.Cookie("a", "b", { partitioned: true }).serialize());
t("sameSite none no secure", () => new Bun.Cookie("a", "b", { sameSite: "none" }).serialize());
t("expires number", () => new Bun.Cookie("a", "b", { expires: 0 }).serialize());
t("encode set", () => new Bun.Cookie("a", "=/:?&+,!*'()~@$#[]\"\\ ").serialize());
t("bad % in map", () => new Bun.CookieMap("a=%E0%A4%A; b=%zz; c=ok").toJSON());
t("quoted in map", () => new Bun.CookieMap('a="x y"').toJSON());
t("cookie toJSON full", () => new Bun.Cookie("a", "b", { domain: "d.com", expires: new Date(1000), maxAge: 5, secure: true }).toJSON());
t("cookie toJSON min", () => new Bun.Cookie("a", "b").toJSON());
t("props", () => { const c = new Bun.Cookie("a", "b"); return [c.domain, c.path, c.expires, c.maxAge, c.secure, c.httpOnly, c.sameSite, c.partitioned]; });
t("isExpired", () => [new Bun.Cookie("a","b",{maxAge:0}).isExpired(), new Bun.Cookie("a","b",{expires:new Date(0)}).isExpired(), new Bun.Cookie("a","b").isExpired(), new Bun.Cookie("a","b",{expires:new Date(0), maxAge: 100}).isExpired()]);
t("parse full", () => Bun.Cookie.parse("sid=v; Domain=.Ex.com; Expires=Wed, 21 Oct 2015 07:28:00 GMT; Path=/; SameSite=Strict; Partitioned; Foo=bar").toJSON());
t("parse sameSite bad", () => Bun.Cookie.parse("a=b; SameSite=weird").toJSON());
t("parse no value", () => Bun.Cookie.parse("a").toJSON());
t("parse empty", () => Bun.Cookie.parse("").toJSON());
t("ctor string", () => new Bun.Cookie("x=1; Path=/y").toJSON());
t("ctor options", () => new Bun.Cookie({ name: "n", value: "v", path: "/p" }).serialize());
t("from", () => Bun.Cookie.from("a", "b", { httpOnly: true }).serialize());
t("sameSite case", () => new Bun.Cookie("a", "b", { sameSite: "Strict" }).serialize());
t("sameSite invalid", () => new Bun.Cookie("a", "b", { sameSite: "bogus" }).serialize());
t("map object", () => [...new Bun.CookieMap({ b: "2", a: "1" })]);
t("map pairs", () => [...new Bun.CookieMap([["a", "1"], ["b", "2"]])].concat([new Bun.CookieMap([["a","1"]]).size]));
t("map set cookie obj", () => { const m = new Bun.CookieMap(); m.set(new Bun.Cookie("a", "b", { httpOnly: true })); m.set({ name: "c", value: "d" }); return [m.toSetCookieHeaders(), m.get("a"), m.size]; });
t("map delete opts", () => { const m = new Bun.CookieMap("a=1"); m.delete({ name: "a", path: "/p", domain: "d.com" }); m.delete("zz", { path: "/q" }); return [m.toSetCookieHeaders(), m.has("a"), m.get("a"), m.size]; });
t("map set then delete", () => { const m = new Bun.CookieMap(); m.set("a", "1"); m.delete("a"); return m.toSetCookieHeaders(); });
t("map set twice", () => { const m = new Bun.CookieMap(); m.set("a", "1"); m.set("a", "2"); return [m.toSetCookieHeaders(), m.get("a")]; });
t("map keys/values/forEach", () => { const m = new Bun.CookieMap("a=1; b=2"); const f = []; m.forEach((v, k) => f.push(k + v)); return [[...m.keys()], [...m.values()], [...m.entries()], f]; });
t("map get missing", () => new Bun.CookieMap().get("x"));
t("map set invalid", () => { const m = new Bun.CookieMap(); m.set("a b", "v"); return m.toSetCookieHeaders(); });
t("map whitespace", () => new Bun.CookieMap("  a = 1 ;b=2  ").toJSON());
t("value undefined", () => new Bun.Cookie("a").serialize());
t("maxAge float", () => new Bun.Cookie("a", "b", { maxAge: 1.5 }).serialize());
t("expires invalid", () => new Bun.Cookie("a", "b", { expires: new Date("x") }).serialize());
t("domain semicolon", () => new Bun.Cookie("a", "b", { domain: "a;b" }).serialize());
t("path semicolon", () => new Bun.Cookie("a", "b", { path: "/a;b" }).serialize());
t("toString", () => String(new Bun.Cookie("a", "b")));
