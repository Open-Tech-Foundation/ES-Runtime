//! `mongodb+srv://`: MongoDB's initial DNS seedlist discovery (DECISIONS D188).
//!
//! One host name becomes the seed list through an SRV lookup, and a TXT
//! lookup on the same name may supply `authSource`, `replicaSet` and
//! `loadBalanced` defaults. Here is the part that decides which hosts the
//! driver will connect to — so here, not in JavaScript, every returned name is
//! verified against the SRV name's domain before any connection is made.
//! Which options win over the TXT record's is JavaScript's, beside the rest of
//! the connection string.
//!
//! The lookups go through the agent's [`NetProvider`], so the host allowlist
//! judges them as it judges `resolveDns`.

use es_runtime_common::IntoException;
use es_runtime_providers::{BoxFuture, DnsQuery, DnsRecord, ProviderError};

/// What discovery found: the seeds, as `host:port`, and the TXT record's
/// options as `(name, value)` in the order written.
#[derive(Debug, PartialEq)]
pub(crate) struct Seedlist {
    pub hosts: Vec<String>,
    pub txt: Vec<(String, String)>,
}

/// Why discovery gave no seeds: a lookup the host allowlist refused, which
/// keeps its code, or anything else, which is the connection string's fault
/// or the DNS records'.
#[derive(Debug)]
pub(crate) enum Refused {
    Denied(ProviderError),
    Invalid(String),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Denied(e) => f.write_str(&e.exception_message()),
            Refused::Invalid(message) => f.write_str(message),
        }
    }
}

impl From<String> for Refused {
    fn from(message: String) -> Self {
        Refused::Invalid(message)
    }
}

/// A lookup's failure: a denial as itself, anything else described.
fn lookup_failed(e: ProviderError, what: String) -> Refused {
    if e.code() == Some(es_runtime_common::ErrorCode::PermissionDenied) {
        Refused::Denied(e)
    } else {
        Refused::Invalid(format!("{what}: {}", e.exception_message()))
    }
}

/// The options a TXT record may set; any other is an error.
const TXT_OPTIONS: [&str; 3] = ["authSource", "replicaSet", "loadBalanced"];

/// The lookups discovery makes: an SRV or TXT query.
pub(crate) type Resolve<'a> =
    &'a dyn Fn(DnsQuery) -> BoxFuture<Result<Vec<DnsRecord>, ProviderError>>;

/// Finds the seeds for `mongodb+srv://{host}` with service `service`, keeping
/// at most `max_hosts` (zero: all) chosen by `pick`, which returns an index
/// below its argument.
pub(crate) async fn discover(
    resolve: Resolve<'_>,
    host: &str,
    service: &str,
    max_hosts: usize,
    pick: &mut dyn FnMut(usize) -> usize,
) -> Result<Seedlist, Refused> {
    let host = normalize(host);
    check_service(service)?;
    let srv_name = format!("_{service}._tcp.{host}");
    let records = resolve(query(&srv_name, "SRV"))
        .await
        .map_err(|e| lookup_failed(e, format!("{srv_name} found no hosts")))?;
    let mut hosts = Vec::new();
    for record in records {
        if let DnsRecord::Srv { port, target, .. } = record {
            let target = normalize(&target);
            check_target(&host, &target)?;
            hosts.push(format!("{target}:{port}"));
        }
    }
    if hosts.is_empty() {
        return Err(format!("{srv_name} has no SRV records, so it names no hosts").into());
    }
    if max_hosts > 0 && max_hosts < hosts.len() {
        // Fisher-Yates, as the specification recommends, stopping once the
        // first `max_hosts` are chosen.
        for i in 0..max_hosts {
            let j = i + pick(hosts.len() - i);
            hosts.swap(i, j);
        }
        hosts.truncate(max_hosts);
    }
    let txt = match resolve(query(&host, "TXT")).await {
        Ok(records) => txt_options(&records)?,
        // No TXT record is no defaults.
        Err(ProviderError::Coded {
            code: es_runtime_common::ErrorCode::NotFound,
            ..
        }) => Vec::new(),
        Err(e) => {
            return Err(lookup_failed(
                e,
                format!("the TXT lookup for {host} failed"),
            ));
        }
    };
    Ok(Seedlist { hosts, txt })
}

fn query(name: &str, record_type: &str) -> DnsQuery {
    DnsQuery {
        name: name.to_string(),
        record_type: record_type.to_string(),
        name_server: None,
    }
}

/// A host name as the specification compares them: without a trailing dot,
/// in lowercase. (Names arrive in ASCII — the resolver asks only ASCII names
/// and escapes any other byte in what it reads — so the A-label step is a
/// no-op here.)
fn normalize(name: &str) -> String {
    name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase()
}

/// An SRV service name: letters, digits and hyphens, as RFC 6335 has it,
/// short enough that `_{service}` is one DNS label.
fn check_service(service: &str) -> Result<(), String> {
    let valid = !service.is_empty()
        && service.len() <= 62
        && service
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !service.starts_with('-')
        && !service.ends_with('-');
    if valid {
        Ok(())
    } else {
        Err(format!("{service:?} is not an SRV service name"))
    }
}

/// Whether `target`, returned for SRV name `host`, may be connected to: it
/// must be a plain host name in the SRV name's domain — the name without its
/// first label when it has three or more labels, else the whole name, with at
/// least one more label in front. This is what stops a DNS answer sending the
/// driver, and its credentials, to a host the connection string did not name.
pub(crate) fn check_target(host: &str, target: &str) -> Result<(), String> {
    let plain = !target.is_empty()
        && target.split('.').all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
    if !plain {
        return Err(format!(
            "the SRV record for {host} returned {target:?}, which is not a host name"
        ));
    }
    let labels = host.split('.').count();
    let domain = if labels >= 3 {
        host.split_once('.').map_or(host, |(_, rest)| rest)
    } else {
        host
    };
    if !target.ends_with(&format!(".{domain}")) {
        return Err(format!(
            "the SRV record for {host} returned {target}, which is not in the domain {domain}"
        ));
    }
    Ok(())
}

/// The options in the TXT records for the SRV name: at most one record,
/// whose strings join into one query string of only [`TXT_OPTIONS`].
fn txt_options(records: &[DnsRecord]) -> Result<Vec<(String, String)>, String> {
    let texts: Vec<String> = records
        .iter()
        .filter_map(|r| match r {
            DnsRecord::Txt(parts) => Some(parts.concat()),
            _ => None,
        })
        .collect();
    let text = match texts.as_slice() {
        [] => return Ok(Vec::new()),
        [one] => one,
        _ => {
            return Err(
                "the host has more than one TXT record; at most one may give options".into(),
            );
        }
    };
    let mut options = Vec::new();
    for pair in text.split('&').filter(|p| !p.is_empty()) {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("the TXT record's {pair:?} is not name=value"));
        };
        let Some(name) = TXT_OPTIONS.iter().find(|o| o.eq_ignore_ascii_case(key)) else {
            return Err(format!(
                "a TXT record may set only authSource, replicaSet and loadBalanced, not {key}"
            ));
        };
        options.push(((*name).to_string(), percent_decode(value)?));
    }
    Ok(options)
}

/// `%XX` escapes, as in a connection string's options.
fn percent_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            let Some(b) = hex else {
                return Err(format!("{text:?} has a bad % escape"));
            };
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| format!("{text:?} is not UTF-8 once unescaped"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use es_runtime_common::ErrorCode;

    /// A resolver answering from `records`: (name, type) → records. An absent
    /// name is NXDOMAIN.
    fn fake(
        records: Vec<(&'static str, &'static str, Vec<DnsRecord>)>,
    ) -> impl Fn(DnsQuery) -> BoxFuture<Result<Vec<DnsRecord>, ProviderError>> {
        move |q: DnsQuery| {
            let found: Option<Vec<DnsRecord>> = if records.iter().any(|(n, _, _)| *n == q.name) {
                Some(
                    records
                        .iter()
                        .filter(|(n, t, _)| *n == q.name && *t == q.record_type)
                        .flat_map(|(_, _, r)| r.clone())
                        .collect(),
                )
            } else {
                None
            };
            Box::pin(async move {
                found.ok_or(ProviderError::Coded {
                    code: ErrorCode::NotFound,
                    message: "no such name".into(),
                })
            })
        }
    }

    fn srv(port: u16, target: &str) -> DnsRecord {
        DnsRecord::Srv {
            priority: 0,
            weight: 0,
            port,
            target: target.into(),
        }
    }

    fn run(resolve: Resolve<'_>, host: &str, max: usize) -> Result<Seedlist, String> {
        let mut next = 0;
        let mut pick = |n: usize| {
            next += 1;
            next % n
        };
        futures_util::FutureExt::now_or_never(discover(resolve, host, "mongodb", max, &mut pick))
            .unwrap()
            .map_err(|e| e.to_string())
    }

    #[test]
    fn hosts_and_txt_options_are_found() {
        let dns = fake(vec![
            (
                "_mongodb._tcp.server.mongodb.com",
                "SRV",
                vec![
                    srv(27317, "MongoDB1.mongodb.com."),
                    srv(27017, "mongodb2.mongodb.com."),
                ],
            ),
            (
                "server.mongodb.com",
                "TXT",
                vec![DnsRecord::Txt(vec![
                    "replicaSet=replProd&auth".into(),
                    "Source=auth%2FDB".into(),
                ])],
            ),
        ]);
        assert_eq!(
            run(&dns, "Server.MongoDB.com.", 0).unwrap(),
            Seedlist {
                hosts: vec![
                    "mongodb1.mongodb.com:27317".into(),
                    "mongodb2.mongodb.com:27017".into()
                ],
                txt: vec![
                    ("replicaSet".into(), "replProd".into()),
                    ("authSource".into(), "auth/DB".into())
                ],
            }
        );
    }

    // The specification's prose tests 1–4, with stubbed DNS.
    #[test]
    fn a_name_of_fewer_than_three_parts_is_allowed() {
        for host in ["localhost", "mongo.local"] {
            let name: &'static str = Box::leak(format!("_mongodb._tcp.{host}").into_boxed_str());
            let target: &'static str = Box::leak(format!("db.{host}").into_boxed_str());
            let dns = fake(vec![(name, "SRV", vec![srv(27017, target)])]);
            assert_eq!(
                run(&dns, host, 0).unwrap().hosts,
                [format!("{target}:27017")]
            );
        }
    }

    #[test]
    fn a_target_outside_the_domain_or_identical_to_a_short_name_is_refused() {
        for (host, target) in [
            ("localhost", "localhost.mongodb"),
            ("mongo.local", "test_1.evil.local"),
            ("blogs.mongodb.com", "blogs.evil.com"),
            ("localhost", "localhost"),
            ("mongo.local", "mongo.local"),
            ("localhost", "test_1.cluster_1localhost"),
            ("mongo.local", "test_1.my_hostmongo.local"),
            ("blogs.mongodb.com", "cluster.testmongodb.com"),
        ] {
            assert!(check_target(host, target).is_err(), "{host} → {target}");
        }
        assert!(
            check_target("test.build.10gen.cc", "test.build.10gen.cc").is_ok(),
            "three parts may return itself"
        );
        assert!(check_target("blogs.mongodb.com", "a.b.mongodb.com").is_ok());
    }

    #[test]
    fn a_target_that_is_not_a_plain_host_name_is_refused() {
        for target in [
            "evil\\.com.mongodb.com",
            "a..mongodb.com",
            "a b.mongodb.com",
            "a\\000.mongodb.com",
        ] {
            assert!(
                check_target("blogs.mongodb.com", target).is_err(),
                "{target}"
            );
        }
    }

    #[test]
    fn one_bad_target_refuses_the_whole_answer() {
        let dns = fake(vec![(
            "_mongodb._tcp.blogs.mongodb.com",
            "SRV",
            vec![srv(27017, "a.mongodb.com"), srv(27017, "a.evil.com")],
        )]);
        assert!(
            run(&dns, "blogs.mongodb.com", 0)
                .unwrap_err()
                .contains("not in the domain")
        );
    }

    #[test]
    fn no_srv_records_is_an_error_and_no_txt_record_is_none() {
        let none = fake(vec![]);
        assert!(
            run(&none, "a.example.com", 0)
                .unwrap_err()
                .contains("found no hosts")
        );
        let empty = fake(vec![("_mongodb._tcp.a.example.com", "TXT", vec![])]);
        assert!(
            run(&empty, "a.example.com", 0)
                .unwrap_err()
                .contains("no SRV records")
        );
        let no_txt = fake(vec![(
            "_mongodb._tcp.a.example.com",
            "SRV",
            vec![srv(1, "b.example.com")],
        )]);
        assert_eq!(run(&no_txt, "a.example.com", 0).unwrap().txt, []);
    }

    #[test]
    fn txt_records_are_held_to_three_options_in_one_record() {
        let one = |s: &str| DnsRecord::Txt(vec![s.into()]);
        assert!(
            txt_options(&[one("replicaSet=a"), one("authSource=b")])
                .unwrap_err()
                .contains("more than one")
        );
        assert!(
            txt_options(&[one("ssl=false")])
                .unwrap_err()
                .contains("not ssl")
        );
        assert!(txt_options(&[one("socketTimeoutMS=500")]).is_err());
        assert!(
            txt_options(&[one("authSource")])
                .unwrap_err()
                .contains("name=value")
        );
        assert_eq!(
            txt_options(&[one("loadbalanced=true")]).unwrap(),
            [("loadBalanced".to_string(), "true".to_string())]
        );
    }

    #[test]
    fn srv_max_hosts_keeps_that_many_distinct_hosts() {
        let dns = fake(vec![(
            "_mongodb._tcp.a.example.com",
            "SRV",
            (1..=5).map(|p| srv(p, "b.example.com")).collect(),
        )]);
        let hosts = run(&dns, "a.example.com", 2).unwrap().hosts;
        assert_eq!(hosts.len(), 2);
        assert_ne!(hosts[0], hosts[1]);
        assert_eq!(run(&dns, "a.example.com", 5).unwrap().hosts.len(), 5);
        assert_eq!(run(&dns, "a.example.com", 9).unwrap().hosts.len(), 5);
    }

    #[test]
    fn a_service_name_must_be_one_plain_label() {
        for bad in ["", "a.b", "-x", "x_y", &"x".repeat(63)] {
            assert!(check_service(bad).is_err(), "{bad:?}");
        }
        assert!(check_service("customname").is_ok());
    }
}
