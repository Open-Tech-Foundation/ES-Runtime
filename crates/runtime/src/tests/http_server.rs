//! `runtime:http` `serve()`: options as they cross to the provider.

use super::*;

/// An [`HttpServerProvider`] that records what `serve` was asked for and
/// binds nothing. The ALPN list a guest's `serve()` produces is a documented
/// default and part of what a client negotiates against, so it is pinned
/// here — at the layer that decides it — rather than inferred from a
/// handshake.
#[derive(Default)]
struct RecordingHttpServer {
    options: Arc<Mutex<Vec<es_runtime_providers::HttpServeOptions>>>,
    /// Requests to hand over, one batch, before reporting the server
    /// closed. Lets a test drive a handler without a socket — which is the
    /// only way to reach the "the host has no peer to report" branch, since
    /// a real connection always has one.
    deliver: Arc<Mutex<Vec<es_runtime_providers::HttpServerRequest>>>,
    /// What the handler answered, in order.
    responses: Arc<Mutex<Vec<es_runtime_providers::HttpServerResponse>>>,
}

/// A request with no body, as a provider with no socket would report it.
fn canned_request(
    remote_address: &str,
    remote_port: u16,
) -> es_runtime_providers::HttpServerRequest {
    es_runtime_providers::HttpServerRequest {
        method: "GET".to_string(),
        url: "http://127.0.0.1:8443/who".to_string(),
        headers: Vec::new().into(),
        body: es_runtime_providers::HttpServerBody::Empty,
        remote_address: remote_address.to_string(),
        remote_port,
        peer_certificate: None,
    }
}

impl es_runtime_providers::HttpServerProvider for RecordingHttpServer {
    fn serve(
        &self,
        options: es_runtime_providers::HttpServeOptions,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            (u64, es_runtime_providers::SocketInfo),
            es_runtime_providers::ProviderError,
        >,
    > {
        let info = es_runtime_providers::SocketInfo {
            remote_address: String::new(),
            remote_port: 0,
            local_address: options.host.clone(),
            local_port: 8443,
            alpn: None,
            peer_certificate: None,
        };
        self.options.lock().unwrap().push(options);
        Box::pin(std::future::ready(Ok((1, info))))
    }

    fn next_requests(
        &self,
        _id: u64,
        _max: usize,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            Vec<(u64, es_runtime_providers::HttpServerRequest)>,
            es_runtime_providers::ProviderError,
        >,
    > {
        // Empty ⇒ "the server is closed", which ends the accept loop rather
        // than holding the module open on a server nothing will call. A
        // queued batch goes over first, and is only handed out once.
        let queued: Vec<_> = std::mem::take(&mut *self.deliver.lock().unwrap())
            .into_iter()
            .enumerate()
            .map(|(i, req)| (i as u64 + 1, req))
            .collect();
        Box::pin(std::future::ready(Ok(queued)))
    }

    fn respond(
        &self,
        _request_id: u64,
        response: es_runtime_providers::HttpServerResponse,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        self.responses.lock().unwrap().push(response);
        Box::pin(std::future::ready(Ok(())))
    }

    fn close(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        Box::pin(std::future::ready(Ok(())))
    }
}

fn http_runtime(http: Arc<RecordingHttpServer>) -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    let providers = HostProviders::new(
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
        Arc::new(TestConsole::default()),
        Arc::new(MockNet::stub()),
        Arc::new(TestEntropy::new()),
    )
    .with_http_server(http);
    Runtime::new(Box::new(engine), providers).expect("runtime")
}

/// The TLS ALPN list a `serve()` with no `alpn` advertises. Both versions,
/// h2 first — ALPN order is the server's preference, so this is what decides
/// that an h2-capable client gets HTTP/2.
#[test]
fn serve_advertises_h2_then_http11_by_default() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ hostname: '127.0.0.1', port: 8443, secureTransport: 'on', \
                           cert: 'PEM-CERT', key: 'PEM-KEY' }, () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    let tls = options[0].tls.as_ref().expect("secureTransport: on");
    assert_eq!(tls.alpn, vec!["h2".to_string(), "http/1.1".to_string()]);
}

/// …and naming `alpn` replaces that list rather than adding to it, which is
/// what pins a listener to one version for a client that mishandles h2.
#[test]
fn an_explicit_alpn_replaces_the_default() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8443, secureTransport: 'on', cert: 'C', key: 'K', \
                           alpn: ['http/1.1'] }, () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    let tls = options[0].tls.as_ref().expect("secureTransport: on");
    assert_eq!(tls.alpn, vec!["http/1.1".to_string()]);
}

/// A cleartext `serve()` carries no TLS at all — h2c is decided by the
/// connection preface on the wire, so there is nothing for ALPN to say here.
#[test]
fn a_cleartext_serve_sends_no_tls_options() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080 }, () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    assert!(http.options.lock().unwrap()[0].tls.is_none());
}

/// A guest that says nothing about timeouts gets the provider's defaults —
/// the point of them. The prelude must send "unset" rather than a copy of
/// the numbers, so this asserts the values arrive equal to
/// `HttpTimeouts::default()` rather than to any literal written here.
#[test]
fn serve_without_timeouts_uses_the_provider_defaults() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080 }, () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    assert_eq!(
        http.options.lock().unwrap()[0].timeouts,
        es_runtime_providers::HttpTimeouts::default()
    );
}

#[test]
fn each_timeout_can_be_set_and_they_cross_as_milliseconds() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080, timeouts: { handshake: 1000, headerRead: 2000, \
                           h2KeepAlive: 3000 } }, () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    assert_eq!(
        options[0].timeouts,
        es_runtime_providers::HttpTimeouts {
            handshake: Some(std::time::Duration::from_secs(1)),
            header_read: Some(std::time::Duration::from_secs(2)),
            h2_keep_alive: Some(std::time::Duration::from_secs(3)),
            ..es_runtime_providers::HttpTimeouts::default()
        }
    );
}

/// The body bound is two numbers that mean different things — a duration
/// and a rate — so both have to arrive as themselves.
#[test]
fn the_body_bound_crosses_as_a_grace_and_a_rate() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080, timeouts: { bodyRead: 5000, bodyMinRate: 4096 } }, \
                         () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    assert_eq!(
        options[0].timeouts.body_read,
        Some(std::time::Duration::from_secs(5))
    );
    assert_eq!(options[0].timeouts.body_min_rate, 4096);
}

/// `0` is a rate, not a disabled timeout: it says a slow body earns no
/// extension, which turns the grace into a flat cap. Reading it as "unset"
/// would hand back the default allowance instead — the opposite of what
/// was asked for.
#[test]
fn a_zero_body_rate_is_a_rate_and_not_an_omission() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080, timeouts: { bodyMinRate: 0 } }, \
                         () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let defaults = es_runtime_providers::HttpTimeouts::default();
    let options = http.options.lock().unwrap();
    assert_eq!(options[0].timeouts.body_min_rate, 0);
    // And naming the rate alone leaves the grace where it was.
    assert_eq!(options[0].timeouts.body_read, defaults.body_read);
}

/// `null` is off, and it has to survive the crossing as off rather than as
/// "unset" — otherwise a guest asking for no timeout would silently get the
/// default instead, which is the one mistake here that fails safe-looking.
#[test]
fn a_null_timeout_disables_it_rather_than_falling_back_to_the_default() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080, timeouts: { handshake: null, headerRead: null, \
                           h2KeepAlive: null } }, () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    assert_eq!(options[0].timeouts.handshake, None);
    assert_eq!(options[0].timeouts.header_read, None);
    assert_eq!(options[0].timeouts.h2_keep_alive, None);
}

/// The body bound is removed by `bodyRead: null`, the same spelling as the
/// rest — the rate has nothing to extend once there is no grace.
#[test]
fn a_null_body_read_removes_the_bound_entirely() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080, timeouts: { bodyRead: null } }, \
                         () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    assert_eq!(http.options.lock().unwrap()[0].timeouts.body_read, None);
}

/// One timeout named, the rest defaulted — the common case, and the one a
/// naive "read the object" implementation gets wrong by zeroing the others.
#[test]
fn naming_one_timeout_leaves_the_others_at_their_defaults() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const s = serve({ port: 8080, timeouts: { headerRead: 5000 } }, \
                         () => new Response('x')); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let defaults = es_runtime_providers::HttpTimeouts::default();
    let options = http.options.lock().unwrap();
    assert_eq!(
        options[0].timeouts.header_read,
        Some(std::time::Duration::from_secs(5))
    );
    assert_eq!(options[0].timeouts.handshake, defaults.handshake);
    assert_eq!(options[0].timeouts.h2_keep_alive, defaults.h2_keep_alive);
    assert_eq!(options[0].timeouts.body_read, defaults.body_read);
    assert_eq!(options[0].timeouts.body_min_rate, defaults.body_min_rate);
}

/// Body of the single response the handler produced.
fn recorded_body(http: &RecordingHttpServer) -> String {
    let responses = http.responses.lock().unwrap();
    let one = responses.first().expect("the handler answered");
    match &one.body {
        es_runtime_providers::HttpServerBody::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => panic!("expected a buffered body"),
    }
}

/// The handler's second argument carries the peer, in the shape
/// `Deno.serve` passes — so the same handler runs on either runtime.
#[test]
fn a_handler_is_told_which_peer_a_request_came_from() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    http.deliver
        .lock()
        .unwrap()
        .push(canned_request("203.0.113.7", 54321));
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         serve({ port: 8443 }, (request, info) => \
           new Response(`${info.remoteAddr.transport}/${info.remoteAddr.hostname}/\
${info.remoteAddr.port}`));",
        MapLoader::new(&[]),
    );
    assert_eq!(recorded_body(&http), "tcp/203.0.113.7/54321");
}

/// A provider with no socket — a mock, an embedder's own transport — has no
/// peer to report, and saying so is `null`. An address-shaped object full of
/// blanks would be worse than useless: a handler would happily key a rate
/// limit on the empty string.
#[test]
fn an_unknown_peer_is_null_rather_than_a_blank_address() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    http.deliver.lock().unwrap().push(canned_request("", 0));
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         serve({ port: 8443 }, (request, info) => \
           new Response(String(info.remoteAddr)));",
        MapLoader::new(&[]),
    );
    assert_eq!(recorded_body(&http), "null");
}

/// The argument is additive: every handler written before it exists takes
/// one parameter and must keep working untouched.
#[test]
fn a_handler_that_ignores_the_second_argument_still_works() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    http.deliver
        .lock()
        .unwrap()
        .push(canned_request("203.0.113.7", 54321));
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         serve({ port: 8443 }, (request) => new Response(new URL(request.url).pathname));",
        MapLoader::new(&[]),
    );
    assert_eq!(recorded_body(&http), "/who");
}

/// No cap unless one is asked for: the right number follows from a
/// deployment's descriptor budget, and one guessed here would throttle real
/// traffic with nothing to explain it.
#[test]
fn serve_is_uncapped_unless_max_connections_says_otherwise() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const a = serve({ port: 8080 }, () => new Response('x')); \
         const b = serve({ port: 8081, maxConnections: 512 }, () => new Response('x')); \
         globalThis.result = (await a.addr).port + (await b.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    assert_eq!(options[0].max_connections, None);
    assert_eq!(options[1].max_connections, Some(512));
    // The per-peer half is separately off: a whole-server cap says nothing
    // about whose connections fill it, and asking for one must not imply
    // the other.
    assert_eq!(options[0].max_connections_per_ip, None);
    assert_eq!(options[1].max_connections_per_ip, None);
}

/// The two caps are independent numbers that answer different questions —
/// how much the deployment spends, and whose connections it spends it on.
#[test]
fn the_per_peer_cap_crosses_independently_of_the_whole_server_one() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const a = serve({ port: 8080, maxConnectionsPerIp: 4 }, () => new Response('x')); \
         const b = serve({ port: 8081, maxConnections: 512, maxConnectionsPerIp: 8 }, \
                         () => new Response('x')); \
         globalThis.result = (await a.addr).port + (await b.addr).port;",
        MapLoader::new(&[]),
    );
    let options = http.options.lock().unwrap();
    assert_eq!(options[0].max_connections, None);
    assert_eq!(options[0].max_connections_per_ip, Some(4));
    assert_eq!(options[1].max_connections, Some(512));
    assert_eq!(options[1].max_connections_per_ip, Some(8));
}

/// A cap of zero would serve nothing at all, and a fractional one is a
/// mistake rather than an intention — both are rejected at the call.
#[test]
fn an_unusable_connection_cap_is_rejected_before_the_port_is_bound() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const names = []; \
         for (const bad of ['lots', 0, -1, 1.5]) { \
           try { serve({ port: 8080, maxConnections: bad }, () => new Response('x')); } \
           catch (e) { names.push(e.constructor.name); } \
           try { serve({ port: 8080, maxConnectionsPerIp: bad }, () => new Response('x')); } \
           catch (e) { names.push(e.constructor.name); } \
         } \
         globalThis.result = names.join(',');",
        MapLoader::new(&[]),
    );
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String(
            "TypeError,TypeError,RangeError,RangeError,RangeError,RangeError,RangeError,\
             RangeError"
                .to_string()
        )
    );
    assert!(
        http.options.lock().unwrap().is_empty(),
        "a rejected option must not have reached the provider"
    );
}

/// Rejected in the prelude, before the bind: a typo in a timeout must not
/// claim a port and then serve with a silently different policy.
#[test]
fn a_bad_timeout_is_rejected_before_the_port_is_bound() {
    let _g = v8_guard();
    let http = Arc::new(RecordingHttpServer::default());
    let mut rt = http_runtime(http.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:http'; \
         const bad = [['handshake', 'soon'], ['headerRead', -1], ['h2KeepAlive', NaN], \
                      ['bodyRead', 'later'], ['bodyMinRate', -1], ['bodyMinRate', 'fast']]; \
         const names = []; \
         for (const [key, value] of bad) { \
           try { serve({ port: 8080, timeouts: { [key]: value } }, () => new Response('x')); } \
           catch (e) { names.push(e.constructor.name); } \
         } \
         try { serve({ port: 8080, timeouts: 5 }, () => new Response('x')); } \
         catch (e) { names.push(e.constructor.name); } \
         globalThis.result = names.join(',');",
        MapLoader::new(&[]),
    );
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String(
            "TypeError,RangeError,RangeError,TypeError,RangeError,TypeError,TypeError".to_string()
        )
    );
    assert!(
        http.options.lock().unwrap().is_empty(),
        "a rejected option must not have reached the provider"
    );
}
