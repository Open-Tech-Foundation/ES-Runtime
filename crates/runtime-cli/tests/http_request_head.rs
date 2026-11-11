//! A served request's headers and peer stay in the host until the guest reads
//! them or the response is sent (DECISIONS D216), through the real binary.

use std::process::{Command, Output};

mod common;

fn esrun(code: &str) -> Output {
    Command::new(common::esrun_bin())
        .args(["--allow-listen", "--allow-net", &format!("-e={code}")])
        .output()
        .expect("spawn esrun")
}

fn lines(out: &Output) -> Vec<String> {
    assert!(
        out.status.success(),
        "esrun failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

/// A handler that reads its headers and peer, then answers, can read them again
/// afterwards — from a continuation that outlives the response.
#[test]
fn what_a_handler_read_stays_readable_after_the_response() {
    let out = esrun(
        "import { serve } from 'runtime:http'; \
         let later; \
         const server = serve({ hostname: '127.0.0.1', port: 0 }, (req, info) => { \
           const now = `${req.headers.get('x-probe')} ${info.remoteAddr.hostname}`; \
           later = new Promise((r) => setTimeout(() => \
             r(`${req.headers.get('x-probe')} ${info.remoteAddr.hostname} ${info.peerCertificate}`), 20)); \
           return new Response(now); \
         }); \
         const { port } = await server.addr; \
         const res = await fetch(`http://127.0.0.1:${port}/`, { headers: { 'x-probe': 'p1' } }); \
         console.log(await res.text()); \
         console.log(await later); \
         await server.stop();",
    );
    assert_eq!(lines(&out), ["p1 127.0.0.1", "p1 127.0.0.1 null"]);
}

/// A first read after the response was sent throws, naming why, instead of
/// answering with nothing.
#[test]
fn a_first_read_after_the_response_is_a_type_error() {
    let out = esrun(
        "import { serve } from 'runtime:http'; \
         let later; \
         const attempt = (f) => { try { f(); return 'read'; } catch (e) { return `${e.name}: ${e.message}`; } }; \
         const server = serve({ hostname: '127.0.0.1', port: 0 }, (req, info) => { \
           later = new Promise((r) => setTimeout(() => r([ \
             attempt(() => req.headers.get('x-probe')), \
             attempt(() => info.remoteAddr), \
             attempt(() => req.headers), \
           ]), 20)); \
           return new Response('ok'); \
         }); \
         const { port } = await server.addr; \
         await (await fetch(`http://127.0.0.1:${port}/`, { headers: { 'x-probe': 'p1' } })).text(); \
         for (const line of await later) console.log(line); \
         await server.stop();",
    );
    let got = lines(&out);
    assert_eq!(got.len(), 3, "{got:?}");
    for line in &got {
        assert_eq!(
            line,
            "TypeError: request headers and peer are released once the response is sent; \
             read them before the handler returns"
        );
    }
}

/// The head ops take a request id, and an id is not authority: one this agent
/// was never handed is refused as foreign, not answered (D50).
#[test]
fn a_request_id_this_agent_was_not_handed_is_refused() {
    let out = esrun(
        "for (const op of ['http_request_headers', 'http_request_peer']) { \
           try { __ops[op](424242); console.log(op, 'answered'); } \
           catch (e) { console.log(op, e.code); } \
         }",
    );
    assert_eq!(
        lines(&out),
        [
            "http_request_headers ERR_FOREIGN_HANDLE",
            "http_request_peer ERR_FOREIGN_HANDLE"
        ]
    );
}

/// Every header arrives, in order, with repeats kept — the conversion moved,
/// its result did not.
#[test]
fn headers_read_on_demand_are_complete_and_in_order() {
    let out = esrun(
        "import { serve } from 'runtime:http'; \
         const server = serve({ hostname: '127.0.0.1', port: 0 }, (req) => \
           new Response(JSON.stringify([req.headers.get('x-a'), req.headers.get('x-b'), \
             [...req.headers].filter(([n]) => n.startsWith('x-')).length]))); \
         const { port } = await server.addr; \
         const headers = new Headers([['x-a', '1'], ['x-b', '2'], ['x-a', '3']]); \
         console.log(await (await fetch(`http://127.0.0.1:${port}/`, { headers })).text()); \
         await server.stop();",
    );
    assert_eq!(lines(&out), [r#"["1, 3","2",2]"#]);
}

/// The connection info serializes with both fields, as the plain object it
/// replaced did.
#[test]
fn connection_info_serializes_with_both_fields() {
    let out = esrun(
        "import { serve } from 'runtime:http'; \
         const server = serve({ hostname: '127.0.0.1', port: 0 }, (req, info) => \
           new Response(JSON.stringify(info))); \
         const { port } = await server.addr; \
         const seen = JSON.parse(await (await fetch(`http://127.0.0.1:${port}/`)).text()); \
         console.log(Object.keys(seen).join(','), seen.remoteAddr.transport, seen.peerCertificate); \
         await server.stop();",
    );
    assert_eq!(lines(&out), ["remoteAddr,peerCertificate tcp null"]);
}
