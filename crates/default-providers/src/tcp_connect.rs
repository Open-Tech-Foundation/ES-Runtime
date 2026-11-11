//! Opening the client's TCP connections (DECISIONS D217, phase 2): name
//! resolution, Happy Eyeballs, and the socket options every connection gets.
//!
//! Names resolve through the system resolver (`getaddrinfo`, via tokio), so
//! `/etc/hosts` and the platform's name-service configuration apply as they
//! do to every other program — unlike `resolveDns`, whose stub resolver asks
//! DNS servers directly by design (D188).
//!
//! With addresses of both families, the first family is tried and the other
//! is raced against it once [`FALLBACK_DELAY`] passes without a connection
//! (Happy Eyeballs, RFC 8305 §5): a host whose IPv6 route is broken still
//! connects quickly over IPv4. Within a family, addresses are tried in turn.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use es_runtime_common::ErrorCode;
use tokio::net::{TcpSocket, TcpStream};

/// How long the first address family has before the other is tried beside
/// it. RFC 8305 recommends 250 ms; this is the 300 ms the previous connector
/// used, so nothing changes for a host that is slow on one family.
pub(crate) const FALLBACK_DELAY: Duration = Duration::from_millis(300);

/// A failed connect, with the code a guest sees.
#[derive(Debug)]
pub(crate) struct TcpError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
}

/// Connects to `host:port`: a literal address directly, a name through the
/// system resolver. `keepalive` probes an idle connection, so a peer that
/// vanished without a FIN is noticed rather than pooled.
pub(crate) async fn connect(
    host: &str,
    port: u16,
    keepalive: Duration,
) -> Result<TcpStream, TcpError> {
    let addresses: Vec<SocketAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| TcpError {
                code: ErrorCode::Dns,
                message: format!("dns error: {e}"),
            })?
            .collect(),
    };
    if addresses.is_empty() {
        return Err(TcpError {
            code: ErrorCode::Dns,
            message: format!("dns error: no addresses for {host}"),
        });
    }
    connect_to(&addresses, keepalive).await
}

/// Connects to the first of `addresses` that answers, by Happy Eyeballs.
pub(crate) async fn connect_to(
    addresses: &[SocketAddr],
    keepalive: Duration,
) -> Result<TcpStream, TcpError> {
    let Some(first) = addresses.first() else {
        return Err(TcpError {
            code: ErrorCode::Dns,
            message: "dns error: no addresses".to_string(),
        });
    };
    let first_is_v6 = first.is_ipv6();
    let (preferred, fallback): (Vec<_>, Vec<_>) =
        addresses.iter().partition(|a| a.is_ipv6() == first_is_v6);

    let stream = if fallback.is_empty() {
        try_in_turn(&preferred).await?
    } else {
        let preferred_attempt = try_in_turn(&preferred);
        tokio::pin!(preferred_attempt);
        tokio::select! {
            result = &mut preferred_attempt => match result {
                Ok(stream) => stream,
                // The first family failed before the delay: go straight on.
                Err(_) => try_in_turn(&fallback).await?,
            },
            () = tokio::time::sleep(FALLBACK_DELAY) => {
                let fallback_attempt = try_in_turn(&fallback);
                tokio::pin!(fallback_attempt);
                // Both families racing: the first to connect wins, and only
                // when both fail is the first family's error reported.
                tokio::select! {
                    result = &mut preferred_attempt => match result {
                        Ok(stream) => stream,
                        Err(first) => fallback_attempt.await.map_err(|_| first)?,
                    },
                    result = &mut fallback_attempt => match result {
                        Ok(stream) => stream,
                        Err(_) => preferred_attempt.await?,
                    },
                }
            }
        }
    };
    // Set explicitly: a request written in pieces would otherwise wait on the
    // peer's delayed ACK (Nagle's algorithm).
    let _ = stream.set_nodelay(true);
    let _ = socket2::SockRef::from(&stream)
        .set_tcp_keepalive(&socket2::TcpKeepalive::new().with_time(keepalive));
    Ok(stream)
}

/// Each address in turn; the last error if none connects.
async fn try_in_turn(addresses: &[&SocketAddr]) -> Result<TcpStream, TcpError> {
    let mut last = None;
    for address in addresses {
        let socket = if address.is_ipv6() {
            TcpSocket::new_v6()
        } else {
            TcpSocket::new_v4()
        };
        let attempt = match socket {
            Ok(socket) => socket.connect(**address).await,
            Err(e) => Err(e),
        };
        match attempt {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    let e = last.unwrap_or_else(|| std::io::Error::other("no address to connect to"));
    Err(TcpError {
        code: ErrorCode::from_io_kind(e.kind()),
        message: format!("tcp connect error: {e}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_literal_address_connects_without_resolving() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        let stream = connect("127.0.0.1", port, Duration::from_secs(60))
            .await
            .unwrap();
        assert!(stream.nodelay().unwrap(), "Nagle must be off");
    }

    #[tokio::test]
    async fn localhost_resolves_through_the_system_resolver() {
        // `localhost` comes from the hosts file, which only the system
        // resolver reads.
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        connect("localhost", port, Duration::from_secs(60))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn failures_carry_their_codes() {
        let refused = connect("127.0.0.1", 1, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(
            refused.code,
            ErrorCode::ConnectionRefused,
            "{}",
            refused.message
        );
        let unknown = connect("no-such-host.invalid", 80, Duration::from_secs(60))
            .await
            .unwrap_err();
        assert_eq!(unknown.code, ErrorCode::Dns, "{}", unknown.message);
    }

    /// An IPv6 address that never answers must not hold the connection back
    /// when IPv4 works: the fallback starts after the delay.
    #[tokio::test]
    async fn a_dead_first_family_falls_back_after_the_delay() {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        // A blackholed IPv6 documentation address first, then the live one.
        let dead: SocketAddr = format!("[2001:db8::1]:{port}").parse().unwrap();
        let live: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let started = std::time::Instant::now();
        let stream = connect_to(&[dead, live], Duration::from_secs(60))
            .await
            .unwrap();
        assert!(stream.peer_addr().unwrap().is_ipv4());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }
}
