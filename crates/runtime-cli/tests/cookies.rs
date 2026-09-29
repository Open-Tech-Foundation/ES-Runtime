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
