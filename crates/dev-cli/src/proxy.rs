//! Forwarding a path prefix to another server, for the page esdev serves
//! (`dev.server.proxy`, D150).
//!
//! A frontend calls its API with relative URLs — `fetch("/api/users")` — so the
//! same build works behind the deployment's reverse proxy. In development the
//! page comes from `esdev start` or `esdev preview`, and this is what sends
//! `/api/users` on to the API server instead of answering it with a 404.
//!
//! # A tunnel, not a client
//!
//! The request head is rewritten and the rest of the connection is piped both
//! ways. esdev never parses a body or a response, so a chunked upload, a
//! streamed response, server-sent events and a WebSocket upgrade all pass
//! through as the bytes they are, with nothing here to get wrong about any of
//! them.
//!
//! What the head gains is what a reverse proxy sends: `Host` becomes the
//! target's, since a virtual-hosted or TLS target needs its own name, and the
//! page's is kept in `X-Forwarded-Host`. It asks for `Connection: close`: the
//! next request on a kept-alive connection may be for a file esdev serves
//! itself, and a tunnel cannot hand a connection back. An upgrade keeps its
//! `Connection: Upgrade`, because the connection is the WebSocket from then on.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// One `dev.server.proxy` entry, ready to forward to.
#[derive(Debug, Clone)]
pub struct Rule {
    /// The start of the request paths it takes.
    pub prefix: String,
    /// Where they go.
    target: url::Url,
}

impl Rule {
    /// Rules from the config's `(prefix, URL)` pairs, which it validated and
    /// ordered longest prefix first.
    pub fn from_config(pairs: &[(String, String)]) -> Vec<Rule> {
        pairs
            .iter()
            .filter_map(|(prefix, target)| {
                Some(Rule {
                    prefix: prefix.clone(),
                    target: url::Url::parse(target).ok()?,
                })
            })
            .collect()
    }

    /// The target as it was written, for a message.
    pub fn target(&self) -> &str {
        self.target.as_str().trim_end_matches('/')
    }

    fn secure(&self) -> bool {
        self.target.scheme() == "https"
    }

    fn host(&self) -> &str {
        self.target.host_str().unwrap_or("localhost")
    }

    fn port(&self) -> u16 {
        self.target.port_or_known_default().unwrap_or(80)
    }

    /// `Host` for the target: its name, with the port when it is not the
    /// scheme's own.
    fn authority(&self) -> String {
        match self.target.port() {
            Some(port) => format!("{}:{port}", self.host()),
            None => self.host().to_string(),
        }
    }

    /// The request target sent on: the target's own path, then what the page
    /// asked for.
    fn forwarded_path(&self, requested: &str) -> String {
        let base = self.target.path().trim_end_matches('/');
        format!("{base}{requested}")
    }
}

/// The rule that takes `path`, the longest prefix first.
pub fn matching<'a>(rules: &'a [Rule], path: &str) -> Option<&'a Rule> {
    rules.iter().find(|rule| path.starts_with(&rule.prefix))
}

/// How long a target has to accept the connection before the page gets a 502.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Forwards one request, whose head has already been read, and everything
/// after it on the connection.
pub async fn forward(mut client: TcpStream, head: &str, rule: &Rule) {
    let request_target = crate::inspect::request_path(head).unwrap_or_else(|| "/".to_string());
    let upstream = match connect(rule).await {
        Ok(upstream) => upstream,
        Err(e) => {
            eprintln!(
                "esdev: proxy: cannot reach {} for {request_target}: {e}",
                rule.target()
            );
            let body = format!(
                "esdev could not reach {} to forward {request_target}: {e}\n\n\
                 Start the server it names, or change `dev.server.proxy` in esdev.json.\n",
                rule.target()
            );
            let _ = client
                .write_all(
                    format!(
                        "HTTP/1.1 502 Bad Gateway\r\nContent-Type: text/plain; charset=utf-8\r\n\
                         Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await;
            return;
        }
    };
    let peer = client
        .peer_addr()
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string());
    let rewritten = rewrite(head, rule, &peer);
    match upstream {
        Upstream::Plain(stream) => pipe(client, stream, rewritten).await,
        Upstream::Tls(stream) => pipe(client, *stream, rewritten).await,
    }
}

enum Upstream {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::client::TlsStream<TcpStream>>),
}

async fn connect(rule: &Rule) -> Result<Upstream, String> {
    let address = format!("{}:{}", rule.host(), rule.port());
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&address))
        .await
        .map_err(|_| format!("no answer within {}s", CONNECT_TIMEOUT.as_secs()))?
        .map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);
    if !rule.secure() {
        return Ok(Upstream::Plain(stream));
    }
    let name = tokio_rustls::rustls::pki_types::ServerName::try_from(rule.host().to_string())
        .map_err(|e| format!("{} is not a name TLS can verify: {e}", rule.host()))?;
    let stream = tokio_rustls::TlsConnector::from(tls_config())
        .connect(name, stream)
        .await
        .map_err(|e| format!("TLS: {e}"))?;
    Ok(Upstream::Tls(Box::new(stream)))
}

/// A client configuration verifying against the bundled Mozilla roots, as
/// `fetch` does. Built once.
fn tls_config() -> Arc<tokio_rustls::rustls::ClientConfig> {
    static CONFIG: std::sync::OnceLock<Arc<tokio_rustls::rustls::ClientConfig>> =
        std::sync::OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = tokio_rustls::rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider());
            let config = tokio_rustls::rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("the default protocol versions are supported")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// Writes the rewritten head, then copies both ways until either side is done.
async fn pipe<S>(mut client: TcpStream, mut upstream: S, head: String)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if upstream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
}

/// The request head as the target receives it.
fn rewrite(head: &str, rule: &Rule, peer: &str) -> String {
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let requested = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("HTTP/1.1");

    let mut host = None;
    let mut upgrade = false;
    let mut headers = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let lower = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match lower.as_str() {
            "host" => host = Some(value.to_string()),
            "upgrade" => {
                upgrade = true;
                headers.push(format!("{}: {value}", name.trim()));
            }
            // Hop-by-hop: they describe the connection to esdev, not to the
            // target. Replaced below.
            "connection" | "keep-alive" | "proxy-connection" => {}
            // Ours to state, not the page's to pass along.
            "x-forwarded-host" | "x-forwarded-proto" | "x-forwarded-for" => {}
            _ => headers.push(format!("{}: {value}", name.trim())),
        }
    }

    let mut out = format!(
        "{method} {} {version}\r\nHost: {}\r\n",
        rule.forwarded_path(requested),
        rule.authority()
    );
    for header in headers {
        out.push_str(&header);
        out.push_str("\r\n");
    }
    if let Some(host) = host {
        out.push_str(&format!("X-Forwarded-Host: {host}\r\n"));
    }
    out.push_str("X-Forwarded-Proto: http\r\n");
    out.push_str(&format!("X-Forwarded-For: {peer}\r\n"));
    out.push_str(if upgrade {
        "Connection: Upgrade\r\n"
    } else {
        "Connection: close\r\n"
    });
    out.push_str("\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(prefix: &str, target: &str) -> Rule {
        Rule::from_config(&[(prefix.to_string(), target.to_string())]).remove(0)
    }

    /// The longest prefix wins, which the config's order already says.
    #[test]
    fn the_first_matching_rule_takes_the_path() {
        let rules = Rule::from_config(&[
            ("/api/admin".to_string(), "http://admin:9000".to_string()),
            ("/api".to_string(), "http://localhost:8080".to_string()),
        ]);
        assert_eq!(
            matching(&rules, "/api/admin/users").unwrap().prefix,
            "/api/admin"
        );
        assert_eq!(matching(&rules, "/api/users").unwrap().prefix, "/api");
        assert!(matching(&rules, "/index.html").is_none());
    }

    /// The target's own path goes first, and the page's path and query after
    /// it, unchanged.
    #[test]
    fn the_target_path_is_prepended() {
        assert_eq!(
            rule("/api", "http://localhost:8080").forwarded_path("/api/users?page=2"),
            "/api/users?page=2"
        );
        assert_eq!(
            rule("/api", "https://example.com/v1/").forwarded_path("/api/users"),
            "/v1/api/users"
        );
    }

    /// `Host` is the target's; the page's is kept, and the connection is
    /// closed after one exchange.
    #[test]
    fn the_head_is_what_a_reverse_proxy_sends() {
        let head = "POST /api/users HTTP/1.1\r\nHost: localhost:5173\r\n\
                    Content-Length: 2\r\nConnection: keep-alive\r\nX-Forwarded-For: 6.6.6.6\r\n\r\n";
        let out = rewrite(head, &rule("/api", "http://localhost:8080"), "127.0.0.1");
        assert!(
            out.starts_with("POST /api/users HTTP/1.1\r\nHost: localhost:8080\r\n"),
            "{out}"
        );
        assert!(out.contains("Content-Length: 2\r\n"), "{out}");
        assert!(
            out.contains("X-Forwarded-Host: localhost:5173\r\n"),
            "{out}"
        );
        assert!(out.contains("X-Forwarded-For: 127.0.0.1\r\n"), "{out}");
        assert!(!out.contains("6.6.6.6"), "{out}");
        assert!(out.contains("Connection: close\r\n"), "{out}");
        assert!(!out.contains("keep-alive"), "{out}");
        assert!(out.ends_with("\r\n\r\n"), "{out}");
    }

    /// An upgrade stays one: the connection is the WebSocket from then on.
    #[test]
    fn an_upgrade_keeps_its_connection() {
        let head = "GET /api/ws HTTP/1.1\r\nHost: localhost:5173\r\nUpgrade: websocket\r\n\
                    Connection: Upgrade\r\nSec-WebSocket-Key: abc\r\n\r\n";
        let out = rewrite(head, &rule("/api", "https://example.com"), "127.0.0.1");
        assert!(out.contains("Host: example.com\r\n"), "{out}");
        assert!(out.contains("Upgrade: websocket\r\n"), "{out}");
        assert!(out.contains("Connection: Upgrade\r\n"), "{out}");
        assert!(!out.contains("Connection: close"), "{out}");
    }
}
