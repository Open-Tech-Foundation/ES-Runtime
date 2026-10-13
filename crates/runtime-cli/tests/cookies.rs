//! `Cookie` and `CookieMap` from `runtime:http` (DECISIONS D144), through the
//! real binary: parity with Bun where it is sound, refusal where a browser
//! would discard the cookie, and `request.cookies` sent with the response.

use std::process::{Command, Output};

fn esrun(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
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
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(fixtures)
        .arg("cookie-cases.mjs")
        .output()
        .expect("spawn esrun");
    let expected = std::fs::read_to_string(format!("{fixtures}/cookie-cases.expected"))
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
