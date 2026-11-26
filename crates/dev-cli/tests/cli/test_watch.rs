//! `esdev test`: what runs at once, and what happens on a save.

use super::*;

/// `--isolation=none` is intentionally the opposite of the default: test
/// files share one module map and global object, which is useful for a large
/// suite whose common dependency graph costs more than its tests.
#[test]
fn tests_can_share_one_process_and_module_cache() {
    let dir = build_dir("b_test_no_isolation");
    write_in(
        &dir,
        "shared.mjs",
        "globalThis.shared_loads = (globalThis.shared_loads ?? 0) + 1;\n\
         export const loads = globalThis.shared_loads;\n",
    );
    write_in(
        &dir,
        "a.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { loads } from './shared.mjs';\n\
         globalThis.from_first_file = loads;\n\
         test('loads the shared module once', () => assertEquals(loads, 1));\n",
    );
    write_in(
        &dir,
        "b.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { loads } from './shared.mjs';\n\
         test('keeps the first file and shared module alive', () => {\n\
           assertEquals(loads, 1);\n\
           assertEquals(globalThis.from_first_file, 1);\n\
         });\n",
    );

    let out = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("2 passed"), "{}", stdout(&out));
}

/// A case's own trailing promise chain finishes before its cleanup runs. A
/// suite written against a browser runner leaves `resolve().then(assert)`
/// un-awaited and expects `assert` to see the DOM the case rendered; tearing
/// down after a single microtask turn hands it a dismantled one. Found porting
/// Preact's suspense suite, where it read as a scheduler difference.
#[test]
fn test_a_cases_trailing_promises_settle_before_its_cleanup() {
    let dir = build_dir("b_test_trailing_chain");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test, afterEach, assertEquals } from 'runtime:test';\n\
         const seq = [];\n\
         afterEach(() => seq.push('afterEach'));\n\
         test('leaves a chain', () => {\n\
           Promise.resolve().then(() => seq.push('one')).then(() => seq.push('two'))\n\
             .then(() => seq.push('three'));\n\
         });\n\
         test('the chain ran first', () => {\n\
           assertEquals(seq, ['one', 'two', 'three', 'afterEach']);\n\
         });\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("2 passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stray promise costs one test, not the file's whole report. Before this,
/// the process died where the rejection surfaced and a suite of a hundred
/// passing tests printed nothing at all — the one thing a test runner must
/// never do.
#[test]
fn test_an_unhandled_rejection_fails_its_case_and_the_rest_still_report() {
    let dir = build_dir("b_test_unhandled_rejection");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('one passes', () => {});\n\
         test('two leaks a rejection', async () => {\n\
           Promise.reject(new Error('boom'));\n\
           await new Promise((resolve) => setTimeout(resolve, 1));\n\
         });\n\
         test('three passes', () => {});\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "a leaked rejection passed:\n{text}");
    assert!(text.contains("2 passed, 1 failed"), "{text}");
    assert!(text.contains("two leaks a rejection"), "{text}");
    assert!(text.contains("boom"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// And when something kills the run outright — a rejection after the last case,
/// with no test left to blame — what already ran is still printed.
#[test]
fn test_a_run_that_dies_still_reports_what_ran() {
    let dir = build_dir("b_test_dies_after_running");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test } from 'runtime:test';\n\
         test('one passes', () => {});\n\
         setTimeout(() => { Promise.reject(new Error('after the run')); }, 300);\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("after the run"), "{text}");
    // Late enough that it lands after the case, but a loaded machine could
    // still deliver it inside one — either way the tally is printed, which is
    // what this is about.
    assert!(text.contains("passed"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_isolation_can_come_from_project_config() {
    let dir = build_dir("b_test_no_isolation_config");
    write_in(
        &dir,
        "esdev.json",
        r#"{"test": {"isolation": "none"}, "build": {"targets": {"app": {"entry": "app.mjs", "out": "dist/app.mjs"}}}}"#,
    );
    write_in(&dir, "app.mjs", "export {};\n");
    write_in(
        &dir,
        "a.test.mjs",
        "import { test } from 'runtime:test';\nglobalThis.config_shared = true;\ntest('a', () => {});\n",
    );
    write_in(
        &dir,
        "b.test.mjs",
        "import { test, assert } from 'runtime:test';\ntest('b', () => assert(globalThis.config_shared));\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("2 passed"), "{}", stdout(&out));
}

#[test]
fn unisolated_tests_refuse_file_scoped_options() {
    let dir = build_dir("b_test_no_isolation_options");
    write_in(
        &dir,
        "one.test.mjs",
        "import { test } from 'runtime:test';\ntest('ok', () => {});\n",
    );
    for flag in ["--jobs=1", "--timeout=100", "--reporter=json"] {
        let out = esdev_in(&dir)
            .args(["test", "--isolation=none", flag])
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success(), "{flag}: {}", stdout(&out));
        assert!(
            stderr(&out).contains("--isolation=none"),
            "{flag}: {}",
            stderr(&out)
        );
    }
}

/// A file is a process, so the machine's cores are the runner's to use — and
/// with more than one running, output is held and printed whole rather than
/// interleaved line by line with another file's.
#[test]
fn tests_run_in_parallel_and_keep_each_files_output_together() {
    let dir = build_dir("b_test_jobs");
    for i in 1..=4 {
        write_in(
            &dir,
            &format!("slow{i}.test.ts"),
            &format!(
                "import {{ test, assertEquals }} from \"runtime:test\";\n\
                 test(\"slow {i}\", async () => {{\n  \
                 await new Promise((r) => setTimeout(r, 300));\n  \
                 console.log(\"line one of {i}\");\n  \
                 console.log(\"line two of {i}\");\n  \
                 assertEquals(1 + 1, 2);\n}});\n"
            ),
        );
    }
    write_in(
        &dir,
        "broken.test.ts",
        "import { test, assertEquals } from \"runtime:test\";\n\
         test(\"fails\", () => { assertEquals(1, 2); });\n",
    );

    let started = std::time::Instant::now();
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let elapsed = started.elapsed();
    assert!(
        !out.status.success(),
        "a failing file passed: {}",
        stdout(&out)
    );

    let report = stdout(&out);
    assert!(report.contains("1 of 5 files failed"), "{report}");
    // Four 300ms files, so anything near 1.2s means they ran one after another.
    assert!(
        elapsed < Duration::from_millis(1100),
        "ran serially: {elapsed:?}\n{report}"
    );
    // Each file's lines stayed together: nothing came between them.
    for i in 1..=4 {
        let together = format!("line one of {i}\nline two of {i}");
        assert!(
            report.replace("\r\n", "\n").contains(&together),
            "output interleaved:\n{report}"
        );
    }

    // --jobs=1 is the other half of the deal: one at a time, writing straight
    // through, for the run where a test is hanging and you want to see it.
    let serial = esdev_in(&dir)
        .args(["test", "--jobs=1", "slow1"])
        .output()
        .expect("spawn esdev test");
    assert!(serial.status.success(), "{}", stderr(&serial));
    assert!(
        stdout(&serial).contains("1 file passed"),
        "{}",
        stdout(&serial)
    );
}

/// `--file` is one run of one file — the shape the parent invokes for each
/// child — so there is nothing to schedule and nothing to re-run.
#[test]
fn test_refuses_to_schedule_a_single_file() {
    let dir = build_dir("b_test_args");
    write_in(
        &dir,
        "one.test.ts",
        "import { test } from \"runtime:test\";\ntest(\"ok\", () => {});\n",
    );

    for flag in ["--watch", "--jobs=2"] {
        let out = esdev_in(&dir)
            .args(["test", "--file=one.test.ts", flag])
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success(), "{flag}: {}", stdout(&out));
        assert!(stderr(&out).contains("--file"), "{flag}: {}", stderr(&out));
    }

    let bad = esdev_in(&dir)
        .args(["test", "--jobs=0"])
        .output()
        .expect("spawn esdev test");
    assert!(!bad.status.success(), "{}", stdout(&bad));
    assert!(stderr(&bad).contains("--jobs=0"), "{}", stderr(&bad));
}

/// The port scheme's own invariant — see [`test_port`].
///
/// Two names hashing together is a *permanent* failure in whichever test binds
/// second, and it reads as that test being broken rather than as a name clash.
/// Asserted here so a new one fails with the reason.
#[test]
fn every_named_port_is_its_own_and_below_the_ephemeral_range() {
    // Every name passed to `test_port` in this file.
    const NAMED: &[&str] = &[
        "p_react_package",
        "preview",
        "s_asset",
        "s_broken_app",
        "s_frontend",
        "s_norestart_app",
        "s_plugin_edit",
        "s_plugin_stylesheet",
        "s_plugins",
        "s_refresh",
        "s_takenport",
    ];

    let mut taken: Vec<(u16, &str)> = Vec::new();
    for name in NAMED {
        let port = port_base(name);
        assert!(
            port < 32768,
            "{name}'s port {port} is inside the ephemeral range, where the \
             kernel may already have handed it out to an outbound socket"
        );
        if let Some((_, other)) = taken.iter().find(|(held, _)| *held == port) {
            panic!("{name} and {other} both want port {port} — change the range or a name");
        }
        taken.push((port, name));
    }
}

/// **A port nobody named is a convenience, and esdev finds another when the
/// usual one is taken.** Two projects open in two terminals is an ordinary
/// afternoon; refusing to start over a number the developer never chose is the
/// tool inventing a problem.
#[test]
fn start_finds_a_free_port_when_none_was_named() {
    let dir = watch_dir("s_freeport");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.mjs", "document.title = 'FREE';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    // No `port` key, so nothing named one.
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );

    // The default must be taken for the length of the test. This test holds
    // it when it can. Other tests start esdev on the default port too, so a
    // failed bind is usually one of them, which lets it go within seconds —
    // and then this test's esdev could take it. Something holding it longer
    // than that is a long-lived server on the machine (a developer's own
    // `esdev start`), which is the same precondition held by somebody else.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let blocker = loop {
        match std::net::TcpListener::bind(("127.0.0.1", 5173)) {
            Ok(listener) => break Some(listener),
            Err(_) if std::time::Instant::now() >= deadline => break None,
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    };

    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let port = announced_port(&log);
    assert_ne!(port, 5173, "esdev bound the port that was taken");

    let document = wait_for_http(port, "/", |body| body.contains("<div id=root>"));
    assert!(document.contains("200 OK"), "{document}");
    assert!(
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains("5173 was taken"),
        "esdev moved without saying so"
    );

    drop(blocker);
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A port that *was* named is a promise.** Moving quietly off it would leave
/// a bookmark, a proxy rule or a second terminal pointing at whatever is
/// already there, so this fails and says what to do instead.
#[test]
fn start_refuses_a_named_port_that_is_taken() {
    let dir = watch_dir("s_takenport");
    let port = test_port("s_takenport");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.mjs", "document.title = 'X';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );

    let _held = std::net::TcpListener::bind(("127.0.0.1", port)).expect("hold the port");
    let out = esdev_in(&dir)
        .args(["start", &format!("--port={port}")])
        .output()
        .expect("spawn esdev start");

    assert!(!out.status.success(), "{}", stdout(&out));
    let err = stderr(&out);
    assert!(err.contains(&format!("127.0.0.1:{port}")), "{err}");
    assert!(err.contains("--port"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn start_needs_a_project_to_start() {
    let dir = build_dir("s_noconfig");
    let out = esdev_in(&dir).arg("start").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("esdev.json"), "{}", stderr(&out));
}

/// The dev loop holds one bundler across rebuilds, and the pass that emits an
/// imported file is inside it — so the collector has to be held too, or a
/// rebuild the bundler answered from its cache forgets a file that is still
/// imported and stops writing it.
#[test]
fn the_dev_loop_serves_an_imported_asset_and_maps_its_bundle() {
    let dir = watch_dir("s_asset");
    let port = test_port("s_asset");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\ndev").expect("write png");
    write_in(
        &dir,
        "src/main.ts",
        "import logo from './logo.png';\n\
         document.body.innerHTML = `<img id=logo src=\"${logo}\">`;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body>\
         <script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outDir": "dist" }} }} }},
                 "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);
    let document = wait_for_http(port, "/", |body| body.contains("<script"));
    assert!(document.contains("200 OK"), "{document}");

    // The bundle names the file, and the file is there to be fetched.
    let bundle = http_get(port, "/assets/main.js").expect("a bundle");
    let url = bundle
        .split("/assets/logo-")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .map(|name| format!("/assets/logo-{name}"))
        .expect("an asset URL in the bundle");
    let asset = http_get(port, &url).expect("the asset");
    assert!(asset.contains("200 OK"), "{url} was not served:\n{asset}");

    // …and the dev bundle carries its map, so what breaks points at the source.
    assert!(
        bundle.contains("sourceMappingURL=data:application/json"),
        "the dev bundle is unmapped"
    );
}

/// A project with no server of its own: esdev serves the output, falls back to
/// index.html for a client-side route, and reloads on a change.
#[test]
fn start_serves_a_frontend_project_and_reloads_it() {
    let dir = watch_dir("s_frontend");
    let port = test_port("s_frontend");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.mjs", "document.title = 'FIRST';\n");
    write_in(&dir, "styles.css", "body{color:red}\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./styles.css\">\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outDir": "dist" }} }} }},
                 "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);

    let document = wait_for_http(port, "/", |body| body.contains("<div id=root>"));
    assert!(document.contains("200 OK"), "{document}");

    // Dev names are stable — no hash — so a reload keeps its cache and a stack
    // trace stays readable.
    assert!(document.contains(r#"src="/assets/main.js""#), "{document}");
    assert!(
        document.contains(r#"href="/assets/styles.css""#),
        "{document}"
    );
    // The update client is esdev's, and it is in the output only.
    assert!(document.contains("WebSocket"), "{document}");
    assert!(document.contains("/@esdev/hmr"), "{document}");
    assert!(
        !std::fs::read_to_string(dir.join("index.html"))
            .expect("read source")
            .contains("WebSocket"),
        "the source document was written to"
    );

    // The bundle is served, and a client-side route falls back to the document.
    let bundle = http_get(port, "/assets/main.js").unwrap_or_default();
    assert!(bundle.contains("FIRST"), "{bundle}");
    assert!(
        bundle.contains("text/javascript"),
        "served with the wrong type: {bundle}"
    );
    let route = http_get(port, "/about").unwrap_or_default();
    assert!(route.contains("<div id=root>"), "{route}");
    // …but a missing file is missing. HTML answered for a .js is a syntax
    // error three steps from its cause.
    let missing = http_get(port, "/assets/nope.js").unwrap_or_default();
    assert!(missing.contains("404"), "{missing}");

    // A change rebuilds, and the new bundle is what is served.
    write_in(&dir, "src/main.mjs", "document.title = 'SECOND';\n");
    let rebuilt = wait_for_http(port, "/assets/main.js", |body| body.contains("SECOND"));
    assert!(rebuilt.contains("SECOND"), "the change never landed");

    let _ = std::fs::remove_dir_all(&dir);
}

/// **A browser-only edit does not restart the server.** Every rebuild used to
/// SIGTERM the child and start it again, so editing a stylesheet cost every
/// open connection and every warm cache the process had, to deliver a server
/// byte for byte identical to the one just stopped.
///
/// The server is restarted when the build changed something it reads, and the
/// browser is told to reload either way. Proved by a nonce fixed at startup: if
/// the process is the same one, the nonce is the same one.
#[test]
fn a_browser_only_change_reloads_without_restarting_the_server() {
    let dir = watch_dir("s_norestart");
    let served = test_port("s_norestart_app");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    // The nonce is created once, when the module is evaluated. A restart is the
    // only thing that can change it.
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             const nonce = String(Math.random());\n\
             serve({{ port: {served} }}, () => new Response('SERVER-A ' + nonce));\n"
        ),
    );
    write_in(&dir, "src/main.mjs", "document.title = 'CLIENT-ONE';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "build": { "targets": {
                   "server": { "entry": "src/server.mjs", "out": "dist/server.js" },
                   "web": { "entry": "index.html", "outDir": "dist" } } },
                 "dev": { "servers": { "server": {} } } }"#,
    );

    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let first = wait_for_http(served, "/", |body| body.contains("SERVER-A"));
    assert!(first.contains("SERVER-A"), "never came up: {first}");
    let nonce = nonce_of(&first);

    // A browser-only edit. The client bundle is rebuilt, under the dev
    // directory like every other development build…
    write_in(&dir, "src/main.mjs", "document.title = 'CLIENT-TWO';\n");
    let bundle = dir.join(".dev/dist/assets/main.js");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if std::fs::read_to_string(&bundle)
            .unwrap_or_default()
            .contains("CLIENT-TWO")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        std::fs::read_to_string(&bundle)
            .unwrap_or_default()
            .contains("CLIENT-TWO"),
        "the client change never rebuilt\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    // …and the same process is still answering. Given a moment, because a
    // restart would take one.
    std::thread::sleep(Duration::from_secs(2));
    let after = http_get(served, "/").unwrap_or_default();
    assert!(after.contains("SERVER-A"), "{after}");
    assert_eq!(
        nonce_of(&after),
        nonce,
        "a browser-only change restarted the server"
    );

    // A server edit still restarts it, which is the other half of the promise.
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             const nonce = String(Math.random());\n\
             serve({{ port: {served} }}, () => new Response('SERVER-B ' + nonce));\n"
        ),
    );
    let restarted = wait_for_http(served, "/", |body| body.contains("SERVER-B"));
    assert!(
        restarted.contains("SERVER-B"),
        "a server change did not restart it: {restarted}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The digits after `SERVER-x ` in a response body.
fn nonce_of(response: &str) -> String {
    let at = response.rfind("SERVER-").expect("a marked body");
    response[at..]
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_string()
}

/// The promise that makes the loop usable: a syntax error mid-edit costs a
/// message, not the server you were about to fix it on.
#[test]
fn a_failed_build_leaves_the_running_server_alone() {
    let dir = watch_dir("s_broken");
    let served = test_port("s_broken_app");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             serve({{ port: {served} }}, () => new Response('ALIVE'));\n"
        ),
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "build": { "targets": { "server": { "entry": "src/server.mjs", "out": "dist/server.js" } } },
                 "dev": { "servers": { "server": {} } } }"#,
    );

    let _supervisor = start_in(&dir);
    let alive = wait_for_http(served, "/", |body| body.contains("ALIVE"));
    assert!(alive.contains("ALIVE"), "the server never came up: {alive}");

    // Break it, and give the watcher long enough to have acted.
    write_in(
        &dir,
        "src/server.mjs",
        "import { serve } from 'runtime:http'; serve({\n",
    );
    std::thread::sleep(Duration::from_secs(3));
    let still = http_get(served, "/").unwrap_or_default();
    assert!(
        still.contains("ALIVE"),
        "a failed build took the server down: {still:?}"
    );

    // Fix it, and the fix is what is running.
    write_in(
        &dir,
        "src/server.mjs",
        &format!(
            "import {{ serve }} from 'runtime:http';\n\
             serve({{ port: {served} }}, () => new Response('FIXED'));\n"
        ),
    );
    let fixed = wait_for_http(served, "/", |body| body.contains("FIXED"));
    assert!(fixed.contains("FIXED"), "the fix never landed: {fixed}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A syntax error mid-edit is said once, by the build, with its codeframe —
/// not first by the hot update that could not be computed for the same reason.
#[test]
fn a_parse_error_in_the_loop_is_reported_once() {
    let dir = watch_dir("s_parse_once");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.ts", "document.title = 'working';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body><script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build":{"targets":{"web":{"entry":"index.html","outDir":"dist"}}},"dev":{"servers":{"web":{}}}}"#,
    );
    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let _port = announced_port(&log);

    write_in(&dir, "src/main.ts", "document.title = ;\n");
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut said = String::new();
    while std::time::Instant::now() < deadline {
        said = std::fs::read_to_string(&log).unwrap_or_default();
        if said.contains("PARSE_ERROR") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Long enough for a second report to have followed the first.
    std::thread::sleep(Duration::from_millis(500));
    let said = std::fs::read_to_string(&log).unwrap_or(said);
    assert!(
        said.contains("PARSE_ERROR"),
        "the error was never reported:\n{said}"
    );
    assert!(!said.contains("no hot update"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The templates are released apart from esdev (D220), so its help names
/// none: a list there is one that goes stale — it named two that were gone.
#[test]
fn create_help_points_at_the_list_rather_than_naming_templates() {
    let out = esdev_in(&std::env::temp_dir())
        .args(["create", "--help"])
        .output()
        .expect("spawn esdev create --help");
    let help = stdout(&out);
    assert!(help.contains("--list names"), "{help}");
    for gone in ["spa,", "fullstack,"] {
        assert!(!help.contains(gone), "{help}");
    }
}

/// `esdev test --watch` with its keys pressed, through a pipe
/// (`--_watch-keys`): what each run printed is read off stdout, and a run has
/// ended when stderr says the watch is waiting again.
struct KeyedWatch {
    child: std::process::Child,
    out: PathBuf,
    err: PathBuf,
    seen: usize,
    passes: usize,
}

impl KeyedWatch {
    fn start(dir: &Path, name: &str) -> KeyedWatch {
        let out = dir.join(format!("{name}.out"));
        let err = dir.join(format!("{name}.err"));
        let child = esdev_in(dir)
            .args(["test", "--watch", "--_watch-keys", "--jobs=1"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::fs::File::create(&out).unwrap())
            .stderr(std::fs::File::create(&err).unwrap())
            .spawn()
            .expect("spawn esdev test --watch");
        KeyedWatch {
            child,
            out,
            err,
            seen: 0,
            passes: 0,
        }
    }

    fn press(&mut self, keys: &str) {
        use std::io::Write;
        let stdin = self.child.stdin.as_mut().expect("stdin");
        stdin.write_all(keys.as_bytes()).expect("press");
        stdin.flush().expect("flush");
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.err).unwrap_or_default()
    }

    /// Waits for the next run to end, and returns what it printed.
    fn next_run(&mut self) -> String {
        self.passes += 1;
        let want = self.passes;
        let err = wait_for_file(&self.err, Duration::from_secs(30), |text| {
            text.matches("watching for changes").count() >= want
        });
        assert!(
            err.matches("watching for changes").count() >= want,
            "run {want} never ended:\n{err}"
        );
        let out = std::fs::read_to_string(&self.out).unwrap_or_default();
        let run = out[self.seen..].to_string();
        self.seen = out.len();
        run
    }

    fn quit(mut self) {
        self.press("q");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().expect("wait") {
                assert!(status.success(), "{status}");
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "q did not end the watch"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A watching run restarts when `esdev.json` changes, as Vitest does on its
/// config: every setting was resolved from the file, so a run that went on
/// with what it read first would be testing a project that no longer exists.
#[test]
fn test_watch_restarts_when_the_project_file_changes() {
    let dir = build_dir("t_watch_restart");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    write_in(&dir, "a/x.ts", "export default 'from a';\n");
    write_in(&dir, "b/x.ts", "export default 'from b';\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "resolve": { "alias": { "@": "./a" } } }"#,
    );
    write_in(
        &dir,
        "which.test.ts",
        "import { test } from \"runtime:test\";\nimport x from \"@/x.ts\";\n\
         test(\"which\", () => console.log(\"VALUE\", x));\n",
    );
    let mut watch = KeyedWatch::start(&dir, "restart");
    let run = watch.next_run();
    assert!(run.contains("VALUE from a"), "{run}");

    write_in(
        &dir,
        "esdev.json",
        r#"{ "resolve": { "alias": { "@": "./b" } } }"#,
    );
    // Waited for on stdout, where the new session's run lands: the idle line
    // on stderr follows any pass, including one that found nothing to run.
    let out = wait_for_file(&watch.out, Duration::from_secs(30), |text| {
        text.contains("VALUE from b")
    });
    let err = watch.stderr();
    assert!(err.contains("esdev.json changed — restarting"), "{err}");
    assert!(
        out.contains("VALUE from b"),
        "the restarted session did not run with the new alias.\nstdout:\n{out}\nstderr:\n{err}"
    );
    watch.passes = 2;
    watch.quit();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Vitest's watch keys: `f` reruns what failed, `t` and `p` filter, `a`
/// clears the filters, `r` reruns as filtered, Escape leaves a filter as it
/// was, `h` explains, `q` quits.
#[test]
fn test_watch_keys_rerun_filter_and_quit() {
    let dir = build_dir("t_watch_keys");
    write_in(
        &dir,
        "math.test.ts",
        "import { expect, test } from \"runtime:test\";\n\
         test(\"adds\", () => expect(1 + 1).toBe(2));\n\
         test(\"breaks\", () => expect(1).toBe(2));\n",
    );
    write_in(
        &dir,
        "greet.test.ts",
        "import { expect, test } from \"runtime:test\";\n\
         test(\"greets\", () => expect(\"hi\").toBe(\"hi\"));\n",
    );
    let mut watch = KeyedWatch::start(&dir, "keys");
    let run = watch.next_run();
    assert!(run.contains("1 of 2 files failed"), "{run}");
    assert!(
        watch
            .stderr()
            .contains("watching for changes — press h for help, q to quit"),
        "{}",
        watch.stderr()
    );

    watch.press("f");
    let run = watch.next_run();
    assert!(
        run.contains("math.test.ts") && !run.contains("greet.test.ts"),
        "{run}"
    );

    watch.press("t");
    watch.press("adds\n");
    let run = watch.next_run();
    assert!(run.contains("2 files passed"), "{run}");
    assert!(!run.contains("FAIL breaks"), "{run}");
    assert!(watch.stderr().contains("filter: tests matching /adds/"));

    watch.press("p");
    watch.press("greet\n");
    let run = watch.next_run();
    assert!(
        run.contains("greet.test.ts") && !run.contains("math.test.ts"),
        "{run}"
    );
    assert!(
        watch
            .stderr()
            .contains("filter: files matching greet, tests matching /adds/")
    );

    // Escape: the filter stays, and nothing runs until a key asks.
    watch.press("p");
    watch.press("zzz");
    watch.press("\u{1b}");
    watch.press("r");
    let run = watch.next_run();
    assert!(
        run.contains("greet.test.ts") && !run.contains("math.test.ts"),
        "{run}"
    );

    watch.press("a");
    let run = watch.next_run();
    assert!(run.contains("1 of 2 files failed"), "{run}");
    assert!(run.contains("FAIL breaks"), "{run}");

    watch.press("h");
    let help = wait_for_file(&watch.err, Duration::from_secs(10), |text| {
        text.contains("Watch usage")
    });
    assert!(
        help.contains("press f to rerun only the files that failed"),
        "{help}"
    );
    watch.quit();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A key that starts a run cancels the one in progress, and its test process
/// with it; `u` reruns updating snapshots.
#[test]
fn test_watch_keys_cancel_a_run_and_update_snapshots() {
    let dir = build_dir("t_watch_keys_cancel");
    write_in(
        &dir,
        "slow.test.ts",
        "import { test } from \"runtime:test\";\n\
         import { env } from \"runtime:process\";\n\
         import { write } from \"runtime:fs\";\n\
         test(\"slow\", async () => {\n\
           await new Promise((r) => setTimeout(r, 2500));\n\
           await write(\"finished.txt\", \"x\");\n\
         });\n",
    );
    let snap = |value: &str| {
        format!(
            "import {{ expect, test }} from \"runtime:test\";\n\
             test(\"value\", () => expect({value:?}).toMatchSnapshot());\n"
        )
    };
    write_in(&dir, "snap.test.ts", &snap("one"));
    let mut watch = KeyedWatch::start(&dir, "cancel");
    let run = watch.next_run();
    assert!(run.contains("2 files passed"), "{run}");
    std::fs::remove_file(dir.join("finished.txt")).expect("the first run finished");

    // Cancelled while the slow file runs: it never gets to write.
    watch.press("r");
    wait_for_file(&watch.out, Duration::from_secs(10), |text| {
        text[watch.seen..].contains("slow.test.ts")
    });
    std::thread::sleep(Duration::from_millis(500));
    watch.press("a");
    let run = watch.next_run();
    assert!(watch.stderr().contains("cancelled"), "{}", watch.stderr());
    assert!(run.contains("2 files passed"), "{run}");
    std::fs::remove_file(dir.join("finished.txt")).expect("the second run finished");

    // A snapshot that changed fails; `u` accepts it.
    write_in(&dir, "snap.test.ts", &snap("two"));
    let run = watch.next_run();
    assert!(run.contains("snapshot changed"), "{run}");
    watch.press("u");
    let run = watch.next_run();
    assert!(run.contains("2 files passed"), "{run}");
    let stored =
        std::fs::read_to_string(dir.join("__snapshots__/snap.test.ts.snap")).expect("snapshot");
    assert!(stored.contains("\"two\""), "{stored}");

    // `q` during a run ends it, and the file being run with it.
    std::fs::remove_file(dir.join("finished.txt")).expect("the update run finished");
    watch.press("r");
    wait_for_file(&watch.out, Duration::from_secs(10), |text| {
        text[watch.seen..].contains("slow.test.ts")
    });
    watch.quit();
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !dir.join("finished.txt").exists(),
        "a cancelled test ran on"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
