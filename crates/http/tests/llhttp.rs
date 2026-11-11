//! llhttp's strict request cases (crates/http/tests/corpus/llhttp), run
//! through our parser and decoder.
//!
//! Each case is an `http` block and the event log llhttp produces for it. What
//! is compared is the verdict: did llhttp refuse the bytes, and how many
//! messages did it complete. Disagreements fall into two kinds:
//!
//! - **llhttp refuses, we accept.** We would be the more lenient parser — the
//!   side request smuggling exploits. There must be none.
//! - **llhttp accepts, we refuse.** Deliberate strictness (D217): HTTP/1.1
//!   only, a required `Host`, CRLF line endings. Each such case must refuse
//!   for one of the reasons in [`DELIBERATELY_STRICTER`]; a refusal for any
//!   other reason is a bug to look at, not a difference to accept.

// The summary line is the point of running this by hand.
#![allow(clippy::print_stderr)]

use es_runtime_http::h1::{
    Body, Chunk, ChunkedDecoder, Limits, Parsed, RequestError, parse_request,
};

/// Why we may refuse what llhttp accepts. Every entry is a rule D217 chose.
const DELIBERATELY_STRICTER: &[&str] = &[
    // HTTP/1.1 only: llhttp parses 1.0 and 0.9; we answer 505.
    "HTTP version not supported",
    // RFC 9112 §3.2: a server MUST answer 400 to an HTTP/1.1 request without
    // exactly one Host. llhttp leaves that to its user.
    "malformed request: missing Host",
    "malformed request: more than one Host",
    // llhttp accepts any Expect and leaves it to the user; we answer 417.
    "unsupported expectation",
    // Other protocols on HTTP's syntax (RTSP/1.0, ICE/1.0) and HTTP/0.9's
    // version-less request line: llhttp parses them, we speak HTTP/1.1.
    "malformed request: invalid HTTP version",
    "malformed request: no version",
    // RFC 9112 §3.2.4: the asterisk form is for server-wide OPTIONS. llhttp
    // also allows it for SSDP's M-SEARCH, which is not HTTP.
    "malformed request: * is only for OPTIONS",
    // A coding under `chunked` that we do not decode: RFC 9112 §6.1 says a
    // server should answer 501; llhttp passes the body on still encoded.
    "unsupported transfer coding",
    // After a switch the connection speaks another protocol; a body in between
    // is where the two ends could disagree. Refused, as Deno's parser does.
    "malformed request: an upgrade request with a body",
];

#[derive(Debug)]
struct Case {
    file: String,
    name: String,
    input: Vec<u8>,
    llhttp_error: Option<String>,
    llhttp_messages: usize,
}

fn unescape(block: &str) -> Vec<u8> {
    // The converter llhttp's own test runner uses (test/md-test.ts), in order:
    // drop the final newline, join escaped line breaks, make every newline a
    // CRLF, then expand \r \n \t \f \xHH and octal escapes.
    let mut s = block.strip_suffix('\n').unwrap_or(block).to_string();
    s = s.replace("\\\r\n", "").replace("\\\n", "");
    s = s
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n");
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                    continue;
                }
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                    continue;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                    continue;
                }
                b'f' => {
                    out.push(0x0c);
                    i += 2;
                    continue;
                }
                b'x' => {
                    let hex: String = bytes[i + 2..]
                        .iter()
                        .take_while(|b| b.is_ascii_hexdigit())
                        .map(|&b| b as char)
                        .collect();
                    if !hex.is_empty() {
                        out.push(u8::from_str_radix(&hex, 16).unwrap());
                        i += 2 + hex.len();
                        continue;
                    }
                }
                b'0'..=b'7' => {
                    let oct: String = bytes[i + 1..]
                        .iter()
                        .take(3)
                        .take_while(|b| (b'0'..=b'7').contains(b))
                        .map(|&b| b as char)
                        .collect();
                    out.push(u8::from_str_radix(&oct, 8).unwrap());
                    i += 1 + oct.len();
                    continue;
                }
                _ => {}
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn load() -> Vec<Case> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus/llhttp/request");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut cases = Vec::new();
    for path in files {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(&path).unwrap();
        let mut name = String::new();
        let mut lines = text.lines().peekable();
        while let Some(line) = lines.next() {
            if let Some(h) = line.strip_prefix('#') {
                name = h.trim_start_matches('#').trim().to_string();
                continue;
            }
            if !line.starts_with("<!-- meta=") {
                continue;
            }
            let meta = line.replace(' ', "");
            let strict = meta.contains("\"type\":\"request\"");
            // The next ```http block, then the next ```log block.
            let mut http = String::new();
            let mut log = String::new();
            for (fence, sink) in [("```http", &mut http), ("```log", &mut log)] {
                for l in lines.by_ref() {
                    if l.starts_with(fence) {
                        break;
                    }
                }
                for l in lines.by_ref() {
                    if l.starts_with("```") {
                        break;
                    }
                    sink.push_str(l);
                    sink.push('\n');
                }
            }
            if !strict {
                continue;
            }
            // Pauses (codes 21–23: paused, paused on upgrade, paused on h2
            // upgrade) stop the parser without refusing anything.
            let llhttp_error = log
                .lines()
                .find(|l| {
                    l.contains(" error code=")
                        && !l.contains("code=21")
                        && !l.contains("code=22")
                        && !l.contains("code=23")
                })
                .map(|l| l.split_once("error ").unwrap().1.to_string());
            let llhttp_messages = log
                .lines()
                .filter(|l| l.ends_with("message complete"))
                .count();
            cases.push(Case {
                file: file.clone(),
                name: name.clone(),
                input: unescape(&http),
                llhttp_error,
                llhttp_messages,
            });
        }
    }
    cases
}

/// Our verdict: Err(reason) if refused, else the number of complete messages.
fn ours(input: &[u8]) -> Result<usize, String> {
    let limits = Limits::default();
    let mut fields = Vec::new();
    let mut at = 0;
    let mut messages = 0;
    loop {
        let rest = &input[at..];
        if rest.is_empty() {
            return Ok(messages);
        }
        let head = match parse_request(rest, &limits, &mut fields) {
            Ok(Parsed::Complete(head)) => head,
            Ok(Parsed::Partial) => return Ok(messages),
            Err(e) => return Err(error_text(&e)),
        };
        at += head.len;
        if head.upgrade || head.target_form == es_runtime_http::h1::TargetForm::Authority {
            // The connection becomes something else; the rest is not HTTP.
            return Ok(messages + 1);
        }
        match head.body {
            Body::Empty => {}
            Body::Length(n) => {
                if ((input.len() - at) as u64) < n {
                    return Ok(messages);
                }
                at += n as usize;
            }
            Body::Chunked => {
                let mut d = ChunkedDecoder::new(limits);
                loop {
                    match d.decode(&input[at..]) {
                        Ok((used, Chunk::Data(_))) => at += used,
                        Ok((used, Chunk::End)) => {
                            at += used;
                            break;
                        }
                        Ok((_, Chunk::NeedMore)) => return Ok(messages),
                        Err(e) => return Err(e.to_string()),
                    }
                }
            }
        }
        messages += 1;
        if !head.keep_alive {
            // The server stops reading after a request that closes the
            // connection: anything after it is never served, which is the
            // outcome llhttp reports as an error.
            if at < input.len() {
                return Err("data after Connection: close is not served".into());
            }
            return Ok(messages);
        }
    }
}

fn error_text(e: &RequestError) -> String {
    e.to_string()
}

#[test]
fn llhttp_strict_request_cases_agree_or_differ_only_deliberately() {
    let cases = load();
    assert!(
        cases.len() > 100,
        "corpus not found or not parsed: {} cases",
        cases.len()
    );
    let mut more_lenient = Vec::new();
    let mut unexplained = Vec::new();
    let mut stricter = 0;
    let mut agreed = 0;
    for case in &cases {
        let verdict = ours(&case.input);
        match (&case.llhttp_error, &verdict) {
            (Some(_), Err(_)) => agreed += 1,
            (None, Ok(n)) if *n == case.llhttp_messages => agreed += 1,
            (Some(theirs), Ok(_)) => more_lenient.push(format!(
                "{}: {} — llhttp: {theirs}; we accepted\n    {:?}",
                case.file,
                case.name,
                String::from_utf8_lossy(&case.input)
            )),
            (None, Err(why)) if DELIBERATELY_STRICTER.contains(&why.as_str()) => stricter += 1,
            (None, other) => unexplained.push(format!(
                "{}: {} — llhttp completed {} message(s); we: {other:?}\n    {:?}",
                case.file,
                case.name,
                case.llhttp_messages,
                String::from_utf8_lossy(&case.input)
            )),
        }
    }
    eprintln!(
        "llhttp corpus: {} cases — {agreed} agree, {stricter} deliberately stricter, {} more lenient, {} unexplained",
        cases.len(),
        more_lenient.len(),
        unexplained.len()
    );
    assert!(
        more_lenient.is_empty() && unexplained.is_empty(),
        "\nMORE LENIENT THAN LLHTTP (must be none):\n{}\n\nUNEXPLAINED REFUSALS:\n{}\n",
        more_lenient.join("\n"),
        unexplained.join("\n")
    );
}
