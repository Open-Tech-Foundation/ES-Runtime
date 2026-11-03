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

    /// `(cert, key)` PEM for a certificate with subject CN `cn` and serial
    /// `serial`, valid for `localhost`.
    fn named(&self, cn: &str, serial: u64) -> (String, String) {
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params.distinguished_name.push(DnType::CommonName, cn);
        params.serial_number = Some(rcgen::SerialNumber::from(serial));
        let key = KeyPair::generate().unwrap();
        let cert = params
            .signed_by(&key, &Issuer::from_params(&self.params, &self.key))
            .unwrap();
        (cert.pem(), key.serialize_pem())
    }

    /// A CRL revoking `serials`, current until 2090.
    fn crl(&self, serials: &[u64]) -> String {
        let params = rcgen::CertificateRevocationListParams {
            this_update: rcgen::date_time_ymd(2021, 1, 1),
            next_update: rcgen::date_time_ymd(2090, 1, 1),
            crl_number: rcgen::SerialNumber::from(1u64),
            issuing_distribution_point: None,
            revoked_certs: serials
                .iter()
                .map(|&serial| rcgen::RevokedCertParams {
                    serial_number: rcgen::SerialNumber::from(serial),
                    revocation_time: rcgen::date_time_ymd(2021, 1, 1),
                    reason_code: None,
                    invalidity_date: None,
                })
                .collect(),
            key_identifier_method: rcgen::KeyIdMethod::Sha256,
        };
        params
            .signed_by(&Issuer::from_params(&self.params, &self.key))
            .unwrap()
            .pem()
            .unwrap()
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

/// Runs `script` with the grants these tests need and `env`, returning stdout
/// and stderr. `flags` go before the script.
fn run_script(name: &str, script: &str, flags: &[String], env: &[(&str, &str)]) -> Output {
    let app = file(name, script);
    let mut command = Command::new(common::esrun_bin());
    command
        .current_dir(common::tmpdir())
        .args(["--allow-net", "--allow-listen", "--allow-env"])
        .args(flags)
        .arg(&app);
    for (k, v) in env {
        command.env(k, v);
    }
    command.output().expect("spawn esrun")
}

/// A `serve` that requires client certificates from `CLIENT_CA` and answers
/// with the CN it verified; then `fetch`es itself three ways.
const MTLS_SERVE: &str = r#"
import { serve } from "runtime:http";
import { env, unmask } from "runtime:process";
const server = serve(
  {
    hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: unmask(env.KEY),
    requestCert: true, ca: env.CLIENT_CA,
  },
  (request, info) => new Response(info.peerCertificate?.subject.CN ?? "none"),
);
const { port } = await server.addr;
const url = `https://localhost:${port}/`;
const attempt = async (init) => {
  try { return await (await fetch(url, { ...init, signal: AbortSignal.timeout(5000) })).text(); }
  catch (e) { return e.name === "TypeError" && /tls\./.test(e.message) ? "invalid" : "refused"; }
};
const withCert = await attempt({ tls: { ca: env.SERVER_CA, cert: env.CLIENT_CERT, key: unmask(env.CLIENT_KEY) } });
const withoutCert = await attempt({ tls: { ca: env.SERVER_CA } });
const skipping = await attempt({ tls: { ca: env.SERVER_CA, rejectUnauthorized: false } });
console.log(`with=${withCert} without=${withoutCert} skipping=${skipping}`);
await server.stop();
"#;

// D201 end to end: `fetch`'s `tls` member presents a certificate, the server
// requires one, and the handler reads who it was.
#[test]
fn fetch_presents_a_client_certificate_a_server_requires() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let (cert, key) = server_ca.localhost();
    let (client_cert, client_key) = client_ca.named("billing", 5);
    let out = run_script(
        "mtls-serve.mjs",
        MTLS_SERVE,
        &[],
        &[
            ("CERT", &cert),
            ("KEY", &key),
            ("SERVER_CA", &server_ca.pem),
            ("CLIENT_CA", &client_ca.pem),
            ("CLIENT_CERT", &client_cert),
            ("CLIENT_KEY", &client_key),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("with=billing without=refused skipping=invalid"),
        "{}",
        stdout(&out)
    );
}

// The same over `runtime:net`, with a CRL: a revoked client is refused and the
// accepted socket carries the verified certificate.
#[test]
fn a_listener_requires_and_revokes_client_certificates() {
    let server_ca = Ca::new("Server CA");
    let client_ca = Ca::new("Client CA");
    let (cert, key) = server_ca.localhost();
    let (good_cert, good_key) = client_ca.named("good", 1);
    let (bad_cert, bad_key) = client_ca.named("revoked", 2);
    let script = r#"
import { connect, listen } from "runtime:net";
import { env, unmask } from "runtime:process";
const server = listen({
  hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: unmask(env.KEY),
  requestCert: true, ca: env.CLIENT_CA, crl: env.CRL,
});
const { port } = await server.addr;
const seen = [];
(async () => { for await (const socket of server) { const info = await socket.opened; seen.push(info.peerCertificate.subject.CN); await socket.close(); } })();
const dial = async (cert, key) => {
  const socket = connect({ hostname: "localhost", port }, { secureTransport: "on", ca: env.SERVER_CA, cert, key });
  try { await socket.opened; await socket.writable.getWriter().write(new Uint8Array([1])); } catch {}
  await new Promise((r) => setTimeout(r, 300));
  try { await socket.close(); } catch {}
};
await dial(env.GOOD_CERT, unmask(env.GOOD_KEY));
await dial(env.BAD_CERT, unmask(env.BAD_KEY));
await dial(undefined, undefined);
console.log("SEEN:" + seen.join(","));
await server.close();
"#;
    let out = run_script(
        "mtls-listen.mjs",
        script,
        &[],
        &[
            ("CERT", &cert),
            ("KEY", &key),
            ("SERVER_CA", &server_ca.pem),
            ("CLIENT_CA", &client_ca.pem),
            ("CRL", &client_ca.crl(&[2])),
            ("GOOD_CERT", &good_cert),
            ("GOOD_KEY", &good_key),
            ("BAD_CERT", &bad_cert),
            ("BAD_KEY", &bad_key),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("SEEN:good\n"), "{}", stdout(&out));
}

// D200: a running server presents a new certificate to new connections, and
// a replacement whose key does not match is refused with the old one kept.
#[test]
fn set_secure_context_rotates_a_running_server() {
    let old_ca = Ca::new("Old CA");
    let new_ca = Ca::new("New CA");
    let (old_cert, old_key) = old_ca.localhost();
    let (new_cert, new_key) = new_ca.localhost();
    let (_, other_key) = new_ca.localhost();
    let script = r#"
import { serve } from "runtime:http";
import { connect } from "runtime:net";
import { env, unmask } from "runtime:process";
const server = serve(
  { hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.OLD_CERT, key: unmask(env.OLD_KEY) },
  () => new Response("ok"),
);
const { port } = await server.addr;
// A fresh connection each time, verifying against one CA.
const trusts = async (ca) => {
  const socket = connect({ hostname: "localhost", port }, { secureTransport: "on", ca });
  try { await socket.opened; await socket.close(); return true; } catch { return false; }
};
const before = [await trusts(env.OLD_CA), await trusts(env.NEW_CA)];
let mismatch = "accepted";
try { await server.setSecureContext({ cert: env.NEW_CERT, key: unmask(env.OTHER_KEY) }); }
catch (e) { mismatch = e.name; }
const afterMismatch = [await trusts(env.OLD_CA), await trusts(env.NEW_CA)];
await server.setSecureContext({ cert: env.NEW_CERT, key: unmask(env.NEW_KEY) });
const after = [await trusts(env.OLD_CA), await trusts(env.NEW_CA)];
let unknown = "accepted";
try { await server.setSecureContext({ passphrase: "x" }); } catch (e) { unknown = e.name; }
console.log(`before=${before} mismatch=${mismatch} kept=${afterMismatch} after=${after} unknown=${unknown}`);
await server.stop();
"#;
    let out = run_script(
        "rotate.mjs",
        script,
        &[],
        &[
            ("OLD_CERT", &old_cert),
            ("OLD_KEY", &old_key),
            ("NEW_CERT", &new_cert),
            ("NEW_KEY", &new_key),
            ("OTHER_KEY", &other_key),
            ("OLD_CA", &old_ca.pem),
            ("NEW_CA", &new_ca.pem),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains(
            "before=true,false mismatch=TypeError kept=true,false after=false,true unknown=TypeError"
        ),
        "{}",
        stdout(&out)
    );
}

// D202, client side: `--tls-crl` refuses a revoked server for every client.
#[test]
fn tls_crl_refuses_a_revoked_server() {
    let ca = Ca::new("Private CA");
    let pem = file("crl-ca.pem", &ca.pem);
    let crl = file("server.crl", &ca.crl(&[9]));
    let revoked = ca.named("revoked", 9);
    let flags = [tls_ca(&pem), format!("--tls-crl={}", crl.display())];
    assert_clients(&all_clients(&revoked, &flags), REFUSED);
    let good = ca.named("good", 10);
    assert_clients(&all_clients(&good, &flags), TRUSTED);
}

#[test]
fn a_crl_file_that_cannot_be_used_stops_startup() {
    let ca = Ca::new("Private CA");
    for (what, path, message) in [
        (
            "missing",
            common::tmpdir().join("tls-trust/no.crl"),
            "cannot read it",
        ),
        (
            "a certificate",
            file("not-a.crl", &ca.pem),
            "CERTIFICATE block",
        ),
    ] {
        let err = refused_at_startup(&[format!("--tls-crl={}", path.display())]);
        assert!(err.contains(message), "{what}: {err}");
    }
}

// Every way to ask a server to accept a client it did not verify is refused
// before anything is bound.
#[test]
fn client_certificate_options_are_validated() {
    let ca = Ca::new("CA");
    let (cert, key) = ca.localhost();
    let script = r#"
import { serve } from "runtime:http";
import { listen } from "runtime:net";
import { env, unmask } from "runtime:process";
const tls = { hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: unmask(env.KEY) };
const cases = [
  () => serve({ ...tls, requestCert: true }, () => new Response()),
  () => serve({ ...tls, ca: env.CA }, () => new Response()),
  () => serve({ ...tls, requestCert: true, ca: env.CA, rejectUnauthorized: false }, () => new Response()),
  () => listen({ ...tls, requestCert: true }),
  () => listen({ ...tls, crl: "x" }),
  () => listen({ ...tls, requestCert: true, ca: env.CA, rejectUnauthorized: false }),
];
const results = cases.map((make) => { try { make(); return "accepted"; } catch (e) { return e.name; } });
console.log("RESULTS:" + results.join(","));
"#;
    let out = run_script(
        "mtls-options.mjs",
        script,
        &[],
        &[("CERT", &cert), ("KEY", &key), ("CA", &ca.pem)],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out)
            .contains("RESULTS:TypeError,TypeError,TypeError,TypeError,TypeError,TypeError"),
        "{}",
        stdout(&out)
    );
}

// A standalone runtime:websocket server serves `wss:` (its own certificate,
// verified by the client against `--tls-ca`), and refuses half-given options.
#[test]
fn the_standalone_websocket_server_serves_wss() {
    let ca = Ca::new("Private CA");
    let pem = file("wss-ca.pem", &ca.pem);
    let (cert, key) = ca.localhost();
    let script = r#"
import { serve } from "runtime:websocket";
import { env, unmask } from "runtime:process";
const server = serve({ hostname: "127.0.0.1", port: 0, secureTransport: "on", cert: env.CERT, key: unmask(env.KEY) });
const { port } = await server.addr;
(async () => { for await (const socket of server) socket.onmessage = (e) => socket.send("echo:" + e.data); })();
const echoed = await new Promise((resolve, reject) => {
  const ws = new WebSocket(`wss://localhost:${port}/`);
  ws.onopen = () => ws.send("hi");
  ws.onmessage = (e) => { resolve(e.data); ws.close(); };
  ws.onerror = () => reject(new Error("refused"));
  setTimeout(() => reject(new Error("timed out")), 5000);
}).catch((e) => e.message);
const refused = [
  () => serve({ port: 0, secureTransport: "on", cert: env.CERT }),
  () => serve({ port: 0, cert: env.CERT, key: unmask(env.KEY) }),
  () => serve({ port: 0, secureTransport: "on", cert: env.CERT, key: unmask(env.KEY), requestCert: true }),
].map((make) => { try { make(); return "accepted"; } catch (e) { return e.name; } });
console.log(`WSS:${echoed} REFUSED:${refused.join(",")}`);
await server.close();
"#;
    let out = run_script(
        "wss.mjs",
        script,
        &[tls_ca(&pem)],
        &[("CERT", &cert), ("KEY", &key)],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("WSS:echo:hi REFUSED:TypeError,TypeError,TypeError"),
        "{}",
        stdout(&out)
    );
}
