//! Binding a UDP socket, and the options that go with it (DECISIONS D58).
//!
//! The companion to [`listener`](crate::listener), and it exists for the same
//! reason: `SO_REUSEADDR` and `SO_REUSEPORT` have to be set on the socket
//! *between* `socket()` and `bind()`, so a socket that wants either is built a
//! step at a time rather than in one call. Unlike the TCP case there is no
//! shortcut path for the plain bind — every datagram socket goes through here,
//! because the remaining options (TTL, broadcast, multicast) are per-family and
//! the family is only known once the address is resolved.
//!
//! **The v4/v6 split is not cosmetic.** IPv4 and IPv6 carry the same three
//! concepts under different socket options, and setting the wrong one is not an
//! error the OS reports — it silently does nothing. So the address family
//! decides, once, which of each pair is set — at the bind from the resolved
//! address, and afterwards from the socket's own local address rather than from
//! a guess about the value it was handed.
//!
//! Multicast membership and the post-bind options live here too, for the same
//! reason: they are the other half of the same v4/v6 problem.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{DatagramOption, DatagramOptions, MulticastMembership, ProviderError};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

/// Binds a UDP socket on `host:port` and applies `opts`.
///
/// `port` `0` binds an ephemeral port, readable from the returned socket's
/// `local_addr`.
pub(crate) async fn bind(
    host: &str,
    port: u16,
    opts: &DatagramOptions,
) -> Result<UdpSocket, ProviderError> {
    let context = format!("bind udp {host}:{port}");
    let addr = resolve(host, port, &context).await?;
    let io = |e: std::io::Error| ProviderError::from_io(context.clone(), &e);

    let socket =
        Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP)).map_err(&io)?;

    // Before the bind, because that is the only time they mean anything.
    if opts.reuse_address {
        socket.set_reuse_address(true).map_err(&io)?;
    }
    if opts.reuse_port {
        set_reuse_port(&socket, &context)?;
    }
    // Also before the bind: on most systems `IPV6_V6ONLY` cannot be changed
    // once the socket is bound.
    if let Some(only) = opts.ipv6_only
        && addr.is_ipv6()
    {
        socket.set_only_v6(only).map_err(&io)?;
    }
    socket.set_nonblocking(true).map_err(&io)?;
    socket.bind(&addr.into()).map_err(&io)?;

    let v6 = addr.is_ipv6();
    if opts.broadcast {
        // IPv6 has no broadcast at all — the concept was replaced by multicast —
        // so asking for it there is a mistake worth naming rather than a flag
        // that sets nothing.
        if v6 {
            return Err(ProviderError::Other(format!(
                "{context}: broadcast is IPv4-only (IPv6 has no broadcast address; use multicast)"
            )));
        }
        socket.set_broadcast(true).map_err(&io)?;
    }
    if let Some(ttl) = opts.ttl {
        if v6 {
            socket.set_unicast_hops_v6(ttl).map_err(&io)?;
        } else {
            socket.set_ttl_v4(ttl).map_err(&io)?;
        }
    }
    if let Some(ttl) = opts.multicast_ttl {
        if v6 {
            socket.set_multicast_hops_v6(ttl).map_err(&io)?;
        } else {
            socket.set_multicast_ttl_v4(ttl).map_err(&io)?;
        }
    }
    if let Some(on) = opts.multicast_loopback {
        if v6 {
            socket.set_multicast_loop_v6(on).map_err(&io)?;
        } else {
            socket.set_multicast_loop_v4(on).map_err(&io)?;
        }
    }

    UdpSocket::from_std(std::net::UdpSocket::from(socket)).map_err(&io)
}

/// Joins or leaves a multicast group on `socket`.
///
/// `interface` is an IPv4 local address for a v4 group and an interface index
/// for a v6 one; empty means "let the OS choose" (`0.0.0.0` / index `0`). The
/// group's family decides, so a v6 group with an IPv4 interface is refused
/// rather than quietly joined on the default one.
///
/// With a `source`, this is **source-specific** multicast (RFC 4607): the
/// network delivers that sender's traffic to this group and nobody else's, so
/// the filter costs the receiver nothing and cannot be talked past. IPv4 only —
/// `IP_ADD_SOURCE_MEMBERSHIP` has no portable v6 twin in socket2, and the
/// deployed protocols that use SSM are v4.
pub(crate) fn set_membership(
    socket: &UdpSocket,
    membership: &MulticastMembership,
    join: bool,
) -> Result<(), ProviderError> {
    let MulticastMembership {
        group,
        interface,
        source,
    } = membership;
    let action = if join { "join" } else { "leave" };
    let address: IpAddr = group.parse().map_err(|_| {
        invalid(format!(
            "{action} multicast: {group} is not an IP address (a multicast group is an address, not a name)"
        ))
    })?;
    if !address.is_multicast() {
        return Err(invalid(format!(
            "{action} multicast: {group} is not a multicast address"
        )));
    }
    let context = format!("{action} multicast {group}");
    let result = match address {
        IpAddr::V4(group) => {
            let iface: Ipv4Addr = if interface.is_empty() {
                Ipv4Addr::UNSPECIFIED
            } else {
                interface.parse().map_err(|_| {
                    invalid(format!(
                        "{context}: interface {interface} is not an IPv4 address (an IPv4 membership names the local interface by address)"
                    ))
                })?
            };
            match source {
                None => {
                    if join {
                        socket.join_multicast_v4(group, iface)
                    } else {
                        socket.leave_multicast_v4(group, iface)
                    }
                }
                Some(source) => {
                    let source: Ipv4Addr = source.parse().map_err(|_| {
                        invalid(format!("{context}: source {source} is not an IPv4 address"))
                    })?;
                    // socket2 rather than tokio: source-specific membership has
                    // no `UdpSocket` method. `SockRef` borrows the fd, so the
                    // socket keeps owning it.
                    let raw = socket2::SockRef::from(socket);
                    if join {
                        raw.join_ssm_v4(&source, &group, &iface)
                    } else {
                        raw.leave_ssm_v4(&source, &group, &iface)
                    }
                }
            }
        }
        IpAddr::V6(group) => {
            if source.is_some() {
                return Err(invalid(format!(
                    "{context}: source-specific multicast is IPv4-only here"
                )));
            }
            let index: u32 = if interface.is_empty() {
                0
            } else {
                interface.parse().map_err(|_| {
                    invalid(format!(
                        "{context}: interface {interface} is not an interface index (an IPv6 membership names the local interface by index, not by address)"
                    ))
                })?
            };
            if join {
                socket.join_multicast_v6(&group, index)
            } else {
                socket.leave_multicast_v6(&group, index)
            }
        }
    };
    result.map_err(|e| ProviderError::from_io(context, &e))
}

/// Applies one post-bind socket option.
///
/// The v4/v6 split is made the same way as at the bind, and from the same
/// evidence: the socket's own local address, not a guess from the option's
/// value.
pub(crate) fn set_option(socket: &UdpSocket, option: DatagramOption) -> Result<(), ProviderError> {
    let v6 = socket
        .local_addr()
        .map(|addr| addr.is_ipv6())
        .map_err(|e| ProviderError::from_io("socket option", &e))?;
    let raw = socket2::SockRef::from(socket);
    let result = match &option {
        DatagramOption::Ttl(ttl) => {
            if v6 {
                raw.set_unicast_hops_v6(*ttl)
            } else {
                raw.set_ttl_v4(*ttl)
            }
        }
        DatagramOption::Broadcast(on) => {
            if v6 {
                return Err(invalid(
                    "broadcast is IPv4-only (IPv6 has no broadcast address; use multicast)".into(),
                ));
            }
            raw.set_broadcast(*on)
        }
        DatagramOption::MulticastTtl(ttl) => {
            if v6 {
                raw.set_multicast_hops_v6(*ttl)
            } else {
                raw.set_multicast_ttl_v4(*ttl)
            }
        }
        DatagramOption::MulticastLoopback(on) => {
            if v6 {
                raw.set_multicast_loop_v6(*on)
            } else {
                raw.set_multicast_loop_v4(*on)
            }
        }
        DatagramOption::MulticastInterface(interface) => {
            if v6 {
                let index: u32 = if interface.is_empty() {
                    0
                } else {
                    interface.parse().map_err(|_| {
                        invalid(format!(
                            "multicast interface {interface} is not an interface index (an IPv6 socket names it by index, not by address)"
                        ))
                    })?
                };
                raw.set_multicast_if_v6(index)
            } else {
                let address: Ipv4Addr = if interface.is_empty() {
                    Ipv4Addr::UNSPECIFIED
                } else {
                    interface.parse().map_err(|_| {
                        invalid(format!(
                            "multicast interface {interface} is not an IPv4 address (an IPv4 socket names it by address)"
                        ))
                    })?
                };
                raw.set_multicast_if_v4(&address)
            }
        }
    };
    result.map_err(|e| ProviderError::from_io("socket option", &e))
}

/// A malformed argument. Uncoded, like the other "that is not a valid X"
/// refusals in this crate: nothing was attempted, so there is no I/O outcome to
/// classify, and the message is what the caller acts on.
fn invalid(message: String) -> ProviderError {
    ProviderError::Other(message)
}

/// Resolves `host:port` to the one address the socket will be built for. A name
/// with several addresses binds the first, which is what `UdpSocket::bind`
/// does with the same input.
async fn resolve(host: &str, port: u16, context: &str) -> Result<SocketAddr, ProviderError> {
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))?
        .next()
        .ok_or_else(|| ProviderError::Coded {
            code: ErrorCode::Dns,
            message: format!("{context}: the address resolved to nothing"),
        })
}

#[cfg(unix)]
fn set_reuse_port(socket: &Socket, context: &str) -> Result<(), ProviderError> {
    socket
        .set_reuse_port(true)
        .map_err(|e| ProviderError::from_io(context.to_string(), &e))
}

/// Windows has no `SO_REUSEPORT`, and its `SO_REUSEADDR` is a different thing —
/// refused rather than substituted, exactly as for a TCP listener
/// ([`listener`](crate::listener)).
#[cfg(not(unix))]
fn set_reuse_port(_socket: &Socket, context: &str) -> Result<(), ProviderError> {
    Err(ProviderError::Other(format!(
        "{context}: reusePort is not supported on this platform (SO_REUSEPORT is Unix-only)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain (any-source) membership, which is what most of these want.
    fn group(group: &str, interface: &str) -> MulticastMembership {
        MulticastMembership {
            group: group.to_string(),
            interface: interface.to_string(),
            source: None,
        }
    }

    #[tokio::test]
    async fn a_plain_bind_takes_an_ephemeral_port() {
        let socket = bind("127.0.0.1", 0, &DatagramOptions::default())
            .await
            .expect("bind");
        assert!(socket.local_addr().expect("addr").port() > 0);
    }

    #[tokio::test]
    async fn a_port_already_taken_is_address_in_use() {
        let held = bind("127.0.0.1", 0, &DatagramOptions::default())
            .await
            .expect("bind");
        let port = held.local_addr().expect("addr").port();
        let err = bind("127.0.0.1", port, &DatagramOptions::default())
            .await
            .expect_err("the second bind must fail");
        assert_eq!(err.code(), Some(ErrorCode::AddressInUse), "{err}");
    }

    /// `reuseAddress` is what lets two sockets hold one port — the shape mDNS
    /// and SSDP need. Both halves are asserted, so the option is doing the work
    /// rather than the port having been free.
    #[tokio::test]
    async fn reuse_address_lets_a_second_socket_share_the_port() {
        let opts = || DatagramOptions {
            reuse_address: true,
            // Linux wants both for two *unicast* binds to share a port; the
            // pair is what a real multicast listener sets anyway.
            reuse_port: cfg!(unix),
            ..DatagramOptions::default()
        };
        let first = bind("127.0.0.1", 0, &opts()).await.expect("first bind");
        let port = first.local_addr().expect("addr").port();
        bind("127.0.0.1", port, &opts())
            .await
            .expect("second bind shares the port");
        assert!(
            bind("127.0.0.1", port, &DatagramOptions::default())
                .await
                .is_err(),
            "a plain bind must still be refused"
        );
    }

    #[tokio::test]
    async fn options_reach_the_socket() {
        let socket = bind(
            "127.0.0.1",
            0,
            &DatagramOptions {
                broadcast: true,
                ttl: Some(7),
                multicast_ttl: Some(3),
                multicast_loopback: Some(false),
                ..DatagramOptions::default()
            },
        )
        .await
        .expect("bind");
        assert!(socket.broadcast().expect("broadcast"));
        assert_eq!(socket.ttl().expect("ttl"), 7);
        assert_eq!(socket.multicast_ttl_v4().expect("multicast ttl"), 3);
        assert!(!socket.multicast_loop_v4().expect("multicast loop"));
    }

    /// A v6 socket sets the v6 spelling of each option. Without the split, the
    /// v4 call would return `Ok(())` on some systems and change nothing.
    #[tokio::test]
    async fn a_v6_socket_takes_the_v6_options() {
        let bound = bind(
            "::1",
            0,
            &DatagramOptions {
                ttl: Some(9),
                multicast_ttl: Some(4),
                multicast_loopback: Some(false),
                ..DatagramOptions::default()
            },
        )
        .await;
        // A host with IPv6 disabled cannot run this; skip rather than fail.
        let Ok(socket) = bound else { return };
        assert!(socket.local_addr().expect("addr").is_ipv6());
        assert!(!socket.multicast_loop_v6().expect("multicast loop"));
    }

    #[tokio::test]
    async fn broadcast_over_ipv6_is_refused_rather_than_ignored() {
        let bound = bind(
            "::1",
            0,
            &DatagramOptions {
                broadcast: true,
                ..DatagramOptions::default()
            },
        )
        .await;
        let Err(err) = bound else {
            panic!("IPv6 has no broadcast; asking for it must fail");
        };
        assert!(err.to_string().contains("IPv4-only"), "{err}");
    }

    #[tokio::test]
    async fn joining_a_group_that_is_not_multicast_is_refused() {
        let socket = bind("0.0.0.0", 0, &DatagramOptions::default())
            .await
            .expect("bind");
        let err = set_membership(&socket, &group("127.0.0.1", ""), true)
            .expect_err("a unicast address is not a group");
        assert!(err.to_string().contains("not a multicast address"), "{err}");
        let err = set_membership(&socket, &group("all-hosts.local", ""), true)
            .expect_err("a name is not a group");
        assert!(err.to_string().contains("not an IP address"), "{err}");
    }

    #[tokio::test]
    async fn a_group_can_be_joined_and_left() {
        let socket = bind("0.0.0.0", 0, &DatagramOptions::default())
            .await
            .expect("bind");
        set_membership(&socket, &group("224.0.0.251", ""), true).expect("join");
        set_membership(&socket, &group("224.0.0.251", ""), false).expect("leave");
    }

    /// An IPv6 membership names its interface by index. An address there is a
    /// mistake that would otherwise be parsed as garbage or silently ignored.
    #[tokio::test]
    async fn a_v6_membership_refuses_an_address_as_its_interface() {
        let socket = bind("0.0.0.0", 0, &DatagramOptions::default())
            .await
            .expect("bind");
        let err = set_membership(&socket, &group("ff02::fb", "192.168.1.1"), true)
            .expect_err("an IPv6 membership takes an index");
        assert!(err.to_string().contains("interface index"), "{err}");
    }
}
