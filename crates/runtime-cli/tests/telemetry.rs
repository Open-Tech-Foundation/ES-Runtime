//! End-to-end tests for what `esrun` reports about itself.
//!
//! The runtime crates only *emit* `tracing` events; installing a subscriber is
//! a process-global act that belongs to the binary. That seam is exactly where
//! this silently broke — every event the servers emitted was discarded because
//! nothing in the tree ever called `init_tracing` — so the assertion has to run
//! the real binary. A unit test on the emitting side cannot see this.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

/// A TLS server on an ephemeral port that prints `PORT <n>` when listening.
const TLS_SERVER: &str = r#"
import { serve } from "runtime:http";
import { env } from "runtime:process";
const server = serve(
  {
    hostname: "127.0.0.1",
    port: 0,
    secureTransport: "on",
    cert: env.CERT,
    key: env.KEY,
  },
  () => new Response("ok"),
);
console.log("PORT " + (await server.addr).port);
"#;

/// Stands the server up under `rust_log`, sends plaintext at its TLS port —
/// which rustls rejects as a corrupt first record — and returns its stderr.
fn stderr_after_a_failed_handshake(name: &str, rust_log: Option<&str>) -> String {
    let (cert, key) = self_signed();
    let app = temp(name);
    std::fs::write(&app, TLS_SERVER).expect("write app");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_esrun"));
    // Fixture, not subject (D65): this is a test about tracing output.
    cmd.arg("--allow-all")
        .arg(&app)
        .env("CERT", cert)
        .env("KEY", key)
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(filter) = rust_log {
        cmd.env("RUST_LOG", filter);
    }
    let mut child = cmd.spawn().expect("spawn esrun");

    let port = read_port(child.stdout.as_mut().expect("stdout"));
    let mut tcp = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let _ = tcp.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n");
    drop(tcp);
    // The server logs from the connection's own task, which this test does not
    // join; the wait is for that task to run, not for a fixed duration.
    std::thread::sleep(Duration::from_millis(500));

    let _ = child.kill();
    let out = child.wait_with_output().expect("wait");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Reads the `PORT <n>` line the app prints once it is listening.
fn read_port(stdout: &mut std::process::ChildStdout) -> u16 {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut seen = String::new();
    let mut byte = [0u8; 1];
    while Instant::now() < deadline {
        match stdout.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                seen.push(byte[0] as char);
                if byte[0] == b'\n'
                    && let Some(rest) = seen.trim().strip_prefix("PORT ")
                {
                    return rest.trim().parse().expect("a port number");
                }
                if byte[0] == b'\n' {
                    seen.clear();
                }
            }
            Err(e) => panic!("reading the server's stdout: {e}"),
        }
    }
    panic!("the server never reported a port; saw {seen:?}");
}

/// A throwaway cert/key for `localhost`, as PEM strings.
fn self_signed() -> (String, String) {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    (ck.cert.pem(), ck.signing_key.serialize_pem())
}

/// The filter has to reach the servers' targets through the real binary. This
/// is the case that was broken: the event was emitted and nothing was
/// subscribed, so no filter could have revealed it.
#[test]
fn rust_log_reveals_a_failed_handshake_with_its_peer() {
    let stderr = stderr_after_a_failed_handshake("telemetry-on.mjs", Some("runtime::http=debug"));
    assert!(
        stderr.contains("tls handshake failed"),
        "RUST_LOG must reach the http target; stderr was: {stderr}",
    );
    assert!(
        stderr.contains("peer=127.0.0.1:"),
        "the connection span must carry the peer; stderr was: {stderr}",
    );
    assert!(
        stderr.contains("DEBUG"),
        "the event must arrive at debug; stderr was: {stderr}",
    );
}

/// And without the filter it must stay quiet. A peer can produce this failure
/// on demand, so a server that reported it by default would let any scanner set
/// its log volume.
#[test]
fn nothing_is_reported_by_default() {
    let stderr = stderr_after_a_failed_handshake("telemetry-off.mjs", None);
    assert!(
        !stderr.contains("tls handshake failed"),
        "a peer-driven failure must not be reported at the default filter; \
         stderr was: {stderr}",
    );
}
