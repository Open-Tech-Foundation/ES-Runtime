//! DNS lookups for `runtime:net`'s `resolveDns` and `mongodb+srv://`
//! (DECISIONS D188): our own resolver, rather than a crate's.
//!
//! It is a stub resolver: it asks the system's configured servers (or the
//! caller's `nameServer`), which do the recursion, and reads the answer. The
//! message format is `wire`, pure and fuzzed; this is the I/O around it.
//!
//! **What it does for safety.** Each question carries a random id from the OS,
//! goes out on a connected UDP socket with a fresh ephemeral port, and a reply
//! counts only if its id, its question and its source all match — anything else
//! is ignored, as a spoofed reply would be. A truncated reply is asked again
//! over TCP. Every wait has a deadline. Names are asked as written, never
//! through search domains, and there is no cache to poison.

mod config;
#[cfg(windows)]
mod windows;
pub(crate) mod wire;

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use es_runtime_common::ErrorCode;
use es_runtime_providers::{DnsQuery, DnsRecord, ProviderError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, UdpSocket};
use tokio::time::{Instant, timeout};

use wire::Answer;

/// How long one server has to answer one attempt.
const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);
/// How many times each server is tried, in turn.
const ROUNDS: usize = 2;
/// Large enough for any UDP reply, whatever was offered in EDNS0.
const UDP_BUFFER: usize = 65_535;

/// Answers `query`, by the contract of `NetProvider::resolve_dns`.
pub(crate) async fn resolve(query: DnsQuery) -> Result<Vec<DnsRecord>, ProviderError> {
    let qtype = wire::type_code(&query.record_type).ok_or_else(|| {
        dns_error(format!(
            "{} is not a record type resolveDns reads",
            query.record_type
        ))
    })?;
    // A name that cannot be asked fails here, once, rather than at every server.
    wire::query(0, &query.name, qtype).map_err(|e| dns_error(e.0))?;
    let servers = match &query.name_server {
        Some((ip, port)) => {
            let ip: IpAddr = ip
                .parse()
                .map_err(|_| dns_error(format!("{ip} is not an IP address")))?;
            vec![SocketAddr::new(ip, *port)]
        }
        None => config::system_servers(),
    };
    if servers.is_empty() {
        return Err(dns_error("no DNS servers are configured".into()));
    }
    let mut failures = Vec::new();
    for _ in 0..ROUNDS {
        for server in &servers {
            match ask(*server, &query.name, qtype).await {
                Ok(Answer::Records(records)) => return Ok(records),
                Ok(Answer::NoSuchName) => {
                    return Err(ProviderError::Coded {
                        code: ErrorCode::NotFound,
                        message: format!("{} does not exist", query.name),
                    });
                }
                Ok(Answer::ServerFailed(code)) => {
                    failures.push(format!("{server} answered with code {code}"))
                }
                Ok(Answer::Truncated) => failures.push(format!("{server} sent a truncated answer")),
                Err(e) => failures.push(format!("{server}: {e}")),
            }
        }
    }
    failures.dedup();
    Err(dns_error(format!(
        "no DNS server answered for {}: {}",
        query.name,
        failures.join("; ")
    )))
}

fn dns_error(message: String) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::Dns,
        message,
    }
}

fn random_id() -> Result<u16, String> {
    let mut id = [0u8; 2];
    getrandom::fill(&mut id).map_err(|e| format!("no randomness for a query id: {e}"))?;
    Ok(u16::from_ne_bytes(id))
}

/// One question to one server over UDP, and over TCP if the answer is cut short.
async fn ask(server: SocketAddr, name: &str, qtype: u16) -> Result<Answer, String> {
    let id = random_id()?;
    let message = wire::query(id, name, qtype).map_err(|e| e.0)?;
    let local: SocketAddr = if server.is_ipv4() {
        (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let socket = UdpSocket::bind(local).await.map_err(|e| e.to_string())?;
    // Connected, so the kernel drops datagrams from any other source.
    socket.connect(server).await.map_err(|e| e.to_string())?;
    socket.send(&message).await.map_err(|e| e.to_string())?;
    let deadline = Instant::now() + ATTEMPT_TIMEOUT;
    let mut buffer = vec![0u8; UDP_BUFFER];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let received = match timeout(left, socket.recv(&mut buffer)).await {
            Err(_) => return Err("timed out".into()),
            Ok(Err(e)) => return Err(e.to_string()),
            Ok(Ok(n)) => n,
        };
        match wire::response(&buffer[..received], id, name, qtype) {
            // Not an answer to this question: keep waiting for the real one.
            Ok(None) | Err(_) => {}
            Ok(Some(Answer::Truncated)) => return ask_tcp(server, name, qtype).await,
            Ok(Some(answer)) => return Ok(answer),
        }
    }
}

/// One question over TCP: each message prefixed by its two-byte length.
async fn ask_tcp(server: SocketAddr, name: &str, qtype: u16) -> Result<Answer, String> {
    let id = random_id()?;
    let message = wire::query(id, name, qtype).map_err(|e| e.0)?;
    let exchange = async {
        let mut stream = TcpStream::connect(server)
            .await
            .map_err(|e| e.to_string())?;
        let len = u16::try_from(message.len()).map_err(|_| "the query is too large".to_string())?;
        let mut framed = len.to_be_bytes().to_vec();
        framed.extend_from_slice(&message);
        stream.write_all(&framed).await.map_err(|e| e.to_string())?;
        let mut head = [0u8; 2];
        stream
            .read_exact(&mut head)
            .await
            .map_err(|e| e.to_string())?;
        let mut body = vec![0u8; usize::from(u16::from_be_bytes(head))];
        stream
            .read_exact(&mut body)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(body)
    };
    let body = timeout(ATTEMPT_TIMEOUT, exchange)
        .await
        .map_err(|_| "timed out over TCP".to_string())??;
    match wire::response(&body, id, name, qtype) {
        Ok(Some(Answer::Truncated)) => Err("a truncated answer over TCP".into()),
        Ok(Some(answer)) => Ok(answer),
        Ok(None) => Err("the TCP answer was to another question".into()),
        Err(e) => Err(e.0),
    }
}
