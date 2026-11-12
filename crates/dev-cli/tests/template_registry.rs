//! `esdev create` reading its templates from the registry (DECISIONS D220).
//!
//! A local registry serves `@opentf/esdev-templates` as npm would: metadata
//! naming two releases — `0.1.0`, which this esdev reads, and `0.9.0`, which
//! needs an esdev that does not exist yet — and a tarball packed here from the
//! repository's own package, under `package/`, with its true sha512. Each test
//! changes one thing the registry says and checks that esdev takes it or
//! refuses it, and that a refusal writes nothing.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// What the registry says, per test.
#[derive(Clone)]
struct Says {
    /// The integrity it publishes for 0.1.0: the tarball's own, or another's.
    integrity: String,
    /// Where it says 0.1.0's tarball is: its own origin, or another.
    tarball_origin: Option<String>,
}

/// The repository's package as npm packs it: every file under `package/`.
fn packed() -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/esdev-templates");
    let mut files = Vec::new();
    fn walk(root: &Path, dir: &Path, into: &mut Vec<(String, PathBuf)>) {
        for entry in std::fs::read_dir(dir).expect("read the package").flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n == "node_modules") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, into);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                into.push((relative, path));
            }
        }
    }
    walk(&root, &root.join("templates"), &mut files);
    files.push(("package.json".to_string(), root.join("package.json")));
    files.sort();
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (relative, path) in files {
        let bytes = std::fs::read(&path).expect("read a package file");
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, format!("package/{relative}"), bytes.as_slice())
            .expect("pack a file");
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn sri(bytes: &[u8]) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    format!(
        "sha512-{}",
        base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(bytes))
    )
}

/// Starts the registry; its URL.
fn registry(says: impl Fn(&str) -> Says + Send + Sync + 'static) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the registry");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let tarball = std::sync::Arc::new(packed());
    let says = std::sync::Arc::new(says);
    let here = origin.clone();
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { continue };
            let tarball = tarball.clone();
            let says = says.clone();
            let here = here.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    return;
                }
                let path = line.split(' ').nth(1).unwrap_or("/").to_string();
                loop {
                    let mut header = String::new();
                    if reader.read_line(&mut header).is_err()
                        || header == "\r\n"
                        || header.is_empty()
                    {
                        break;
                    }
                }
                let said = says(&tarball_integrity(&tarball));
                let (kind, body): (&str, Vec<u8>) = if path.ends_with(".tgz") {
                    ("application/octet-stream", tarball.to_vec())
                } else if path.contains("esdev-templates") {
                    let at = said.tarball_origin.unwrap_or_else(|| here.clone());
                    let metadata = serde_json::json!({
                        "dist-tags": { "latest": "0.9.0" },
                        "versions": {
                            "0.1.0": {
                                "engines": { "esdev": ">=0.0.1" },
                                "dist": { "tarball": format!("{at}/t-0.1.0.tgz"), "integrity": said.integrity }
                            },
                            "0.9.0": {
                                "engines": { "esdev": ">=99.0.0" },
                                "dist": { "tarball": format!("{here}/t-0.9.0.tgz"), "integrity": "sha512-unread" }
                            }
                        }
                    });
                    ("application/json", metadata.to_string().into_bytes())
                } else {
                    (
                        "application/json",
                        br#"{"dist-tags":{"latest":"1.2.3"},"versions":{"1.2.3":{}}}"#.to_vec(),
                    )
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            });
        }
    });
    origin
}

fn tarball_integrity(tarball: &[u8]) -> String {
    sri(tarball)
}

/// `esdev create app <args…> --no-install` against `registry`, in a fresh
/// parent; the run and the project's directory.
fn create(label: &str, registry: &str, args: &[&str]) -> (Output, PathBuf) {
    let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("registry-{label}"));
    let _ = std::fs::remove_dir_all(&parent);
    std::fs::create_dir_all(&parent).expect("create the parent");
    let out = Command::new(env!("CARGO_BIN_EXE_esdev"))
        .args(["create", "app"])
        .args(args)
        .arg("--no-install")
        .current_dir(&parent)
        .stdin(Stdio::null())
        .env("npm_config_registry", registry)
        .env("NPM_CONFIG_REGISTRY", registry)
        .output()
        .expect("spawn esdev create");
    (out, parent.join("app"))
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The newest release this esdev reads is fetched, checked and written from —
/// and the newer one it cannot read is named, with the way to read it.
#[test]
fn templates_come_from_the_newest_release_this_esdev_reads() {
    let registry = registry(|own| Says {
        integrity: own.to_string(),
        tarball_origin: None,
    });
    let (out, app) = create("newest", &registry, &["--template=api"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("Templates: @opentf/esdev-templates@0.1.0"),
        "{said}"
    );
    assert!(
        said.contains("@opentf/esdev-templates@0.9.0 needs esdev >=99.0.0")
            && said.contains("esdev upgrade"),
        "{said}"
    );
    assert!(app.join("package.json").is_file(), "{said}");
    assert!(app.join("src/server.ts").is_file(), "{said}");
}

/// Bytes that are not what the registry says were published are refused
/// before anything is read from them, and nothing is written.
#[test]
fn a_tarball_that_does_not_match_its_integrity_is_refused() {
    let registry = registry(|_| Says {
        integrity: sri(b"what was really published"),
        tarball_origin: None,
    });
    let (out, app) = create("tampered", &registry, &["--template=api"]);
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("not what was published"), "{said}");
    assert!(!app.exists(), "a refused package wrote a project");
}

/// A tarball the registry points at somewhere else is not fetched.
#[test]
fn a_tarball_on_another_origin_is_refused() {
    let registry = registry(|own| Says {
        integrity: own.to_string(),
        tarball_origin: Some("http://127.0.0.1:9".to_string()),
    });
    let (out, app) = create("elsewhere", &registry, &["--template=api"]);
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("not from the registry"), "{said}");
    assert!(!app.exists());
}

/// `--templates` takes the release named — and refuses one this esdev
/// cannot read, saying what it needs.
#[test]
fn a_pinned_release_is_taken_or_refused() {
    let registry = registry(|own| Says {
        integrity: own.to_string(),
        tarball_origin: None,
    });
    let (out, app) = create(
        "pinned",
        &registry,
        &["--template=api", "--templates=0.1.0"],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(app.join("package.json").is_file());
    let (out, app) = create(
        "pinned-unread",
        &registry,
        &["--template=api", "--templates=0.9.0"],
    );
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("needs esdev >=99.0.0"), "{said}");
    assert!(!app.exists());
}
