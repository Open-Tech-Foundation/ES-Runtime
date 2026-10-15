//! `Cookie` and `CookieMap` from `runtime:http` (DECISIONS D144), through the
//! real binary: parity with Bun where it is sound, refusal where a browser
//! would discard the cookie, and `request.cookies` sent with the response.

use std::process::{Command, Output};

mod common;

fn esrun(args: &[&str]) -> Output {
    Command::new(common::esrun_bin())
        .args(args)
        .output()
        .expect("spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Every constructor, attribute, parse and map operation, against output
/// recorded from Bun on the same script, differing only where D144 says.
#[test]
fn cookies_behave_as_bun_except_where_d144_says() {
    let fixtures = common::fixtures();
    let out = Command::new(common::esrun_bin())
        .current_dir(&fixtures)
        .arg("cookie-cases.mjs")
        .output()
        .expect("spawn esrun");
    let expected = std::fs::read_to_string(format!("{}/cookie-cases.expected", fixtures.display()))
        .expect("read expected output");
    let actual = stdout(&out);
    for (index, (got, want)) in actual.lines().zip(expected.lines()).enumerate() {
        assert_eq!(got, want, "case {index}");
    }
    assert_eq!(actual.lines().count(), expected.lines().count(), "{actual}");
}

/// A handler reads `request.cookies`, changes it, and returns any `Response`:
/// the changes go out as `Set-Cookie` headers beside the response's own. A
/// handler that never touches it sends none.
#[test]
fn request_cookies_changes_are_sent_with_the_response() {
    let code = "import { serve } from 'runtime:http'; \
        const server = serve({ hostname: '127.0.0.1', port: 0 }, (req) => { \
          if (new URL(req.url).pathname === '/plain') return new Response('plain'); \
          const seen = [req.cookies.get('sid'), req.cookies.get('theme'), req.cookies.size]; \
          req.cookies.set('visited', 'yes', { httpOnly: true }); \
          req.cookies.delete('theme'); \
          return new Response(JSON.stringify(seen), { headers: { 'set-cookie': 'own=1' } }); \
        }); \
        const { port } = await server.addr; \
        const res = await fetch(`http://127.0.0.1:${port}/`, { \
          headers: { cookie: 'sid=a%20b; theme=dark; sid=second' } }); \
        console.log(await res.text()); \
        console.log(JSON.stringify(res.headers.getSetCookie())); \
        const plain = await fetch(`http://127.0.0.1:${port}/plain`, { headers: { cookie: 'a=1' } }); \
        await plain.text(); \
        console.log(JSON.stringify(plain.headers.getSetCookie())); \
        await server.stop();";
    let out = esrun(&["--allow-listen", "--allow-net", &format!("-e={code}")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<String> = stdout(&out).lines().map(str::to_string).collect();
    // Decoded, and the first of a duplicated name.
    assert_eq!(lines[0], r#"["a b","dark",2]"#);
    assert_eq!(
        lines[1],
        r#"["own=1","visited=yes; Path=/; HttpOnly; SameSite=Lax","theme=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; SameSite=Lax"]"#
    );
    assert_eq!(lines[2], "[]");
}

/// Server requests retain the standard Request interface, while the cookie
/// getter lives on their shared prototype instead of being installed per
/// request. Requests constructed by application code do not gain the server
/// cookie API.
#[test]
fn server_cookie_accessor_is_shared_and_client_requests_are_unchanged() {
    let code = "import { serve } from 'runtime:http'; \
        const server = serve({ hostname: '127.0.0.1', port: 0 }, (req) => { \
          console.log(req instanceof Request, 'cookies' in req, Object.hasOwn(req, 'cookies')); \
          console.log(req.constructor === Request); \
          console.log(req); \
          const ordinary = new Request('http://example.test/'); \
          console.log(ordinary instanceof Request, 'cookies' in ordinary, Object.hasOwn(ordinary, 'cookies')); \
          return new Response('ok'); \
        }); \
        const { port } = await server.addr; \
        const res = await fetch(`http://127.0.0.1:${port}/`, { headers: { cookie: 'sid=a' } }); \
        await res.text(); \
        await server.stop();";
    let out = esrun(&["--allow-listen", "--allow-net", &format!("-e={code}")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = stdout(&out);
    let lines: Vec<&str> = printed.lines().collect();
    assert_eq!(lines[0], "true true false");
    assert_eq!(lines[1], "true");
    assert!(lines[2].starts_with("Request {"), "{printed}");
    assert_eq!(lines.last(), Some(&"true false false"));
}

/// A cookie a browser would discard is an error at the line that made it, not
/// a header that looks sent.
#[test]
fn a_cookie_a_browser_would_discard_throws() {
    let code = "import { CookieMap } from 'runtime:http'; \
        const m = new CookieMap(); \
        for (const [name, options] of [ \
          ['__Host-sid', { secure: true, domain: 'example.com' }], \
          ['__Host-sid', { secure: true, path: '/app' }], \
          ['__Secure-sid', {}], \
          ['sid', { sameSite: 'none' }], \
          ['sid', { partitioned: true }], \
        ]) { \
          try { m.set(name, 'v', options); console.log('SET', name); } \
          catch (e) { console.log(e.name, e.message); } \
        } \
        m.set('__Host-sid', 'v', { secure: true }); \
        m.delete('__Host-sid', { secure: true }); \
        console.log(JSON.stringify(m.toSetCookieHeaders()), m.size);";
    let out = esrun(&[&format!("-e={code}")]);
    assert_eq!(
        stdout(&out),
        "TypeError Cookie \"__Host-sid\" would be rejected by browsers: a __Host- cookie must not have a Domain\n\
         TypeError Cookie \"__Host-sid\" would be rejected by browsers: a __Host- cookie must have Path=/\n\
         TypeError Cookie \"__Secure-sid\" would be rejected by browsers: a __Secure- cookie requires Secure\n\
         TypeError Cookie \"sid\" would be rejected by browsers: SameSite=None requires Secure\n\
         TypeError Cookie \"sid\" would be rejected by browsers: Partitioned requires Secure\n\
         [\"__Host-sid=; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure; SameSite=Lax\"] 0\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// RFC 6265bis §5.6: a browser drops a cookie whose name and value pass 4096
/// octets, and ignores a Domain or Path past 1024. Measured as sent, so a value
/// that percent-encoding triples is counted tripled.
#[test]
fn a_cookie_over_the_browser_size_limit_throws() {
    let code = "import { Cookie } from 'runtime:http'; \
        const tryIt = (name, value, options = {}) => { \
          try { new Cookie(name, value, options).serialize(); console.log('OK', name.length + value.length); } \
          catch (e) { console.log(e.name, e.message); } \
        }; \
        tryIt('sid', 'a'.repeat(4093)); \
        tryIt('sid', 'a'.repeat(4094)); \
        tryIt('sid', 'é'.repeat(700)); \
        tryIt('sid', 'v', { path: '/' + 'p'.repeat(1023) }); \
        tryIt('sid', 'v', { path: '/' + 'p'.repeat(1024) }); \
        tryIt('sid', 'v', { domain: 'é'.repeat(513) });";
    let out = esrun(&[&format!("-e={code}")]);
    assert_eq!(
        stdout(&out),
        "OK 4096\n\
         TypeError Cookie \"sid\" would be rejected by browsers: name and value are 4097 bytes once encoded; the limit is 4096\n\
         TypeError Cookie \"sid\" would be rejected by browsers: name and value are 4203 bytes once encoded; the limit is 4096\n\
         OK 4\n\
         TypeError Cookie \"sid\" would be rejected by browsers: its Path is 1025 bytes; the limit is 1024\n\
         TypeError Cookie \"sid\" would be rejected by browsers: its Domain is 1026 bytes; the limit is 1024\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The browser's half of a round trip: the `name=value` part of each
/// `Set-Cookie` header, as the next request's `Cookie` header (D161).
const ROUND_TRIP: &str = "import { CookieMap } from 'runtime:http'; \
    import { Secret } from 'runtime:process'; \
    const back = (m) => new CookieMap(m.toSetCookieHeaders().map((h) => h.split(';')[0]).join('; ')); \
    const K1 = 'k'.repeat(32), K2 = 'q'.repeat(32);";

/// Signed: readable, unforgeable, bound to its name and expiry, and rotatable.
#[test]
fn a_signed_cookie_reads_back_only_as_it_was_written() {
    let code = format!(
        "{ROUND_TRIP} \
        const m = new CookieMap(); \
        m.setSigned('uid', 'user.42', {{ secrets: [K1], httpOnly: true }}); \
        const header = m.toSetCookieHeaders()[0]; \
        console.log(header.startsWith('uid=user.42.0.'), header.includes('HttpOnly')); \
        const req = back(m); \
        console.log(req.getSigned('uid', {{ secrets: [K1] }})); \
        console.log(req.getSigned('uid', {{ secrets: [K2, K1] }})); \
        console.log(req.getSigned('uid', {{ secrets: [K2] }})); \
        const raw = req.get('uid'); \
        console.log(new CookieMap({{ uid: raw.replace('user.42', 'user.43') }}).getSigned('uid', {{ secrets: [K1] }})); \
        console.log(new CookieMap({{ admin: raw }}).getSigned('admin', {{ secrets: [K1] }})); \
        console.log(new CookieMap({{ uid: 'no-tag' }}).getSigned('uid', {{ secrets: [K1] }})); \
        console.log(new CookieMap().getSigned('uid', {{ secrets: [K1] }})); \
        const old = new CookieMap(); old.setSigned('uid', 'u', {{ secrets: [K1], maxAge: -1 }}); \
        console.log(back(old).getSigned('uid', {{ secrets: [K1] }})); \
        const live = new CookieMap(); live.setSigned('uid', 'u', {{ secrets: [new Secret(K1)], maxAge: 60 }}); \
        console.log(back(live).getSigned('uid', {{ secrets: [K1] }}));"
    );
    let out = esrun(&[&format!("-e={code}")]);
    assert_eq!(
        stdout(&out),
        "true true\nuser.42\nuser.42\nnull\nnull\nnull\nnull\nnull\nnull\nu\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Sealed: the same guarantees, and the value is not in the header at all.
#[test]
fn a_sealed_cookie_hides_its_value_and_reads_back_only_as_written() {
    let code = format!(
        "{ROUND_TRIP} \
        const m = new CookieMap(); \
        m.setSealed('session', '{{\"user\":42}}', {{ secrets: [K1], maxAge: 3600 }}); \
        const header = m.toSetCookieHeaders()[0]; \
        console.log(header.includes('user'), header.includes('Max-Age=3600')); \
        const req = back(m); \
        console.log(req.getSealed('session', {{ secrets: [K1] }})); \
        console.log(req.getSealed('session', {{ secrets: [K2, K1] }})); \
        console.log(req.getSealed('session', {{ secrets: [K2] }})); \
        const raw = req.get('session'); \
        const flipped = raw.slice(0, -2) + (raw.at(-2) === 'A' ? 'B' : 'A') + raw.at(-1); \
        console.log(new CookieMap({{ session: flipped }}).getSealed('session', {{ secrets: [K1] }})); \
        console.log(new CookieMap({{ other: raw }}).getSealed('other', {{ secrets: [K1] }})); \
        const old = new CookieMap(); old.setSealed('s', 'v', {{ secrets: [K1], expires: new Date(Date.now() - 1000) }}); \
        console.log(back(old).getSealed('s', {{ secrets: [K1] }})); \
        const a = new CookieMap(); a.setSealed('s', 'v', {{ secrets: [K1] }}); \
        const b = new CookieMap(); b.setSealed('s', 'v', {{ secrets: [K1] }}); \
        console.log(a.get('s') !== b.get('s'));"
    );
    let out = esrun(&[&format!("-e={code}")]);
    assert_eq!(
        stdout(&out),
        "false true\n{\"user\":42}\n{\"user\":42}\nnull\nnull\nnull\nnull\ntrue\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A weak or missing key is a programming error, so it throws at the call.
#[test]
fn cookie_secrets_must_be_present_and_long_enough() {
    let code = "import { CookieMap } from 'runtime:http'; \
        const m = new CookieMap(); \
        for (const call of [ \
          () => m.setSigned('a', 'v', {}), \
          () => m.setSealed('a', 'v', { secrets: [] }), \
          () => m.getSigned('a', { secrets: ['short'] }), \
          () => m.getSealed('a', { secrets: [42] }), \
          () => m.setSigned('a', 42, { secrets: ['k'.repeat(32)] }), \
          () => m.setSealed('bad name', 'v', { secrets: ['k'.repeat(32)] }), \
        ]) { try { call(); console.log('NO THROW'); } catch (e) { console.log(e.name, e.message); } }";
    let out = esrun(&[&format!("-e={code}")]);
    assert_eq!(
        stdout(&out),
        "TypeError secrets must be a non-empty array, newest first\n\
         TypeError secrets must be a non-empty array, newest first\n\
         TypeError secrets[0] is 5 bytes; a cookie secret needs at least 32\n\
         TypeError secrets[0] must be a string, bytes, or a Secret\n\
         TypeError a signed or sealed cookie value must be a string\n\
         TypeError Invalid cookie name: contains invalid characters\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// `request.cookies` in `serve()`: a sealed session set by one request is
/// read by the next, through real headers.
#[test]
fn a_sealed_session_survives_a_real_request_round_trip() {
    let code = "import { serve } from 'runtime:http'; \
        const secrets = ['k'.repeat(32)]; \
        const server = serve({ hostname: '127.0.0.1', port: 0 }, (req) => { \
          const seen = req.cookies.getSealed('session', { secrets }); \
          if (seen === null) req.cookies.setSealed('session', 'hello', { secrets, httpOnly: true }); \
          return new Response(String(seen)); \
        }); \
        const { port } = await server.addr; \
        const url = `http://127.0.0.1:${port}/`; \
        const first = await fetch(url); \
        const cookie = first.headers.getSetCookie()[0].split(';')[0]; \
        console.log(await first.text(), cookie.includes('hello')); \
        const second = await fetch(url, { headers: { cookie } }); \
        console.log(await second.text(), second.headers.getSetCookie().length); \
        await server.stop();";
    let out = esrun(&["--allow-listen", "--allow-net", &format!("-e={code}")]);
    assert_eq!(
        stdout(&out),
        "null false\nhello 0\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
