//! Hello-world HTTP servers with no JavaScript, for measuring what each layer
//! under `runtime:http` costs on its own (`bench/cpu-per-request.sh`, rows
//! `hyper`, `hyper-h1` and `provider`).
//!
//!   cargo build --release -p es-runtime-default-providers --example http_floor
//!   BENCH_PORT=3000 target/release/examples/http_floor <mode>
//!
//! Modes, each on a current-thread tokio runtime as esrun's own server is:
//!
//! - `hyper`: bare hyper behind hyper-util's version-detecting `auto` builder,
//!   the same builder `SystemHttpServer` uses, with none of its layers.
//! - `hyper-h1`: hyper's HTTP/1-only server, so the difference from `hyper` is
//!   what version detection costs.
//! - `provider`: `SystemHttpServer` itself — its timeouts, limits, channels and
//!   request conversion — answered by a Rust loop in place of the JS handler.
//!   The difference from full esrun is the JS boundary and the handler's JS.
//!
//! Every mode answers like `bench/scripts/helloserver.js` on esrun: 200,
//! `content-type: text/plain;charset=UTF-8`, `Hello, World!`.

use std::convert::Infallible;

use bytes::Bytes;
use es_runtime_default_providers::SystemHttpServer;
use es_runtime_providers::{
    HttpServeOptions, HttpServerBody, HttpServerProvider, HttpServerResponse, HttpTimeouts,
};
use http_body_util::Full;
use hyper::{Request, Response, body::Incoming, service::service_fn};
use hyper_util::rt::{TokioExecutor, TokioIo};

const BODY: &str = "Hello, World!";
const CONTENT_TYPE: &str = "text/plain;charset=UTF-8";

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let port: u16 = std::env::var("BENCH_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3000);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async move {
        match mode.as_str() {
            "hyper" => bare(port, false).await,
            "hyper-h1" => bare(port, true).await,
            "provider" => provider(port).await,
            other => panic!("unknown mode {other:?}: hyper | hyper-h1 | provider"),
        }
    });
}

async fn hello(_: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
    let mut response = Response::new(Full::new(Bytes::from_static(BODY.as_bytes())));
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static(CONTENT_TYPE),
    );
    Ok(response)
}

async fn bare(port: u16, h1_only: bool) {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("bind");
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let io = TokioIo::new(stream);
        tokio::spawn(async move {
            if h1_only {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service_fn(hello))
                    .await;
            } else {
                let _ = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection(io, service_fn(hello))
                    .await;
            }
        });
    }
}

async fn provider(port: u16) {
    let http = SystemHttpServer::new();
    let (id, _) = http
        .serve(HttpServeOptions {
            host: "127.0.0.1".into(),
            port,
            tls: None,
            timeouts: HttpTimeouts::default(),
            max_connections: None,
            max_connections_per_ip: None,
            reuse_port: false,
        })
        .await
        .expect("serve");
    // The same batch size `runtime:http` asks for.
    while let Ok(requests) = http.next_requests(id, 64).await {
        if requests.is_empty() {
            break;
        }
        for (rid, _request) in requests {
            let response = HttpServerResponse {
                status: 200,
                headers: vec![("content-type".into(), CONTENT_TYPE.into())],
                body: HttpServerBody::Bytes(BODY.as_bytes().to_vec()),
                trailers: None,
            };
            let _ = http.respond(rid, response).await;
        }
    }
}
