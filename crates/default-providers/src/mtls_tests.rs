//! Certificate rotation (D200), mutual TLS (D201) and revocation (D202), end
//! to end through the providers: a private PKI, real handshakes on loopback,
//! and every way a client or server certificate must be refused.

use std::sync::Arc;
use std::time::Duration;

use es_runtime_common::ErrorCode;
use es_runtime_providers::{
    ClientIdentity, ConnectOptions, HttpServeOptions, HttpServerBody, HttpServerProvider,
    HttpServerResponse, HttpServerTls, ListenOptions, NetProvider, PeerCertificate, SecureContext,
};

use crate::client::{HttpClient, Outbound};
use crate::trust::test_pki::{Ca, Leaf};
use crate::{SystemHttpServer, SystemNet, TrustStore};

fn pem(leaf: &Leaf) -> (Vec<u8>, Vec<u8>) {
    (
        leaf.cert_pem.clone().into_bytes(),
        leaf.key_pem.clone().into_bytes(),
    )
}

/// A `runtime:net` TLS listener presenting `server`, requiring client
/// certificates from `client_ca` when it is given, checked against `crl`.
async fn listener(server: &Leaf, client_ca: Option<&Ca>, crl: &str) -> (SystemNet, u64, u16) {
    let net = SystemNet::new();
    let (cert, key) = pem(server);
    let (id, info) = net
        .listen(
            "127.0.0.1".to_string(),
            0,
            ListenOptions {
                cert,
                key,
                request_cert: client_ca.is_some(),
                client_ca: client_ca
                    .map(|ca| ca.pem.clone().into_bytes())
                    .unwrap_or_default(),
                crl: crl.as_bytes().to_vec(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    (net, id, info.local_port)
}

/// Connects to `port` trusting `server_ca`, presenting `identity`, and reports
/// what the listener saw: `Some(peer certificate)` when it accepted the
/// connection (`None` inside when no certificate was sent), `None` when the
/// handshake was refused.
async fn handshake(
    listener: &SystemNet,
    id: u64,
    port: u16,
    server_ca: &Ca,
    identity: ClientIdentity,
) -> Option<Option<Arc<PeerCertificate>>> {
    let client = SystemNet::new().with_trust(server_ca.trust());
    // The listener's address, not `localhost`: Windows resolves that to `::1`
    // first and spends seconds on the refusal there before trying 127.0.0.1,
    // which outlasts the accept below. The certificate is still checked
    // against `localhost`, through SNI.
    let connect = client.connect(
        "127.0.0.1".to_string(),
        port,
        ConnectOptions {
            secure: true,
            identity,
            sni: Some("localhost".to_string()),
            ..Default::default()
        },
    );
    let accept = tokio::time::timeout(Duration::from_secs(2), listener.accept(id));
    let (connected, accepted) = tokio::join!(connect, accept);
    let accepted = accepted.ok()?.ok()??;
    // A TLS 1.3 client finishes its side before the server has judged its
    // certificate, so the listener's verdict is the one that counts.
    connected.ok()?;
    Some(accepted.1.peer_certificate)
}

#[tokio::test]
async fn a_listener_that_requires_certificates_accepts_one_its_ca_signed() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let (net, id, port) = listener(&server_ca.leaf(&["localhost"]), Some(&client_ca), "").await;
    let client = client_ca.leaf_named(&["svc.internal"], "orders-service", 7);
    let peer = handshake(&net, id, port, &server_ca, client.identity())
        .await
        .expect("a certificate the CA signed must be accepted")
        .expect("the verified certificate reaches the program");
    assert!(
        peer.subject
            .contains(&("CN".to_string(), "orders-service".to_string()))
    );
    assert_eq!(peer.subjectaltname, "DNS:svc.internal");
    assert_eq!(peer.raw, client.chain[0].as_ref());
    assert_eq!(peer.fingerprint256.len(), 32 * 3 - 1);
}

// D201: there is no way in without a certificate the CA signed.
#[tokio::test]
async fn a_listener_that_requires_certificates_refuses_everything_else() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let other_ca = Ca::new("Other CA");
    let (net, id, port) = listener(&server_ca.leaf(&["localhost"]), Some(&client_ca), "").await;
    let cases = [
        ("no certificate", ClientIdentity::default()),
        ("another CA's", other_ca.leaf(&["x"]).identity()),
        (
            "an expired one",
            client_ca.leaf_valid(&["x"], 2001, 2002).identity(),
        ),
    ];
    for (what, identity) in cases {
        assert!(
            handshake(&net, id, port, &server_ca, identity)
                .await
                .is_none(),
            "{what} was accepted"
        );
    }
}

#[tokio::test]
async fn a_listener_without_request_cert_reports_no_peer() {
    let server_ca = Ca::new("Server CA");
    let (net, id, port) = listener(&server_ca.leaf(&["localhost"]), None, "").await;
    let peer = handshake(&net, id, port, &server_ca, ClientIdentity::default())
        .await
        .expect("a plain TLS listener accepts");
    assert!(peer.is_none());
}

// D202: a CRL revokes one certificate and leaves the CA's others alone.
#[tokio::test]
async fn a_revoked_client_certificate_is_refused() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let crl = client_ca.crl(&[7], 2090);
    let (net, id, port) = listener(&server_ca.leaf(&["localhost"]), Some(&client_ca), &crl).await;
    let revoked = client_ca.leaf_named(&["a"], "revoked", 7);
    let good = client_ca.leaf_named(&["b"], "good", 8);
    assert!(
        handshake(&net, id, port, &server_ca, revoked.identity())
            .await
            .is_none()
    );
    assert!(
        handshake(&net, id, port, &server_ca, good.identity())
            .await
            .is_some()
    );
}

// An outdated CRL cannot say a certificate is still good, so the
// certificates it covers are refused until it is refreshed.
#[tokio::test]
async fn an_outdated_crl_refuses_what_it_covers() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let crl = client_ca.crl(&[], 2022);
    let (net, id, port) = listener(&server_ca.leaf(&["localhost"]), Some(&client_ca), &crl).await;
    let good = client_ca.leaf_named(&["b"], "good", 8);
    assert!(
        handshake(&net, id, port, &server_ca, good.identity())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn client_certificate_options_are_checked_at_bind() {
    let server_ca = Ca::new("Server CA");
    let (cert, key) = pem(&server_ca.leaf(&["localhost"]));
    let net = SystemNet::new();
    let bind = |request_cert: bool, client_ca: &[u8], crl: &[u8]| {
        net.listen(
            "127.0.0.1".to_string(),
            0,
            ListenOptions {
                cert: cert.clone(),
                key: key.clone(),
                request_cert,
                client_ca: client_ca.to_vec(),
                crl: crl.to_vec(),
                ..Default::default()
            },
        )
    };
    let ca = server_ca.pem.as_bytes();
    for (what, result) in [
        ("requestCert without a ca", bind(true, b"", b"").await),
        ("a ca without requestCert", bind(false, ca, b"").await),
        ("a crl without requestCert", bind(false, b"", b"x").await),
        ("a crl that is not one", bind(true, ca, b"not a crl").await),
    ] {
        assert!(result.is_err(), "{what} was accepted");
    }
}

// A client certificate whose key belongs to another certificate is refused
// before anything is sent.
#[tokio::test]
async fn a_client_key_that_does_not_match_its_certificate_is_refused() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let (net, _id, port) = listener(&server_ca.leaf(&["localhost"]), Some(&client_ca), "").await;
    let a = client_ca.leaf(&["a"]);
    let b = client_ca.leaf(&["b"]);
    let mismatched = ClientIdentity {
        cert: a.cert_pem.into_bytes(),
        key: b.key_pem.into_bytes(),
    };
    let client = SystemNet::new().with_trust(server_ca.trust());
    let err = client
        .connect(
            "127.0.0.1".to_string(),
            port,
            ConnectOptions {
                secure: true,
                identity: mismatched,
                sni: Some("localhost".to_string()),
                ..Default::default()
            },
        )
        .await
        .err()
        .expect("a mismatched key must be refused");
    assert_eq!(err.code(), Some(ErrorCode::Tls), "{err}");
    let _ = net;
}

// D200: new handshakes present the replacement; a bad replacement changes
// nothing.
#[tokio::test]
async fn set_secure_context_replaces_the_listener_certificate() {
    let old_ca = Ca::new("Old CA");
    let new_ca = Ca::new("New CA");
    let (net, id, port) = listener(&old_ca.leaf(&["localhost"]), None, "").await;
    assert!(
        handshake(&net, id, port, &old_ca, ClientIdentity::default())
            .await
            .is_some()
    );
    assert!(
        handshake(&net, id, port, &new_ca, ClientIdentity::default())
            .await
            .is_none()
    );

    let replacement = new_ca.leaf(&["localhost"]);
    let other = new_ca.leaf(&["localhost"]);
    let mismatched = net.set_secure_context(
        id,
        SecureContext {
            cert: Some(replacement.cert_pem.clone().into_bytes()),
            key: Some(other.key_pem.clone().into_bytes()),
            ..Default::default()
        },
    );
    assert!(
        mismatched.is_err(),
        "a key that is not the certificate's was installed"
    );
    assert!(
        handshake(&net, id, port, &old_ca, ClientIdentity::default())
            .await
            .is_some(),
        "a refused replacement changed the certificate"
    );

    let (cert, key) = pem(&replacement);
    net.set_secure_context(
        id,
        SecureContext {
            cert: Some(cert),
            key: Some(key),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        handshake(&net, id, port, &new_ca, ClientIdentity::default())
            .await
            .is_some()
    );
    assert!(
        handshake(&net, id, port, &old_ca, ClientIdentity::default())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn set_secure_context_rotates_the_client_ca_and_crl() {
    let server_ca = Ca::new("Server CA");
    let first = Ca::new("First client CA");
    let second = Ca::new("Second client CA");
    let (net, id, port) = listener(&server_ca.leaf(&["localhost"]), Some(&first), "").await;
    let from_second = second.leaf_named(&["x"], "x", 3);
    assert!(
        handshake(&net, id, port, &server_ca, from_second.identity())
            .await
            .is_none()
    );
    net.set_secure_context(
        id,
        SecureContext {
            ca: Some(second.pem.clone().into_bytes()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        handshake(&net, id, port, &server_ca, from_second.identity())
            .await
            .is_some()
    );
    net.set_secure_context(
        id,
        SecureContext {
            crl: Some(second.crl(&[3], 2090).into_bytes()),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        handshake(&net, id, port, &server_ca, from_second.identity())
            .await
            .is_none()
    );
}

#[tokio::test]
async fn set_secure_context_needs_a_tls_listener() {
    let net = SystemNet::new();
    let (id, _) = net
        .listen("127.0.0.1".to_string(), 0, ListenOptions::default())
        .await
        .unwrap();
    assert!(
        net.set_secure_context(id, SecureContext::default())
            .is_err()
    );
    assert!(
        net.set_secure_context(999, SecureContext::default())
            .is_err()
    );
}

/// A `runtime:http` server presenting `server`, requiring client
/// certificates from `client_ca`. It answers each request with the CN of the
/// certificate it verified, or `none`.
async fn http_server(server: &Leaf, client_ca: &Ca) -> (SystemHttpServer, u64, u16) {
    let http = SystemHttpServer::new();
    let (cert, key) = pem(server);
    let (id, info) = http
        .serve(HttpServeOptions {
            host: "127.0.0.1".to_string(),
            port: 0,
            tls: Some(HttpServerTls {
                cert,
                key,
                alpn: vec!["h2".to_string(), "http/1.1".to_string()],
                request_cert: true,
                client_ca: client_ca.pem.clone().into_bytes(),
                crl: Vec::new(),
            }),
            timeouts: Default::default(),
            max_connections: None,
            max_connections_per_ip: None,
            reuse_port: false,
        })
        .await
        .unwrap();
    let responder = http.clone();
    tokio::spawn(async move {
        while let Ok(requests) = responder.next_requests(id, 8).await {
            if requests.is_empty() {
                break;
            }
            for (rid, request) in requests {
                let cn = request
                    .peer_certificate
                    .as_ref()
                    .and_then(|c| {
                        c.subject
                            .iter()
                            .find(|(k, _)| k == "CN")
                            .map(|(_, v)| v.clone())
                    })
                    .unwrap_or_else(|| "none".to_string());
                let _ = responder
                    .respond(
                        rid,
                        HttpServerResponse {
                            status: 200,
                            headers: vec![],
                            body: HttpServerBody::Bytes(cn.into_bytes()),
                            trailers: None,
                        },
                    )
                    .await;
            }
        }
    });
    (http, id, info.local_port)
}

async fn get(client: &HttpClient, port: u16) -> Result<String, String> {
    let url = url::Url::parse(&format!("https://localhost:{port}/")).unwrap();
    let response = client
        .send(Outbound::get(url))
        .await
        .map_err(|e| e.to_string())?;
    let bytes = response.bytes(1 << 20).await.map_err(|e| e.to_string())?;
    Ok(String::from_utf8(bytes).unwrap())
}

// D201 over HTTP, both halves: `fetch`'s client presents a certificate, and
// the server's handler reads who it was.
#[tokio::test]
async fn an_http_client_certificate_reaches_the_handler() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let (_http, _id, port) = http_server(&server_ca.leaf(&["localhost"]), &client_ca).await;
    let client_leaf = client_ca.leaf_named(&["svc"], "billing", 11);
    let with = HttpClient::with_tls(&server_ca.trust(), b"", &client_leaf.identity()).unwrap();
    assert_eq!(get(&with, port).await.unwrap(), "billing");
    let without = HttpClient::new(&server_ca.trust());
    assert!(
        get(&without, port).await.is_err(),
        "a request without a certificate was served"
    );
}

// A request's own `ca` adds to the process store for that client only.
#[tokio::test]
async fn a_request_ca_trusts_a_server_the_process_store_does_not() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let (_http, _id, port) = http_server(&server_ca.leaf(&["localhost"]), &client_ca).await;
    let identity = client_ca.leaf_named(&["svc"], "svc", 12).identity();
    let refused = HttpClient::with_tls(&TrustStore::bundled(), b"", &identity).unwrap();
    assert!(get(&refused, port).await.is_err());
    let trusted =
        HttpClient::with_tls(&TrustStore::bundled(), server_ca.pem.as_bytes(), &identity).unwrap();
    assert_eq!(get(&trusted, port).await.unwrap(), "svc");
}

#[tokio::test]
async fn set_secure_context_replaces_an_http_server_certificate() {
    let old_ca = Ca::new("Old CA");
    let new_ca = Ca::new("New CA");
    let client_ca = Ca::new("Client CA");
    let (http, id, port) = http_server(&old_ca.leaf(&["localhost"]), &client_ca).await;
    let identity = client_ca.leaf_named(&["svc"], "svc", 5).identity();
    let trusting = |ca: &Ca| HttpClient::with_tls(&ca.trust(), b"", &identity).unwrap();
    assert!(get(&trusting(&old_ca), port).await.is_ok());
    let (cert, key) = pem(&new_ca.leaf(&["localhost"]));
    http.set_secure_context(
        id,
        SecureContext {
            cert: Some(cert),
            key: Some(key),
            ..Default::default()
        },
    )
    .unwrap();
    // Fresh clients, so fresh connections: an open one keeps its certificate.
    assert!(get(&trusting(&new_ca), port).await.is_ok());
    assert!(get(&trusting(&old_ca), port).await.is_err());
}

// D202, client side: `--tls-crl` refuses a revoked server certificate and
// leaves the CA's others, and other CAs, alone.
#[tokio::test]
async fn a_revoked_server_certificate_is_refused_by_clients() {
    let ca = Ca::new("Server CA");
    let dir = std::env::temp_dir().join(format!("esrun-crl-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("server.crl");
    std::fs::write(&file, ca.crl(&[21], 2090)).unwrap();
    let trust = ca.trust().with_crl_files(&[file]).unwrap();
    let connect = |leaf: Leaf| {
        let trust = trust.clone();
        async move {
            let (port, server) = crate::trust::test_pki::tls_server(leaf, &[], b"").await;
            let net = SystemNet::new().with_trust(trust);
            let result = net
                .connect(
                    "127.0.0.1".to_string(),
                    port,
                    ConnectOptions {
                        secure: true,
                        sni: Some("localhost".to_string()),
                        ..Default::default()
                    },
                )
                .await;
            server.abort();
            result.map(|_| ())
        }
    };
    let revoked = connect(ca.leaf_named(&["localhost"], "revoked", 21)).await;
    assert_eq!(revoked.err().and_then(|e| e.code()), Some(ErrorCode::Tls));
    assert!(
        connect(ca.leaf_named(&["localhost"], "good", 22))
            .await
            .is_ok()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
