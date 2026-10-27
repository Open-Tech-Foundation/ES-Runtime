//! DNS messages (RFC 1035, with RFC 3596's AAAA, RFC 2782's SRV, RFC 3403's
//! NAPTR, RFC 8659's CAA and RFC 6891's EDNS0): one query out, one response
//! in. Pure — no I/O — and written for hostile input: every length is checked
//! against the bytes it claims, compressed names may only point backwards and
//! only so many times, and nothing is indexed without a bound.

use std::net::{Ipv4Addr, Ipv6Addr};

use es_runtime_providers::DnsRecord;

/// The UDP payload size offered in EDNS0: the size the 2020 DNS flag day
/// settled on, which no path fragments.
pub(crate) const UDP_PAYLOAD: u16 = 1232;

const HEADER: usize = 12;
const CLASS_IN: u16 = 1;
const TYPE_OPT: u16 = 41;
/// The most a name may be, on the wire.
const MAX_NAME: usize = 255;
/// How many compression pointers one name may follow.
const MAX_POINTERS: usize = 64;

/// The record types `resolveDns` asks for.
pub(crate) fn type_code(name: &str) -> Option<u16> {
    Some(match name {
        "A" => 1,
        "NS" => 2,
        "CNAME" => 5,
        "SOA" => 6,
        "PTR" => 12,
        "MX" => 15,
        "TXT" => 16,
        "AAAA" => 28,
        "SRV" => 33,
        "NAPTR" => 35,
        "CAA" => 257,
        _ => return None,
    })
}

/// Why a name cannot be asked about, or a response cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireError(pub String);

impl WireError {
    fn new(message: impl Into<String>) -> Self {
        WireError(message.into())
    }
}

/// A query for `name`, of type `qtype`, with recursion desired and an EDNS0
/// record offering [`UDP_PAYLOAD`].
pub(crate) fn query(id: u16, name: &str, qtype: u16) -> Result<Vec<u8>, WireError> {
    let mut out = Vec::with_capacity(HEADER + name.len() + 2 + 4 + 11);
    out.extend_from_slice(&id.to_be_bytes());
    // RD: ask the server to recurse.
    out.extend_from_slice(&0x0100u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // questions
    out.extend_from_slice(&0u16.to_be_bytes()); // answers
    out.extend_from_slice(&0u16.to_be_bytes()); // authority
    out.extend_from_slice(&1u16.to_be_bytes()); // additional: the OPT record
    encode_name(&mut out, name)?;
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    // OPT: the root name, the payload size in the class field, no flags.
    out.push(0);
    out.extend_from_slice(&TYPE_OPT.to_be_bytes());
    out.extend_from_slice(&UDP_PAYLOAD.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    Ok(out)
}

/// A name as labels. The name is taken as fully qualified — never extended by
/// search domains — and must be ASCII: an internationalized name is asked
/// about in its A-label (`xn--`) form.
fn encode_name(out: &mut Vec<u8>, name: &str) -> Result<(), WireError> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() {
        out.push(0);
        return Ok(());
    }
    if !name.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(WireError::new(format!(
            "{name:?} is not a DNS name: use ASCII, and the xn-- form for an international name"
        )));
    }
    let start = out.len();
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(WireError::new(format!(
                "{name:?} has a label of {} characters",
                label.len()
            )));
        }
        out.push(u8::try_from(label.len()).unwrap_or(63));
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    if out.len() - start > MAX_NAME {
        return Err(WireError::new(format!("{name:?} is longer than 255 bytes")));
    }
    Ok(())
}

/// What a response said.
#[derive(Debug, PartialEq)]
pub(crate) enum Answer {
    /// The records of the type asked for; empty when the name has none.
    Records(Vec<DnsRecord>),
    /// NXDOMAIN: the name does not exist.
    NoSuchName,
    /// The response was cut short: ask again over TCP.
    Truncated,
    /// The server failed (SERVFAIL, REFUSED…): ask another.
    ServerFailed(u8),
}

/// Reads a response. `None` when it does not answer this query — another id,
/// another question, not a response — which a resolver must ignore, since that
/// is what a spoofed reply looks like.
pub(crate) fn response(
    bytes: &[u8],
    id: u16,
    name: &str,
    qtype: u16,
) -> Result<Option<Answer>, WireError> {
    let mut r = Reader { bytes, pos: 0 };
    let got_id = r.u16()?;
    let flags = r.u16()?;
    let questions = r.u16()?;
    let answers = r.u16()?;
    r.u16()?; // authority
    r.u16()?; // additional
    let is_response = flags & 0x8000 != 0;
    let opcode = (flags >> 11) & 0xf;
    if got_id != id || !is_response || opcode != 0 || questions != 1 {
        return Ok(None);
    }
    let asked = read_name(bytes, &mut r.pos)?;
    let asked_type = r.u16()?;
    let asked_class = r.u16()?;
    let wanted = name.strip_suffix('.').unwrap_or(name);
    if !asked.trim_end_matches('.').eq_ignore_ascii_case(wanted)
        || asked_type != qtype
        || asked_class != CLASS_IN
    {
        return Ok(None);
    }
    if flags & 0x0200 != 0 {
        return Ok(Some(Answer::Truncated));
    }
    match (flags & 0xf) as u8 {
        0 => {}
        3 => return Ok(Some(Answer::NoSuchName)),
        code => return Ok(Some(Answer::ServerFailed(code))),
    }
    let mut records = Vec::new();
    for _ in 0..answers {
        read_name(bytes, &mut r.pos)?;
        let rtype = r.u16()?;
        let class = r.u16()?;
        r.u32()?; // ttl
        let len = usize::from(r.u16()?);
        let start = r.pos;
        let end = start
            .checked_add(len)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| short("a record"))?;
        r.pos = end;
        // Answers to a query can include the CNAME chain that led to the
        // records; only the type asked for is returned.
        if rtype == qtype && class == CLASS_IN {
            records.push(rdata(bytes, start, end, rtype)?);
        }
    }
    Ok(Some(Answer::Records(records)))
}

fn rdata(bytes: &[u8], start: usize, end: usize, rtype: u16) -> Result<DnsRecord, WireError> {
    let data = &bytes[start..end];
    let mut r = Reader {
        bytes: &bytes[..end],
        pos: start,
    };
    let record = match rtype {
        1 => {
            let octets: [u8; 4] = data.try_into().map_err(|_| short("an A record"))?;
            DnsRecord::Text(Ipv4Addr::from(octets).to_string())
        }
        28 => {
            let octets: [u8; 16] = data.try_into().map_err(|_| short("an AAAA record"))?;
            DnsRecord::Text(Ipv6Addr::from(octets).to_string())
        }
        2 | 5 | 12 => DnsRecord::Text(name_in(bytes, &mut r.pos, end)?),
        15 => {
            let preference = r.u16()?;
            DnsRecord::Mx {
                preference,
                exchange: name_in(bytes, &mut r.pos, end)?,
            }
        }
        16 => {
            let mut strings = Vec::new();
            while r.pos < end {
                strings.push(r.character_string()?);
            }
            DnsRecord::Txt(strings)
        }
        33 => {
            let priority = r.u16()?;
            let weight = r.u16()?;
            let port = r.u16()?;
            DnsRecord::Srv {
                priority,
                weight,
                port,
                target: name_in(bytes, &mut r.pos, end)?,
            }
        }
        6 => {
            let mname = name_in(bytes, &mut r.pos, end)?;
            let rname = name_in(bytes, &mut r.pos, end)?;
            let serial = r.u32()?;
            #[expect(
                clippy::cast_possible_wrap,
                reason = "RFC 1035 calls these signed 32-bit"
            )]
            let (refresh, retry, expire) = (r.u32()? as i32, r.u32()? as i32, r.u32()? as i32);
            DnsRecord::Soa {
                mname,
                rname,
                serial,
                refresh,
                retry,
                expire,
                minimum: r.u32()?,
            }
        }
        35 => {
            let order = r.u16()?;
            let preference = r.u16()?;
            let flags = r.character_string()?;
            let services = r.character_string()?;
            let regexp = r.character_string()?;
            DnsRecord::Naptr {
                order,
                preference,
                flags,
                services,
                regexp,
                replacement: name_in(bytes, &mut r.pos, end)?,
            }
        }
        257 => {
            let flags = r.u8()?;
            let tag_len = usize::from(r.u8()?);
            let tag = r.take(tag_len)?;
            let value = &bytes[r.pos..end];
            DnsRecord::Caa {
                critical: flags & 0x80 != 0,
                tag: String::from_utf8_lossy(tag).into_owned(),
                value: String::from_utf8_lossy(value).into_owned(),
            }
        }
        other => return Err(WireError::new(format!("record type {other} is not read"))),
    };
    Ok(record)
}

/// A name inside record data, which must end where the record does.
fn name_in(bytes: &[u8], pos: &mut usize, end: usize) -> Result<String, WireError> {
    let name = read_name(bytes, pos)?;
    if *pos > end {
        return Err(short("a name in a record"));
    }
    Ok(name)
}

/// A name at `*pos`, following compression pointers, as presentation text
/// with a trailing dot. A pointer must point backwards (so a chain cannot
/// loop) and at most [`MAX_POINTERS`] are followed; the name may total at
/// most [`MAX_NAME`] bytes. Bytes outside printable ASCII, and `.` or `\` in a
/// label, are escaped as `\DDD` or `\.`, so a hostile label cannot pass for
/// two.
fn read_name(bytes: &[u8], pos: &mut usize) -> Result<String, WireError> {
    let mut out = String::new();
    let mut at = *pos;
    let mut resume = None;
    let mut pointers = 0;
    let mut wire_len = 0usize;
    loop {
        let len = *bytes.get(at).ok_or_else(|| short("a name"))?;
        match len & 0xc0 {
            0x00 => {
                if len == 0 {
                    at += 1;
                    break;
                }
                let len = usize::from(len);
                let label = bytes
                    .get(at + 1..at + 1 + len)
                    .ok_or_else(|| short("a label"))?;
                wire_len += len + 1;
                if wire_len > MAX_NAME {
                    return Err(WireError::new("a name is longer than 255 bytes"));
                }
                for &b in label {
                    match b {
                        b'.' | b'\\' => {
                            out.push('\\');
                            out.push(char::from(b));
                        }
                        0x21..=0x7e => out.push(char::from(b)),
                        _ => out.push_str(&format!("\\{b:03}")),
                    }
                }
                out.push('.');
                at += 1 + len;
            }
            0xc0 => {
                let low = *bytes.get(at + 1).ok_or_else(|| short("a name pointer"))?;
                let target = usize::from(u16::from_be_bytes([len & 0x3f, low]));
                if target >= at {
                    return Err(WireError::new("a name pointer does not point backwards"));
                }
                pointers += 1;
                if pointers > MAX_POINTERS {
                    return Err(WireError::new("a name follows too many pointers"));
                }
                resume.get_or_insert(at + 2);
                at = target;
            }
            _ => return Err(WireError::new("a name uses a reserved label type")),
        }
    }
    *pos = resume.unwrap_or(at);
    if out.is_empty() {
        out.push('.');
    }
    Ok(out)
}

fn short(what: &str) -> WireError {
    WireError::new(format!("the response ends inside {what}"))
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| short("a field"))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A `<character-string>`: a length byte, then that many bytes.
    fn character_string(&mut self) -> Result<String, WireError> {
        let len = usize::from(self.u8()?);
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }
}

/// Reads `bytes` as a response for the fuzz target: any input either reads
/// or is refused, and nothing panics. The id, name and type expected are
/// taken from the input's own header and question, so that random bytes reach
/// the record readers rather than stopping at "not this question".
#[cfg(feature = "fuzzing")]
pub fn fuzz(bytes: &[u8]) {
    let Some(id) = bytes.get(..2).map(|b| u16::from_be_bytes([b[0], b[1]])) else {
        return;
    };
    let mut pos = HEADER;
    let Ok(name) = read_name(bytes, &mut pos) else {
        return;
    };
    let Some(qtype) = bytes
        .get(pos..pos + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
    else {
        return;
    };
    let _ = response(bytes, id, &name, qtype);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response to `query(id, name, qtype)` carrying `answers`, each written
    /// as (type, rdata), with the owner name a pointer to the question.
    fn reply(id: u16, name: &str, qtype: u16, flags: u16, answers: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&(0x8000 | flags).to_be_bytes());
        out.extend_from_slice(&1u16.to_be_bytes());
        out.extend_from_slice(&u16::try_from(answers.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        encode_name(&mut out, name).unwrap();
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&CLASS_IN.to_be_bytes());
        for (rtype, data) in answers {
            out.extend_from_slice(&[0xc0, 12]);
            out.extend_from_slice(&rtype.to_be_bytes());
            out.extend_from_slice(&CLASS_IN.to_be_bytes());
            out.extend_from_slice(&300u32.to_be_bytes());
            out.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
            out.extend_from_slice(data);
        }
        out
    }

    fn name(text: &str) -> Vec<u8> {
        let mut out = Vec::new();
        encode_name(&mut out, text).unwrap();
        out
    }

    #[test]
    fn a_query_asks_one_question_with_edns() {
        let q = query(0xabcd, "example.com", 33).unwrap();
        assert_eq!(&q[..4], &[0xab, 0xcd, 0x01, 0x00]);
        assert_eq!(&q[4..12], &[0, 1, 0, 0, 0, 0, 0, 1]);
        assert_eq!(&q[12..25], b"\x07example\x03com\x00");
        assert_eq!(&q[25..29], &[0, 33, 0, 1]);
        assert_eq!(&q[29..], &[0, 0, 41, 0x04, 0xd0, 0, 0, 0, 0, 0, 0]);
        // Fully qualified either way.
        assert_eq!(
            query(1, "example.com.", 1).unwrap(),
            query(1, "example.com", 1).unwrap()
        );
    }

    #[test]
    fn a_name_that_cannot_be_asked_is_refused() {
        for bad in [
            "a..b",
            &"x".repeat(64),
            "bücher.de",
            "a b.com",
            &format!("{}.com", ["abcdefghij"; 26].join(".")),
        ] {
            assert!(query(1, bad, 1).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_record_type_is_read() {
        let mut srv = vec![0, 10, 0, 5, 0x69, 0x89];
        srv.extend(name("db1.example.com"));
        let mut mx = vec![0, 10];
        mx.extend(name("mail.example.com"));
        let txt = b"\x05hello\x05world".to_vec();
        let caa = b"\x80\x05issueletsencrypt.org".to_vec();
        let mut soa = name("ns1.example.com");
        soa.extend(name("admin.example.com"));
        for n in [1u32, 7200, 3600, 1_209_600, 300] {
            soa.extend_from_slice(&n.to_be_bytes());
        }
        let mut naptr = vec![0, 100, 0, 10, 1, b'u', 7];
        naptr.extend_from_slice(b"E2U+sip");
        naptr.push(0);
        naptr.push(0);
        let cases: Vec<(u16, Vec<u8>, DnsRecord)> = vec![
            (1, vec![192, 0, 2, 1], DnsRecord::Text("192.0.2.1".into())),
            (
                28,
                vec![0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
                DnsRecord::Text("2001:db8::1".into()),
            ),
            (
                5,
                name("alias.example.com"),
                DnsRecord::Text("alias.example.com.".into()),
            ),
            (
                15,
                mx,
                DnsRecord::Mx {
                    preference: 10,
                    exchange: "mail.example.com.".into(),
                },
            ),
            (
                16,
                txt,
                DnsRecord::Txt(vec!["hello".into(), "world".into()]),
            ),
            (
                33,
                srv,
                DnsRecord::Srv {
                    priority: 10,
                    weight: 5,
                    port: 27017,
                    target: "db1.example.com.".into(),
                },
            ),
            (
                257,
                caa,
                DnsRecord::Caa {
                    critical: true,
                    tag: "issue".into(),
                    value: "letsencrypt.org".into(),
                },
            ),
            (
                6,
                soa,
                DnsRecord::Soa {
                    mname: "ns1.example.com.".into(),
                    rname: "admin.example.com.".into(),
                    serial: 1,
                    refresh: 7200,
                    retry: 3600,
                    expire: 1_209_600,
                    minimum: 300,
                },
            ),
            (
                35,
                naptr,
                DnsRecord::Naptr {
                    order: 100,
                    preference: 10,
                    flags: "u".into(),
                    services: "E2U+sip".into(),
                    regexp: String::new(),
                    replacement: ".".into(),
                },
            ),
        ];
        for (rtype, data, want) in cases {
            let bytes = reply(7, "example.com", rtype, 0, &[(rtype, data)]);
            assert_eq!(
                response(&bytes, 7, "example.com", rtype).unwrap(),
                Some(Answer::Records(vec![want]))
            );
        }
    }

    #[test]
    fn only_the_type_asked_for_is_returned() {
        let bytes = reply(
            7,
            "www.example.com",
            1,
            0,
            &[(5, name("example.com")), (1, vec![192, 0, 2, 9])],
        );
        assert_eq!(
            response(&bytes, 7, "www.example.com", 1).unwrap(),
            Some(Answer::Records(vec![DnsRecord::Text("192.0.2.9".into())]))
        );
    }

    #[test]
    fn a_reply_to_something_else_is_ignored() {
        let bytes = reply(7, "example.com", 1, 0, &[(1, vec![192, 0, 2, 1])]);
        assert_eq!(
            response(&bytes, 8, "example.com", 1).unwrap(),
            None,
            "another id"
        );
        assert_eq!(
            response(&bytes, 7, "evil.com", 1).unwrap(),
            None,
            "another question"
        );
        assert_eq!(
            response(&bytes, 7, "example.com", 28).unwrap(),
            None,
            "another type"
        );
        assert!(
            response(&bytes, 7, "EXAMPLE.com.", 1).unwrap().is_some(),
            "names compare case-insensitively"
        );
        let mut query = query(7, "example.com", 1).unwrap();
        assert_eq!(
            response(&query, 7, "example.com", 1).unwrap(),
            None,
            "not a response"
        );
        query.truncate(5);
        assert!(response(&query, 7, "example.com", 1).is_err());
    }

    #[test]
    fn codes_and_truncation_are_reported() {
        assert_eq!(
            response(&reply(1, "x.com", 1, 3, &[]), 1, "x.com", 1).unwrap(),
            Some(Answer::NoSuchName)
        );
        assert_eq!(
            response(&reply(1, "x.com", 1, 2, &[]), 1, "x.com", 1).unwrap(),
            Some(Answer::ServerFailed(2))
        );
        assert_eq!(
            response(&reply(1, "x.com", 1, 0x0200, &[]), 1, "x.com", 1).unwrap(),
            Some(Answer::Truncated)
        );
        assert_eq!(
            response(&reply(1, "x.com", 16, 0, &[]), 1, "x.com", 16).unwrap(),
            Some(Answer::Records(vec![]))
        );
    }

    #[test]
    fn hostile_names_are_refused_or_escaped() {
        // A pointer to itself, and one pointing forward.
        let mut bytes = reply(1, "x.com", 1, 0, &[]);
        let at = bytes.len();
        bytes.extend_from_slice(&[0xc0, u8::try_from(at).unwrap()]);
        let mut pos = at;
        assert!(read_name(&bytes, &mut pos).is_err());
        let mut forward = vec![0xc0, 5, 0, 0, 0, 1, b'a', 0];
        let mut pos = 0;
        assert!(read_name(&forward, &mut pos).is_err());
        forward[0] = 0x80;
        let mut pos = 0;
        assert!(
            read_name(&forward, &mut pos).is_err(),
            "reserved label type"
        );
        // A label holding a dot and a NUL is escaped, not split.
        let raw = b"\x05a.b\x00c\x03com\x00";
        let mut pos = 0;
        assert_eq!(read_name(raw, &mut pos).unwrap(), "a\\.b\\000c.com.");
        // A record whose data runs past the message.
        let mut bytes = reply(1, "x.com", 1, 0, &[(1, vec![1, 2, 3, 4])]);
        let len = bytes.len();
        bytes[len - 5] = 0xff;
        assert!(response(&bytes, 1, "x.com", 1).is_err());
    }
}
