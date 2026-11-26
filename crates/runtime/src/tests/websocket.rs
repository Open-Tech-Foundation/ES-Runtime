//! WebSocket and WebSocketStream clients, and `runtime:websocket` servers.

use super::*;

/// A scripted WebSocketProvider for the `WebSocket` global (DECISIONS D29):
/// `connect` hands back a fixed protocol; `recv` replays a pre-seeded frame
/// sequence (each future Ready on first poll, so no waker is needed under the
/// tick driver); `send`/`close` are no-ops.
struct MockWs {
    inbound: std::sync::Mutex<std::collections::VecDeque<es_runtime_providers::WsIncoming>>,
    protocol: String,
    /// Every `serve()` this provider was asked for, in order. What the
    /// prelude sent is the thing under test for the server options — the
    /// bind itself is the provider's business.
    served: std::sync::Mutex<Vec<es_runtime_providers::WsServeOptions>>,
    /// Connection ids `accept` still has to hand out before reporting the
    /// server closed. Lets a test get real `WebSocketConnection` objects,
    /// which is what `broadcast` takes.
    pending_accepts: std::sync::Mutex<std::collections::VecDeque<u64>>,
    /// The id lists `broadcast` was called with, in order.
    broadcasts: std::sync::Mutex<Vec<Vec<u64>>>,
    /// With no frame queued, `recv` waits instead of reporting the end:
    /// connections that stay open, for tests about live ones.
    held_open: bool,
}
impl MockWs {
    fn new(frames: Vec<es_runtime_providers::WsIncoming>, protocol: &str) -> Self {
        MockWs {
            inbound: std::sync::Mutex::new(frames.into()),
            protocol: protocol.to_string(),
            served: std::sync::Mutex::new(Vec::new()),
            pending_accepts: std::sync::Mutex::new(std::collections::VecDeque::new()),
            broadcasts: std::sync::Mutex::new(Vec::new()),
            held_open: false,
        }
    }

    /// Queues `n` connections for `accept` to yield before it reports the
    /// server closed.
    fn accepting(self, n: u64) -> Self {
        *self.pending_accepts.lock().unwrap() = (1..=n).collect();
        self
    }

    fn held_open(self) -> Self {
        MockWs {
            held_open: true,
            ..self
        }
    }
}
impl es_runtime_providers::WebSocketProvider for MockWs {
    fn connect(
        &self,
        _url: String,
        _protocols: Vec<String>,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            (u64, es_runtime_providers::WebSocketInfo),
            es_runtime_providers::ProviderError,
        >,
    > {
        let protocol = self.protocol.clone();
        Box::pin(async move {
            Ok((
                1u64,
                es_runtime_providers::WebSocketInfo {
                    protocol,
                    extensions: String::new(),
                },
            ))
        })
    }
    fn send(
        &self,
        _id: u64,
        _message: es_runtime_providers::WsMessage,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        Box::pin(async { Ok(()) })
    }
    fn broadcast(
        &self,
        ids: Vec<u64>,
        _message: es_runtime_providers::WsMessage,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        self.broadcasts.lock().unwrap().push(ids);
        Box::pin(async { Ok(()) })
    }
    fn recv(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            Option<es_runtime_providers::WsIncoming>,
            es_runtime_providers::ProviderError,
        >,
    > {
        let item = self.inbound.lock().unwrap().pop_front();
        if item.is_none() && self.held_open {
            return Box::pin(std::future::pending());
        }
        Box::pin(async move { Ok(item) })
    }
    fn close(
        &self,
        _id: u64,
        _code: Option<u16>,
        _reason: String,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        Box::pin(async { Ok(()) })
    }
    fn serve(
        &self,
        options: es_runtime_providers::WsServeOptions,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            (u64, es_runtime_providers::SocketInfo),
            es_runtime_providers::ProviderError,
        >,
    > {
        let port = options.port;
        self.served.lock().unwrap().push(options);
        Box::pin(async move {
            Ok((
                1u64,
                es_runtime_providers::SocketInfo {
                    local_address: "127.0.0.1".to_string(),
                    local_port: port,
                    ..Default::default()
                },
            ))
        })
    }
    fn accept(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<
        std::result::Result<
            Option<(u64, es_runtime_providers::WebSocketInfo)>,
            es_runtime_providers::ProviderError,
        >,
    > {
        let next = self.pending_accepts.lock().unwrap().pop_front();
        let protocol = self.protocol.clone();
        Box::pin(async move {
            Ok(next.map(|id| {
                (
                    id,
                    es_runtime_providers::WebSocketInfo {
                        protocol,
                        extensions: String::new(),
                    },
                )
            }))
        })
    }
    fn close_server(
        &self,
        _id: u64,
    ) -> es_runtime_providers::BoxFuture<std::result::Result<(), es_runtime_providers::ProviderError>>
    {
        Box::pin(async { Ok(()) })
    }
}

fn runtime_with_ws(ws: Arc<dyn es_runtime_providers::WebSocketProvider>) -> Runtime {
    let engine = V8Engine::new(Limits::default()).expect("engine");
    Runtime::new(
        Box::new(engine),
        HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_web_socket(ws),
    )
    .expect("runtime")
}

#[test]
fn websocket_open_message_close_round_trip() {
    use es_runtime_providers::WsIncoming;
    let _g = v8_guard();
    let ws = MockWs::new(
        vec![
            WsIncoming::Text("hello".into()),
            WsIncoming::Binary(vec![1, 2, 3]),
            WsIncoming::Close {
                code: 1000,
                reason: "bye".into(),
            },
        ],
        "chat",
    );
    let mut rt = runtime_with_ws(Arc::new(ws));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const log = []; \
         const ws = new WebSocket('ws://echo.test/sub', 'chat'); \
         ws.binaryType = 'arraybuffer'; \
         await new Promise((resolve) => { \
           ws.addEventListener('open', () => { log.push('open:' + ws.readyState + ':' + ws.protocol); ws.send('ping'); }); \
           ws.addEventListener('message', (e) => { \
             if (typeof e.data === 'string') log.push('txt:' + e.data + ':' + e.origin); \
             else log.push('bin:' + new Uint8Array(e.data).join(',')); \
           }); \
           ws.addEventListener('close', (e) => { log.push('close:' + e.code + ':' + e.reason + ':' + e.wasClean + ':' + ws.readyState); resolve(); }); \
           ws.addEventListener('error', () => { log.push('error'); resolve(); }); \
         }); \
         return log.join('|');",
    );
    assert_eq!(
        out,
        Value::String(
            "open:1:chat|txt:hello:ws://echo.test|bin:1,2,3|close:1000:bye:true:3".into()
        )
    );
}

#[test]
fn websocket_validates_url_state_and_close_args() {
    let _g = v8_guard();
    let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    // All synchronous (no tick): the constructor's connect stays pending.
    assert_true(
        &mut rt,
        "(() => { const errs = []; \
         try { new WebSocket('http://x/'); } catch (e) { errs.push(e.name); } \
         try { new WebSocket('ws://x/#f'); } catch (e) { errs.push(e.name); } \
         try { new WebSocket('ws://x/', ['a', 'a']); } catch (e) { errs.push(e.name); } \
         const ws = new WebSocket('ws://x/'); \
         try { ws.send('x'); } catch (e) { errs.push(e.name); } \
         try { ws.close(1234); } catch (e) { errs.push(e.name); } \
         try { ws.close(1000, 'x'.repeat(200)); } catch (e) { errs.push(e.name); } \
         return errs.join(',') === \
           'SyntaxError,SyntaxError,SyntaxError,InvalidStateError,InvalidAccessError,SyntaxError'; })()",
    );
}

#[test]
fn websocket_connect_requires_net_capability() {
    let _g = v8_guard();
    // Provider present, but Net is denied by default — the op gate fails the
    // connect, surfacing as a non-clean close (1006).
    let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
    let out = eval_async(
        &mut rt,
        "const ws = new WebSocket('ws://x/'); \
         return await new Promise((resolve) => { \
           ws.addEventListener('close', (e) => resolve(e.code + ':' + e.wasClean + ':' + ws.readyState)); \
         });",
    );
    assert_eq!(out, Value::String("1006:false:3".into()));
}

#[test]
fn websocketstream_round_trip_with_backpressured_reads() {
    use es_runtime_providers::WsIncoming;
    let _g = v8_guard();
    let ws = MockWs::new(
        vec![
            WsIncoming::Text("hello".into()),
            WsIncoming::Binary(vec![1, 2, 3]),
            WsIncoming::Close {
                code: 1000,
                reason: "bye".into(),
            },
        ],
        "chat",
    );
    let mut rt = runtime_with_ws(Arc::new(ws));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const wss = new WebSocketStream('ws://echo.test/', { protocols: ['chat'] }); \
         const { readable, writable, protocol } = await wss.opened; \
         const log = ['open:' + protocol]; \
         const writer = writable.getWriter(); \
         await writer.write('ping'); \
         await writer.write(new Uint8Array([9])); \
         writer.releaseLock(); \
         const reader = readable.getReader(); \
         for (;;) { \
           const { value, done } = await reader.read(); \
           if (done) { log.push('done'); break; } \
           log.push(typeof value === 'string' ? 'txt:' + value : 'bin:' + Array.from(value).join(',')); \
         } \
         const c = await wss.closed; \
         log.push('close:' + c.closeCode + ':' + c.reason); \
         return log.join('|');",
    );
    assert_eq!(
        out,
        Value::String("open:chat|txt:hello|bin:1,2,3|done|close:1000:bye".into())
    );
}

#[test]
fn websocketstream_local_close_settles_closed_via_drain() {
    use es_runtime_providers::WsIncoming;
    let _g = v8_guard();
    // The guest closes without reading `readable`; the internal drain must
    // still observe the peer's close frame and settle `closed`.
    let ws = MockWs::new(
        vec![WsIncoming::Close {
            code: 1000,
            reason: "ok".into(),
        }],
        "",
    );
    let mut rt = runtime_with_ws(Arc::new(ws));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const wss = new WebSocketStream('ws://x/'); \
         await wss.opened; \
         wss.close({ closeCode: 1000, reason: 'done' }); \
         const c = await wss.closed; \
         return c.closeCode + ':' + c.reason;",
    );
    assert_eq!(out, Value::String("1000:ok".into()));
}

#[test]
fn websocketstream_abnormal_close_errors_reads_and_closed() {
    let _g = v8_guard();
    // No inbound frames: the first recv returns null (abnormal close), so a
    // read and the `closed` promise both reject with a WebSocketError.
    let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    let out = eval_async(
        &mut rt,
        "const wss = new WebSocketStream('ws://x/'); \
         const { readable } = await wss.opened; \
         const log = []; \
         try { await readable.getReader().read(); log.push('read-ok'); } \
         catch (e) { log.push('read:' + e.name); } \
         try { await wss.closed; } \
         catch (e) { log.push('closed:' + e.name + ':' + (e instanceof WebSocketError) + ':' + (e instanceof DOMException)); } \
         return log.join('|');",
    );
    assert_eq!(
        out,
        Value::String("read:WebSocketError|closed:WebSocketError:true:true".into())
    );
}

#[test]
fn websocketstream_validates_url_close_args_and_wserror_init() {
    let _g = v8_guard();
    let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    assert_true(
        &mut rt,
        "(() => { const errs = []; \
         try { new WebSocketStream('http://x/'); } catch (e) { errs.push(e.name); } \
         const wss = new WebSocketStream('ws://x/'); \
         try { wss.close({ closeCode: 1234 }); } catch (e) { errs.push(e.name); } \
         try { wss.close({ reason: 'x'.repeat(200) }); } catch (e) { errs.push(e.name); } \
         try { new WebSocketError('m', { closeCode: 1234 }); } catch (e) { errs.push(e.name); } \
         const we = new WebSocketError('m', { reason: 'r' }); \
         errs.push(we.name + ':' + we.closeCode + ':' + we.reason); \
         return errs.join(',') === \
           'SyntaxError,InvalidAccessError,SyntaxError,InvalidAccessError,WebSocketError:1000:r'; })()",
    );
}

#[test]
fn websocketstream_connect_requires_net_capability() {
    let _g = v8_guard();
    // Provider present, Net denied: `opened` and `closed` both reject.
    let mut rt = runtime_with_ws(Arc::new(MockWs::new(vec![], "")));
    let out = eval_async(
        &mut rt,
        "const wss = new WebSocketStream('ws://x/'); \
         const log = []; \
         try { await wss.opened; } catch (e) { log.push('opened:' + e.name); } \
         try { await wss.closed; } catch (e) { log.push('closed:' + e.name); } \
         return log.join('|');",
    );
    assert_eq!(
        out,
        Value::String("opened:WebSocketError|closed:WebSocketError".into())
    );
}

/// A guest that says nothing about a WebSocket server's posture gets the
/// provider's defaults. The prelude must send "unset" rather than a copy of
/// the number, so this asserts what arrives equals `WsTimeouts::default()`
/// rather than any literal written here — one copy of the value, on the
/// Rust side, is what keeps the two from drifting.
#[test]
fn ws_serve_without_options_uses_the_provider_defaults() {
    let _g = v8_guard();
    let ws = Arc::new(MockWs::new(vec![], ""));
    let mut rt = runtime_with_ws(ws.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:websocket'; \
         const s = serve({ port: 4001 }); \
         globalThis.result = (await s.addr).port;",
        MapLoader::new(&[]),
    );
    let served = ws.served.lock().unwrap();
    assert_eq!(
        served[0].timeouts,
        es_runtime_providers::WsTimeouts::default()
    );
    assert_eq!(served[0].max_connections, None);
    assert_eq!(served[0].max_connections_per_ip, None);
    // The two bounds that are *on* unless the guest turns them off — a
    // queue or a message nobody bounds is memory a peer can spend, and the
    // right numbers do not depend on anything the deployment knows.
    assert_eq!(served[0].limits, es_runtime_providers::WsLimits::default());
}

/// `0` is the guest turning a bound off, the same spelling the timeouts
/// use. Reading it as "unset" would hand back the default, which is the
/// opposite of what was asked; and each bound crosses on its own.
#[test]
fn a_zero_bound_turns_it_off_rather_than_defaulting() {
    let _g = v8_guard();
    let ws = Arc::new(MockWs::new(vec![], ""));
    let mut rt = runtime_with_ws(ws.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:websocket'; \
         const a = serve({ port: 4001, maxBufferedAmount: 0, maxMessageSize: 0 }); \
         const b = serve({ port: 4002, maxBufferedAmount: 1024, maxMessageSize: 4096 }); \
         const c = serve({ port: 4003, maxMessageSize: 512 }); \
         globalThis.result = (await a.addr).port + (await b.addr).port + (await c.addr).port;",
        MapLoader::new(&[]),
    );
    let served = ws.served.lock().unwrap();
    use es_runtime_providers::WsLimits;
    assert_eq!(
        served[0].limits,
        WsLimits {
            max_buffered_amount: None,
            max_message_size: None
        }
    );
    assert_eq!(
        served[1].limits,
        WsLimits {
            max_buffered_amount: Some(1024),
            max_message_size: Some(4096)
        }
    );
    assert_eq!(
        served[2].limits,
        WsLimits {
            max_message_size: Some(512),
            ..WsLimits::default()
        }
    );
}

/// The two knobs cross as they are written: milliseconds for the timeout,
/// and an explicit `null` meaning "off" rather than "use the default".
#[test]
fn ws_serve_options_cross_as_written() {
    let _g = v8_guard();
    let ws = Arc::new(MockWs::new(vec![], ""));
    let mut rt = runtime_with_ws(ws.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:websocket'; \
         const a = serve({ port: 4001, timeouts: { handshake: 1500 }, maxConnections: 64 }); \
         const b = serve({ port: 4002, timeouts: { handshake: null } }); \
         globalThis.result = (await a.addr).port + (await b.addr).port;",
        MapLoader::new(&[]),
    );
    let served = ws.served.lock().unwrap();
    assert_eq!(
        served[0].timeouts.handshake,
        Some(std::time::Duration::from_millis(1500))
    );
    assert_eq!(served[0].max_connections, Some(64));
    assert_eq!(
        served[1].timeouts.handshake, None,
        "an explicit null must disable the timeout, not fall back to the default",
    );
}

/// `broadcast` skipped anything it did not recognize, so
/// `broadcast([...room, undefined], msg)` delivered to the rest and said
/// nothing, and a list that was entirely the wrong type broadcast to nobody
/// and still returned normally — the failure mode where a chat room goes
/// quiet and every call looks like it worked.
///
/// The connection id is set in the constructor and never removed, so its
/// absence is a brand check rather than a liveness one: a *closed*
/// connection still has one and is still passed to the host, which owns the
/// live socket table. Only something that was never a connection throws.
#[test]
fn ws_broadcast_refuses_an_element_that_is_not_a_connection() {
    let _g = v8_guard();
    let ws = Arc::new(MockWs::new(vec![], "").accepting(2));
    let mut rt = runtime_with_ws(ws.clone());
    run_module(
        &mut rt,
        "import { serve, broadcast } from 'runtime:websocket'; \
         const s = serve({ port: 4001 }); \
         const conns = []; \
         for await (const c of s) conns.push(c); \
         const names = []; \
         for (const bad of [null, undefined, {}, 42, 'conn']) { \
           try { broadcast([...conns, bad], 'x'); names.push('no throw'); } \
           catch (e) { names.push(e.constructor.name); } \
         } \
         globalThis.result = conns.length + ':' + names.join(',');",
        MapLoader::new(&[]),
    );
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("2:TypeError,TypeError,TypeError,TypeError,TypeError".into()),
    );
    assert!(
        ws.broadcasts.lock().unwrap().is_empty(),
        "a bad element must fail the whole call, not half-deliver it",
    );
}

/// The other half: the check must not cost valid connections their
/// broadcast, which is the regression the brand test could have introduced.
#[test]
fn ws_broadcast_still_reaches_every_valid_connection() {
    let _g = v8_guard();
    // Open connections: one whose stream had ended would be skipped (D183).
    let ws = Arc::new(MockWs::new(vec![], "").accepting(3).held_open());
    let mut rt = runtime_with_ws(ws.clone());
    run_module(
        &mut rt,
        "import { serve, broadcast } from 'runtime:websocket'; \
         const s = serve({ port: 4001 }); \
         const conns = []; \
         for await (const c of s) conns.push(c); \
         broadcast(conns, 'hello'); \
         broadcast([], 'to nobody'); \
         globalThis.result = conns.length;",
        MapLoader::new(&[]),
    );
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(3.0));
    let sent = ws.broadcasts.lock().unwrap();
    assert_eq!(
        sent.len(),
        1,
        "an empty list is a no-op, not a host crossing: {sent:?}"
    );
    assert_eq!(
        sent[0].len(),
        3,
        "every accepted connection must be sent to"
    );
}

/// Rejected at the call rather than after the bind: a server that is
/// listening with a policy the guest did not ask for is worse than an error.
#[test]
fn ws_serve_rejects_unusable_options_before_the_port_is_bound() {
    let _g = v8_guard();
    let ws = Arc::new(MockWs::new(vec![], ""));
    let mut rt = runtime_with_ws(ws.clone());
    run_module(
        &mut rt,
        "import { serve } from 'runtime:websocket'; \
         const names = []; \
         for (const bad of ['lots', 0, -1, 1.5]) { \
           try { serve({ port: 4001, maxConnections: bad }); } \
           catch (e) { names.push(e.constructor.name); } \
           try { serve({ port: 4001, maxConnectionsPerIp: bad }); } \
           catch (e) { names.push(e.constructor.name); } \
         } \
         for (const bad of ['lots', -1, 1.5]) { \
           try { serve({ port: 4001, maxBufferedAmount: bad }); } \
           catch (e) { names.push(e.constructor.name); } \
         } \
         for (const bad of ['soon', -1, Infinity]) { \
           try { serve({ port: 4001, timeouts: { handshake: bad } }); } \
           catch (e) { names.push(e.constructor.name); } \
         } \
         globalThis.result = names.join(',');",
        MapLoader::new(&[]),
    );
    assert!(
        ws.served.lock().unwrap().is_empty(),
        "no bind may be attempted for an option that was rejected",
    );
}
