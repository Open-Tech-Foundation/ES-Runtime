//! `esdev test --browser`: the same files, in a real page (BiDi).

use super::*;

/// A browser build takes the `browser` key rather than the `worker` one, on
/// both paths. Asserting `worker` for a browser hands over a build written for
/// somewhere with no `document`, and the failure is in someone's browser.
#[test]
fn runtime_build_asserts_browser_for_a_browser_platform() {
    let dir = build_dir("rb_conditions_browser");
    write_resolution_fixture(&dir);
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js", platform: "browser" });
const { output } = await bundle.generate({});
console.log("code", JSON.stringify(output[0].code));
"#,
    );

    let run = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(run.status.success(), "{}", stderr(&run));
    let code = stdout(&run);
    assert!(code.contains("browser-build"), "{code}");
    assert!(!code.contains("worker-build"), "{code}");
}

/// The flag a real-browser test runs with, or `None` when no browser is named.
///
/// A real browser is driven only when `ESDEV_TEST_BROWSER` names one, as each
/// `tsr test:browser` does when given one. Unnamed, the test says
/// it did not run: which browsers a machine happens to carry is not something
/// the general suite can depend on. A runner image that began shipping a
/// Chromium whose sandbox cannot start failed every test job that way.
fn browser_flag() -> Option<String> {
    match std::env::var("ESDEV_TEST_BROWSER") {
        Ok(name) if !name.is_empty() => Some(format!("--browser={name}")),
        _ => {
            eprintln!("ESDEV_TEST_BROWSER is not set; the real-browser run did not happen");
            None
        }
    }
}

/// Whether a real-browser test stops here because nothing could be driven:
/// what `err` says, unless a browser was named. A job that names its browser
/// and cannot drive it has failed, and must not pass as a skip.
fn no_browser(err: &str) -> bool {
    if !err.contains("no browser that can run the tests") {
        return false;
    }
    if let Ok(name) = std::env::var("ESDEV_TEST_BROWSER")
        && !name.is_empty()
    {
        panic!("ESDEV_TEST_BROWSER={name}, and it could not be driven:\n{err}");
    }
    eprintln!("no browser can be driven here; the real-browser run did not happen");
    true
}

/// In a page, the test is bundled with the project's plugins, as its browser
/// build would be.
#[test]
fn test_compiles_through_the_plugins_in_a_browser_run() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = framework_project("p_plugins_test_browser");
    let out = esdev_in(&dir)
        .args(["test", &browser])
        .output()
        .expect("spawn esdev test --browser");
    if no_browser(&stderr(&out)) {
        return;
    }
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// In a page: `mock.module` is refused by name — its modules are bundled before
/// it loads, so there is no load to replace a module at — while `mock.when`
/// and the clock work, the clock faking what only a page has — and global
/// setup runs beside the browser.
#[test]
fn test_mocks_and_the_clock_in_a_browser_run() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = module_mock_project("t_mock_module_browser");
    write_in(
        &dir,
        "src/mocked.test.ts",
        "import { expect, mock, test } from \"runtime:test\";\n\
         import { send } from \"./mail.ts\";\n\
         mock.module(\"./mail.ts\", () => ({ send: () => \"mocked\" }));\n\
         test(\"x\", () => expect(send(\"a\")).toBe(\"mocked\"));\n",
    );
    // What needs no module loader works in a page; the environment does not.
    write_in(
        &dir,
        "src/answers.test.ts",
        r#"import { expect, mock, test } from "runtime:test";
test("when in a page", () => {
  const spy = mock.fn();
  using w = mock.when(spy).calledWith(1).thenReturn("one");
  expect(spy(1)).toBe("one");
  expect(w).toHaveBeenExhausted();
  expect(() => mock.env("X", "1")).toThrow("mock.env is not available in browser runs");
});
"#,
    );
    // Global setup runs beside the browser, and a page injects what it gave.
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "globalSetup": "./src/provide.js" } }"#,
    );
    write_in(
        &dir,
        "src/provide.js",
        "export function setup({ provide }) { provide(\"origin\", \"http://127.0.0.1:1\"); }\n",
    );
    write_in(
        &dir,
        "src/inject.test.js",
        "import { expect, inject, test } from \"runtime:test\";\n\
         test(\"injects\", () => expect(inject(\"origin\")).toBe(\"http://127.0.0.1:1\"));\n",
    );
    // The clock fakes what a page has and a process does not.
    write_in(
        &dir,
        "src/idle.test.js",
        r#"import { afterEach, expect, test, clock } from "runtime:test";
afterEach(() => clock.release());
test("idle when nothing waits, else after 50ms or the timeout", () => {
  clock.freeze();
  const seen = [];
  requestIdleCallback((d) => seen.push(["now", d.didTimeout]));
  clock.advance(0);
  expect(seen).toEqual([["now", false]]);
  setTimeout(() => {}, 1000);
  requestIdleCallback((d) => seen.push(["later", d.didTimeout]));
  requestIdleCallback((d) => seen.push(["soon", d.didTimeout]), { timeout: 10 });
  clock.advance(10);
  expect(seen.at(-1)).toEqual(["soon", true]);
  clock.advance(40);
  expect(seen.at(-1)).toEqual(["later", false]);
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", &browser, "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser");
    let (text, err) = (stdout(&out), stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
    if no_browser(&err) {
        return;
    }
    assert!(!out.status.success(), "{text}{err}");
    assert!(
        text.contains("FAIL the file failed to load\n    TypeError: mock.module is not available in browser runs yet"),
        "{text}{err}"
    );
    assert!(
        text.contains("src/answers.test.ts\n  1 passed, 0 failed"),
        "{text}{err}"
    );
    assert!(
        text.contains("src/idle.test.js\n  1 passed, 0 failed"),
        "{text}{err}"
    );
    assert!(
        text.contains("src/inject.test.js\n  1 passed, 0 failed"),
        "{text}{err}"
    );
}

/// `esdev test --browser` against a machine made of stubs: a `PATH` holding
/// only the executables written here, each printing the version it is given,
/// so the choice does not depend on which browsers the machine running the
/// tests happens to have. The versions are fixtures, not anybody's machine.
///
/// **Linux only**, because only there is `PATH` the whole search. On macOS and
/// Windows esdev also looks where the vendor's installer puts a browser
/// (`/Applications`, `Program Files`), and a CI runner has real browsers
/// there that a replaced `PATH` cannot hide. Those locations are covered by
/// the unit tests in `browser.rs`, against a described machine.
#[cfg(target_os = "linux")]
fn browser_machine(name: &str, executables: &[(&str, &str)]) -> (PathBuf, String) {
    use std::os::unix::fs::PermissionsExt;
    let dir = build_dir(name);
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("create bin");
    for (command, version) in executables {
        let path = bin.join(command);
        std::fs::write(&path, format!("#!/bin/sh\necho \"{version}\"\n")).expect("write stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
    // Discovery comes before choosing a browser: a run with no test files
    // has nothing to open one for.
    write_in(
        &dir,
        "a.test.js",
        "import { test } from \"runtime:test\";\ntest(\"t\", () => {});\n",
    );
    let path = bin.display().to_string();
    (dir, path)
}

#[cfg(target_os = "linux")]
#[test]
fn test_browser_takes_the_first_that_can_be_driven_and_says_what_it_skipped() {
    let (dir, path) = browser_machine(
        "t_browser_auto",
        &[
            ("google-chrome", "Google Chrome 120.0.6099.71"),
            (
                "chromium",
                "Chromium 131.0.6778.85 built on Debian GNU/Linux 12 (bookworm)",
            ),
            (
                "chromedriver",
                "ChromeDriver 131.0.6778.85 (0a1b2c3d-refs/branch-heads/6778@{#1})",
            ),
            ("firefox", "Mozilla Firefox 128.0"),
        ],
    );
    let out = esdev_in(&dir)
        .args(["test", "--browser"])
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test --browser");
    let err = slash_paths(&stderr(&out));
    assert!(
        err.contains("browser: chromium 131 (") && err.contains("/bin/chromium, driven by "),
        "{err}"
    );
    assert!(
        err.contains("skipped chrome: chromedriver 131 (")
            && err.contains("does not match chrome 120"),
        "{err}"
    );
    // The choice goes to stderr: stdout belongs to the reporter.
    assert_eq!(stdout(&out), "");
    // A stub is not a driver: it says its version and exits, and the run
    // fails saying so rather than passing a suite that never ran.
    assert!(!out.status.success());
    assert!(err.contains("exited without saying its port"), "{err}");

    // Named, the same mismatch is the answer rather than a reason to move on.
    let out = esdev_in(&dir)
        .args(["test", "--browser=chrome"])
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test --browser=chrome");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("cannot run the tests in chrome"), "{err}");
    assert!(
        !err.contains("browser: "),
        "a named browser fell back: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "linux")]
#[test]
fn test_browser_with_nothing_to_drive_says_so_and_downloads_nothing() {
    let (dir, path) = browser_machine(
        "t_browser_none",
        &[("google-chrome", "Google Chrome 120.0")],
    );
    let out = esdev_in(&dir)
        .args(["test", "--browser"])
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test --browser");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("no browser that can run the tests"), "{err}");
    assert!(
        err.contains("install the chromedriver 120 matching"),
        "{err}"
    );
    for browser in ["chrome", "chromium", "firefox", "edge", "safari"] {
        assert!(
            err.contains(&format!("\n  {browser}: ")),
            "{browser} missing: {err}"
        );
    }
    assert!(
        err.contains("downloads neither browsers nor drivers"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(target_os = "linux")]
#[test]
fn test_browser_comes_from_esdev_json_and_the_flag_wins() {
    let (dir, path) = browser_machine("t_browser_config", &[("firefox", "Mozilla Firefox 128.0")]);
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "browser": "firefox" } }"#,
    );
    let out = esdev_in(&dir)
        .arg("test")
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test");
    let err = stderr(&out);
    assert!(err.contains("browser: firefox 128 ("), "{err}");
    assert!(
        err.contains("firefox exited without serving WebDriver BiDi"),
        "{err}"
    );

    let out = esdev_in(&dir)
        .args(["test", "--browser=safari"])
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test --browser=safari");
    let err = stderr(&out);
    assert!(err.contains("cannot run the tests in safari"), "{err}");
    assert!(err.contains("not ready"), "{err}");

    write_in(&dir, "esdev.json", r#"{ "test": { "browser": "opera" } }"#);
    let out = esdev_in(&dir)
        .arg("test")
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("`test`'s `browser`"), "{err}");
    assert!(err.contains("`opera` is not a browser"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_browser_refuses_what_has_no_meaning_in_a_page() {
    for (args, want) in [
        (&["test", "--browser=opera"][..], "`opera` is not a browser"),
        (&["test", "--browser", "--dom"][..], "--dom and --browser"),
        (
            &["test", "--browser", "--deny-all"][..],
            "permission flags rehearse",
        ),
        (
            &["test", "--browser", "--isolation=none"][..],
            "--isolation=none",
        ),
        (
            &["test", "--headed"][..],
            "--headed shows the browser a run uses",
        ),
    ] {
        let out = esdev().args(args).output().expect("spawn esdev test");
        assert!(!out.status.success(), "{args:?} was accepted");
        let err = stderr(&out);
        assert!(err.contains(want), "{args:?}: {err}");
    }
}

/// The whole run, in whichever real browser this machine can drive: a file
/// that passes, fails, skips, logs and measures layout, and one that throws
/// outside any test. On a machine that can drive none it checks the refusal
/// instead — which browsers a CI runner carries is not this suite's to decide,
/// and the stubbed tests above cover the choosing everywhere.
#[test]
fn test_browser_runs_the_files_in_a_real_page() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = build_dir("t_browser_real");
    write_in(
        &dir,
        "dom.test.ts",
        "import { test, describe, expect } from \"runtime:test\";\n\
         \n\
         describe(\"page\", () => {\n\
           test(\"has layout\", () => {\n\
             const box = document.createElement(\"div\");\n\
             box.style.width = \"120px\";\n\
             document.body.append(box);\n\
             console.log(\"measured\", box.getBoundingClientRect().width);\n\
             expect(box.getBoundingClientRect().width).toBe(120);\n\
           });\n\
           test(\"fails where it was written\", () => {\n\
             expect(1 + 1).toBe(3);\n\
           });\n\
           test.skip(\"skipped\", () => {});\n\
           test(\"matchers read the real DOM\", () => {\n\
             const box = document.createElement(\"div\");\n\
             box.style.width = \"120px\";\n\
             document.body.append(box);\n\
             expect(box).toBeInTheDocument();\n\
             expect(box).toBeVisible();\n\
             expect(box).toHaveStyle({ width: \"120px\" });\n\
           });\n\
         });\n",
    );
    write_in(
        &dir,
        "timer.test.js",
        "import { test } from \"runtime:test\";\n\
         test(\"waits\", async () => { await new Promise((r) => setTimeout(r, 10)); });\n\
         setTimeout(() => { throw new Error(\"thrown outside any test\"); }, 0);\n",
    );
    // Nothing to run: every test skipped, so no queue drains to end it.
    write_in(
        &dir,
        "skipped.test.js",
        "import { test } from \"runtime:test\";\ntest.skip(\"later\", () => {});\n",
    );
    let out = esdev_in(&dir)
        .args(["test", &browser, "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser");
    let (out_text, err) = (stdout(&out), stderr(&out));
    if no_browser(&err) {
        assert!(!out.status.success());
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(err.starts_with("browser: "), "{err}");
    assert!(
        !out.status.success(),
        "a failing test passed:\n{out_text}{err}"
    );
    // Real layout, and the page's console in the file's report.
    assert!(out_text.contains("measured 120"), "{out_text}");
    assert!(
        out_text
            .contains("  FAIL page > fails where it was written\n    Error: expected 2 to be 3"),
        "{out_text}"
    );
    // The frame names the line in the file that was written, not the bundle.
    assert!(out_text.contains("dom.test.ts:12:"), "{out_text}");
    assert!(!out_text.contains("127.0.0.1"), "{out_text}");
    assert!(
        out_text.contains("2 passed, 1 failed, 1 skipped"),
        "{out_text}"
    );
    // An error no test caught is the file's failure, not a silence.
    assert!(out_text.contains("FAIL uncaught error"), "{out_text}");
    assert!(out_text.contains("thrown outside any test"), "{out_text}");
    assert!(
        out_text.contains("0 passed, 0 failed, 1 skipped"),
        "{out_text}"
    );
    assert!(out_text.contains("2 of 3 files failed"), "{out_text}");

    // JSON on its own: a file that writes nothing to the console, since what
    // a test prints shares stdout with the reporter in either runner.
    write_in(
        &dir,
        "quiet.test.js",
        "import { test } from \"runtime:test\";\n\
         test(\"passes\", () => {});\n\
         test(\"fails\", () => { throw new Error(\"no\"); });\n\
         test.skip(\"skipped\", () => {});\n",
    );
    let json = esdev_in(&dir)
        .args([
            "test",
            &browser,
            "--reporter=json",
            "--timeout=60000",
            "quiet",
        ])
        .output()
        .expect("spawn esdev test --browser --reporter=json");
    let json_out = stdout(&json);
    let lines: Vec<serde_json::Value> = json_out
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|err| panic!("not JSON ({err}): {line}\n{json_out}"))
        })
        .collect();
    let file = lines
        .iter()
        .find(|line| line["type"] == "file")
        .unwrap_or_else(|| panic!("no file line:\n{json_out}"));
    // Named as a process run names it: by the path the file was run from.
    assert!(
        std::path::Path::new(file["file"].as_str().unwrap()).is_absolute(),
        "{json_out}"
    );
    assert_eq!(
        (&file["passed"], &file["failed"], &file["skipped"]),
        (
            &serde_json::json!(1),
            &serde_json::json!(1),
            &serde_json::json!(1)
        ),
        "{json_out}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Snapshots in a browser run are the same snapshots: what a process run wrote
/// matches in the page, a change fails with the host's diff, and what the
/// browser run updates is what the process run then matches. Needs a browser
/// that can be driven; on a machine without one it checks nothing.
#[test]
fn test_browser_snapshots_are_the_process_runs_snapshots() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = build_dir("t_browser_snapshots");
    let file = |user: &str| {
        write_in(
            &dir,
            "snap.test.js",
            &format!(
                "import {{ test, expect }} from \"runtime:test\";\n\
                 test(\"value\", () => {{ expect({{ user: \"{user}\", ids: [1, 2] }}).toMatchSnapshot(); }});\n\
                 test(\"file\", () => {{ expect(new Uint8Array([1, 2, 3])).toMatchFileSnapshot(\"bytes.bin\"); }});\n"
            ),
        );
    };
    file("ada");
    let written = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        written.status.success(),
        "{}{}",
        stdout(&written),
        stderr(&written)
    );

    let matched = esdev_in(&dir)
        .args(["test", &browser, "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser");
    let (out, err) = (stdout(&matched), stderr(&matched));
    if no_browser(&err) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(matched.status.success(), "{out}{err}");
    assert!(
        out.contains("snapshots: 2 matched, 0 failed, 0 written"),
        "{out}"
    );

    file("bob");
    let changed = esdev_in(&dir)
        .args(["test", &browser, "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser");
    let out = stdout(&changed);
    assert!(!changed.status.success(), "{out}");
    assert!(
        out.contains("snapshot changed — value: snapshot 1"),
        "{out}"
    );
    assert!(
        out.contains("-   \"user\": \"ada\",\n    +   \"user\": \"bob\","),
        "the host's diff, not the page's one line:\n{out}"
    );

    let updated = esdev_in(&dir)
        .args(["test", &browser, "--update-snapshots", "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser --update-snapshots");
    assert!(
        stdout(&updated).contains("snapshots: 1 updated, 0 written, 0 removed, 1 unchanged"),
        "{}",
        stdout(&updated)
    );
    let after = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        stdout(&after).contains("snapshots: 2 matched, 0 failed, 0 written"),
        "{}",
        stdout(&after)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--watch` with `--browser`: one browser for every pass, and a change runs
/// the files again in it. Stopped the way a person stops it, with ^C, so the
/// browser is closed rather than orphaned.
#[cfg(unix)]
#[test]
fn test_browser_watch_runs_again_when_a_file_changes() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = build_dir("t_browser_watch");
    let test = dir.join("w.test.js");
    let body = |want: u32| {
        format!(
            "import {{ test, expect }} from \"runtime:test\";\n\
             test(\"adds\", () => {{ expect(1 + 1).toBe({want}); }});\n"
        )
    };
    std::fs::write(&test, body(2)).expect("write test");
    let out = dir.join("out.txt");
    let err = dir.join("err.txt");
    let mut child = esdev_in(&dir)
        .args(["test", &browser, "--watch", "--timeout=60000"])
        .stdout(std::fs::File::create(&out).expect("create out"))
        .stderr(std::fs::File::create(&err).expect("create err"))
        .spawn()
        .expect("spawn esdev test --browser --watch");
    let stop = |child: &mut std::process::Child| {
        let _ = Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status();
        let _ = child.wait();
    };

    let first = wait_for_file(&out, Duration::from_secs(60), |s| {
        s.contains("1 file passed")
    });
    if !first.contains("1 file passed") {
        let errors = std::fs::read_to_string(&err).unwrap_or_default();
        stop(&mut child);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            no_browser(&errors),
            "the first pass never finished:\n{first}\n{errors}"
        );
        return;
    }
    std::fs::write(&test, body(3)).expect("rewrite test");
    let second = wait_for_file(&out, Duration::from_secs(60), |s| {
        s.contains("1 of 1 file failed")
    });
    stop(&mut child);
    let errors = std::fs::read_to_string(&err).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        second.contains("1 of 1 file failed"),
        "the change did not rerun it:\n{second}"
    );
    // One browser for both passes.
    assert_eq!(errors.matches("browser: ").count(), 1, "{errors}");
}

/// What `runtime:test` added since browser runs began, in a real page:
/// fixtures, tags, concurrent tests (one at a time there) and benchmarks.
#[test]
fn test_browser_runs_fixtures_tags_concurrency_and_benchmarks() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = build_dir("t_browser_phase4");
    write_in(
        &dir,
        "esdev.json",
        r#"{ "test": { "tags": [{ "name": "db" }, { "name": "slow" }] } }"#,
    );
    write_in(
        &dir,
        "features.test.ts",
        r#"import { describe, expect, test } from "runtime:test";
const it = test.extend("user", async ({}, { onCleanup }) => {
  const user = { name: "ada", closed: false };
  onCleanup(() => { user.closed = true; });
  return user;
});
it("a fixture", ({ user, task }) => {
  expect(user.name).toBe("ada");
  expect(task.name).toBe("a fixture");
});
test("tagged db", { tags: ["db"] }, () => {});
test("tagged slow", { tags: ["slow"] }, () => { throw new Error("filtered out"); });
describe.concurrent("concurrent", () => {
  for (const i of [1, 2, 3]) {
    test(`case ${i}`, async () => {
      await new Promise((r) => setTimeout(r, 5));
      expect.assertions(1);
      expect(i).toBeGreaterThan(0);
    });
  }
});
"#,
    );
    write_in(
        &dir,
        "parse.bench.ts",
        r#"import { expect, test } from "runtime:test";
test("compare", async ({ bench }) => {
  const results = await bench.compare(
    bench("parse", () => JSON.parse("[1,2,3]")),
    bench("stringify", () => JSON.stringify([1, 2, 3])),
    { time: 50 },
  );
  expect(results.get("parse")!.samples).toBeGreaterThan(0);
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", &browser, "--tags-filter=!slow", "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser");
    let (text, err) = (stdout(&out), stderr(&out));
    if no_browser(&err) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(out.status.success(), "{text}{err}");
    assert!(
        text.contains("5 passed, 0 failed, 1 skipped"),
        "{text}{err}"
    );

    let out = esdev_in(&dir)
        .args(["bench", &browser, "--reporter=json", "--timeout=60000"])
        .output()
        .expect("spawn esdev bench --browser");
    let text = stdout(&out);
    assert!(out.status.success(), "{text}{}", stderr(&out));
    assert_eq!(
        text.lines()
            .filter(|line| line.contains(r#""type":"bench""#))
            .count(),
        2,
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// In a page, a stylesheet's `url()` and a module's `new URL()` load the files
/// they name (D155), and the project's `public/` is served at the root, as a
/// build places them.
#[test]
fn test_browser_serves_stylesheet_assets_and_public() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = build_dir("t_browser_assets");
    std::fs::create_dir_all(dir.join("src/img")).unwrap();
    std::fs::create_dir_all(dir.join("public/icons")).unwrap();
    // A PNG, binary past its signature: served as text, it would not arrive whole.
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend((0u8..=255).collect::<Vec<_>>());
    std::fs::write(dir.join("src/img/dot.png"), &png).unwrap();
    std::fs::write(
        dir.join("public/icons/logo.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
    )
    .unwrap();
    write_in(
        &dir,
        "src/box.css",
        ".box { background-image: url(\"./img/dot.png\"); }\n",
    );
    write_in(&dir, "src/img/note.txt", "hello from a module URL\n");
    write_in(
        &dir,
        "src/assets.test.ts",
        r#"import { expect, test } from "runtime:test";
import "./box.css";
test("a stylesheet's url() is served", async () => {
  const box = document.createElement("div");
  box.className = "box";
  document.body.append(box);
  const url = getComputedStyle(box).backgroundImage.slice(5, -2);
  const res = await fetch(url);
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toBe("image/png");
  expect((await res.arrayBuffer()).byteLength).toBe(264);
});
test("public/ is at the root", async () => {
  const res = await fetch("/icons/logo.svg");
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toBe("image/svg+xml");
  expect((await fetch("/icons/missing.svg")).status).toBe(404);
  expect((await fetch("/../Cargo.toml")).status).toBe(404);
});
test("a module's new URL() file is served", async () => {
  const res = await fetch(new URL("./img/note.txt", import.meta.url));
  expect(res.status).toBe(200);
  expect(await res.text()).toBe("hello from a module URL\n");
});
"#,
    );
    let out = esdev_in(&dir)
        .args(["test", &browser, "--timeout=60000"])
        .output()
        .expect("spawn esdev test --browser");
    let (text, err) = (stdout(&out), stderr(&out));
    let _ = std::fs::remove_dir_all(&dir);
    if no_browser(&err) {
        return;
    }
    assert!(out.status.success(), "{text}{err}");
    assert!(text.contains("3 passed, 0 failed"), "{text}");
}

/// Several browsers, each named: one that cannot be driven fails the run and
/// says so last, and the others are still tried.
#[cfg(target_os = "linux")]
#[test]
fn test_browser_several_that_cannot_be_driven_fail_the_run() {
    let (dir, path) = browser_machine(
        "t_browser_several_none",
        &[("google-chrome", "Google Chrome 120.0")],
    );
    let out = esdev_in(&dir)
        .args(["test", "--browser=chrome,edge"])
        .env("PATH", &path)
        .output()
        .expect("spawn esdev test --browser=chrome,edge");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("cannot run the tests in chrome"), "{err}");
    assert!(err.contains("cannot run the tests in edge"), "{err}");
    assert!(
        err.trim_end()
            .ends_with("2 of 2 browsers could not run the tests"),
        "{err}"
    );

    for (flag, says) in [
        ("--browser=chrome,chrome", "chrome is named twice"),
        ("--browser=auto,firefox", "cannot be one of several"),
    ] {
        let out = esdev_in(&dir)
            .args(["test", flag])
            .output()
            .expect("spawn esdev test");
        assert!(!out.status.success());
        assert!(stderr(&out).contains(says), "{flag}: {}", stderr(&out));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every file in each of two real browsers, into one report: each file's
/// report names its browser, and the tally counts every run. Where
/// `ESDEV_TEST_BROWSER` names a browser, as in CI, whose machines have all
/// three; elsewhere this machine may not have two.
#[test]
fn test_browser_runs_every_file_in_each_of_several() {
    let Ok(named) = std::env::var("ESDEV_TEST_BROWSER") else {
        eprintln!("ESDEV_TEST_BROWSER is not set; the two-browser run did not happen");
        return;
    };
    let other = if named == "firefox" {
        "chrome"
    } else {
        "firefox"
    };
    let dir = build_dir("t_browser_several");
    write_in(
        &dir,
        "a.test.ts",
        "import { expect, test } from \"runtime:test\";\n\
         test(\"agent\", () => expect(navigator.userAgent.length).toBeGreaterThan(0));\n",
    );
    let out = esdev_in(&dir)
        .args([
            "test",
            &format!("--browser={named},{other}"),
            "--timeout=60000",
        ])
        .output()
        .expect("spawn esdev test --browser=a,b");
    let (text, err) = (stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}{err}");
    assert!(text.contains(&format!("a.test.ts [{named}]")), "{text}");
    assert!(text.contains(&format!("a.test.ts [{other}]")), "{text}");
    assert!(text.contains("2 files passed"), "{text}");

    let out = esdev_in(&dir)
        .args([
            "test",
            &format!("--browser={named},{other}"),
            "--reporter=json",
            "--timeout=60000",
        ])
        .output()
        .expect("spawn esdev test --reporter=json");
    let text = stdout(&out);
    for browser in [&named[..], other] {
        assert!(
            text.contains(&format!(r#""browser":"{browser}""#)),
            "{text}"
        );
    }
    assert_eq!(text.matches(r#""type":"summary""#).count(), 1, "{text}");
    assert!(
        text.contains(r#"{"type":"summary","files":2,"failed":0}"#),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `toMatchScreenshot` in a page: a first run writes the reference and fails
/// for review; a match passes; a change fails and leaves the actual and diff
/// images under `.esdev/`; `--update-snapshots` accepts it; `--ci` writes
/// nothing. Outside a browser run it says it needs one.
#[test]
fn test_browser_screenshots_are_compared_with_a_reference() {
    let Some(browser) = browser_flag() else {
        return;
    };
    let dir = build_dir("t_browser_screenshots");
    let test = |colour: &str| {
        format!(
            r#"import {{ expect, test }} from "runtime:test";
test("button", async () => {{
  const b = document.createElement("button");
  b.textContent = "Save";
  b.style.cssText = "background: {colour}; color: white; padding: 8px 16px; border: 0; font: 16px sans-serif";
  document.body.append(b);
  await expect(b).toMatchScreenshot();
}});
"#
        )
    };
    write_in(&dir, "button.test.ts", &test("rgb(0, 100, 200)"));
    let run = |args: &[&str]| {
        let out = esdev_in(&dir)
            .args(["test", &browser, "--timeout=60000"])
            .args(args)
            .output()
            .expect("spawn esdev test --browser");
        (out.status.success(), stdout(&out), stderr(&out))
    };

    let (ok, out, err) = run(&["--ci"]);
    if no_browser(&err) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    assert!(!ok, "{out}{err}");
    assert!(out.contains("--ci does not write them"), "{out}");
    assert!(!dir.join("__screenshots__").exists());

    let (ok, out, _) = run(&[]);
    assert!(!ok, "{out}");
    assert!(out.contains("no reference screenshot was found"), "{out}");
    let references: Vec<_> = std::fs::read_dir(dir.join("__screenshots__/button.test.ts"))
        .expect("the reference's directory")
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(references.len(), 1, "{references:?}");
    assert!(references[0].starts_with("button-1-"), "{references:?}");
    assert!(references[0].ends_with(".png"), "{references:?}");

    let (ok, out, err) = run(&[]);
    assert!(ok, "the reference matches itself:\n{out}{err}");

    write_in(&dir, "button.test.ts", &test("rgb(200, 0, 0)"));
    let (ok, out, _) = run(&[]);
    assert!(!ok, "{out}");
    assert!(
        out.contains("screenshot differs from __screenshots__/button.test.ts/"),
        "{out}"
    );
    assert!(out.contains("diff:   .esdev/screenshots/"), "{out}");
    assert_eq!(
        std::fs::read_to_string(dir.join(".esdev/.gitignore")).unwrap(),
        "*\n"
    );

    let (ok, out, err) = run(&["--update-snapshots"]);
    assert!(ok, "{out}{err}");
    let (ok, out, err) = run(&[]);
    assert!(ok, "the updated reference matches:\n{out}{err}");

    let out = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom");
    assert!(
        stdout(&out).contains("toMatchScreenshot needs a real browser"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(&dir);
}
