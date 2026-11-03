//! End-to-end tests for the process trust store (DECISIONS.md D197): every
//! TLS client verifies against the bundled Mozilla roots, or exactly what
//! `--tls-ca` lists, and nothing else changes that.
//!
//! The binary serves HTTPS and `wss:` itself, from a certificate a private CA
//! signed, and dials it back with `fetch`, `runtime:net` and `WebSocket`. One
//! program and one port, so each test shows all three clients agreeing about
//! one server — which is the claim, since the bug this replaces was two of
//! them disagreeing.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, KeyUsagePurpose};

mod common;

/// A private CA, kept whole so it can sign servers.
struct Ca {
    params: CertificateParams,
    key: KeyPair,
    pem: String,
}

impl Ca {
    fn new(name: &str) -> Ca {
        Self::valid(name, 2020, 2090)
    }

    fn valid(name: &str, from: i32, to: i32) -> Ca {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.distinguished_name.push(DnType::CommonName, name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        params.not_before = rcgen::date_time_ymd(from, 1, 1);
        params.not_after = rcgen::date_time_ymd(to, 1, 1);
        let key = KeyPair::generate().unwrap();
        let pem = params.self_signed(&key).unwrap().pem();
        Ca { params, key, pem }
    }

    /// `(cert, key)` PEM for a server named `names`, valid `from`..`to`.
    fn server(&self, names: &[&str], from: i32, to: i32) -> (String, String) {
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let mut params = CertificateParams::new(names).unwrap();
        params.not_before = rcgen::date_time_ymd(from, 1, 1);
        params.not_after = rcgen::date_time_ymd(to, 1, 1);
        let key = KeyPair::generate().unwrap();
        let cert = params
            .signed_by(&key, &Issuer::from_params(&self.params, &self.key))
            .unwrap();
        (cert.pem(), key.serialize_pem())
    }

    fn localhost(&self) -> (String, String) {
        self.server(&["localhost"], 2020, 2090)
    }
}

/// Writes `contents` to a file of this test's own and returns its path.
fn file(name: &str, contents: &str) -> PathBuf {
    let dir = common::tmpdir().join("tls-trust");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{}-{name}", std::process::id()));
    std::fs::write(&path, contents).unwrap();
    path
}

/// Serves HTTPS and `wss:` with `CERT`/`KEY`, then reaches it with all three
/// clients and prints `fetch=… net=… ws=…`, each `ok` (`ws:x` for the socket)
/// or `refused`.
const ALL_CLIENTS: &str = r#"
import { serve } from "runtime:http";
import { upgradeWebSocket } from "runtime:websocket";
import { connect } from "runtime:net";
import { env } from "runtime:process";
const server = serve(
  { hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: env.KEY },
  (request) => {
    if (request.headers.get("upgrade") === "websocket") {
      const { response, socket } = upgradeWebSocket(request);
      socket.onmessage = (e) => socket.send("ws:" + e.data);
      return response;
    }
    return new Response("ok");
  },
);
const { port } = await server.addr;
const attempt = async (f) => { try { return await f(); } catch { return "refused"; } };
const fetched = await attempt(async () =>
  (await fetch(`https://localhost:${port}/`, { signal: AbortSignal.timeout(5000) })).text());
const net = await attempt(async () => {
  const socket = connect({ hostname: "localhost", port }, { secureTransport: "on", ca: env.CONN_CA });
  await socket.opened;
  await socket.close();
  return "ok";
});
const ws = await attempt(() => new Promise((resolve, reject) => {
  const socket = new WebSocket(`wss://localhost:${port}/`);
  socket.onopen = () => socket.send("x");
  socket.onmessage = (e) => { resolve(e.data); socket.close(); };
  socket.onerror = () => reject(new Error("refused"));
  setTimeout(() => reject(new Error("timed out")), 5000);
}));
console.log(`fetch=${fetched} net=${net} ws=${ws}`);
await server.stop();
"#;

const TRUSTED: &str = "fetch=ok net=ok ws=ws:x";
const REFUSED: &str = "fetch=refused net=refused ws=refused";

/// esrun with the grants `ALL_CLIENTS` needs, and none of the CA variables a
/// developer's shell might carry.
fn esrun() -> Command {
    let mut command = Command::new(common::esrun_bin());
    command
        .current_dir(common::tmpdir())
        .arg("--allow-net")
        .arg("--allow-listen")
        .arg("--allow-env=CERT,KEY,CERT2,KEY2,CONN_CA,CA_PATH");
    for name in [
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "NODE_EXTRA_CA_CERTS",
        "NODE_USE_SYSTEM_CA",
        "DENO_CERT",
        "DENO_TLS_CA_STORE",
    ] {
        command.env_remove(name);
    }
    command
}

/// Runs `ALL_CLIENTS` against `server` with `flags` before the script.
fn all_clients(server: &(String, String), flags: &[String]) -> Output {
    let app = file("all-clients.mjs", ALL_CLIENTS);
    esrun()
        .args(flags)
        .arg(&app)
        .env("CERT", &server.0)
        .env("KEY", &server.1)
        .output()
        .expect("spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn tls_ca(path: &Path) -> String {
    format!("--tls-ca={}", path.display())
}

fn assert_clients(out: &Output, expected: &str) {
    assert!(out.status.success(), "stderr: {}", stderr(out));
    assert!(
        stdout(out).contains(expected),
        "expected {expected}\nstdout: {}\nstderr: {}",
        stdout(out),
        stderr(out)
    );
}

#[test]
fn without_tls_ca_every_client_refuses_a_private_ca() {
    let ca = Ca::new("Private CA");
    assert_clients(&all_clients(&ca.localhost(), &[]), REFUSED);
}

#[test]
fn a_tls_ca_file_is_trusted_by_every_client() {
    let ca = Ca::new("Private CA");
    let pem = file("ca.pem", &ca.pem);
    assert_clients(&all_clients(&ca.localhost(), &[tls_ca(&pem)]), TRUSTED);
}

#[test]
fn bundled_and_a_file_trust_the_file_too() {
    let ca = Ca::new("Private CA");
    let pem = file("ca-bundled.pem", &ca.pem);
    let flags = ["--tls-ca=bundled".to_string(), tls_ca(&pem)];
    assert_clients(&all_clients(&ca.localhost(), &flags), TRUSTED);
}

#[test]
fn a_server_another_ca_signed_is_refused_by_every_client() {
    let trusted = Ca::new("Trusted CA");
    let other = Ca::new("Other CA");
    let pem = file("trusted.pem", &trusted.pem);
    for flags in [
        vec![tls_ca(&pem)],
        vec!["--tls-ca=bundled".to_string(), tls_ca(&pem)],
    ] {
        assert_clients(&all_clients(&other.localhost(), &flags), REFUSED);
    }
}

#[test]
fn a_trusted_ca_does_not_excuse_the_wrong_hostname() {
    let ca = Ca::new("Private CA");
    let pem = file("ca-name.pem", &ca.pem);
    let server = ca.server(&["other.test"], 2020, 2090);
    assert_clients(&all_clients(&server, &[tls_ca(&pem)]), REFUSED);
}

#[test]
fn a_trusted_ca_does_not_excuse_an_expired_server_certificate() {
    let ca = Ca::new("Private CA");
    let pem = file("ca-expired-leaf.pem", &ca.pem);
    let server = ca.server(&["localhost"], 2001, 2002);
    assert_clients(&all_clients(&server, &[tls_ca(&pem)]), REFUSED);
}

// The environment variables Node, Deno and OpenSSL read change nothing: the
// private CA they name stays untrusted by all three clients. `fetch` used to
// trust whatever `SSL_CERT_FILE` named, through the OS-store verifier.
#[test]
fn ca_environment_variables_are_ignored() {
    let ca = Ca::new("Private CA");
    let pem = file("ca-env.pem", &ca.pem);
    let dir = pem.parent().unwrap().to_path_buf();
    let app = file("all-clients-env.mjs", ALL_CLIENTS);
    let server = ca.localhost();
    let out = esrun()
        .arg(&app)
        .env("CERT", &server.0)
        .env("KEY", &server.1)
        .env("SSL_CERT_FILE", &pem)
        .env("SSL_CERT_DIR", &dir)
        .env("NODE_EXTRA_CA_CERTS", &pem)
        .env("NODE_USE_SYSTEM_CA", "1")
        .env("DENO_CERT", &pem)
        .env("DENO_TLS_CA_STORE", "system")
        .output()
        .expect("spawn esrun");
    assert_clients(&out, REFUSED);
    let err = stderr(&out);
    assert!(err.contains("warning"), "no warning: {err}");
    for name in [
        "NODE_EXTRA_CA_CERTS",
        "NODE_USE_SYSTEM_CA",
        "DENO_CERT",
        "DENO_TLS_CA_STORE",
    ] {
        assert!(err.contains(name), "{name} not named: {err}");
    }
    assert!(err.contains("--tls-ca"), "the fix is not named: {err}");
    assert!(
        !err.contains("SSL_CERT_FILE"),
        "SSL_CERT_FILE warned about: {err}"
    );
}

#[test]
fn no_warning_without_ca_variables() {
    let out = esrun().arg("-e=console.log('RAN')").output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(!stderr(&out).contains("warning"), "{}", stderr(&out));
}

// A per-connection `ca` adds to a store `--tls-ca` built, for that
// connection only.
#[test]
fn a_per_connection_ca_adds_to_the_tls_ca_store() {
    let process = Ca::new("Process CA");
    let connection = Ca::new("Connection CA");
    let pem = file("process.pem", &process.pem);
    let app = file("conn-ca.mjs", ALL_CLIENTS);
    let server = connection.localhost();
    let out = esrun()
        .arg(tls_ca(&pem))
        .arg(&app)
        .env("CERT", &server.0)
        .env("KEY", &server.1)
        .env("CONN_CA", &connection.pem)
        .output()
        .expect("spawn esrun");
    // Only `runtime:net` named the connection's CA, so only it connects.
    assert_clients(&out, "fetch=refused net=ok ws=refused");
}

// Each redirect hop is verified: a trusted server cannot hand `fetch` on to
// one the store does not trust.
#[test]
fn a_redirect_to_an_untrusted_server_is_refused() {
    let trusted = Ca::new("Trusted CA");
    let other = Ca::new("Other CA");
    let pem = file("redirect.pem", &trusted.pem);
    let first = trusted.localhost();
    let second = other.localhost();
    let app = file(
        "redirect.mjs",
        r#"
        import { serve } from "runtime:http";
        import { env } from "runtime:process";
        const tls = (cert, key) => ({ hostname: "127.0.0.1", port: 0, secureTransport: "on", cert, key });
        const untrusted = serve(tls(env.CERT2, env.KEY2), () => new Response("untrusted"));
        const to = (await untrusted.addr).port;
        const trusted = serve(tls(env.CERT, env.KEY), () =>
          Response.redirect(`https://localhost:${to}/`, 302));
        const from = (await trusted.addr).port;
        let result;
        try { result = await (await fetch(`https://localhost:${from}/`)).text(); }
        catch (e) { result = "refused"; }
        console.log("REDIRECT:" + result);
        const manual = await fetch(`https://localhost:${from}/`, { redirect: "manual" });
        console.log("MANUAL:" + manual.status);
        await trusted.stop();
        await untrusted.stop();
        "#,
    );
    let out = esrun()
        .arg(tls_ca(&pem))
        .arg(&app)
        .env("CERT", &first.0)
        .env("KEY", &first.1)
        .env("CERT2", &second.0)
        .env("KEY2", &second.1)
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // The first hop is trusted (MANUAL shows it answered); following it is not.
    assert!(stdout(&out).contains("MANUAL:302"), "{}", stdout(&out));
    assert!(
        stdout(&out).contains("REDIRECT:refused"),
        "{}",
        stdout(&out)
    );
}

// The operator's file is read by the host: it needs no `--allow-read`, and
// the program it protects gains no way to read it.
#[test]
fn the_ca_file_needs_no_grant_and_grants_nothing() {
    let ca = Ca::new("Private CA");
    let pem = file("ungranted.pem", &ca.pem);
    let app = file(
        "ungranted.mjs",
        r#"
        import { file } from "runtime:fs";
        import { env } from "runtime:process";
        try { await file(env.CA_PATH).text(); console.log("READ:allowed"); }
        catch (e) { console.log("READ:" + e.name); }
        "#,
    );
    let out = esrun()
        .arg(tls_ca(&pem))
        .arg(&app)
        .env("CA_PATH", &pem)
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("READ:NotAllowedError"),
        "{}",
        stdout(&out)
    );
}

/// Runs a program that prints `RAN` with `flags`, expecting startup to fail
/// before it does. Returns stderr.
fn refused_at_startup(flags: &[String]) -> String {
    let out = esrun()
        .args(flags)
        .arg("-e=console.log('RAN')")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "started anyway: {}", stdout(&out));
    assert!(!stdout(&out).contains("RAN"), "the program ran");
    stderr(&out)
}

#[test]
fn a_ca_file_that_cannot_be_trusted_stops_startup() {
    let ca = Ca::new("Private CA");
    let key = KeyPair::generate().unwrap().serialize_pem();
    let expired = Ca::valid("Expired CA", 2001, 2002);
    let missing = common::tmpdir().join("tls-trust/no-such-ca.pem");
    let cases = [
        ("missing", missing.clone(), "cannot read it"),
        ("empty", file("empty.pem", ""), "no CERTIFICATE block"),
        (
            "not PEM",
            file("not-pem.pem", "hello\n"),
            "no CERTIFICATE block",
        ),
        ("a key", file("key.pem", &key), "private key"),
        (
            "a cert and its key",
            file("cert-key.pem", &format!("{}{key}", ca.pem)),
            "private key",
        ),
        (
            "expired",
            file("expired-ca.pem", &expired.pem),
            "expired on 2002-01-01",
        ),
    ];
    for (what, path, message) in cases {
        let err = refused_at_startup(&[tls_ca(&path)]);
        assert!(
            err.contains(&tls_ca(&path)),
            "{what}: the flag and file are not named: {err}"
        );
        assert!(err.contains(message), "{what}: {err}");
        // Never the file's contents: a key in the wrong file stays out of logs.
        for line in key.lines().filter(|l| !l.starts_with("-----")) {
            assert!(!err.contains(line), "{what}: the error quotes the key");
        }
    }
}

#[test]
fn a_source_given_twice_stops_startup() {
    let ca = Ca::new("Private CA");
    let pem = file("twice.pem", &ca.pem);
    let err = refused_at_startup(&[tls_ca(&pem), tls_ca(&pem)]);
    assert!(err.contains("given twice"), "{err}");
    let err = refused_at_startup(&[
        "--tls-ca=bundled".to_string(),
        "--tls-ca=bundled".to_string(),
    ]);
    assert!(err.contains("given twice"), "{err}");
}

#[test]
fn tls_ca_needs_a_value_attached_with_equals() {
    let err = refused_at_startup(&["--tls-ca".to_string()]);
    assert!(err.contains("--tls-ca requires a value"), "{err}");
    let err = refused_at_startup(&["--tls-ca=".to_string()]);
    assert!(err.contains("empty value"), "{err}");
}

// After the script it would be the script's own argument and change nothing,
// which for a trust setting is a deployment silently trusting the defaults.
#[test]
fn tls_ca_after_the_script_is_refused() {
    let app = file("after.mjs", "console.log('RAN');\n");
    let out = esrun()
        .arg(&app)
        .arg("--tls-ca=bundled")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "accepted: {}", stdout(&out));
    assert!(
        stderr(&out).contains("--tls-ca=bundled appears after"),
        "{}",
        stderr(&out)
    );
}
