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
fn runs_an_inline_module_snippet() {
    let out = esrun()
        .arg("-e")
        .arg("console.log('inline', 6 * 7)")
        .output()
        .expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("inline 42"), "{}", stdout(&out));
}

#[test]
fn inline_snippet_supports_top_level_await() {
    let out = esrun()
        .arg("-e")
        .arg("const x = await Promise.resolve(5); console.log('awaited', x)")
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
fn runtime_process_exposes_env_args_platform_cwd() {
    let out = esrun()
        .env("ESRUN_TEST_VAR", "hello")
        .arg("-e")
        .arg(
            "import { env, args, platform, arch, cwd } from 'runtime:process'; \
             console.log(env.ESRUN_TEST_VAR, platform, arch, args.join(','), typeof cwd());",
        )
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
        .arg("-e")
        .arg("import { exit } from 'runtime:process'; console.log('before'); exit(5); console.log('after');")
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
        .arg("-e")
        .arg(
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
        )
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
        .arg("-e")
        .arg(script)
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
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
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
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("NET:ping"), "{}", stdout(&out));
}

#[test]
fn runtime_net_starttls_surface_and_guards() {
    // The startTls() JS surface: a plain socket can't be upgraded, an unknown
    // secureTransport is rejected, and a "starttls" socket opens (upgradable).
    // The TLS handshake itself is covered by hermetic provider tests (the CLI
    // trusts the public webpki roots, so a loopback self-signed cert can't be
    // exercised here).
    let script = "import { connect, listen } from 'runtime:net';\
        const server = listen({ hostname: '127.0.0.1', port: 0 });\
        const { port } = await server.addr;\
        const a = connect({ hostname: '127.0.0.1', port });\
        let g1 = 'none';\
        try { a.startTls(); } catch (e) { g1 = e.constructor.name; }\
        let g2 = 'none';\
        try { connect({ hostname: '127.0.0.1', port }, { secureTransport: 'x' }); }\
        catch (e) { g2 = e.constructor.name; }\
        const b = connect({ hostname: '127.0.0.1', port }, { secureTransport: 'starttls' });\
        console.log('STARTTLS:' + g1 + ':' + g2 + ':' + (b.upgraded === false));\
        await a.close(); await b.close(); await server.close();";
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("STARTTLS:TypeError:TypeError:true"),
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
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
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
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("HALF:hi:true"), "{s}");
    assert!(s.contains("GOT:after"), "half-open write did not reach peer:\n{s}");
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
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
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
        .arg("-e")
        .arg("import 'runtime:nope';")
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
        .arg("-e")
        .arg("setTimeout(() => { Promise.reject(new TypeError('async boom')); }, 0);")
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

        // Test 5: Ignored case
        const p5 = new URLPattern({ pathname: '/API/:id' }, 'https://api.example.com', { ignoreCase: true });
        console.log('MATCH6=' + p5.test('https://api.example.com/api/123'));
    ";
    let out = esrun().arg("-e").arg(script).output().expect("spawn esrun");
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
