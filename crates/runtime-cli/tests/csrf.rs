//! `CSRF` from `runtime:http` (DECISIONS D162), through the real binary: Bun's
//! `generate`/`verify`, bound to a session, `false` for any bad token, and the
//! browser's own `Sec-Fetch-Site`/`Origin` signal when given the request.

use std::process::{Command, Output};

mod common;

fn esrun(args: &[&str]) -> Output {
    Command::new(common::esrun_bin())
        .args(args)
        .output()
        .expect("spawn esrun")
}

/// Runs `code` and returns its stdout, failing with stderr if it did not exit 0.
fn run(args: &[&str], code: &str) -> String {
    let script = format!("-e={code}");
    let mut all: Vec<&str> = args.to_vec();
    all.push(&script);
    let out = esrun(&all);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

const PRELUDE: &str = "import { CSRF } from 'runtime:http'; \
    const K1 = 'k'.repeat(32), K2 = 'q'.repeat(32); \
    const v = (token, extra = {}) => CSRF.verify(token, { secret: K1, sessionId: 's1', ...extra });";

#[test]
fn a_token_verifies_only_for_its_session_and_secret() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const t = CSRF.generate(K1, {{ sessionId: 's1' }}); \
            console.log(typeof t, t.length, /^[A-Za-z0-9_-]+$/.test(t)); \
            console.log(v(t), v(t, {{ sessionId: 's2' }}), v(t, {{ secret: K2 }}), v(t, {{ secret: [K2, K1] }})); \
            console.log(CSRF.generate(K1, {{ sessionId: 's1' }}) !== t); \
            const flipped = t.slice(0, 30) + (t[30] === 'A' ? 'B' : 'A') + t.slice(31); \
            console.log(v(flipped), v(t + 'A'), v(t.slice(1))); \
            console.log(v(undefined), v(null), v(''), v(42), v({{}}));"
        ),
    );
    assert_eq!(
        s,
        "string 86 true\ntrue false false true\ntrue\nfalse false false\nfalse false false false false\n"
    );
}

#[test]
fn a_token_expires_by_its_own_lifetime_and_by_max_age() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const short = CSRF.generate(K1, {{ sessionId: 's1', expiresIn: 1 }}); \
            await new Promise((r) => setTimeout(r, 20)); \
            const fresh = CSRF.generate(K1, {{ sessionId: 's1' }}); \
            await new Promise((r) => setTimeout(r, 20)); \
            console.log(v(short), v(fresh), v(fresh, {{ maxAge: 5 }}), v(fresh, {{ maxAge: 60000 }}));"
        ),
    );
    assert_eq!(s, "false true false true\n");
}

/// A misused API is the programmer's error, so it throws at the call; only the
/// token, which is request input, is answered with `false`.
#[test]
fn a_missing_secret_or_session_or_an_unknown_option_throws() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const t = CSRF.generate(K1, {{ sessionId: 's1' }}); \
            for (const call of [ \
              () => CSRF.generate(undefined, {{ sessionId: 's1' }}), \
              () => CSRF.generate(K1), \
              () => CSRF.generate(K1, {{ sessionId: '' }}), \
              () => CSRF.generate('short', {{ sessionId: 's1' }}), \
              () => CSRF.generate(K1, {{ sessionId: 's1', expiresIn: 0 }}), \
              () => CSRF.generate(K1, {{ sessionId: 's1', encoding: 'hex' }}), \
              () => CSRF.generate(K1, {{ sessionId: 's1', algorithm: 'sha512' }}), \
              () => CSRF.generate(K1, {{ sessionId: 's1', session: 'x' }}), \
              () => CSRF.verify(t, {{ sessionId: 's1' }}), \
              () => CSRF.verify(t, {{ secret: K1 }}), \
              () => CSRF.verify(t, {{ secret: K1, sessionId: 's1', request: {{}} }}), \
            ]) {{ try {{ call(); console.log('NO THROW'); }} catch (e) {{ console.log(e.name, e.message); }} }} \
            console.log(v(t, {{ encoding: 'base64url', algorithm: 'sha256' }}));"
        ),
    );
    assert_eq!(
        s,
        "TypeError CSRF.generate needs a secret\n\
         TypeError sessionId is required: a token not bound to a session would work for every user\n\
         TypeError sessionId is required: a token not bound to a session would work for every user\n\
         TypeError secrets[0] is 5 bytes; a cookie secret needs at least 32\n\
         TypeError expiresIn must be a positive number of milliseconds\n\
         TypeError CSRF tokens are base64url; encoding must be \"base64url\"\n\
         TypeError CSRF tokens are HMAC-SHA256; algorithm must be \"sha256\"\n\
         TypeError unknown CSRF.generate option \"session\"\n\
         TypeError CSRF.verify needs a secret\n\
         TypeError sessionId is required: a token not bound to a session would work for every user\n\
         TypeError request must be a Request\n\
         true\n"
    );
}

/// With the request, the browser's signal comes first: a cross-site or
/// same-site fetch fails even with a valid token, as does a foreign `Origin`.
#[test]
fn the_browser_signal_is_checked_when_the_request_is_given() {
    let s = run(
        &[],
        &format!(
            "{PRELUDE} \
            const t = CSRF.generate(K1, {{ sessionId: 's1' }}); \
            const req = (headers) => new Request('https://app.example/transfer', {{ method: 'POST', headers }}); \
            for (const headers of [ \
              {{ 'sec-fetch-site': 'same-origin' }}, \
              {{ 'sec-fetch-site': 'none' }}, \
              {{ 'sec-fetch-site': 'same-site' }}, \
              {{ 'sec-fetch-site': 'cross-site', origin: 'https://evil.example' }}, \
              {{ origin: 'https://app.example' }}, \
              {{ origin: 'https://evil.example' }}, \
              {{ origin: 'null' }}, \
              {{}}, \
            ]) console.log(JSON.stringify(headers), v(t, {{ request: req(headers) }})); \
            const partner = req({{ 'sec-fetch-site': 'cross-site', origin: 'https://partner.example' }}); \
            console.log('trusted', v(t, {{ request: partner, trustedOrigins: ['https://partner.example/'] }})); \
            console.log('bad token, good request', v('nope', {{ request: req({{ 'sec-fetch-site': 'same-origin' }}) }}));"
        ),
    );
    assert_eq!(
        s,
        "{\"sec-fetch-site\":\"same-origin\"} true\n\
         {\"sec-fetch-site\":\"none\"} true\n\
         {\"sec-fetch-site\":\"same-site\"} false\n\
         {\"sec-fetch-site\":\"cross-site\",\"origin\":\"https://evil.example\"} false\n\
         {\"origin\":\"https://app.example\"} true\n\
         {\"origin\":\"https://evil.example\"} false\n\
         {\"origin\":\"null\"} false\n\
         {} true\n\
         trusted true\n\
         bad token, good request false\n"
    );
}

/// The guide's flow, through `serve()`: a form carries a token for the
/// session in a signed cookie, and only a POST that brings it back succeeds.
#[test]
fn a_form_post_needs_the_token_issued_to_its_session() {
    let s = run(
        &["--allow-listen", "--allow-net"],
        "import { serve, CSRF } from 'runtime:http'; \
        const secret = 'k'.repeat(32); \
        const server = serve({ hostname: '127.0.0.1', port: 0 }, async (request) => { \
          let sessionId = request.cookies.getSigned('sid', { secrets: [secret] }); \
          if (sessionId === null) { \
            sessionId = crypto.randomUUID(); \
            request.cookies.setSigned('sid', sessionId, { secrets: [secret], httpOnly: true }); \
          } \
          if (request.method === 'GET') return new Response(CSRF.generate(secret, { sessionId })); \
          const form = await request.formData(); \
          const ok = CSRF.verify(form.get('csrf'), { secret, sessionId, request }); \
          return new Response(ok ? 'done' : 'forbidden', { status: ok ? 200 : 403 }); \
        }); \
        const { port } = await server.addr; \
        const url = `http://127.0.0.1:${port}/`; \
        const page = await fetch(url); \
        const cookie = page.headers.getSetCookie()[0].split(';')[0]; \
        const token = await page.text(); \
        const post = (body, headers = {}) => fetch(url, { method: 'POST', body, headers: { cookie, ...headers } }) \
          .then(async (r) => `${r.status} ${await r.text()}`); \
        const form = (csrf) => { const f = new FormData(); f.set('csrf', csrf); return f; }; \
        console.log(await post(form(token))); \
        console.log(await post(form('forged'))); \
        console.log(await post(new FormData())); \
        console.log(await post(form(token), { 'sec-fetch-site': 'cross-site' })); \
        const other = await fetch(url); \
        console.log(await fetch(url, { method: 'POST', body: form(token), \
          headers: { cookie: other.headers.getSetCookie()[0].split(';')[0] } }).then((r) => r.status)); \
        await server.stop();",
    );
    assert_eq!(
        s,
        "200 done\n403 forbidden\n403 forbidden\n403 forbidden\n403\n"
    );
}
