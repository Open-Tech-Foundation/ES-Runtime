//! End-to-end tests for the `esdev` binary.
//!
//! The point of these is **parity**, not coverage of the runtime: `esdev` and
//! `esrun` share every line that decides how a run behaves
//! (`es-runtime-cli-common`), and the whole design rests on a program not being
//! able to behave one way under one binary and differently under the other. So
//! these spawn the real binary and assert that the shared surface — the module
//! load, the capability model, the D38 flag grammar, the error block — is the
//! same one `esrun` presents, plus the few things that are `esdev`'s own.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esdev() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esdev"));
    // The sandbox is the working directory (D79): run from the directory these
    // fixtures are written into, as a user runs a program from its own.
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    // Never the developer's terminal. `output()` already closes stdin, but a
    // spawned esdev inherits it, and with `CI` removed (`esdev_in`) it then
    // prompts, or takes keys — and in a process group of its own the kernel
    // stops it for touching the terminal. A test that feeds input says so.
    command.stdin(std::process::Stdio::null());
    let registry = test_registry();
    command
        .env("npm_config_registry", registry)
        .env("NPM_CONFIG_REGISTRY", registry);
    command
}

/// A local npm registry fixture keeps scaffold tests deterministic and
/// verifies that manifests receive exact versions without depending on the
/// public network.
fn test_registry() -> &'static str {
    static REGISTRY: OnceLock<String> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind test registry");
            let address = listener.local_addr().expect("test registry address");
            std::thread::spawn(move || {
                for incoming in listener.incoming() {
                    let Ok(mut stream) = incoming else { continue };
                    std::thread::spawn(move || {
                        let Ok(clone) = stream.try_clone() else { return };
                        let mut reader = BufReader::new(clone);
                        let mut line = String::new();
                        loop {
                            line.clear();
                            if reader.read_line(&mut line).is_err()
                                || line.is_empty()
                                || line == "\r\n"
                            {
                                break;
                            }
                        }
                        let body = r#"{"dist-tags":{"latest":"1.2.3"}}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                    });
                }
            });
            format!("http://{address}")
        })
        .as_str()
}

/// Another workspace binary from the same target directory, or `None` if it has
/// not been built. Cargo only exports `CARGO_BIN_EXE_*` for the *current*
/// package's binaries, and `esrun` belongs to another.
fn sibling_binary(name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_BIN_EXE_esdev"))
        .parent()?
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    path.exists().then_some(path)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Diagnostic paths are native filesystem paths; compare their separators as
/// paths rather than accidentally making a Unix spelling part of the contract.
fn slash_paths(text: &str) -> String {
    es_runtime_default_providers::path::to_slash(text)
}

/// A directory of its own per test: `build` writes files, and two tests sharing
/// `dist/` would race.
fn build_dir(name: &str) -> PathBuf {
    let dir = temp(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create build dir");
    dir
}

fn write_in(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, contents).expect("write file");
    path
}

fn esdev_in(dir: &Path) -> Command {
    let mut cmd = esdev();
    cmd.current_dir(dir);
    // Snapshot setup runs model a developer's local invocation. GitHub sets
    // CI for the test harness too, which would make those runs refuse to write
    // the baselines they are about to verify. CI-specific cases set `CI`
    // explicitly below, overriding this removal.
    cmd.env_remove("CI");
    cmd
}

/// A source tree with a subdirectory, an internal module, and an export that
/// only an outside caller would ever reach for.
fn lib_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src/protocol")).expect("create src");
    write_in(
        &dir,
        "src/protocol/codec.ts",
        // `UNUSED_BY_THE_ENTRY` is the point: no other module in this library
        // touches it, and it is still part of what the library exports.
        "export const UNUSED_BY_THE_ENTRY: readonly string[] = ['a', 'b'];\n\
         export function encode(value: string): string {\n  return `<${value}>`;\n}\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "import { encode } from './protocol/codec.js';\n\
         import { version } from 'some-dependency';\n\
         export function greet(name: string): string {\n  \
         return encode(name) + version;\n}\n",
    );
    dir
}

fn bundled(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("dist/index.d.ts")).expect("read bundled declarations")
}

/// A scratch directory for watch tests, deliberately **not** under
/// `CARGO_TARGET_TMPDIR`.
///
/// That lives inside `target/`, which the watcher ignores on purpose — machine
/// output is not a reason to restart. A watch test staged there watches nothing
/// and passes for the wrong reason.
fn watch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esdev-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create watch dir");
    dir
}

/// Polls `path` until it satisfies `done`, or gives up. Watch tests are about
/// timing, so they wait for a condition rather than for a duration.
fn wait_for_file(path: &Path, timeout: Duration, done: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + timeout;
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        last = std::fs::read_to_string(path).unwrap_or_default();
        if done(&last) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    last
}

/// Every file under `dir`, recursively.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// `esdev start` runs the application as a child of its own, so killing only
/// the supervisor leaves that child holding a port — and, having inherited the
/// test harness's stdout, holding the harness open too. The whole group goes.
/// An `esdev start` that stops when it goes out of scope.
///
/// On `Drop` rather than at the end of a passing test, and the difference is not
/// tidiness. An assertion that fires unwinds past any explicit stop, leaving a
/// supervisor running and holding the port this test derived from its own name.
/// The *next* run then talks to the previous run's dev server, serving the
/// previous run's `dist`, and fails for a reason that has nothing to do with the
/// code — on a machine where the first failure has already been fixed. This cost
/// an afternoon once, and it cost it twice.
struct Supervisor(Option<std::process::Child>);

impl Drop for Supervisor {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            stop_supervisor(&mut child);
        }
    }
}

fn stop_supervisor(child: &mut std::process::Child) {
    // The group is signalled directly, not through a `kill` binary: how one
    // spells a negative group varies between implementations, and Ubuntu
    // 24.04's (procps-ng 4.0.4) sends `-TERM -<pgid>` to every process the
    // user owns — the CI runner's own among them.
    #[cfg(unix)]
    if let Some(group) = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
        std::thread::sleep(Duration::from_millis(300));
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A supervisor in a process group of its own, with its output discarded.
fn start_in(dir: &Path) -> Supervisor {
    let mut command = esdev_in(dir);
    command
        .arg("start")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    Supervisor(Some(command.spawn().expect("spawn esdev start")))
}

/// A port unlikely to collide with anything else on the machine, derived from
/// the test's own name so two tests never pick the same one.
/// A port derived from the test's own name, **below the ephemeral range**.
///
/// The upper bound is the load-bearing part. Linux hands out source ports for
/// outbound connections from `net.ipv4.ip_local_port_range`, 32768–60999 by
/// default, so a fixed port chosen from inside that window is one the kernel
/// may already have given to somebody else's socket — and `bind` then fails
/// with `AddrInUse`. This range was 20000–39999, whose top third overlapped it;
/// under `cargo test --workspace`, where hundreds of short-lived connections
/// are opened at once, that came back as an occasional unexplained failure in
/// whichever test happened to draw the collided number.
///
/// 20000–29999 sits entirely below the floor, and is disjoint from the range
/// `tests/hot.rs` takes for the same reason.
///
/// **And free.** The hashed port is where the search starts, not the answer:
/// a port something else on the machine already listens on is passed over,
/// so a test never sends its requests to a process it did not start. On the
/// Ubuntu CI runner a test kept talking to whatever held its port, and the
/// runner was stopped mid-test with no error. Each name gets one answer per
/// run, and no two names the same one.
fn test_port(name: &str) -> u16 {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static GIVEN: OnceLock<Mutex<HashMap<String, u16>>> = OnceLock::new();
    let mut given = GIVEN
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(port) = given.get(name) {
        return *port;
    }
    let base = port_base(name);
    let port = (0..10_000u16)
        .map(|step| 20000 + (base - 20000 + step) % 10_000)
        .find(|port| {
            !given.values().any(|held| held == port)
                && std::net::TcpListener::bind(("127.0.0.1", *port)).is_ok()
        })
        .expect("a free port in 20000-29999");
    given.insert(name.to_string(), port);
    port
}

/// Where [`test_port`] starts looking for `name`: a hash of it, so each test
/// has a port of its own when the machine has nothing else on them.
fn port_base(name: &str) -> u16 {
    let hash = name.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    20000 + u16::try_from(hash % 10000).unwrap_or(0)
}

/// One HTTP GET, spoken by hand — the same shape the dev server answers.
fn http_get(port: u16, path: &str) -> Option<String> {
    use std::io::{Read, Write};

    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    Some(String::from_utf8_lossy(&response).into_owned())
}

/// Polls until the server answers, or gives up — a build and a process start
/// have to happen first, and how long that takes is the machine's business.
fn wait_for_http(port: u16, path: &str, done: impl Fn(&str) -> bool) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut last = String::new();
    while std::time::Instant::now() < deadline {
        if let Some(response) = http_get(port, path) {
            last = response;
            if done(&last) {
                return last;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    last
}

/// Runs `esdev start` with extra flags, keeping its stderr in a file so a test
/// can read what it announced. The port is in there and nowhere else when
/// esdev picked it.
fn start_in_logging(dir: &Path, args: &[&str]) -> (Supervisor, PathBuf) {
    let log = dir.join("esdev.log");
    let file = std::fs::File::create(&log).expect("create the log");
    let mut command = esdev_in(dir);
    command
        .arg("start")
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(file));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    (
        Supervisor(Some(command.spawn().expect("spawn esdev start"))),
        log,
    )
}

/// Waits for the `http://localhost:<port>` esdev printed, and returns the port.
fn announced_port(log: &Path) -> u16 {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if let Some(at) = text.find("http://localhost:") {
            let rest = &text[at + "http://localhost:".len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty()
                && rest.len() > digits.len()
                && let Ok(port) = digits.parse::<u16>()
            {
                return port;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!(
        "esdev never announced a port:\n{}",
        std::fs::read_to_string(log).unwrap_or_default()
    );
}

/// A framework's compiler, as a project's plugin: it owns `.jsx` and lowers it
/// to plain JavaScript with semantics no generic JSX runtime has. The project
/// names no `jsx` settings, so the file compiles only if the plugin ran.
fn framework_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    write_in(
        &dir,
        "framework.mjs",
        r#"
export default (options) => ({
  name: "framework",
  transform: {
    filter: { id: /\.jsx$/ },
    handler(code, id, ctx) {
      if (code.includes("BROKEN")) throw new Error("cannot compile " + id.split("/").pop());
      const compiled = code.replace(/<Greeting \/>/g, JSON.stringify(options.greeting));
      return { code: compiled, type: "js" };
    },
  },
});
"#,
    );
    write_in(
        &dir,
        "greeting.jsx",
        "export const greeting = <Greeting />;\n",
    );
    write_in(
        &dir,
        "greeting.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { greeting } from './greeting.jsx';\n\
         test('the plugin compiled the component', () => assertEquals(greeting, 'hi from the plugin'));\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": [{"module": "./framework.mjs", "options": {"greeting": "hi from the plugin"}}], "build": {"targets": {"web": {"entry": "greeting.jsx", "out": "dist/web.js"}}}}"#,
    );
    dir
}

/// Writes a project with two packages a bundler can only resolve correctly if
/// it asserts something about where the output runs: one that offers three
/// builds of itself behind `exports` conditions, and one old enough to have no
/// `exports` map at all.
fn write_resolution_fixture(dir: &Path) {
    let dual = dir.join("node_modules/dual");
    std::fs::create_dir_all(&dual).expect("create the dual package");
    write_in(
        &dual,
        "package.json",
        r#"{
  "name": "dual",
  "version": "1.0.0",
  "type": "module",
  "exports": {
    ".": {
      "worker": "./worker.js",
      "browser": "./browser.js",
      "default": "./node.js"
    }
  }
}
"#,
    );
    write_in(&dual, "worker.js", "export const where = 'worker-build';\n");
    write_in(
        &dual,
        "browser.js",
        "export const where = 'browser-build';\n",
    );
    write_in(&dual, "node.js", "export const where = 'node-build';\n");

    let legacy = dir.join("node_modules/legacy");
    std::fs::create_dir_all(&legacy).expect("create the legacy package");
    write_in(
        &legacy,
        "package.json",
        r#"{
  "name": "legacy",
  "version": "1.0.0",
  "module": "./index.mjs",
  "main": "./index.cjs"
}
"#,
    );
    write_in(
        &legacy,
        "index.mjs",
        "export const legacy = 'legacy-esm';\n",
    );
    write_in(
        &legacy,
        "index.cjs",
        "module.exports = { legacy: 'legacy-cjs' };\n",
    );

    write_in(
        dir,
        "app-entry.js",
        "import { where } from 'dual';\nimport { legacy } from 'legacy';\nconsole.log(where, legacy);\n",
    );
}

/// `mock.module` over a small project: a module the file under test imports,
/// a package, and the real module beside its mock.
fn module_mock_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("node_modules/greeting")).unwrap();
    write_in(
        &dir,
        "src/mail.ts",
        "console.log(\"real mail evaluated\");\n\
         export const provider = \"smtp\";\n\
         export function send(to: string): string { return `sent to ${to}`; }\n",
    );
    write_in(
        &dir,
        "src/signup.ts",
        "import { send } from \"./mail\";\n\
         export const signup = (to: string) => send(to);\n",
    );
    write_in(
        &dir,
        "node_modules/greeting/package.json",
        "{\"name\":\"greeting\",\"type\":\"module\",\"exports\":\"./index.js\"}\n",
    );
    write_in(
        &dir,
        "node_modules/greeting/index.js",
        "export default function greet() { return \"real greeting\"; }\n",
    );
    dir
}

/// Runs one `--dom` test file and asserts it passed, naming what it covers.
fn run_dom_test(name: &str, file: &str, source: &str, what: &str) {
    let dir = build_dir(name);
    write_in(&dir, file, source);
    let ran = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("spawn esdev test --dom");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        ran.status.success(),
        "{what}:\n{}{}",
        stdout(&ran),
        stderr(&ran)
    );
}

mod alias_env;
mod assets;
mod bench;
mod build;
mod build_cjs;
mod build_dts;
mod build_lib;
mod config;
mod create;
mod html;
mod import_policy;
mod init;
mod plugins;
mod preview;
mod run;
mod runtime_build;
mod runtime_watch;
mod source_maps;
mod test;
mod test_browser;
mod test_dom;
mod test_dom_forms;
mod test_dom_selectors;
mod test_dom_tree;
mod test_features;
mod test_watch;
mod typescript;
mod watch;
