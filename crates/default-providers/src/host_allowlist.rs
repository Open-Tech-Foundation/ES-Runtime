//! A network address allowlist — the policy behind `esrun --allow-net=<hosts>`
//! and `--allow-listen=<addresses>` (DECISIONS D38).
//!
//! One type, shared by every provider that reaches or binds an address, because
//! the *capability* is one grant and the guest should not have to learn which
//! provider serves which API: `fetch`, `runtime:net` `connect`, and `WebSocket`
//! all consult the same list for `net`, and `runtime:net` `listen` and
//! `runtime:http` `serve` the same list for `listen`.
//!
//! **Matching is exact and never widens.** A host entry matches that host and
//! no other — `example.com` does not admit `api.example.com`, because a
//! subdomain is a different server, often a different operator, and a list that
//! quietly covered them would grant more than it reads as. There are no
//! wildcards for the same reason.

use std::net::IpAddr;

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;

/// The host half of an entry: an IP literal (compared as an address, so `::1`
/// and `0:0:0:0:0:0:0:1` are one host) or a name (compared lowercased, as DNS
/// is case-insensitive).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Host {
    Ip(IpAddr),
    Name(String),
}

impl Host {
    fn parse(text: &str) -> Host {
        match text.parse::<IpAddr>() {
            Ok(ip) => Host::Ip(ip),
            Err(_) => Host::Name(text.to_ascii_lowercase()),
        }
    }

    fn matches(&self, host: &str) -> bool {
        match self {
            // An IP entry matches the same address however it was written; it
            // never matches a *name* that happens to resolve there, since the
            // check runs before resolution and a name is a different fact.
            Host::Ip(ip) => host.parse::<IpAddr>().is_ok_and(|other| other == *ip),
            Host::Name(name) => !host.contains(':') && host.eq_ignore_ascii_case(name),
        }
    }
}

/// One entry of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// `example.com` — that host, on any port.
    AnyPort(Host),
    /// `example.com:443` — that host, that port.
    Exact(Host, u16),
    /// `8080` — any host, that port. Written for `--allow-listen`, where the
    /// port is usually the whole intent and the interface is the deployment's
    /// business.
    AnyHost(u16),
}

/// Addresses a capability may be used with. An empty list permits nothing.
#[derive(Clone, Debug, Default)]
pub struct HostAllowlist {
    entries: Vec<Entry>,
}

impl HostAllowlist {
    /// Parses `entries` — each `host`, `host:port`, or a bare `port` — into a
    /// list, or returns a message naming the entry that is malformed.
    ///
    /// An IPv6 literal must be bracketed when it carries a port
    /// (`[::1]:8080`), exactly as it is in a URL; bare `::1` is a host.
    pub fn parse<I, S>(entries: I) -> Result<HostAllowlist, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut parsed = Vec::new();
        for entry in entries {
            parsed.push(parse_entry(entry.as_ref())?);
        }
        Ok(HostAllowlist { entries: parsed })
    }

    /// Whether `host` on `port` is permitted. `host` is a hostname or an IP
    /// literal, as written by the guest — the check runs **before** resolution,
    /// so a name is judged as a name.
    pub fn permits(&self, host: &str, port: u16) -> bool {
        self.entries.iter().any(|entry| match entry {
            Entry::AnyPort(h) => h.matches(host),
            Entry::Exact(h, p) => *p == port && h.matches(host),
            Entry::AnyHost(p) => *p == port,
        })
    }

    /// [`permits`](Self::permits), as a provider error naming what was refused.
    ///
    /// The code is [`ErrorCode::PermissionDenied`] — a **scoped** denial, and
    /// deliberately not the `ERR_CAPABILITY_DENIED` a missing capability
    /// raises: "you have `net`, but not this host" and "you never had `net`"
    /// are different facts, and a caller that degrades or retries needs to tell
    /// them apart.
    pub fn check(&self, host: &str, port: u16, action: &str) -> Result<(), ProviderError> {
        if self.permits(host, port) {
            return Ok(());
        }
        Err(ProviderError::Coded {
            code: ErrorCode::PermissionDenied,
            message: format!("{host}:{port} is not an allowed address ({action})"),
        })
    }

    /// [`check`](Self::check) against a URL's host and port, defaulting the
    /// port from the scheme (`https` ⇒ 443, `wss` ⇒ 443, …) the way the guest's
    /// own connection will.
    pub fn check_url(&self, url: &url::Url, action: &str) -> Result<(), ProviderError> {
        let host = url.host_str().ok_or_else(|| ProviderError::Coded {
            code: ErrorCode::PermissionDenied,
            message: format!("{url} has no host to check against the allowed addresses"),
        })?;
        // `host_str` keeps IPv6 brackets; the entries store bare addresses.
        let host = host.trim_start_matches('[').trim_end_matches(']');
        let port = url.port_or_known_default().ok_or_else(|| {
            // No default port for the scheme, and none given: there is nothing
            // to match, and guessing would be the wrong direction to guess in.
            ProviderError::Coded {
                code: ErrorCode::PermissionDenied,
                message: format!("{url} has no port to check against the allowed addresses"),
            }
        })?;
        self.check(host, port, action)
    }
}

/// Parses one entry. Returns the entry, or a message naming what is wrong with
/// it — an unusable entry is always an error, never a silently dropped one: a
/// list item that vanished would leave the run *narrower* than it reads, which
/// fails a working program mysteriously, or (if it was the only entry) wider.
fn parse_entry(entry: &str) -> Result<Entry, String> {
    // Bracketed IPv6, with or without a port: [::1] / [::1]:8080.
    if let Some(rest) = entry.strip_prefix('[') {
        let (inside, after) = rest
            .split_once(']')
            .ok_or_else(|| format!("{entry} is missing the closing ']' of its IPv6 address"))?;
        let ip: IpAddr = inside
            .parse()
            .map_err(|_| format!("{inside} (in {entry}) is not an IPv6 address"))?;
        return match after {
            "" => Ok(Entry::AnyPort(Host::Ip(ip))),
            port => Ok(Entry::Exact(Host::Ip(ip), parse_port(entry, port)?)),
        };
    }
    // A bare number is a port on any host: `--allow-listen=8080`.
    if !entry.is_empty() && entry.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Entry::AnyHost(parse_port(entry, entry)?));
    }
    match entry.rsplit_once(':') {
        // `host:port`, unless the left half is itself part of an unbracketed
        // IPv6 address (`::1`, `fe80::1`), which is a host and nothing more.
        Some((host, port)) if !host.contains(':') => {
            if host.is_empty() {
                return Err(format!("{entry} has no host before its port"));
            }
            Ok(Entry::Exact(Host::parse(host), parse_port(entry, port)?))
        }
        _ => {
            if entry.contains(':') {
                // Unbracketed IPv6: accept it as a host, reject a typo.
                let ip: IpAddr = entry.parse().map_err(|_| {
                    format!("{entry} is not an address (bracket IPv6 as [::1]:8080)")
                })?;
                return Ok(Entry::AnyPort(Host::Ip(ip)));
            }
            Ok(Entry::AnyPort(Host::parse(entry)))
        }
    }
}

fn parse_port(entry: &str, port: &str) -> Result<u16, String> {
    let port = port.strip_prefix(':').unwrap_or(port);
    port.parse::<u16>()
        .map_err(|_| format!("{port} (in {entry}) is not a port number"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(entries: &[&str]) -> HostAllowlist {
        HostAllowlist::parse(entries).expect("parse")
    }

    #[test]
    fn a_bare_host_matches_any_port() {
        let allow = list(&["example.com"]);
        assert!(allow.permits("example.com", 443));
        assert!(allow.permits("example.com", 8080));
        assert!(!allow.permits("other.com", 443));
    }

    #[test]
    fn a_host_with_a_port_matches_only_that_port() {
        let allow = list(&["db.internal:5432"]);
        assert!(allow.permits("db.internal", 5432));
        assert!(!allow.permits("db.internal", 5433));
    }

    #[test]
    fn a_bare_port_matches_any_host() {
        // The `--allow-listen=8080` shape: the port is the intent, the
        // interface is the deployment's business.
        let allow = list(&["8080"]);
        assert!(allow.permits("0.0.0.0", 8080));
        assert!(allow.permits("127.0.0.1", 8080));
        assert!(!allow.permits("127.0.0.1", 9090));
    }

    #[test]
    fn a_subdomain_is_a_different_host() {
        // No implicit widening: api.example.com is a different server, and a
        // list that covered it would grant more than it reads as.
        let allow = list(&["example.com"]);
        assert!(!allow.permits("api.example.com", 443));
        assert!(!allow.permits("example.com.evil.test", 443));
        assert!(!allow.permits("notexample.com", 443));
    }

    #[test]
    fn host_names_match_case_insensitively() {
        assert!(list(&["Example.COM"]).permits("example.com", 80));
        assert!(list(&["example.com"]).permits("EXAMPLE.com", 80));
    }

    #[test]
    fn ip_literals_match_as_addresses_not_strings() {
        assert!(list(&["::1"]).permits("0:0:0:0:0:0:0:1", 80));
        assert!(list(&["[::1]:8080"]).permits("::1", 8080));
        assert!(!list(&["[::1]:8080"]).permits("::1", 9090));
        assert!(list(&["127.0.0.1"]).permits("127.0.0.1", 80));
        // Never a name for the address it might resolve to: the check runs
        // before resolution, and DNS is not part of the policy.
        assert!(!list(&["127.0.0.1"]).permits("localhost", 80));
    }

    #[test]
    fn an_empty_list_permits_nothing() {
        let allow = HostAllowlist::default();
        assert!(!allow.permits("example.com", 443));
    }

    #[test]
    fn entries_accumulate() {
        let allow = list(&["example.com", "db.internal:5432", "8080"]);
        assert!(allow.permits("example.com", 443));
        assert!(allow.permits("db.internal", 5432));
        assert!(allow.permits("anything", 8080));
        assert!(!allow.permits("anything", 8081));
    }

    #[test]
    fn a_malformed_entry_is_an_error_not_a_dropped_entry() {
        // Dropping it would leave the run narrower than it reads (a working
        // program fails mysteriously) or, if it was the only entry, wider.
        for bad in [
            "example.com:99999",
            "example.com:http",
            "example.com:",
            ":443",
            "[::1",
            "[not-an-ip]:80",
            "fe80::1:oops",
        ] {
            assert!(
                HostAllowlist::parse([bad]).is_err(),
                "{bad} should not parse"
            );
        }
    }

    #[test]
    fn check_names_the_address_and_the_action() {
        let err = list(&["example.com"])
            .check("evil.test", 443, "fetch")
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("evil.test:443"), "{message}");
        assert!(message.contains("fetch"), "{message}");
        // A scoped denial, distinct from a missing capability.
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
    }

    #[test]
    fn check_url_defaults_the_port_from_the_scheme() {
        let allow = list(&["example.com:443"]);
        let https = url::Url::parse("https://example.com/x").unwrap();
        let http = url::Url::parse("http://example.com/x").unwrap();
        assert!(allow.check_url(&https, "fetch").is_ok());
        assert!(allow.check_url(&http, "fetch").is_err());
    }

    #[test]
    fn check_url_strips_ipv6_brackets_before_matching() {
        let allow = list(&["[::1]:8080"]);
        let url = url::Url::parse("http://[::1]:8080/x").unwrap();
        assert!(allow.check_url(&url, "fetch").is_ok());
    }
}
