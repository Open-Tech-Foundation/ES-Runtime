//! End-to-end tests for the servers the fullstack templates write.
//!
//! A created project's dependencies come from npm, which CI does not reach, so
//! each test scaffolds against a local registry stand-in and replaces only what
//! a package would have provided — `@afw/server`, which the AFW plugin
//! generates, and React's `render.tsx` — with a stub. The server file the
//! template wrote runs unchanged: what is under test is its own request
//! handling.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;

/// Templates from the repository's own package, read as `--from` reads it:
/// the tests never reach a registry for them (DECISIONS D220).
const TEMPLATES: &str = concat!(
    "--from=",
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/esdev-templates"
);

/// A registry that names `1.2.3` as every package's latest version.
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
                        let Ok(clone) = stream.try_clone() else {
                            return;
                        };
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
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

/// `esdev create <name> <args…> --no-install` in a fresh parent; the project.
fn create(label: &str, args: &[&str]) -> PathBuf {
    let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("template-{label}"));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("create parent");
    let out = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .arg("create")
        .arg(TEMPLATES)
        .arg("app")
        .args(args)
        .arg("--no-install")
        .current_dir(&parent)
        .stdin(Stdio::null())
        .env("npm_config_registry", test_registry())
        .env("NPM_CONFIG_REGISTRY", test_registry())
        .output()
        .expect("spawn esdev create");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    parent.join("app")
}

fn write(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write file");
}

/// A built output: the shell and one script under `assets/`.
fn built_output(dir: &Path) {
    write(
        dir,
        "index.html",
        "<!doctype html><title>app</title><!--app-->",
    );
    write(dir, "assets/app-abc.js", "console.log(1);\n");
}

/// A running server; killed when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Runs `entry` under esdev with `PORT=0`, and waits for the line that names
/// the port it took.
fn run(dir: &Path, entry: &str) -> Server {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .arg(entry)
        .current_dir(dir)
        .env("PORT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn the server");
    let stdout = child.stdout.take().expect("stdout");
    let mut lines = BufReader::new(stdout).lines();
    let mut port = None;
    for line in lines.by_ref().map_while(Result::ok) {
        if let Some(at) = line.find("http://localhost:") {
            let digits: String = line[at + "http://localhost:".len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            port = digits.parse::<u16>().ok();
            break;
        }
    }
    let Some(port) = port else {
        let mut err = String::new();
        let _ = child.stderr.take().map(|mut e| e.read_to_string(&mut err));
        let _ = child.kill();
        panic!("the server never named its port:\n{err}");
    };
    std::thread::spawn(move || lines.map_while(Result::ok).for_each(drop));
    Server { child, port }
}

/// One GET: the status code, the `content-type`, and whether the response
/// arrived whole — a body cut off mid-stream ends the read in an error.
fn get(port: u16, path: &str) -> (u16, Option<String>, bool) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .expect("request");
    let mut raw = Vec::new();
    let whole = stream.read_to_end(&mut raw).is_ok();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    let head = text.split("\r\n\r\n").next().unwrap_or("");
    let content_type = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-type")
            .then(|| value.trim().to_string())
    });
    // A chunked body that ends has its terminating chunk; one cut off has not.
    let ended = !head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
        || text.ends_with("0\r\n\r\n");
    (status, content_type, whole && ended)
}

/// The AFW fullstack server answers a directory with the fallback's 404,
/// serves a built file with the runtime's type, and misses with a 404.
#[test]
fn the_fullstack_server_answers_a_directory_with_a_404() {
    let dir = create("afw-fullstack", &["--template=fullstack"]);
    // `@afw/server` as the plugin would generate it, reduced to what decides
    // a static request: files first, then the fallback.
    write(
        &dir,
        "node_modules/@afw/server/package.json",
        r#"{ "name": "@afw/server", "type": "module", "exports": "./index.js" }"#,
    );
    write(
        &dir,
        "node_modules/@afw/server/index.js",
        "export const createPageRenderer = () => () => null;\n\
         export function createHandler({ files, fallback }) {\n\
           return async (request) => (await files(request)) ?? fallback(request);\n\
         }\n",
    );
    // Its plugin is not installed, and running one file needs no config.
    std::fs::remove_file(dir.join("esdev.json")).expect("remove esdev.json");
    let dist = dir.join("dist");
    built_output(&dist);
    std::fs::copy(dir.join("server.js"), dist.join("server.js")).expect("copy the server");

    let server = run(&dir, "dist/server.js");
    let (status, _, whole) = get(server.port, "/assets/");
    assert_eq!(status, 404, "a directory is not a file");
    assert!(whole, "the response was cut off");
    let (status, _, whole) = get(server.port, "/assets");
    assert_eq!(status, 404);
    assert!(whole);
    let (status, content_type, whole) = get(server.port, "/assets/app-abc.js");
    assert_eq!(status, 200);
    assert_eq!(
        content_type.as_deref(),
        Some("text/javascript; charset=utf-8")
    );
    assert!(whole);
    assert_eq!(get(server.port, "/assets/missing.js").0, 404);
}

/// The same for the React fullstack server's `/assets/`.
#[test]
fn the_react_fullstack_server_answers_a_directory_with_a_404() {
    let dir = create("react-fullstack", &["--template=react", "--mode=fullstack"]);
    // `render.tsx` imports React; the stand-in keeps only what the server
    // reads from it — where the output is, and a page.
    write(
        &dir,
        "src/render.tsx",
        "import { dirname, fromFileURL, join } from \"runtime:path\";\n\
         export const here = join(dirname(fromFileURL(import.meta.url)), \"..\", \"dist\");\n\
         export const renderPage = async () => \"<!doctype html><title>app</title>\";\n",
    );
    built_output(&dir.join("dist"));

    let server = run(&dir, "src/server.tsx");
    let (status, _, whole) = get(server.port, "/assets/");
    assert_eq!(status, 404, "a directory is not a file");
    assert!(whole, "the response was cut off");
    let (status, content_type, whole) = get(server.port, "/assets/app-abc.js");
    assert_eq!(status, 200);
    assert_eq!(
        content_type.as_deref(),
        Some("text/javascript; charset=utf-8")
    );
    assert!(whole);
    assert_eq!(get(server.port, "/assets/missing.js").0, 404);
    assert_eq!(get(server.port, "/").0, 200);
}
