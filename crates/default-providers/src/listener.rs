//! Binding a TCP listener, including the options that must be set *before*
//! `bind` and so cannot be reached through [`tokio::net::TcpListener::bind`].
//!
//! There is exactly one such option today: `SO_REUSEPORT`. It is how several
//! processes share one listening port — each binds the same address and the
//! kernel balances new connections across them — which is the ordinary way to
//! run a server across cores without a front proxy, and the ordinary way to
//! restart one without dropping connections. It has to be set on the socket
//! between `socket()` and `bind()`, so a listener that wants it is built a step
//! at a time rather than in one call.
//!
//! Shared by `runtime:net` `listen()` and `runtime:http` `serve()`, so the two
//! bind on identical terms.

use es_runtime_providers::ProviderError;
use tokio::net::TcpListener;

/// The pending-connection queue depth for a hand-built listener. Matches what
/// `TcpListener::bind` uses, so turning `reuse_port` on does not quietly change
/// how many connections the kernel will hold for a busy server.
const BACKLOG: i32 = 1024;

/// Binds a TCP listener on `host:port`.
///
/// With `reuse_port` false this is exactly `TcpListener::bind` — the common
/// path is not rerouted through the manual one, so its behaviour (including
/// address resolution) is unchanged.
pub(crate) async fn bind(
    host: &str,
    port: u16,
    reuse_port: bool,
) -> Result<TcpListener, ProviderError> {
    let context = format!("listen {host}:{port}");
    if !reuse_port {
        return TcpListener::bind((host, port))
            .await
            .map_err(|e| ProviderError::from_io(context, &e));
    }
    bind_reuse_port(host, port, &context).await
}

#[cfg(unix)]
async fn bind_reuse_port(
    host: &str,
    port: u16,
    context: &str,
) -> Result<TcpListener, ProviderError> {
    use socket2::{Domain, Protocol, Socket, Type};

    // Resolved here because a raw socket has to be created for the right
    // address family before it can be bound, which `TcpListener::bind` would
    // otherwise do for us.
    let addr = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?
        .next()
        .ok_or_else(|| ProviderError::Coded {
            code: es_runtime_common::ErrorCode::Dns,
            message: format!("{context}: the address resolved to nothing"),
        })?;

    let domain = Domain::for_address(addr);
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?;
    // Both, and in this order. `SO_REUSEADDR` is what `TcpListener::bind` sets
    // for us on Unix (it lets a restarting server rebind a port still in
    // `TIME_WAIT`); dropping it while adding `SO_REUSEPORT` would trade one
    // convenience for another rather than adding the second.
    socket
        .set_reuse_address(true)
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?;
    socket
        .set_reuse_port(true)
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?;
    socket
        .bind(&addr.into())
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?;
    socket
        .listen(BACKLOG)
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?;

    TcpListener::from_std(std::net::TcpListener::from(socket))
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))
}

/// Windows has no `SO_REUSEPORT`. Its `SO_REUSEADDR` is *not* the same thing —
/// it lets an unrelated process steal a bound port, which is a hijacking
/// primitive rather than a load-balancing one — so it is not substituted here.
///
/// Refused rather than ignored: a caller that asked for `reusePort` is
/// expecting a second process to be able to bind the same port, and silently
/// binding exclusively would hand it a server that works alone and fails the
/// moment it is scaled, with nothing to read that says why.
#[cfg(not(unix))]
async fn bind_reuse_port(
    _host: &str,
    _port: u16,
    context: &str,
) -> Result<TcpListener, ProviderError> {
    Err(ProviderError::Other(format!(
        "{context}: reusePort is not supported on this platform (SO_REUSEPORT is Unix-only)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_plain_bind_still_works() {
        let l = bind("127.0.0.1", 0, false).await.expect("bind");
        assert!(l.local_addr().expect("addr").port() > 0);
    }

    #[tokio::test]
    async fn a_port_already_taken_is_address_in_use() {
        let held = bind("127.0.0.1", 0, false).await.expect("bind");
        let port = held.local_addr().expect("addr").port();
        let err = bind("127.0.0.1", port, false)
            .await
            .expect_err("the second bind must fail");
        assert_eq!(
            err.code(),
            Some(es_runtime_common::ErrorCode::AddressInUse),
            "{err}",
        );
    }

    /// The whole point: two listeners on one port, which is refused without the
    /// option and allowed with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn reuse_port_lets_a_second_listener_share_the_port() {
        let first = bind("127.0.0.1", 0, true).await.expect("first bind");
        let port = first.local_addr().expect("addr").port();

        let second = bind("127.0.0.1", port, true).await.expect("second bind");
        assert_eq!(second.local_addr().expect("addr").port(), port);

        // …and without it, the same second bind is refused, so the option is
        // doing the work rather than the port having been free all along.
        let refused = bind("127.0.0.1", port, false).await;
        assert!(refused.is_err(), "a plain bind must still be refused");
    }
}
