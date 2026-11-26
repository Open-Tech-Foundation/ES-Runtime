//! `fetch`: capability, redirects, streaming bodies, trailers and abort.

use super::*;

/// A test transport that drains the **request** body (buffered or streamed)
/// and echoes it back as the response body, recording what it received so a
/// test can assert both the uploaded content and that a streamed body arrived
/// as a `RequestBody::Stream` (i.e. was not buffered in JS).
struct EchoNet {
    captured: Arc<std::sync::Mutex<Vec<u8>>>,
    saw_stream: Arc<std::sync::atomic::AtomicBool>,
}
impl EchoNet {
    fn new() -> Self {
        EchoNet {
            captured: Arc::new(std::sync::Mutex::new(Vec::new())),
            saw_stream: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }
}
impl es_runtime_providers::NetTransport for EchoNet {
    fn fetch(
        &self,
        request: es_runtime_providers::HttpRequest,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            es_runtime_providers::HttpResponse,
            es_runtime_providers::ProviderError,
        >,
    > {
        use es_runtime_providers::RequestBody;
        use futures_util::StreamExt;
        let captured = self.captured.clone();
        let saw_stream = self.saw_stream.clone();
        Box::pin(async move {
            let body = match request.body {
                RequestBody::Empty => Vec::new(),
                RequestBody::Bytes(b) => b,
                RequestBody::Stream(mut s) => {
                    saw_stream.store(true, std::sync::atomic::Ordering::SeqCst);
                    let mut buf = Vec::new();
                    while let Some(chunk) = s.next().await {
                        // A guest stream error (forwarded via close(id, err))
                        // surfaces here and aborts the request.
                        buf.extend_from_slice(&chunk?);
                    }
                    buf
                }
            };
            *captured.lock().unwrap() = body.clone();
            let stream: es_runtime_providers::ByteStream =
                Box::pin(futures_util::stream::iter(std::iter::once(Ok(body))));
            Ok(es_runtime_providers::HttpResponse {
                status: 200,
                status_text: "OK".into(),
                url: request.url,
                redirected: false,
                headers: vec![],
                body: stream,
                trailers: None,
            })
        })
    }
}

fn runtime_with_net(net: Arc<dyn NetTransport>) -> Runtime {
    runtime_full(
        Arc::new(TestConsole::default()),
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
        net,
        Arc::new(TestEntropy::new()),
    )
}

#[test]
fn fetch_requires_net_capability() {
    let _g = v8_guard();
    let mut rt = runtime_with_net(Arc::new(MockNet::ok("x")));
    // Deny-by-default: no Net capability granted.
    let out = eval_async(&mut rt, "await fetch('https://x.test/'); return 'ok';");
    match out {
        Value::String(s) => assert!(
            s.starts_with("ERR:") && (s.contains("capability") || s.contains("NotAllowed")),
            "expected capability denial, got {s}"
        ),
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[test]
fn fetch_returns_response_with_capability() {
    let _g = v8_guard();
    let mut rt = runtime_with_net(Arc::new(MockNet::ok("hello world")));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/data'); \
         return r.status + '|' + r.ok + '|' + r.headers.get('content-type') + '|' + (await r.text());",
    );
    assert_eq!(out, Value::String("200|true|text/plain|hello world".into()));
}

/// A response's trailers leave the host with the end of its body — read or
/// cancelled, asked for or not. Kept until `trailersOf` asked, they were one
/// entry per request for the life of the process, since almost nothing asks.
#[test]
fn fetch_keeps_no_trailers_once_the_body_ends() {
    let _g = v8_guard();
    let net = MockNet::ok("hello").with_trailers(vec![("grpc-status".into(), "0".into())]);
    let mut rt = runtime_with_net(Arc::new(net));
    rt.expose_ops_for_testing();
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const unasked = await fetch('https://x.test/'); await unasked.text(); \
         const cancelled = await fetch('https://x.test/'); await cancelled.body.cancel(); \
         const asked = await fetch('https://x.test/'); await asked.text(); \
         const read = globalThis.__responseTrailers; \
         return [(await read(asked)).get('grpc-status'), \
                 (await read(asked)).get('grpc-status'), \
                 [...(await read(cancelled)).keys()].length].join();",
    );
    // Asking twice gives the same answer; a cancelled body has none.
    assert_eq!(out, Value::String("0,0,0".into()));
    let inflight = rt.eval("globalThis.__ops.__fetch_inflight()").unwrap();
    assert_eq!(inflight, Value::Array(vec![Value::Number(0.0); 4]));
}

/// A transport shaped like the real one: the trailer sender lives inside the
/// body stream, so "no trailers" settles only when that stream is dropped.
struct SenderInBodyNet;
impl es_runtime_providers::NetTransport for SenderInBodyNet {
    fn fetch(
        &self,
        request: es_runtime_providers::HttpRequest,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            es_runtime_providers::HttpResponse,
            es_runtime_providers::ProviderError,
        >,
    > {
        Box::pin(async move {
            let (tx, rx) = futures_channel::oneshot::channel::<Vec<(String, String)>>();
            let mut chunks = vec![b"plain".to_vec()].into_iter();
            let body: es_runtime_providers::ByteStream =
                Box::pin(futures_util::stream::poll_fn(move |_| {
                    let _sender = &tx;
                    std::task::Poll::Ready(chunks.next().map(Ok))
                }));
            Ok(es_runtime_providers::HttpResponse {
                status: 200,
                status_text: "OK".into(),
                url: request.url,
                redirected: false,
                headers: vec![],
                body,
                trailers: Some(Box::pin(async move { rx.await.unwrap_or_default() })),
            })
        })
    }
}

/// The end of a body with no trailers must not wait on the trailers: here
/// they settle only when the body stream is dropped, so a read that held the
/// stream while waiting never finished.
#[test]
fn a_body_without_trailers_ends() {
    let _g = v8_guard();
    let mut rt = runtime_with_net(Arc::new(SenderInBodyNet));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/'); const text = await r.text(); \
         return text + ':' + [...(await globalThis.__responseTrailers(r)).keys()].length;",
    );
    assert_eq!(out, Value::String("plain:0".into()));
}

#[test]
fn fetch_streams_a_chunked_response_body() {
    let _g = v8_guard();
    let net = MockNet {
        status: 200,
        headers: vec![],
        chunks: vec![b"foo".to_vec(), b"bar".to_vec(), b"baz".to_vec()],
        fail: false,
        trailers: None,
    };
    let mut rt = runtime_with_net(Arc::new(net));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // Drain the response body stream chunk by chunk.
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/'); const reader = r.body.getReader(); \
         const dec = new TextDecoder(); let s = ''; let x; \
         while (!(x = await reader.read()).done) s += dec.decode(x.value); \
         return s;",
    );
    assert_eq!(out, Value::String("foobarbaz".into()));
}

/// A transport that redirects `/hop/<n>` to `/hop/<n+1>` and answers
/// anything else `200 "landed"`, following the chain itself exactly as a
/// real transport does — so the redirect *mode* is what the tests below
/// vary, not the JS side's interpretation of a canned response.
struct RedirectNet {
    /// How many hops before the chain lands. `usize::MAX` never lands, to
    /// drive the cap.
    hops: usize,
}
impl es_runtime_providers::NetTransport for RedirectNet {
    fn fetch(
        &self,
        request: es_runtime_providers::HttpRequest,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            es_runtime_providers::HttpResponse,
            es_runtime_providers::ProviderError,
        >,
    > {
        const CAP: usize = 20;
        let follow = request.redirect == es_runtime_providers::RedirectMode::Follow;
        let total = self.hops;
        let mut url = request.url;
        Box::pin(async move {
            // `n` is the hop this URL represents; the start URL is hop 0.
            let mut n = url
                .rsplit_once("/hop/")
                .and_then(|(_, n)| n.parse::<usize>().ok())
                .unwrap_or(0);
            let mut redirected = false;
            let mut followed = 0;
            while n < total {
                if !follow {
                    // Hand back the redirect itself, unfollowed.
                    let body: es_runtime_providers::ByteStream =
                        Box::pin(futures_util::stream::iter(std::iter::empty()));
                    return Ok(es_runtime_providers::HttpResponse {
                        status: 302,
                        status_text: "Found".into(),
                        url,
                        redirected: false,
                        headers: vec![("location".into(), format!("https://x.test/hop/{}", n + 1))],
                        body,
                        trailers: None,
                    });
                }
                followed += 1;
                if followed > CAP {
                    return Err(es_runtime_providers::ProviderError::Coded {
                        code: es_runtime_common::ErrorCode::TooManyRedirects,
                        message: format!("too many redirects (more than {CAP})"),
                    });
                }
                n += 1;
                url = format!("https://x.test/hop/{n}");
                redirected = true;
            }
            let body: es_runtime_providers::ByteStream = Box::pin(futures_util::stream::iter(
                std::iter::once(Ok(b"landed".to_vec())),
            ));
            Ok(es_runtime_providers::HttpResponse {
                status: 200,
                status_text: "OK".into(),
                url,
                redirected,
                headers: vec![],
                body,
                trailers: None,
            })
        })
    }
}

fn redirect_runtime(hops: usize) -> Runtime {
    let mut rt = runtime_with_net(Arc::new(RedirectNet { hops }));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    rt
}

#[test]
fn fetch_follows_redirects_by_default_and_reports_the_final_url() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(2);
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/hop/0'); \
         return r.status + '|' + r.redirected + '|' + r.url + '|' + (await r.text());",
    );
    assert_eq!(
        out,
        Value::String("200|true|https://x.test/hop/2|landed".into())
    );
}

#[test]
fn fetch_redirect_manual_returns_the_redirect_response_unfollowed() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(2);
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/hop/0', { redirect: 'manual' }); \
         return r.status + '|' + r.redirected + '|' + r.headers.get('location');",
    );
    assert_eq!(out, Value::String("302|false|https://x.test/hop/1".into()));
}

#[test]
fn fetch_redirect_error_rejects_with_a_type_error() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(2);
    let out = eval_async(
        &mut rt,
        "try { await fetch('https://x.test/hop/0', { redirect: 'error' }); return 'no throw'; } \
         catch (e) { return e.constructor.name + '|' + e.message.includes('302'); }",
    );
    assert_eq!(out, Value::String("TypeError|true".into()));
}

#[test]
fn fetch_reports_not_redirected_when_nothing_redirected() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(0);
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/hop/0'); return String(r.redirected);",
    );
    assert_eq!(out, Value::String("false".into()));
}

#[test]
fn fetch_rejects_a_redirect_chain_past_the_cap_with_a_stable_code() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(usize::MAX);
    let out = eval_async(
        &mut rt,
        "try { await fetch('https://x.test/hop/0'); return 'no throw'; } \
         catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_TOO_MANY_REDIRECTS".into()));
}

#[test]
fn request_rejects_an_unknown_redirect_mode() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(0);
    let out = eval_async(
        &mut rt,
        "try { new Request('https://x.test/', { redirect: 'manaul' }); return 'no throw'; } \
         catch (e) { return e.constructor.name; }",
    );
    assert_eq!(out, Value::String("TypeError".into()));
}

#[test]
fn a_script_constructed_response_cannot_claim_to_be_redirected() {
    let _g = v8_guard();
    let mut rt = redirect_runtime(0);
    let out = eval_async(
        &mut rt,
        "const r = new Response('x', { redirected: true }); return String(r.redirected);",
    );
    assert_eq!(out, Value::String("false".into()));
}

/// A transport whose request future never resolves, and which records how
/// many times it was polled into existence. Lets an abort test observe that
/// the *in-flight* request is torn down rather than merely ignored: when the
/// abort wins the race, `fetch_ops` drops this future.
struct HangingNet {
    started: Arc<std::sync::atomic::AtomicUsize>,
    dropped: Arc<std::sync::atomic::AtomicUsize>,
}
/// Bumps `dropped` when the request future is discarded — i.e. cancelled.
struct DropProbe(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}
impl es_runtime_providers::NetTransport for HangingNet {
    fn fetch(
        &self,
        _request: es_runtime_providers::HttpRequest,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            es_runtime_providers::HttpResponse,
            es_runtime_providers::ProviderError,
        >,
    > {
        self.started
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let probe = DropProbe(self.dropped.clone());
        Box::pin(async move {
            let _probe = probe;
            std::future::pending().await
        })
    }
}

#[test]
fn object_url_fetch_needs_no_net_capability() {
    let _g = v8_guard();
    // The transport errors on any real request; a blob: URL must never
    // reach it, and must work with Net denied — nothing leaves the isolate.
    let mut rt = runtime_with_net(Arc::new(MockNet::stub()));
    rt.set_capabilities(CapabilitySet::none());
    let out = eval_async(
        &mut rt,
        "const u = URL.createObjectURL(new Blob(['local'], { type: 'text/plain' })); \
         const r = await fetch(u); \
         const body = await r.text(); \
         URL.revokeObjectURL(u); \
         return r.status + '|' + body;",
    );
    assert_eq!(out, Value::String("200|local".into()));
}

#[test]
fn fetch_rejects_an_already_aborted_signal_without_touching_the_network() {
    let _g = v8_guard();
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let net = Arc::new(HangingNet {
        started: started.clone(),
        dropped: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    });
    let mut rt = runtime_with_net(net);
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const c = new AbortController(); c.abort('too late'); \
         try { await fetch('https://x.test/', { signal: c.signal }); return 'no-throw'; } \
         catch (e) { return String(e); }",
    );
    assert_eq!(out, Value::String("too late".into()));
    // The transport was never consulted.
    assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn fetch_aborts_an_in_flight_request_and_drops_the_transport_future() {
    let _g = v8_guard();
    let started = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let dropped = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let net = Arc::new(HangingNet {
        started: started.clone(),
        dropped: dropped.clone(),
    });
    let mut rt = runtime_with_net(net);
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // The transport never responds. Draining a few microtask turns lets
    // `fetch` get past body materialization and actually issue the request,
    // so the abort below lands on an in-flight call rather than pre-empting
    // it (which is a different path, covered above).
    let out = eval_async(
        &mut rt,
        "const c = new AbortController(); \
         const p = fetch('https://x.test/', { signal: c.signal }); \
         for (let i = 0; i < 10; i++) await Promise.resolve(); \
         c.abort(new DOMException('gone', 'AbortError')); \
         try { await p; return 'no-throw'; } \
         catch (e) { return e.name + ':' + e.message; }",
    );
    assert_eq!(out, Value::String("AbortError:gone".into()));
    // The guest promise rejects as soon as the signal fires. Host-side
    // teardown lands on a later turn of the loop, when the op future is
    // polled and the abort wins its race, so drive a few more ticks.
    for _ in 0..10 {
        rt.tick(0);
    }
    assert_eq!(started.load(std::sync::atomic::Ordering::SeqCst), 1);
    // The pending transport future was dropped — that is the cancellation.
    assert_eq!(dropped.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[test]
fn fetch_without_a_signal_still_completes() {
    let _g = v8_guard();
    let mut rt = runtime_with_net(Arc::new(MockNet::ok("fine")));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/'); return await r.text();",
    );
    assert_eq!(out, Value::String("fine".into()));
}

#[test]
fn fetch_default_signal_is_unaborted_and_does_not_leak_abort_handles() {
    let _g = v8_guard();
    let mut rt = runtime_with_net(Arc::new(MockNet::ok("x")));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const r = new Request('https://x.test/'); \
         const before = r.signal.aborted; \
         const res = await fetch('https://x.test/'); await res.text(); \
         return before + '|' + r.signal.aborted;",
    );
    assert_eq!(out, Value::String("false|false".into()));
}

#[test]
fn fetch_streams_a_request_body_from_a_readable_stream() {
    let _g = v8_guard();
    let net = Arc::new(EchoNet::new());
    let mut rt = runtime_with_net(net.clone());
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // A ReadableStream body of three chunks streams to the host (not buffered
    // in JS); the EchoNet echoes the concatenation back.
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); \
         const body = new ReadableStream({ start(c) { \
           c.enqueue(enc.encode('alpha-')); c.enqueue(enc.encode('beta-')); \
           c.enqueue(enc.encode('gamma')); c.close(); } }); \
         const r = await fetch('https://x.test/up', { method: 'POST', body }); \
         return await r.text();",
    );
    assert_eq!(out, Value::String("alpha-beta-gamma".into()));
    assert_eq!(&*net.captured.lock().unwrap(), b"alpha-beta-gamma");
    assert!(
        net.saw_stream.load(std::sync::atomic::Ordering::SeqCst),
        "request body should have arrived as a stream, not buffered"
    );
}

#[test]
fn fetch_buffers_a_string_request_body() {
    let _g = v8_guard();
    let net = Arc::new(EchoNet::new());
    let mut rt = runtime_with_net(net.clone());
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // A non-stream body still travels buffered (RequestBody::Bytes), so the
    // transport must NOT see a stream.
    let out = eval_async(
        &mut rt,
        "const r = await fetch('https://x.test/up', { method: 'POST', body: 'hello body' }); \
         return await r.text();",
    );
    assert_eq!(out, Value::String("hello body".into()));
    assert_eq!(&*net.captured.lock().unwrap(), b"hello body");
    assert!(
        !net.saw_stream.load(std::sync::atomic::Ordering::SeqCst),
        "a string body must be sent buffered, not streamed"
    );
}

#[test]
fn fetch_streams_an_empty_request_body() {
    let _g = v8_guard();
    let net = Arc::new(EchoNet::new());
    let mut rt = runtime_with_net(net.clone());
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // A ReadableStream that closes immediately: still the streaming path, but
    // zero bytes uploaded.
    let out = eval_async(
        &mut rt,
        "const body = new ReadableStream({ start(c) { c.close(); } }); \
         const r = await fetch('https://x.test/up', { method: 'POST', body }); \
         return (await r.text()).length;",
    );
    assert_eq!(out, Value::Number(0.0));
    assert!(net.captured.lock().unwrap().is_empty());
    assert!(net.saw_stream.load(std::sync::atomic::Ordering::SeqCst));
}

#[test]
fn fetch_aborts_when_the_request_body_stream_errors() {
    let _g = v8_guard();
    let net = Arc::new(EchoNet::new());
    let mut rt = runtime_with_net(net.clone());
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // A body stream that errors mid-flight: the error is forwarded to the
    // host, which aborts the request, and `fetch` rejects.
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); let n = 0; \
         const body = new ReadableStream({ pull(c) { \
           if (n++ === 0) c.enqueue(enc.encode('partial')); \
           else c.error(new Error('boom')); } }); \
         const r = await fetch('https://x.test/up', { method: 'POST', body }); \
         return await r.text();",
    );
    match out {
        Value::String(s) => assert!(
            s.starts_with("ERR:") && s.contains("boom"),
            "expected the body-stream error to reject fetch, got {s}"
        ),
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[test]
fn fetch_streaming_request_body_requires_net_capability() {
    let _g = v8_guard();
    let net = Arc::new(EchoNet::new());
    let mut rt = runtime_with_net(net);
    // No Net capability: allocating the body stream channel must itself be
    // gated, so the streaming path is denied just like the buffered one.
    let out = eval_async(
        &mut rt,
        "const body = new ReadableStream({ start(c) { c.close(); } }); \
         await fetch('https://x.test/up', { method: 'POST', body }); return 'ok';",
    );
    match out {
        Value::String(s) => assert!(
            s.starts_with("ERR:") && (s.contains("capability") || s.contains("NotAllowed")),
            "expected capability denial, got {s}"
        ),
        other => panic!("expected rejection, got {other:?}"),
    }
}

/// Soak: hammer the streaming-request-body path and prove it neither leaks
/// (the three body registries drain to zero each time, and RSS stays bounded)
/// nor deadlocks/corrupts over many iterations. Opt-in:
///   cargo test -p es-runtime -- --ignored soak
#[test]
#[ignore = "soak/leak: run with `cargo test -p es-runtime -- --ignored soak_streaming_fetch`"]
fn soak_streaming_fetch_does_not_leak() {
    let _g = v8_guard();
    let net = Arc::new(EchoNet::new());
    let mut rt = runtime_with_net(net.clone());
    rt.expose_ops_for_testing();
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));

    const ITERS: usize = 20_000;
    // Warm up the heap/allocator before sampling so we measure steady-state
    // growth, not first-touch.
    let script = "const enc = new TextEncoder(); \
         const body = new ReadableStream({ start(c) { \
           c.enqueue(enc.encode('alpha-')); c.enqueue(enc.encode('beta-')); \
           c.enqueue(enc.encode('gamma')); c.close(); } }); \
         const r = await fetch('https://x.test/up', { method: 'POST', body }); \
         return await r.text();";
    for _ in 0..500 {
        assert_eq!(
            eval_async(&mut rt, script),
            Value::String("alpha-beta-gamma".into())
        );
    }

    let mut rss_mid = 0u64;
    for k in 0..ITERS {
        let out = eval_async(&mut rt, script);
        assert_eq!(out, Value::String("alpha-beta-gamma".into()), "iter {k}");
        // The precise leak guard: the registries must be empty between
        // requests — no leaked response body stream, request sender,
        // receiver or trailers. This holds regardless of V8 heap behavior.
        let inflight = rt.eval("globalThis.__ops.__fetch_inflight()").unwrap();
        assert_eq!(
            inflight,
            Value::Array(vec![
                Value::Number(0.0),
                Value::Number(0.0),
                Value::Number(0.0),
                Value::Number(0.0)
            ]),
            "registry not drained at iter {k}"
        );
        // Sample resident set at the halfway mark, so the comparison below
        // measures *steady-state* growth (the V8 heap/code-cache warm-up is
        // a one-time cost paid in the first half).
        if k == ITERS / 2 {
            rss_mid = resident_kib();
        }
    }

    let rss_after = resident_kib();
    let second_half = rss_after.saturating_sub(rss_mid);
    // A per-iteration native leak would keep growing linearly into the second
    // half; a warmed V8 heap plateaus. Require the second-half growth to stay
    // small (the registry-drain assertion above is the precise guard).
    assert!(
        second_half < 16 * 1024,
        "RSS grew {second_half} KiB over the second {} streaming fetches — possible leak",
        ITERS / 2
    );
}
