//! `esdev start` writes development builds under the dev directory.
//!
//! No browser here: what is asserted is where the loop's builds land, not what
//! a page does with them (that is `tests/hot.rs`). Each test starts the real
//! loop on a fixture server project, waits until the app answers, and then
//! checks that the development bundle is under the dev directory — and that
//! `dist/`, the deployment `esdev build` writes, was never touched.
//!
//! The app port is never pinned: the fixture grants one, the loop takes it or
//! any free one, and the test reads which from the loop's own stderr. A fixed
//! port would collide with whatever else is running, and the symptom would not
//! read as a collision.

// A test reporting why it skipped is talking to whoever reads the run.
#![allow(clippy::print_stderr)]

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SERVER: &str = r#"import { serve } from "runtime:http";
import { env } from "runtime:process";
const port = Number(env.PORT ?? "8080");
serve({ port }, () => new Response("ok"));
"#;

fn project(name: &str, esdev_json: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esdev-start-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).expect("create the fixture");
    std::fs::write(dir.join("src/server.ts"), SERVER).expect("write the server");
    std::fs::write(dir.join("esdev.json"), esdev_json).expect("write the config");
    dir
}

/// A running dev loop, stopped and cleaned up however the test ends.
struct Loop {
    child: Child,
    dir: PathBuf,
}

impl Drop for Loop {
    fn drop(&mut self) {
        // Asked to stop, as a person or a CI job would ask, so it stops the
        // server it started. Killed outright, it cannot, and the server
        // outlives the test.
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &self.child.id().to_string()])
                .status();
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = self.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Loop {
    fn start(dir: PathBuf) -> Loop {
        let child = Command::new(env!("CARGO_BIN_EXE_esdev"))
            .arg("start")
            .current_dir(&dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn esdev start");
        Loop { child, dir }
    }

    /// Reads stderr until the loop names the port the app is on, then polls it
    /// until it answers — the loop builds before it spawns, so the port is
    /// known before the server exists.
    fn wait_for_app(&mut self) -> u16 {
        let stderr = self.child.stderr.take().expect("stderr");
        let mut lines = BufReader::new(stderr).lines();
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut port = None;
        while Instant::now() < deadline {
            let Ok(Some(line)) = lines.next().transpose() else {
                break;
            };
            if let Some(at) = line.find("the app is on http://localhost:") {
                port = line[at + "the app is on http://localhost:".len()..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .ok();
                break;
            }
        }
        let port = port.expect("the loop never named the app port");
        while Instant::now() < deadline {
            if get(port).contains("200") {
                return port;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        panic!("the app never answered on {port}");
    }
}

/// A plain HTTP GET over std, since these tests take no client dependency.
fn get(port: u16) -> String {
    let mut stream = match std::net::TcpStream::connect(("127.0.0.1", port)) {
        Ok(stream) => stream,
        Err(_) => return String::new(),
    };
    let _ = stream.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    let mut out = String::new();
    let _ = stream.read_to_string(&mut out);
    out
}

fn server_project(devdir: Option<&str>) -> String {
    let start = match devdir {
        Some(dev) => format!(r#""start": {{ "run": "server", "devdir": "{dev}" }}"#),
        None => r#""start": { "run": "server" }"#.to_string(),
    };
    format!(
        r#"{{ "targets": {{ "server": {{ "entry": "src/server.ts", "out": "dist/server.js" }} }},
              {start},
              "permissions": {{ "allow": {{ "listen": ["8080"], "env": ["PORT"] }} }} }}"#
    )
}

#[test]
fn the_loop_builds_into_dot_dev_and_leaves_dist_alone() {
    let dir = project("default", &server_project(None));
    let mut dev = Loop::start(dir.clone());
    let port = dev.wait_for_app();

    assert!(get(port).contains("ok"), "the dev server answers");
    assert!(
        dir.join(".dev/dist/server.js").is_file(),
        "the development bundle is under .dev: {:?}",
        dir.join(".dev")
    );
    assert!(
        !dir.join("dist").exists(),
        "the deployment directory was written by the dev loop"
    );
}

#[test]
fn a_named_dev_directory_is_used_instead() {
    let dir = project("named", &server_project(Some("tmp-dev")));
    let mut dev = Loop::start(dir.clone());
    let port = dev.wait_for_app();

    assert!(get(port).contains("ok"), "the dev server answers");
    assert!(
        dir.join("tmp-dev/dist/server.js").is_file(),
        "the development bundle is under tmp-dev"
    );
    assert!(
        !dir.join(".dev").exists() && !dir.join("dist").exists(),
        "a build landed outside the named dev directory"
    );
}

/// The loop narrates itself: a banner with ready timing and the app URL on
/// startup, then one line per save. No plumbing (no `ws://` line, no watch
/// path, no `127.0.0.1`) and the port still reads from the app sentence.
#[test]
fn the_loop_narrates_startup_and_every_save() {
    let dir = project("narrates", &server_project(None));
    let mut dev = Loop::start(dir.clone());
    let stderr = dev.child.stderr.take().expect("stderr");
    let mut lines = BufReader::new(stderr).lines();
    let deadline = Instant::now() + Duration::from_secs(120);

    let mut saw_ready = false;
    let mut port = None;
    while Instant::now() < deadline {
        let Ok(Some(line)) = lines.next().transpose() else {
            break;
        };
        assert!(!line.contains("ws://"), "plumbing in the banner: {line}");
        assert!(
            !line.contains("watching "),
            "plumbing in the banner: {line}"
        );
        assert!(!line.contains("127.0.0.1"), "use localhost: {line}");
        if line.contains("ready in") {
            saw_ready = true;
        }
        if let Some(at) = line.find("the app is on http://localhost:") {
            port = line[at + "the app is on http://localhost:".len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .ok();
        }
        if saw_ready && port.is_some() {
            break;
        }
    }
    assert!(saw_ready, "no ready banner");
    let port: u16 = port.expect("the loop never named the app port");

    // A save restarts the server and says so, once.
    std::fs::write(dir.join("src/server.ts"), SERVER.replace("ok", "ok2"))
        .expect("touch the server");
    let mut restarts = 0;
    while Instant::now() < deadline {
        let Ok(Some(line)) = lines.next().transpose() else {
            break;
        };
        if line.contains("restarted server") {
            restarts += 1;
            break;
        }
    }
    assert_eq!(restarts, 1, "no cycle line for the save");
    assert!(get(port).contains("ok"), "the restarted server answers");
}
