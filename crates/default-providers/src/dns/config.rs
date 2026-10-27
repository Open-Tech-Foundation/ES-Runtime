//! Which DNS servers to ask: the system's.
//!
//! On Unix — Linux and macOS alike — that is the `nameserver` lines of
//! `/etc/resolv.conf`, which macOS keeps in step with its active resolvers. A
//! macOS VPN's split DNS lives only in its scoped resolvers, so a name served
//! that way needs `resolveDns`'s `nameServer`. On Windows it is the DNS servers
//! of the adapters that are up (`windows.rs`).

use std::net::SocketAddr;
#[cfg(any(unix, test))]
use std::net::{IpAddr, Ipv4Addr};

/// glibc's `MAXNS`: no more servers than this are asked.
const MAX_SERVERS: usize = 3;

/// The system's DNS servers, in the order to ask them.
pub(crate) fn system_servers() -> Vec<SocketAddr> {
    #[cfg(unix)]
    {
        let text = std::fs::read_to_string("/etc/resolv.conf").unwrap_or_default();
        resolv_conf(&text)
    }
    #[cfg(windows)]
    {
        super::windows::adapter_dns_servers()
            .into_iter()
            .take(MAX_SERVERS)
            .map(|ip| SocketAddr::new(ip, 53))
            .collect()
    }
}

/// The servers a `resolv.conf` names. With none, the local machine, as
/// resolv.conf(5) specifies. A scoped IPv6 address (`fe80::1%eth0`) cannot be
/// reached without its interface and is skipped.
#[cfg(any(unix, test))]
pub(crate) fn resolv_conf(text: &str) -> Vec<SocketAddr> {
    let servers: Vec<SocketAddr> = text
        .lines()
        .filter_map(|line| {
            let line = line.split(['#', ';']).next().unwrap_or("");
            let mut words = line.split_whitespace();
            (words.next() == Some("nameserver"))
                .then(|| words.next())
                .flatten()
        })
        .filter_map(|address| address.parse::<IpAddr>().ok())
        .take(MAX_SERVERS)
        .map(|ip| SocketAddr::new(ip, 53))
        .collect();
    if servers.is_empty() {
        vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53)]
    } else {
        servers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nameserver_lines_are_read_in_order_and_others_ignored() {
        let text = "\
# generated
search corp.example
nameserver 10.0.0.2   # primary
nameserver 2001:db8::53
nameserver fe80::1%eth0
options ndots:5
nameserver 10.0.0.3
nameserver 10.0.0.4
";
        let servers: Vec<String> = resolv_conf(text).iter().map(ToString::to_string).collect();
        assert_eq!(servers, ["10.0.0.2:53", "[2001:db8::53]:53", "10.0.0.3:53"]);
    }

    #[test]
    fn without_a_nameserver_the_local_machine_is_asked() {
        assert_eq!(
            resolv_conf("search example.com\n"),
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 53)]
        );
        assert_eq!(resolv_conf("nameserver not-an-ip\n").len(), 1);
    }
}
