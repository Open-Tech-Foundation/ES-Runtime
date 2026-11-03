//! The outbound HTTP proxy (DECISIONS D205): which targets go through it, and
//! the connection to it.
//!
//! One module so that every client that honors the proxy — `fetch`'s
//! connector, the `WebSocket` client, the `--otel` exporter — reaches it the
//! same way. A `http:` target is *forwarded* (the client sends its request to
//! the proxy in absolute form, so only [`ProxyServer::connect`] is needed);
//! anything else is *tunnelled* with `CONNECT` ([`ProxyServer::tunnel`]) and
//! the target's own TLS, if any, runs inside the tunnel.
//!
//! Credentials come from the proxy URL's userinfo and leave this module only
//! as a `Proxy-Authorization` value. Nothing here formats them: a
//! [`ProxyServer`] displays as `scheme://host:port`.

use std::net::IpAddr;
use std::sync::Arc;

use es_runtime_common::ErrorCode;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;

/// A byte stream to a target or a proxy: TCP, TLS over TCP, or TLS inside a
/// tunnel inside TLS. Boxed so every combination is one type to the clients.
pub(crate) trait Wire: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Wire for T {}

/// A boxed [`Wire`].
pub(crate) type BoxWire = Box<dyn Wire>;

/// The most of a `CONNECT` response head read before the proxy is taken to be
/// misbehaving. A real one is a status line and a few headers.
const HEAD_LIMIT: usize = 16 * 1024;

/// A failure reaching or tunnelling through the proxy, with its code.
#[derive(Debug)]
pub(crate) struct ProxyError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProxyError {}

/// One proxy server: where it is, whether it is spoken to over TLS, and the
/// `Proxy-Authorization` value its URL's credentials make.
#[derive(Clone)]
pub struct ProxyServer {
    tls: bool,
    host: String,
    port: u16,
    authorization: Option<String>,
}

impl std::fmt::Debug for ProxyServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProxyServer({self})")
    }
}

/// `scheme://host:port`, never the credentials.
impl std::fmt::Display for ProxyServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.tls { "https" } else { "http" };
        if self.host.contains(':') {
            write!(f, "{scheme}://[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{scheme}://{}:{}", self.host, self.port)
        }
    }
}

impl ProxyServer {
    /// Parses a proxy URL: `http://` or `https://`, optional `user:pass@`, a
    /// host, an optional port (the scheme's default otherwise). The error
    /// never repeats the URL, which may hold a password.
    pub fn parse(text: &str) -> Result<ProxyServer, String> {
        let url = url::Url::parse(text).map_err(|e| format!("not a URL ({e})"))?;
        let tls = match url.scheme() {
            "http" => false,
            "https" => true,
            other => {
                return Err(format!(
                    "a {other}: proxy is not supported; it must be http:// or https://"
                ));
            }
        };
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or("the URL has no host")?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        let port = url.port_or_known_default().ok_or("the URL has no port")?;
        if !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
            return Err("a proxy URL is a scheme, a host and a port; it has no path".to_string());
        }
        let authorization = if url.username().is_empty() && url.password().is_none() {
            None
        } else {
            let decode = |part: &str| {
                percent_decode(part).ok_or("the URL's credentials are not valid UTF-8")
            };
            let user = decode(url.username())?;
            let pass = decode(url.password().unwrap_or(""))?;
            use base64::Engine;
            let token = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
            Some(format!("Basic {token}"))
        };
        Ok(ProxyServer {
            tls,
            host,
            port,
            authorization,
        })
    }

    /// The `Proxy-Authorization` value, when the URL carried credentials.
    pub fn authorization(&self) -> Option<&str> {
        self.authorization.as_deref()
    }

    /// TCP to the proxy, and TLS when it is an `https://` proxy — verified
    /// against `tls`, which offers no ALPN (the proxy is spoken to over
    /// HTTP/1.1).
    pub(crate) async fn connect(&self, tls: &TlsConnector) -> Result<BoxWire, ProxyError> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port))
            .await
            .map_err(|e| ProxyError {
                code: ErrorCode::from_io_kind(e.kind()),
                message: format!("cannot reach the proxy {self}: {e}"),
            })?;
        let _ = tcp.set_nodelay(true);
        if !self.tls {
            return Ok(Box::new(tcp));
        }
        let name = ServerName::try_from(self.host.clone()).map_err(|_| ProxyError {
            code: ErrorCode::Tls,
            message: format!("the proxy {self} is not a name TLS can verify"),
        })?;
        let stream = tls.connect(name, tcp).await.map_err(|e| ProxyError {
            code: ErrorCode::Tls,
            message: format!("TLS to the proxy {self}: {e}"),
        })?;
        Ok(Box::new(stream))
    }

    /// A tunnel through the proxy to `host:port`: [`connect`](Self::connect),
    /// then `CONNECT`, answered with a 2xx. The returned stream is the bytes
    /// to and from the target; the caller runs the target's TLS over it.
    pub(crate) async fn tunnel(
        &self,
        tls: &TlsConnector,
        host: &str,
        port: u16,
    ) -> Result<BoxWire, ProxyError> {
        let mut wire = self.connect(tls).await?;
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let mut head = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
        if let Some(auth) = &self.authorization {
            head.push_str("Proxy-Authorization: ");
            head.push_str(auth);
            head.push_str("\r\n");
        }
        head.push_str("\r\n");
        let io = |e: std::io::Error| ProxyError {
            code: ErrorCode::from_io_kind(e.kind()),
            message: format!("the proxy {self}: {e}"),
        };
        wire.write_all(head.as_bytes()).await.map_err(io)?;
        wire.flush().await.map_err(io)?;
        // Byte by byte, so nothing past the head is consumed: after a 2xx the
        // next byte is the target's, and the target's TLS reads it.
        let mut response = Vec::with_capacity(256);
        let mut byte = [0u8; 1];
        while !response.ends_with(b"\r\n\r\n") {
            if response.len() >= HEAD_LIMIT {
                return Err(ProxyError {
                    code: ErrorCode::ProxyTunnel,
                    message: format!("the proxy {self} sent no end to its CONNECT response"),
                });
            }
            if wire.read(&mut byte).await.map_err(io)? == 0 {
                return Err(ProxyError {
                    code: ErrorCode::ProxyTunnel,
                    message: format!("the proxy {self} closed the connection on CONNECT"),
                });
            }
            response.push(byte[0]);
        }
        let status_line = String::from_utf8_lossy(&response);
        let status_line = status_line.lines().next().unwrap_or_default();
        let mut parts = status_line.splitn(3, ' ');
        let version = parts.next().unwrap_or_default();
        let status: u16 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if !version.starts_with("HTTP/1.") || status == 0 {
            return Err(ProxyError {
                code: ErrorCode::ProxyTunnel,
                message: format!("the proxy {self} did not answer CONNECT with HTTP"),
            });
        }
        if !(200..300).contains(&status) {
            let reason = parts.next().unwrap_or_default().trim();
            let hint = match status {
                407 if self.authorization.is_none() => {
                    "; it wants credentials, given as http://user:pass@host:port"
                }
                407 => "; it refused the credentials given",
                _ => "",
            };
            return Err(ProxyError {
                code: ErrorCode::ProxyTunnel,
                message: format!(
                    "the proxy {self} refused a tunnel to {authority}: {status} {reason}{hint}"
                ),
            });
        }
        Ok(wire)
    }
}

/// Percent-decodes a URL userinfo part, `None` when the bytes are not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The process's proxy configuration: a server per target scheme, and the
/// targets that go direct.
#[derive(Debug, Clone)]
pub struct Proxies {
    /// For `http:` and `ws:` targets.
    http: Option<ProxyServer>,
    /// For `https:` and `wss:` targets.
    https: Option<ProxyServer>,
    bypass: Vec<Exception>,
}

impl Proxies {
    /// `esrun`'s form: `--proxy=<url>` for every target, `--no-proxy` entries
    /// as exceptions. The error names the flag; it never repeats the URL.
    pub fn from_flags(url: &str, no_proxy: &[String]) -> Result<Proxies, String> {
        let server = ProxyServer::parse(url).map_err(|e| format!("--proxy: {e}"))?;
        Ok(Proxies {
            http: Some(server.clone()),
            https: Some(server),
            bypass: parse_exceptions(no_proxy).map_err(|e| format!("--no-proxy: {e}"))?,
        })
    }

    /// `esdev`'s form: the flags when given, the environment otherwise —
    /// `https_proxy`/`HTTPS_PROXY`, `http_proxy`/`HTTP_PROXY` and
    /// `no_proxy`/`NO_PROXY`, the lowercase name first. `None` when nothing
    /// names a proxy, or `--no-proxy`/`NO_PROXY` is `*`.
    pub fn from_flags_or_env(
        url: Option<&str>,
        no_proxy: Option<&[String]>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Proxies>, String> {
        let var = |lower: &str, upper: &str| {
            env(lower)
                .or_else(|| env(upper))
                .filter(|v| !v.trim().is_empty())
        };
        let bypass = match no_proxy {
            Some(list) => parse_exceptions(list).map_err(|e| format!("--no-proxy: {e}"))?,
            None => match var("no_proxy", "NO_PROXY") {
                Some(list) => parse_exceptions(&[list]).map_err(|e| format!("NO_PROXY: {e}"))?,
                None => Vec::new(),
            },
        };
        let (http, https) = match url {
            Some(url) => {
                let server = ProxyServer::parse(url).map_err(|e| format!("--proxy: {e}"))?;
                (Some(server.clone()), Some(server))
            }
            None => {
                let parse = |name: &str, value: Option<String>| {
                    value
                        .map(|v| ProxyServer::parse(v.trim()).map_err(|e| format!("{name}: {e}")))
                        .transpose()
                };
                (
                    parse("HTTP_PROXY", var("http_proxy", "HTTP_PROXY"))?,
                    parse("HTTPS_PROXY", var("https_proxy", "HTTPS_PROXY"))?,
                )
            }
        };
        let proxies = Proxies {
            http,
            https,
            bypass,
        };
        let off = proxies.bypass.iter().any(|e| matches!(e, Exception::All));
        Ok((!off && (proxies.http.is_some() || proxies.https.is_some())).then_some(proxies))
    }

    /// The proxy a `scheme://host:port` target goes through, `None` for
    /// direct. `scheme` is the target's: `http`/`ws` or `https`/`wss`.
    pub fn for_target(&self, scheme: &str, host: &str, port: u16) -> Option<&ProxyServer> {
        let server = match scheme {
            "http" | "ws" => self.http.as_ref(),
            "https" | "wss" => self.https.as_ref(),
            _ => None,
        }?;
        let host = normalize(host);
        if is_loopback(&host) || self.bypass.iter().any(|e| e.matches(&host, port)) {
            return None;
        }
        Some(server)
    }

    /// Shared ownership, for the clients that hold it.
    pub fn shared(self) -> Arc<Proxies> {
        Arc::new(self)
    }
}

/// Lowercase, brackets and a trailing dot removed: the form exceptions are
/// compared in.
fn normalize(host: &str) -> String {
    host.trim_start_matches('[')
        .trim_end_matches(']')
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

/// Loopback always goes direct: a proxy cannot reach this machine's
/// `localhost`, and nothing means to send it there.
fn is_loopback(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// One `--no-proxy` / `NO_PROXY` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Exception {
    /// `*`: nothing is proxied.
    All,
    /// A domain and its subdomains.
    Domain { name: String, port: Option<u16> },
    /// One IP address.
    Ip { ip: IpAddr, port: Option<u16> },
    /// An address block.
    Block { network: IpAddr, prefix: u8 },
}

impl Exception {
    fn matches(&self, host: &str, target_port: u16) -> bool {
        let port_ok = |port: &Option<u16>| port.is_none_or(|p| p == target_port);
        match self {
            Exception::All => true,
            Exception::Domain { name, port } => {
                port_ok(port)
                    && (host == name
                        || host
                            .strip_suffix(name.as_str())
                            .is_some_and(|rest| rest.ends_with('.')))
            }
            Exception::Ip { ip, port } => port_ok(port) && host.parse::<IpAddr>() == Ok(*ip),
            Exception::Block { network, prefix } => host
                .parse::<IpAddr>()
                .is_ok_and(|ip| in_block(ip, *network, *prefix)),
        }
    }
}

/// Parses exception lists: comma- or whitespace-separated, each entry `*`, a
/// domain (`example.com`, `.example.com`, `*.example.com`), an IP, a CIDR
/// block, any but `*` and a block optionally with `:port`.
fn parse_exceptions(lists: &[String]) -> Result<Vec<Exception>, String> {
    let mut out = Vec::new();
    for entry in lists
        .iter()
        .flat_map(|list| list.split(|c: char| c == ',' || c.is_whitespace()))
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        out.push(parse_exception(entry).map_err(|e| format!("{entry}: {e}"))?);
    }
    Ok(out)
}

fn parse_exception(entry: &str) -> Result<Exception, String> {
    if entry == "*" {
        return Ok(Exception::All);
    }
    if let Some((network, prefix)) = entry.split_once('/') {
        let network: IpAddr = network
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| "a block is an IP address and a prefix length".to_string())?;
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix: u8 = prefix
            .parse()
            .ok()
            .filter(|&p| p <= max)
            .ok_or_else(|| format!("the prefix length must be 0 to {max}"))?;
        return Ok(Exception::Block { network, prefix });
    }
    // An unbracketed IPv6 address is all colons, so try it whole first.
    if let Ok(ip) = entry.parse::<IpAddr>() {
        return Ok(Exception::Ip { ip, port: None });
    }
    let (host, port) = split_port(entry)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(Exception::Ip { ip, port });
    }
    let name = normalize(host.trim_start_matches("*.").trim_start_matches('.'));
    if name.is_empty() || name.contains(['*', '/', '@']) {
        return Err("not a domain, an IP address or a block".to_string());
    }
    Ok(Exception::Domain { name, port })
}

/// `host[:port]`, with `[v6]:port` bracketed.
fn split_port(entry: &str) -> Result<(&str, Option<u16>), String> {
    let (host, port) = if let Some(rest) = entry.strip_prefix('[') {
        match rest.split_once(']') {
            Some((host, "")) => (host, None),
            Some((host, port)) => (host, port.strip_prefix(':')),
            None => return Err("an unclosed [".to_string()),
        }
    } else {
        match entry.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (entry, None),
        }
    };
    let port = port
        .map(|p| {
            p.parse::<u16>()
                .map_err(|_| "the port is not a number".to_string())
        })
        .transpose()?;
    Ok((host, port))
}

fn in_block(ip: IpAddr, network: IpAddr, prefix: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(net)) => {
            let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
            u32::from(ip) & mask == u32::from(net) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) => {
            let mask = u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0);
            u128::from(ip) & mask == u128::from(net) & mask
        }
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod test_proxy {
    //! A forward proxy on loopback for the clients' tests: `CONNECT` tunnels,
    //! anything else is forwarded to the host in its absolute-form target.
    //! Every request head it receives is recorded.

    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// What the proxy does with a request.
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Mode {
        /// Tunnels and forwards.
        Open,
        /// Answers `407` unless `Proxy-Authorization` is this value.
        Auth(&'static str),
        /// Answers every `CONNECT` with `403`.
        Refuse,
    }

    pub(crate) struct TestProxy {
        pub(crate) port: u16,
        pub(crate) heads: Arc<Mutex<Vec<String>>>,
    }

    impl TestProxy {
        pub(crate) fn heads(&self) -> Vec<String> {
            self.heads.lock().unwrap().clone()
        }
    }

    async fn read_head<S: AsyncRead + Unpin>(socket: &mut S) -> Option<String> {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if socket.read(&mut byte).await.ok()? == 0 {
                return None;
            }
            head.push(byte[0]);
        }
        Some(String::from_utf8_lossy(&head).into_owned())
    }

    /// `*.test` names are this machine, so a client's loopback exception
    /// does not route the test target around the proxy.
    fn resolve(authority: &str) -> String {
        match authority.rsplit_once(':') {
            Some((host, port)) if host.ends_with(".test") => format!("127.0.0.1:{port}"),
            _ => authority.to_string(),
        }
    }

    fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
        head.lines().skip(1).find_map(|line| {
            let (n, v) = line.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim())
        })
    }

    async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
        mut socket: S,
        mode: Mode,
        seen: Arc<Mutex<Vec<String>>>,
    ) {
        let Some(head) = read_head(&mut socket).await else {
            return;
        };
        seen.lock().unwrap().push(head.clone());
        if let Mode::Auth(expected) = mode
            && header(&head, "proxy-authorization") != Some(expected)
        {
            let _ = socket
                .write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\ncontent-length: 0\r\n\r\n",
                )
                .await;
            return;
        }
        let target = head.split_whitespace().nth(1).unwrap_or("").to_string();
        if head.starts_with("CONNECT ") {
            if mode == Mode::Refuse {
                let _ = socket
                    .write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n")
                    .await;
                return;
            }
            let Ok(mut upstream) = TcpStream::connect(resolve(&target)).await else {
                let _ = socket.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                return;
            };
            let _ = socket
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await;
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
        } else {
            // Forwarded: the target is absolute-form.
            let Ok(url) = url::Url::parse(&target) else {
                return;
            };
            let addr = format!(
                "{}:{}",
                url.host_str().unwrap_or_default(),
                url.port_or_known_default().unwrap_or(80)
            );
            let Ok(mut upstream) = TcpStream::connect(resolve(&addr)).await else {
                let _ = socket.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                return;
            };
            let _ = upstream.write_all(head.as_bytes()).await;
            let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
        }
    }

    /// A plain-HTTP proxy.
    pub(crate) async fn start(mode: Mode) -> TestProxy {
        start_with(mode, None).await
    }

    /// A proxy spoken to over TLS, presenting `leaf`.
    pub(crate) async fn start_tls(mode: Mode, leaf: crate::trust::test_pki::Leaf) -> TestProxy {
        use tokio_rustls::rustls::ServerConfig;
        use tokio_rustls::rustls::crypto::aws_lc_rs;
        let config = ServerConfig::builder_with_provider(Arc::new(aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(leaf.chain, leaf.key)
            .unwrap();
        start_with(
            mode,
            Some(tokio_rustls::TlsAcceptor::from(Arc::new(config))),
        )
        .await
    }

    async fn start_with(mode: Mode, tls: Option<tokio_rustls::TlsAcceptor>) -> TestProxy {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let heads = Arc::new(Mutex::new(Vec::new()));
        let seen = heads.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let seen = seen.clone();
                let tls = tls.clone();
                tokio::spawn(async move {
                    match tls {
                        Some(acceptor) => {
                            if let Ok(stream) = acceptor.accept(socket).await {
                                serve(stream, mode, seen).await;
                            }
                        }
                        None => serve(socket, mode, seen).await,
                    }
                });
            }
        });
        TestProxy { port, heads }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxies(url: &str, no_proxy: &[&str]) -> Proxies {
        let list: Vec<String> = no_proxy.iter().map(|s| s.to_string()).collect();
        Proxies::from_flags(url, &list).unwrap()
    }

    #[test]
    fn a_proxy_url_is_http_or_https_with_a_default_port() {
        let http = ProxyServer::parse("http://proxy.corp").unwrap();
        assert_eq!(http.to_string(), "http://proxy.corp:80");
        let https = ProxyServer::parse("https://proxy.corp").unwrap();
        assert_eq!(https.to_string(), "https://proxy.corp:443");
        let v6 = ProxyServer::parse("http://[::1]:3128").unwrap();
        assert_eq!(v6.to_string(), "http://[::1]:3128");
        for bad in [
            "socks5://p:1080",
            "proxy.corp:3128",
            "http://p:3128/path",
            "ftp://p",
        ] {
            assert!(ProxyServer::parse(bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn credentials_become_basic_and_are_never_displayed() {
        let server = ProxyServer::parse("http://al%40ice:s3cr%3At@proxy.corp:3128").unwrap();
        // base64("al@ice:s3cr:t")
        assert_eq!(server.authorization(), Some("Basic YWxAaWNlOnMzY3I6dA=="));
        assert_eq!(server.to_string(), "http://proxy.corp:3128");
        assert!(!format!("{server:?}").contains("s3cr"));
        let error = Proxies::from_flags("socks5://u:hunter2@p:1", &[]).unwrap_err();
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[test]
    fn exceptions_match_domains_ips_blocks_and_ports() {
        let p = proxies(
            "http://p:3128",
            &[
                "example.com,.corp.internal",
                "*.svc.local",
                "10.0.0.0/8 192.168.1.5",
                "api.test:8443",
                "fd00::/8",
            ],
        );
        let direct = |host: &str, port: u16| p.for_target("https", host, port).is_none();
        assert!(direct("example.com", 443));
        assert!(direct("www.EXAMPLE.com.", 443));
        assert!(!direct("notexample.com", 443));
        assert!(direct("corp.internal", 443));
        assert!(direct("db.corp.internal", 443));
        assert!(direct("x.svc.local", 443));
        assert!(direct("10.1.2.3", 443));
        assert!(!direct("11.1.2.3", 443));
        assert!(direct("192.168.1.5", 443));
        assert!(direct("api.test", 8443));
        assert!(!direct("api.test", 443));
        assert!(direct("[fd12::1]", 443));
        assert!(!direct("other.org", 443));
    }

    #[test]
    fn loopback_always_goes_direct() {
        let p = proxies("http://p:3128", &[]);
        for host in [
            "localhost",
            "app.localhost",
            "127.0.0.1",
            "127.9.9.9",
            "[::1]",
        ] {
            assert!(
                p.for_target("http", host, 80).is_none(),
                "{host} was proxied"
            );
        }
        assert!(p.for_target("http", "example.org", 80).is_some());
    }

    #[test]
    fn a_bad_exception_is_refused_by_name() {
        let error = Proxies::from_flags("http://p:1", &["10.0.0.0/40".into()]).unwrap_err();
        assert!(error.contains("10.0.0.0/40"), "{error}");
        let error = Proxies::from_flags("http://p:1", &["host:port".into()]).unwrap_err();
        assert!(error.contains("not a number"), "{error}");
    }

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn the_environment_names_a_proxy_per_scheme() {
        let p = Proxies::from_flags_or_env(
            None,
            None,
            env(&[
                ("HTTP_PROXY", "http://plain:1"),
                ("https_proxy", "http://lower:2"),
                ("HTTPS_PROXY", "http://upper:3"),
                ("NO_PROXY", "skip.me"),
            ]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            p.for_target("http", "a.org", 80).unwrap().to_string(),
            "http://plain:1"
        );
        assert_eq!(
            p.for_target("wss", "a.org", 443).unwrap().to_string(),
            "http://lower:2"
        );
        assert!(p.for_target("https", "skip.me", 443).is_none());
    }

    #[test]
    fn flags_override_the_environment() {
        let vars = env(&[("HTTPS_PROXY", "http://env:1"), ("NO_PROXY", "a.org")]);
        let p = Proxies::from_flags_or_env(Some("http://flag:2"), Some(&[]), &vars)
            .unwrap()
            .unwrap();
        assert_eq!(
            p.for_target("https", "a.org", 443).unwrap().to_string(),
            "http://flag:2"
        );
        assert_eq!(
            p.for_target("http", "b.org", 80).unwrap().to_string(),
            "http://flag:2"
        );
        let off = Proxies::from_flags_or_env(None, Some(&["*".to_string()]), &vars).unwrap();
        assert!(off.is_none(), "--no-proxy=* left a proxy on");
        assert!(
            Proxies::from_flags_or_env(None, None, env(&[]))
                .unwrap()
                .is_none()
        );
    }

    fn plain_tls() -> TlsConnector {
        TlsConnector::from(Arc::new(crate::TrustStore::bundled().client_config(&[])))
    }

    #[tokio::test]
    async fn a_tunnel_carries_bytes_to_the_target() {
        let target = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let target_port = target.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = target.accept().await.unwrap();
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });
        let proxy = test_proxy::start(test_proxy::Mode::Auth("Basic dTpw")).await;
        let server = ProxyServer::parse(&format!("http://u:p@127.0.0.1:{}", proxy.port)).unwrap();
        let mut wire = server
            .tunnel(&plain_tls(), "127.0.0.1", target_port)
            .await
            .unwrap();
        wire.write_all(b"ping").await.unwrap();
        let mut echo = [0u8; 4];
        wire.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");
        let heads = proxy.heads();
        assert!(heads[0].starts_with(&format!("CONNECT 127.0.0.1:{target_port} HTTP/1.1\r\n")));
        assert!(heads[0].contains("Proxy-Authorization: Basic dTpw\r\n"));
    }

    #[tokio::test]
    async fn a_refused_tunnel_is_err_proxy_tunnel() {
        let proxy = test_proxy::start(test_proxy::Mode::Refuse).await;
        let server = ProxyServer::parse(&format!("http://127.0.0.1:{}", proxy.port)).unwrap();
        let Err(error) = server.tunnel(&plain_tls(), "example.org", 443).await else {
            panic!("a refused tunnel succeeded");
        };
        assert_eq!(error.code, ErrorCode::ProxyTunnel);
        assert!(error.message.contains("403 Forbidden"), "{}", error.message);
    }

    #[tokio::test]
    async fn a_407_says_the_proxy_wants_credentials() {
        let proxy = test_proxy::start(test_proxy::Mode::Auth("Basic x")).await;
        let server = ProxyServer::parse(&format!("http://127.0.0.1:{}", proxy.port)).unwrap();
        let Err(error) = server.tunnel(&plain_tls(), "example.org", 443).await else {
            panic!("an unauthenticated tunnel succeeded");
        };
        assert_eq!(error.code, ErrorCode::ProxyTunnel);
        assert!(
            error.message.contains("wants credentials"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn an_unreachable_proxy_is_named_without_its_credentials() {
        let free = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = free.local_addr().unwrap().port();
        drop(free);
        let server = ProxyServer::parse(&format!("http://u:hunter2@127.0.0.1:{port}")).unwrap();
        let Err(error) = server.tunnel(&plain_tls(), "example.org", 443).await else {
            panic!("an unreachable proxy tunnelled");
        };
        assert_eq!(error.code, ErrorCode::ConnectionRefused);
        assert!(
            error.message.contains(&format!("127.0.0.1:{port}")),
            "{}",
            error.message
        );
        assert!(!error.message.contains("hunter2"), "{}", error.message);
    }
}
