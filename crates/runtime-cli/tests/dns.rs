//! End-to-end tests for `runtime:net`'s `resolveDns` (DECISIONS D188): real
//! lookups through the `esrun` binary against a small DNS server in this test,
//! reached with the `nameServer` option — so they never touch the network.

use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::process::{Command, Output};
use std::thread;

mod common;

fn esrun() -> Command {
    let mut command = Command::new(common::esrun_bin());
    command.current_dir(common::tmpdir());
    command
}

fn run(flags: &[&str], code: &str) -> Output {
    esrun()
        .args(flags)
        .arg(format!("-e={code}"))
        .output()
        .expect("spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn name(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for label in text.split('.').filter(|l| !l.is_empty()) {
        out.push(u8::try_from(label.len()).unwrap());
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out
}

/// The records this test server holds: (name, type) → each record's data.
fn zone(qname: &str, qtype: u16) -> Option<Vec<Vec<u8>>> {
    let mut srv = vec![0, 10, 0, 5, 0x69, 0x89];
    srv.extend(name("db1.example.test"));
    let mut mx = vec![0, 10];
    mx.extend(name("mail.example.test"));
    Some(match (qname, qtype) {
        ("example.test", 1) => vec![vec![192, 0, 2, 1], vec![192, 0, 2, 2]],
        ("example.test", 15) => vec![mx],
        ("example.test", 16) => vec![b"\x0bv=spf1 -all".to_vec(), b"\x03one\x03two".to_vec()],
        ("_mongodb._tcp.example.test", 33) => vec![srv],
        // Too big for the UDP reply, which comes back truncated; TCP has it.
        ("big.example.test", 16) => (0..40)
            .map(|i| format!("\x3f{:0>63}", i).into_bytes())
            .collect(),
        ("example.test", _) | ("big.example.test", _) | ("_mongodb._tcp.example.test", _) => vec![],
        _ => return None,
    })
}

/// A reply to `query`: its id and question, then the zone's answers. `None`
/// for a question this server ignores (to let a lookup time out).
fn reply(query: &[u8], over_tcp: bool) -> Option<Vec<u8>> {
    let mut at = 12;
    let mut labels = Vec::new();
    while query[at] != 0 {
        let len = usize::from(query[at]);
        labels.push(String::from_utf8_lossy(&query[at + 1..at + 1 + len]).to_lowercase());
        at += 1 + len;
    }
    let question_end = at + 5;
    let qname = labels.join(".");
    let qtype = u16::from_be_bytes([query[at + 1], query[at + 2]]);
    if qname.starts_with("silent.") {
        return None;
    }
    let answers = zone(&qname, qtype);
    let truncate = !over_tcp && qname.starts_with("big.");
    let mut out = query[..2].to_vec();
    let rcode = if answers.is_none() { 3 } else { 0 };
    let flags: u16 = 0x8180 | rcode | if truncate { 0x0200 } else { 0 };
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&[0, 1]);
    let answers = if truncate {
        vec![]
    } else {
        answers.unwrap_or_default()
    };
    out.extend_from_slice(&u16::try_from(answers.len()).unwrap().to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&query[12..question_end]);
    for data in answers {
        out.extend_from_slice(&[0xc0, 12]);
        out.extend_from_slice(&qtype.to_be_bytes());
        out.extend_from_slice(&[0, 1, 0, 0, 1, 44]);
        out.extend_from_slice(&u16::try_from(data.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&data);
    }
    Some(out)
}

/// Starts the server on UDP and TCP at one port of 127.0.0.1, and returns it.
/// Before each real reply over UDP it sends one with the wrong id, which a
/// resolver must ignore.
fn dns_server() -> u16 {
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = udp.local_addr().unwrap().port();
    let tcp = TcpListener::bind(("127.0.0.1", port)).unwrap();
    thread::spawn(move || {
        let mut buffer = [0u8; 1500];
        loop {
            let Ok((n, from)) = udp.recv_from(&mut buffer) else {
                return;
            };
            if let Some(answer) = reply(&buffer[..n], false) {
                let mut spoof = answer.clone();
                spoof[0] ^= 0xff;
                let _ = udp.send_to(&spoof, from);
                let _ = udp.send_to(&answer, from);
            }
        }
    });
    thread::spawn(move || {
        for stream in tcp.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut head = [0u8; 2];
            if stream.read_exact(&mut head).is_err() {
                continue;
            }
            let mut query = vec![0u8; usize::from(u16::from_be_bytes(head))];
            if stream.read_exact(&mut query).is_err() {
                continue;
            }
            if let Some(answer) = reply(&query, true) {
                let mut framed = u16::try_from(answer.len()).unwrap().to_be_bytes().to_vec();
                framed.extend_from_slice(&answer);
                let _ = stream.write_all(&framed);
            }
        }
    });
    port
}

fn lookup(port: u16, body: &str) -> String {
    format!(
        "import {{ resolveDns }} from 'runtime:net';\
         const ns = {{ nameServer: {{ ipAddr: '127.0.0.1', port: {port} }} }};\
         const q = async (name, type, more = {{}}) => {{\
           try {{ return JSON.stringify(await resolveDns(name, type, {{ ...ns, ...more }})); }}\
           catch (e) {{ return e.code ?? e.name; }}\
         }};\
         {body}"
    )
}

#[test]
fn records_of_each_shape_come_back() {
    let port = dns_server();
    let code = lookup(
        port,
        "console.log(await q('example.test', 'A'));\
         console.log(await q('Example.Test.', 'MX'));\
         console.log(await q('example.test', 'TXT'));\
         console.log(await q('_mongodb._tcp.example.test', 'SRV'));",
    );
    let out = run(&["--allow-net"], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).lines().collect::<Vec<_>>(),
        [
            r#"["192.0.2.1","192.0.2.2"]"#,
            r#"[{"preference":10,"exchange":"mail.example.test."}]"#,
            r#"[["v=spf1 -all"],["one","two"]]"#,
            r#"[{"priority":10,"weight":5,"port":27017,"target":"db1.example.test."}]"#,
        ]
    );
}

#[test]
fn missing_names_empty_types_and_truncated_answers() {
    let port = dns_server();
    let code = lookup(
        port,
        "console.log(await q('nope.test', 'A'));\
         console.log(await q('example.test', 'AAAA'));\
         const big = JSON.parse(await q('big.example.test', 'TXT'));\
         console.log(big.length, big[39][0].length);",
    );
    let out = run(&["--allow-net"], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // NXDOMAIN is NotFound; no records is []; a truncated UDP answer is asked
    // again over TCP and arrives whole.
    assert_eq!(
        stdout(&out).lines().collect::<Vec<_>>(),
        ["ERR_NOT_FOUND", "[]", "40 63"]
    );
}

#[test]
fn bad_arguments_are_type_errors() {
    let code = "import { resolveDns } from 'runtime:net';\
        for (const args of [[''], ['a.test', 'ANY'], ['a.test', 'A', { nameServer: '1.1.1.1' }],\
                            ['a.test', 'A', { nameServer: { ipAddr: '127.0.0.1', port: 70000 } }]]) {\
          try { await resolveDns(...args); console.log('resolved'); } catch (e) { console.log(e.name); }\
        }";
    let out = run(&["--allow-net"], code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).lines().collect::<Vec<_>>(), ["TypeError"; 4]);
}

#[test]
fn a_signal_abandons_a_lookup_and_the_process_exits() {
    let port = dns_server();
    // The server never answers `silent.` names; the abort ends the wait, and
    // nothing is left holding the process open.
    let code = lookup(
        port,
        "const c = new AbortController();\
         setTimeout(() => c.abort(new Error('stop')), 100);\
         const started = Date.now();\
         try { await resolveDns('silent.example.test', 'A', { ...ns, signal: c.signal }); }\
         catch (e) { console.log(e.message, Date.now() - started < 1500); }\
         try { await resolveDns('x.test', 'A', { signal: AbortSignal.abort() }); } catch (e) { console.log(e.name); }",
    );
    let started = std::time::Instant::now();
    let out = run(&["--allow-net"], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).lines().collect::<Vec<_>>(),
        ["stop true", "AbortError"]
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "took {:?}",
        started.elapsed()
    );
}

#[test]
fn a_silent_server_times_out_as_a_dns_error() {
    let port = dns_server();
    let out = run(
        &["--allow-net"],
        &lookup(port, "console.log(await q('silent.example.test', 'A'));"),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "ERR_DNS");
}

#[test]
fn a_scoped_allow_net_permits_queries_about_the_hosts_it_names() {
    let port = dns_server();
    let code = lookup(
        port,
        "console.log(await q('example.test', 'A'), await q('_mongodb._tcp.example.test', 'SRV') !== 'ERR_PERMISSION_DENIED',\
                     await q('other.test', 'A'), await q('big.example.test', 'TXT'));",
    );
    // The query goes to the name server, so it must be allowed too.
    let server = format!("--allow-net=example.test,127.0.0.1:{port}");
    let out = run(&["--deny-all", &server], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        r#"["192.0.2.1","192.0.2.2"] true ERR_PERMISSION_DENIED ERR_PERMISSION_DENIED"#
    );

    // The name is allowed but the server is not.
    let out = run(&["--deny-all", "--allow-net=example.test"], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).starts_with("ERR_PERMISSION_DENIED"),
        "{}",
        stdout(&out)
    );

    // No `net` at all.
    let out = run(&["--deny-all"], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).starts_with("ERR_CAPABILITY_DENIED"),
        "{}",
        stdout(&out)
    );
}
