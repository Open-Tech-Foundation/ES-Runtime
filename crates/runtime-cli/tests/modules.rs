//! End-to-end ES module tests: run the real `esrun` binary against fixture
//! `.mjs` files (so the actual `FsModuleLoader` + real filesystem + process
//! exit codes are exercised, which the in-process runtime tests — using an
//! in-memory loader — do not). `CARGO_BIN_EXE_esrun` is set by Cargo and points
//! at the freshly built binary.

use std::path::PathBuf;
use std::process::{Command, Output};

/// A `Command` for the built `esrun` binary.
fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
}

/// Absolute path to a file under `tests/fixtures/`.
fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(rel)
}

fn run_file(rel: &str) -> Output {
    esrun()
        .arg(fixture(rel))
        .output()
        .expect("failed to spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn runs_a_module_with_imports_meta_and_tla() {
    let out = run_file("main.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello modules"), "{stdout}");
    // import.meta.url is a file: URL ending in the entry's path.
    assert!(stdout.contains("URL:file://"), "{stdout}");
    assert!(stdout.contains("main.mjs"), "{stdout}");
    // Top-level await resolved.
    assert!(stdout.contains("AWAITED:42"), "{stdout}");
}

#[test]
fn resolves_parent_directory_imports_on_disk() {
    let out = run_file("sub/nested.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("nested:hello modules"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn streams_a_fetch_request_body_to_a_server() {
    // Real reqwest chunked upload → real runtime:http echo server, end to end.
    let out = run_file("fetch-stream-upload.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("UPLOAD_OK"), "{stdout}");
    assert!(stdout.contains("status:200"), "{stdout}");
}

#[test]
fn streams_an_http_server_response_to_a_client() {
    // Chunked download: a runtime:http handler returns a ReadableStream body
    // produced over time; the real reqwest client must see it incrementally
    // (several reads, no Content-Length) rather than as one buffered payload.
    let out = run_file("http-stream-response.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("STREAM_OK"), "{stdout}");
    assert!(stdout.contains("reads>1:true"), "{stdout}");
    assert!(stdout.contains("content-length:null"), "{stdout}");
    assert!(stdout.contains("x-mode:stream"), "{stdout}");
}

#[test]
fn streams_an_http_request_body_through_to_the_response() {
    // Proxy/echo: `new Response(request.body)` pipes the inbound stream back
    // out on the same request — concurrent pull + push, nothing buffered.
    let out = run_file("http-stream-echo.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("ECHO_OK"), "{stdout}");
    assert!(stdout.contains("status:200"), "{stdout}");
}

#[test]
fn honours_every_fetch_redirect_mode_over_the_wire() {
    // Real 3xx responses from a real runtime:http server through the real
    // reqwest transport — the stub transport in the runtime's own tests cannot
    // prove that the redirect policy actually reaches the client.
    let out = run_file("fetch-redirect.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(
        stdout.contains("FOLLOW status:200 redirected:true landed:true body:landed"),
        "{stdout}"
    );
    assert!(
        stdout.contains("MANUAL status:302 redirected:false location:true"),
        "{stdout}"
    );
    assert!(stdout.contains("ERROR threw:TypeError"), "{stdout}");
    assert!(stdout.contains("DIRECT redirected:false"), "{stdout}");
    assert!(
        stdout.contains("LOOP code:ERR_TOO_MANY_REDIRECTS"),
        "{stdout}"
    );
    assert!(stdout.contains("REDIRECT_OK"), "{stdout}");
}

#[test]
fn decodes_compressed_response_bodies_and_identifies_itself() {
    // Real compressed bytes from a real runtime:http server: the guest must see
    // the decoded body, and the headers describing the compressed form must not
    // survive to describe bytes it never sees.
    let out = run_file("fetch-content-encoding.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    for coding in ["gzip", "deflate", "br"] {
        assert!(
            stdout.contains(&format!(
                "DECODE {coding} ok:true content-encoding:null content-length:null"
            )),
            "{stdout}"
        );
    }
    assert!(
        stdout.contains("ACCEPT gzip:true br:true deflate:true"),
        "{stdout}"
    );
    assert!(stdout.contains("EXPLICIT ok:true"), "{stdout}");
    assert!(
        stdout.contains("UNKNOWN body:true content-encoding:zstd"),
        "{stdout}"
    );
    assert!(
        stdout.contains("UA default:true matches-navigator:true"),
        "{stdout}"
    );
    assert!(stdout.contains("UA override:true"), "{stdout}");
    assert!(stdout.contains("ENCODING_OK"), "{stdout}");
}

#[test]
fn a_server_handler_learns_the_client_hung_up() {
    // Needs a real socket to close, so it cannot be an in-process test. Also
    // asserts the opposite: a request that completed leaves its signal
    // unaborted, and the process exits — a disconnect watch that never settled
    // would hold the driven loop open forever.
    let out = run_file("http-request-signal.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("PLAIN body:plain done"), "{stdout}");
    assert!(
        stdout.contains("ABORT reason:AbortError promptly:true"),
        "{stdout}"
    );
    assert!(stdout.contains("QUICK body:quick done"), "{stdout}");
    assert!(stdout.contains("QUICK aborted:false"), "{stdout}");
    assert!(stdout.contains("SIGNAL_OK"), "{stdout}");
}

#[test]
fn a_deadline_ends_a_request_the_server_never_answers() {
    // The connect timeout cannot help here — the peer accepts and then goes
    // silent — so this pins the documented answer for bounding a whole request.
    let out = run_file("fetch-timeout.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(
        stdout.contains("TIMEOUT name:TimeoutError promptly:true"),
        "{stdout}"
    );
    assert!(stdout.contains("NORMAL body:prompt"), "{stdout}");
    assert!(stdout.contains("TIMEOUT_OK"), "{stdout}");
}

#[test]
fn runs_an_inline_module_snippet() {
    let out = esrun()
        .arg("-e=console.log('inline', 6 * 7)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("inline 42"), "{}", stdout(&out));
}

#[test]
fn inline_snippet_supports_top_level_await() {
    let out = esrun()
        .arg("-e=const x = await Promise.resolve(5); console.log('awaited', x)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("awaited 5"), "{}", stdout(&out));
}

#[test]
fn top_level_throw_fails_with_uncaught_report() {
    let out = run_file("throws.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    // Side effects before the throw still ran...
    assert!(stdout(&out).contains("before throw"), "{}", stdout(&out));
    // ...and the throw is reported once as Uncaught.
    let stderr = stderr(&out);
    assert!(stderr.contains("error: uncaught exception"), "{stderr}");
    assert!(stderr.contains("fixture boom"), "{stderr}");
    assert!(stderr.contains("at file://"), "{stderr}");
}

#[test]
fn missing_import_is_a_load_error() {
    let out = run_file("missing.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("module loading failed"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn uninstalled_bare_package_is_not_found() {
    // bare.mjs imports "lodash", which is not in any node_modules here.
    let out = run_file("bare.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("cannot find package"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn nonexistent_entry_file_errors_cleanly() {
    let out = run_file("no-such-file.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));
}

#[test]
fn resolves_a_bare_esm_package_from_node_modules() {
    let out = run_file("uses-package.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("hi world from greeter"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn rejects_a_commonjs_package() {
    let out = run_file("uses-cjs-package.mjs");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(stderr(&out).contains("CommonJS"), "{}", stderr(&out));
}

#[test]
fn dynamic_import_resolves_relative_and_node_modules() {
    let out = run_file("dynamic.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello modules"), "{stdout}");
    assert!(stdout.contains("hi dynamic from greeter"), "{stdout}");
}

#[test]
fn all_esm_export_import_patterns_work() {
    // esm/consumer.mjs exercises every standardized export/import form against
    // the export fixtures and throws on any mismatch.
    let out = run_file("esm/consumer.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ESM-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn node_modules_export_patterns_work() {
    // A node_modules package with an exports map: ".", a subpath, and a wildcard
    // subpath, with named + default exports and an internal re-export.
    let out = run_file("esm/consumer-pkg.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("PKG-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn node_modules_export_conditions_work() {
    // Condition matching in author order, unasserted conditions skipped, array
    // fallbacks, nested conditions, and a `null`-withdrawn subpath.
    let out = run_file("esm/consumer-conditions.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("COND-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn private_imports_and_self_reference_work() {
    // `#specifier` through the package's own "imports" map (path, pattern, and
    // another package), plus importing this package by its own name.
    let out = run_file("esm/consumer-private.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("PRIVATE-SUITE-OK"), "{}", stdout(&out));
}

#[test]
fn runtime_process_exposes_env_args_platform_cwd() {
    let out = esrun()
        .env("ESRUN_TEST_VAR", "hello")
        .arg(format!(
            "-e={}",
            "import { env, args, platform, arch, cwd } from 'runtime:process'; \
             console.log(env.ESRUN_TEST_VAR, platform, arch, args.join(','), typeof cwd());",
        ))
        .arg("alpha")
        .arg("beta")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("hello"), "env: {s}");
    // platform is the OS-native std value (linux/macos/windows).
    assert!(
        s.contains("linux") || s.contains("macos") || s.contains("windows"),
        "platform: {s}"
    );
    // arch is the OS-native std value (x86_64/aarch64/arm/...).
    assert!(
        s.contains("x86_64") || s.contains("aarch64") || s.contains("arm"),
        "arch: {s}"
    );
    assert!(s.contains("alpha,beta"), "args: {s}"); // user args only, in order
    assert!(s.contains("string"), "cwd: {s}");
}

#[test]
fn runtime_process_exit_sets_exit_code() {
    let out = esrun()
        .arg("-e=import { exit } from 'runtime:process'; console.log('before'); exit(5); console.log('after');")
        .output()
        .expect("spawn esrun");
    assert_eq!(out.status.code(), Some(5), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("before"), "{}", stdout(&out));
    assert!(
        !stdout(&out).contains("after"),
        "exit did not halt: {}",
        stdout(&out)
    );
}

// POSIX-only: separators/roots are platform-specific and the CI test job runs
// on Linux (macOS is also POSIX). Windows path semantics are exercised by hand.
#[cfg(unix)]
#[test]
fn runtime_path_exposes_modern_surface() {
    let out = esrun()
        .arg(format!(
            "-e={}",
            "import * as p from 'runtime:path'; const o=(k,v)=>console.log(k+'='+v);\
             o('sep',p.sep); o('delimiter',p.delimiter);\
             o('join',p.join('a','b','..','c/d/'));\
             o('normalize',p.normalize('/a/./b/../c'));\
             o('isAbs',p.isAbsolute('/a')+','+p.isAbsolute('a'));\
             o('dirname',p.dirname('/a/b/c.txt'));\
             o('basename',p.basename('/a/b/c.txt'));\
             o('extname',p.extname('archive.tar.gz'));\
             o('relative',p.relative('/a/b/c','/a/x/y'));\
             o('parse',JSON.stringify(p.parse('/a/b/c.txt')));\
             o('resolveAbs',p.resolve('/x','y','z'));\
             o('fromFileURL',p.fromFileURL('file:///a/b%20c.txt'));\
             o('toFileURL',p.toFileURL('/a/b c.txt').href);",
        ))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "sep=/",
        "delimiter=:",
        "join=a/c/d",
        "normalize=/a/c",
        "isAbs=true,false",
        "dirname=/a/b",
        "basename=c.txt",
        "extname=.gz",
        "relative=../../x/y",
        "parse={\"root\":\"/\",\"dir\":\"/a/b\",\"base\":\"c.txt\",\"name\":\"c\",\"ext\":\".txt\"}",
        "resolveAbs=/x/y/z",
        "fromFileURL=/a/b c.txt",
        "toFileURL=file:///a/b%20c.txt",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

#[test]
fn runtime_fs_read_write_stat_and_jail() {
    // A scratch dir that becomes the jail root (no package.json there, so the
    // detected root is the dir itself); run with cwd set to it.
    let tmp = std::env::temp_dir().join(format!("esrun-fs-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("mktemp");
    let script = "import { file, write, readDir, stat, mkdir, remove, exists } from 'runtime:fs';\
        await mkdir('sub', { recursive: true });\
        console.log('WROTE=' + await write('sub/a.txt', 'hi'));\
        console.log('TEXT=' + await file('sub/a.txt').text());\
        const s = await stat('sub/a.txt');\
        console.log('SIZE=' + s.size + ' ISFILE=' + s.isFile);\
        console.log('DIR=' + (await readDir('sub')).map(e => e.name).join(','));\
        console.log('EXISTS=' + await exists('sub/a.txt'));\
        await remove('sub', { recursive: true });\
        console.log('GONE=' + !(await exists('sub')));\
        try { await file('../escape.txt').text(); console.log('JAIL=open'); }\
        catch { console.log('JAIL=blocked'); }";
    let out = esrun()
        .current_dir(&tmp)
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "WROTE=2",
        "TEXT=hi",
        "SIZE=2 ISFILE=true",
        "DIR=a.txt",
        "EXISTS=true",
        "GONE=true",
        "JAIL=blocked",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

#[test]
fn runtime_fs_glob_covers_all_patterns() {
    // match() is pure (no FS), so the full pattern set runs without fixtures.
    let script = "import { Glob } from 'runtime:fs'; const m = (p, s) => new Glob(p).match(s);\
        const out = [];\
        out.push('q=' + m('???.ts','foo.ts') + ',' + m('???.ts','foobar.ts'));\
        out.push('star=' + m('*.ts','index.ts') + ',' + m('*.ts','src/index.ts'));\
        out.push('globstar=' + m('**/*.ts','src/index.ts'));\
        out.push('class=' + m('ba[rz].ts','bar.ts') + ',' + m('ba[rz].ts','bat.ts'));\
        out.push('range=' + m('f[a-c].ts','fb.ts') + ',' + m('f[a-c].ts','fz.ts'));\
        out.push('negbang=' + m('f[!o]o.ts','fao.ts') + ',' + m('f[!o]o.ts','foo.ts'));\
        out.push('negcaret=' + m('f[^o]o.ts','fao.ts') + ',' + m('f[^o]o.ts','foo.ts'));\
        out.push('brace=' + m('{a,b}.ts','a.ts') + ',' + m('{a,b}.ts','c.ts'));\
        out.push('not=' + m('!index.ts','a.ts') + ',' + m('!index.ts','index.ts'));\
        out.push('escape=' + m('\\\\!x.ts','!x.ts') + ',' + m('\\\\!x.ts','x.ts'));\
        console.log(out.join('\\n'));";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for expected in [
        "q=true,false",
        "star=true,false",
        "globstar=true",
        "class=true,false",
        "range=true,false",
        "negbang=true,false",
        "negcaret=true,false",
        "brace=true,false",
        "not=true,false",
        "escape=true,false",
    ] {
        assert!(s.contains(expected), "missing {expected:?} in:\n{s}");
    }
}

#[test]
fn runtime_net_tcp_echo_roundtrip() {
    // Loopback: a one-shot echo server + a client, exercising connect/listen/
    // accept, the Socket read/write streams, half-close, and clean shutdown
    // (the process must exit, not hang).
    let script = "import { connect, listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        (async () => {\
          for await (const conn of server) {\
            const w = conn.writable.getWriter();\
            for await (const chunk of conn.readable) await w.write(chunk);\
            await w.close();\
            await server.close();\
          }\
        })();\
        const sock = connect({ hostname: '127.0.0.1', port });\
        const w = sock.writable.getWriter();\
        await w.write(new TextEncoder().encode('ping'));\
        await w.close();\
        let out = ''; const dec = new TextDecoder();\
        for await (const chunk of sock.readable) out += dec.decode(chunk);\
        console.log('NET:' + out);";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("NET:ping"), "{}", stdout(&out));
}

#[test]
fn runtime_net_starttls_surface_and_guards() {
    // The startTls() JS surface: a plain socket can't be upgraded, an unknown
    // secureTransport is rejected, and a "starttls" socket opens (upgradable).
    // Also asserts the WinterTC SocketError shape (a TypeError whose message is
    // prefixed "SocketError: ", marked "+SE" below) across both a synchronous
    // option-validation throw and a runtime failure (a refused connect rejecting
    // .opened — exercising the socketOp() op-rejection wrapper). The TLS
    // handshake itself is covered by hermetic provider tests (the CLI trusts the
    // public webpki roots, so a loopback self-signed cert can't be exercised here).
    let script = "import { connect, listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        const a = connect({ hostname: '127.0.0.1', port });\
        const tag = (e) => e.constructor.name + (e.message.startsWith('SocketError: ') ? '+SE' : '');\
        let g1 = 'none';\
        try { a.startTls(); } catch (e) { g1 = tag(e); }\
        let g2 = 'none';\
        try { connect({ hostname: '127.0.0.1', port }, { secureTransport: 'x' }); }\
        catch (e) { g2 = tag(e); }\
        const b = connect({ hostname: '127.0.0.1', port }, { secureTransport: 'starttls' });\
        let g3 = 'none';\
        try { const c = connect('127.0.0.1:1'); await c.opened; } catch (e) { g3 = tag(e); }\
        console.log('STARTTLS:' + g1 + ':' + g2 + ':' + g3 + ':' + (b.upgraded === false));\
        await a.close('done'); await b.close(); await server.close();";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("STARTTLS:TypeError+SE:TypeError+SE:TypeError+SE:true"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_net_listener_close_ends_accept_loop() {
    // A detached `for await (conn of server)` loop, closed from the main flow,
    // must terminate (and let the process exit) — the parked accept resolves to
    // null. Regression for the listener-close cancellation.
    let script = "import { listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        await server.addr;\
        let ended = false;\
        const loop = (async () => { for await (const _ of server) {} ended = true; })();\
        await server.close();\
        await loop;\
        console.log('CLOSED:' + ended);";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("CLOSED:true"), "{}", stdout(&out));
}

#[test]
fn runtime_net_half_open_and_combined_address() {
    // allowHalfOpen: the server FINs its write; the client (allowHalfOpen: true)
    // sees read EOF yet can still write — a default socket would be torn down.
    // Also checks SocketInfo.remoteAddress is the WinterTC "host:port" form.
    let script = "import { connect, listen } from 'runtime:net';\
        const enc = new TextEncoder(); const dec = new TextDecoder();\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        (async () => {\
          for await (const conn of server) {\
            const w = conn.writable.getWriter();\
            await w.write(enc.encode('hi'));\
            await w.close();\
            let got = '';\
            for await (const chunk of conn.readable) got += dec.decode(chunk);\
            console.log('GOT:' + got);\
            await server.close();\
          }\
        })();\
        const sock = connect({ hostname: '127.0.0.1', port }, { allowHalfOpen: true });\
        const info = await sock.opened;\
        let out = '';\
        for await (const chunk of sock.readable) out += dec.decode(chunk);\
        const w = sock.writable.getWriter();\
        await w.write(enc.encode('after'));\
        await w.close();\
        console.log('HALF:' + out + ':' + info.remoteAddress.includes(':'));";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("HALF:hi:true"), "{s}");
    assert!(
        s.contains("GOT:after"),
        "half-open write did not reach peer:\n{s}"
    );
}

#[test]
fn runtime_net_listen_tls_surface_and_bind() {
    // Server-side TLS termination on listen(): secureTransport: "on" needs a
    // cert+key (TypeError without), an unknown mode is rejected, and a real PEM
    // cert/key binds a TLS listener (proving cert/key bytes thread through the op
    // and the ServerConfig builds). The handshake itself is covered by hermetic
    // provider tests — the CLI trusts the public webpki roots, so a loopback
    // self-signed cert can't be verified end to end here.
    let script = r#"import { listen } from 'runtime:net';
        const cert = `-----BEGIN CERTIFICATE-----
MIIBfzCCASWgAwIBAgIUW7VE71ojFZyS30mgfs6/aXgiVxkwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MDYxODIwMzUyNFoYDzIxMjYwNTI1
MjAzNTI0WjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAARMmfJSPruUifoGAbRY3gh/Sss+GDYDVXKwlHaaiSsPtueuWJ1GwC4P
m9kbriVs1/9YTXpKdjsPga00am7iwK7co1MwUTAdBgNVHQ4EFgQU+R9EaUJyGVun
alMb5fKe5Hlx53QwHwYDVR0jBBgwFoAU+R9EaUJyGVunalMb5fKe5Hlx53QwDwYD
VR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNIADBFAiEA/jRmQTJnYabU4zgNrGeI
bO2qBiYf5YwjN+WfeyP3ecUCIHFmUGu2HNVscjPnIlBRJpeBIw29Xm8r+ddP95M+
hMU9
-----END CERTIFICATE-----`;
        const key = `-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgoI+sRkHefoxwbeyv
0GUmYblUBM3eh+YRg6PRzrJEB5yhRANCAARMmfJSPruUifoGAbRY3gh/Sss+GDYD
VXKwlHaaiSsPtueuWJ1GwC4Pm9kbriVs1/9YTXpKdjsPga00am7iwK7c
-----END PRIVATE KEY-----`;
        let r = '';
        const tag = (e) => e.constructor.name + (e.message.startsWith('SocketError: ') ? '+SE' : '');
        try { listen({ port: 0, secureTransport: 'on' }); r += 'NOCERT:no-throw'; }
        catch (e) { r += 'NOCERT:' + tag(e); }
        try { listen({ port: 0, secureTransport: 'bogus' }); r += ':MODE:no-throw'; }
        catch (e) { r += ':MODE:' + tag(e); }
        const server = listen({ hostname: '127.0.0.1', port: 0, secureTransport: 'on', cert, key, alpn: ['h2'] });
        const { port } = await server.addr;
        r += ':BOUND:' + (port > 0);
        await server.close();
        console.log(r);"#;
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("NOCERT:TypeError+SE:MODE:TypeError+SE:BOUND:true"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn runtime_http_serve_and_fetch_roundtrip() {
    // Loopback: serve() an echo-ish handler, fetch() it through the real HTTP
    // client, read body + a custom header, then stop the server so the process
    // exits cleanly (must not hang).
    let script = "import { serve } from 'runtime:http';\
        const server = serve({ hostname: '127.0.0.1', port: 0 }, async (req) => {\
          const who = await req.text();\
          return new Response('hello ' + (who || 'world'), {\
            status: 201, headers: { 'x-greeting': 'hi' },\
          });\
        });\
        const { port } = await server.addr;\
        const res = await fetch(`http://127.0.0.1:${port}/`, { method: 'POST', body: 'bun' });\
        console.log('HTTP:' + res.status + ':' + res.headers.get('x-greeting') + ':' + (await res.text()));\
        await server.stop();";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("HTTP:201:hi:hello bun"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn unknown_runtime_builtin_module_errors() {
    let out = esrun()
        .arg("-e=import 'runtime:nope';")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "should exit non-zero");
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn types_command_emits_declarations() {
    let out = esrun().arg("types").output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    for m in ["runtime:process", "runtime:path", "runtime:fs"] {
        assert!(
            s.contains(&format!("declare module \"{m}\"")),
            "missing declaration for {m} in:\n{s}"
        );
    }
}

#[test]
fn types_install_writes_package_and_wires_tsconfig() {
    let dir = std::env::temp_dir().join(format!("esrun-types-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let out = esrun()
        .arg("types")
        .arg("--install")
        .current_dir(&dir)
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    // A type package is written under node_modules/@opentf/esrun.
    let dts = dir.join("node_modules/@opentf/esrun/index.d.ts");
    assert!(dts.exists(), "index.d.ts not written");
    assert!(dir.join("node_modules/@opentf/esrun/package.json").exists());
    assert!(
        std::fs::read_to_string(&dts)
            .unwrap()
            .contains("declare module \"runtime:fs\"")
    );

    // tsconfig.json is created and wired up (typeRoots + types).
    let ts = std::fs::read_to_string(dir.join("tsconfig.json")).unwrap();
    assert!(
        ts.contains("node_modules/@opentf"),
        "typeRoots missing:\n{ts}"
    );
    assert!(ts.contains("\"esrun\""), "types entry missing:\n{ts}");

    // Re-running is idempotent — `esrun` isn't duplicated in `types`.
    let out2 = esrun()
        .arg("types")
        .arg("--install")
        .current_dir(&dir)
        .output()
        .expect("spawn esrun");
    assert!(out2.status.success());
    let ts2 = std::fs::read_to_string(dir.join("tsconfig.json")).unwrap();
    assert_eq!(
        ts2.matches("\"esrun\"").count(),
        1,
        "esrun duplicated:\n{ts2}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn version_flag_succeeds() {
    let out = esrun().arg("--version").output().expect("spawn esrun");
    assert!(out.status.success());
    assert!(stdout(&out).contains("esrun"), "{}", stdout(&out));
}

#[test]
fn unhandled_rejection_reports_stack_trace() {
    let out = esrun()
        .arg("-e=setTimeout(() => { Promise.reject(new TypeError('async boom')); }, 0);")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "should exit non-zero");
    let stderr = stderr(&out);
    assert!(
        stderr.contains("error: 1 unhandled promise rejection(s)"),
        "{stderr}"
    );
    assert!(stderr.contains("TypeError: async boom"), "{stderr}");
    assert!(stderr.contains("at file://"), "{stderr}");
}

#[test]
fn runtime_urlpattern_works_globally() {
    let script = "
        // Test 1: Basic string pattern with base
        const p1 = new URLPattern('/api/users/:id', 'https://api.example.com');
        console.log('MATCH1=' + p1.test('https://api.example.com/api/users/123'));
        console.log('MATCH2=' + p1.test('https://api.example.com/api/posts/123'));
        console.log('ID1=' + p1.exec('https://api.example.com/api/users/456').pathname.groups.id);

        // Test 2: Absolute pattern string
        const p2 = new URLPattern('https://api.example.com/api/users/:id');
        console.log('MATCH3=' + p2.test('https://api.example.com/api/users/123'));

        // Test 3: Object pattern with wildcards
        const p3 = new URLPattern({ protocol: 'http*', hostname: '*.example.com', pathname: '/data/*' });
        console.log('MATCH4=' + p3.test('https://sub.example.com/data/123/456'));
        console.log('MATCH5=' + p3.test('ftp://sub.example.com/data/123'));

        // Test 4: Parameter mapping in different parts
        const p4 = new URLPattern({ hostname: ':sub.example.com', pathname: '/files/:file' });
        const exec4 = p4.exec('https://test.example.com/files/document.txt');
        console.log('SUB=' + exec4.hostname.groups.sub);
        console.log('FILE=' + exec4.pathname.groups.file);

        // Test 5: Ignored case. A dictionary carries its own baseURL — pairing
        // one with a separate base argument is a TypeError.
        const p5 = new URLPattern(
          { pathname: '/API/:id', baseURL: 'https://api.example.com' },
          { ignoreCase: true },
        );
        console.log('MATCH6=' + p5.test('https://api.example.com/api/123'));
    ";
    let out = esrun()
        .arg(format!("-e={}", script))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("MATCH1=true"), "{}", s);
    assert!(s.contains("MATCH2=false"), "{}", s);
    assert!(s.contains("ID1=456"), "{}", s);
    assert!(s.contains("MATCH3=true"), "{}", s);
    assert!(s.contains("MATCH4=true"), "{}", s);
    assert!(s.contains("MATCH5=false"), "{}", s);
    assert!(s.contains("SUB=test"), "{}", s);
    assert!(s.contains("FILE=document.txt"), "{}", s);
    assert!(s.contains("MATCH6=true"), "{}", s);
}

#[test]
fn import_meta_resolve_resolves_against_the_module() {
    let out = run_file("resolve.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);

    assert!(stdout.contains("TYPE:function"), "{stdout}");
    // Relative and parent specifiers resolve against import.meta.url, so the
    // result is an absolute file: URL naming the sibling / parent file.
    assert!(stdout.contains("REL:file://"), "{stdout}");
    assert!(stdout.contains("/fixtures/greet.mjs"), "{stdout}");
    assert!(stdout.contains("PARENT:file://"), "{stdout}");
    assert!(
        stdout.contains("/up.mjs") && !stdout.contains("/fixtures/up.mjs"),
        "`../` must climb out of the fixtures directory: {stdout}"
    );
    assert!(stdout.contains("ABS:file:///abs/z.mjs"), "{stdout}");
    assert!(stdout.contains("URL:file:///q.mjs"), "{stdout}");
    // A `runtime:` builtin is already absolute and resolves to itself.
    assert!(stdout.contains("BUILTIN:runtime:process"), "{stdout}");
    // Resolution does no I/O: a path that does not exist still resolves.
    assert!(stdout.contains("MISSING:file://"), "{stdout}");
    assert!(stdout.contains("/definitely-not-here.mjs"), "{stdout}");
    // A bare specifier would need node_modules read synchronously; rather than
    // answer with a URL it never resolved, it refuses.
    assert!(stdout.contains("BARE!TypeError"), "{stdout}");
    assert!(stdout.contains("PRIV!TypeError"), "{stdout}");
    assert!(stdout.contains("NODE!TypeError"), "{stdout}");
    // Each refusal names the kind of specifier it got: a #private one resolves
    // through the package's "imports" map, not through node_modules.
    assert!(stdout.contains("PRIVATE:true"), "{stdout}");
    assert!(stdout.contains("BAREMSG:true"), "{stdout}");
}

#[test]
fn serve_rejects_incomplete_tls_options() {
    // Failing at `serve` beats binding a port and then rejecting every
    // handshake, which looks like a working server nothing can talk to.
    let out = esrun()
        .arg(format!(
            "-e={}",
            "import { serve } from 'runtime:http'; \
             const cases = [ \
               { secureTransport: 'on' }, \
               { secureTransport: 'on', cert: 'x' }, \
               { secureTransport: 'on', key: 'x' }, \
               { secureTransport: 'yes', cert: 'x', key: 'x' }, \
             ]; \
             for (const o of cases) { \
               try { serve({ port: 0, ...o }, () => new Response('x')); console.log('NO THROW'); } \
               catch (e) { console.log(`THREW ${e.constructor.name}`); } \
             } \
             console.log('TLS_OPTS_OK');",
        ))
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert_eq!(
        stdout.matches("THREW TypeError").count(),
        4,
        "every incomplete or unknown TLS option must throw: {stdout}"
    );
    assert!(!stdout.contains("NO THROW"), "{stdout}");
    assert!(stdout.contains("TLS_OPTS_OK"), "{stdout}");
}

#[test]
fn the_added_fs_surface_works_against_a_real_disk() {
    // The in-process tests use an in-memory filesystem, so this is the one that
    // exercises the real jail, real temp-name generation, and real permissions.
    let out = run_file("fs-surface.mjs");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("TEMPDIR named:true"), "{stdout}");
    assert!(stdout.contains("TEMPDIR unique:true"), "{stdout}");
    assert!(stdout.contains("TEMPFILE inside:true"), "{stdout}");
    assert!(stdout.contains("COPY bytes:11 same:true"), "{stdout}");
    assert!(stdout.contains("TRUNCATE text:hello"), "{stdout}");
    assert!(stdout.contains("REALPATH clean:true"), "{stdout}");
    assert!(stdout.contains("CHMOD ok:true"), "{stdout}");
    assert!(
        stdout.contains("REALPATH missing:ERR_NOT_FOUND"),
        "{stdout}"
    );
    assert!(stdout.contains("FS_SURFACE_OK"), "{stdout}");
}
