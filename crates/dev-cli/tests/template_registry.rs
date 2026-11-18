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
        said.contains("Template packages: @opentf/esdev-templates@0.1.0"),
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

/// A package laid out on disk, with one template shaped as AFW's App is
/// planned: a render choice, a server question asked only for some renders,
/// and a language whose files depend on whether there is a server.
fn conditional_package(label: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("package-{label}"));
    let _ = std::fs::remove_dir_all(&root);
    let put = |path: &str, text: &str| {
        let at = root.join("templates/app").join(path);
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        std::fs::write(at, text).unwrap();
    };
    put(
        "template.json",
        r#"{ "label": "App", "description": "An app", "options": [
             { "name": "render", "question": "Render?", "default": "client", "choices": [
               { "value": "server", "label": "Server", "description": "per request" },
               { "value": "client", "label": "Client", "description": "in the browser" } ] },
             { "name": "server", "kind": "confirm", "question": "Add a server?", "default": "no",
               "when": { "render": ["client"] }, "otherwise": "yes", "choices": [
               { "value": "yes", "label": "Yes", "description": "API routes" },
               { "value": "no", "label": "No", "description": "browser only" } ] },
             { "name": "language", "question": "Language?", "default": "js", "choices": [
               { "value": "js", "label": "JS", "description": "JavaScript" },
               { "value": "ts", "label": "TS", "description": "TypeScript", "omit": ["app/page.jsx"] } ] } ] }"#,
    );
    put("package.json", "{\n  \"name\": \"{{name}}\"\n}\n");
    std::fs::write(
        root.join("package.json"),
        r#"{ "name": "fixture", "esdev": { "templates": "templates" } }"#,
    )
    .unwrap();
    put("app/page.jsx", "page");
    put("_option/server/yes/app/todos/page.jsx", "todos");
    put("_option/language/ts/app/page.tsx", "typed page");
    put(
        "_option/language/ts/_option/server/yes/app/todos/page.tsx",
        "typed todos",
    );
    put(
        "_option/language/ts/_option/server/yes/app/todos/page.jsx",
        "unused",
    );
    root
}

/// The option asked only for some renders is decided by `otherwise` for the
/// others, a flag contradicting that is refused, and a file under two options
/// is written only with both.
#[test]
fn a_conditional_option_and_a_nested_overlay_work_from_a_directory() {
    let package = conditional_package("conditional");
    let from = format!("--from={}", package.display());
    let registry = registry(|own| Says {
        integrity: own.to_string(),
        tarball_origin: None,
    });
    let files = |app: &Path| {
        let mut found = Vec::new();
        let mut stack = vec![app.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap().flatten() {
                if entry.path().is_dir() {
                    stack.push(entry.path());
                } else {
                    found.push(es_runtime_default_providers::path::to_slash(
                        entry.path().strip_prefix(app).unwrap(),
                    ));
                }
            }
        }
        found.sort();
        found
    };

    // Server rendering has a server: not asked, and the TypeScript todos are
    // the nested overlay's.
    let (out, app) = create(
        "server-ts",
        &registry,
        &[&from, "--template=app", "--render=server", "--language=ts"],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        files(&app),
        [
            "app/page.tsx",
            "app/todos/page.jsx",
            "app/todos/page.tsx",
            "package.json"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(app.join("app/todos/page.jsx")).unwrap(),
        "unused"
    );

    // Client rendering asks, and unattended takes the default: no server.
    let (out, app) = create(
        "client-ts",
        &registry,
        &[&from, "--template=app", "--render=client", "--language=ts"],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(files(&app), ["app/page.tsx", "package.json"]);

    // --no-server cannot be: server rendering has one.
    let (out, app) = create(
        "server-no-server",
        &registry,
        &[&from, "--template=app", "--render=server", "--no-server"],
    );
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(
        said.contains("server is yes when render is server") && said.contains("--no-server"),
        "{said}"
    );
    assert!(!app.exists());
}

/// One release of a package the multi-package registry serves.
struct Published {
    version: &'static str,
    esdev: &'static str,
    files: Vec<(String, String)>,
}

/// Files as npm packs them, under `package/`.
fn pack(files: &[(String, String)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, text) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        builder
            .append_data(&mut header, format!("package/{path}"), text.as_bytes())
            .unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

/// A registry serving `packages` — every release, its tarball and its true
/// sha512 — and `1.2.3` for anything else a template pins.
fn registry_of(packages: Vec<(&'static str, Vec<Published>)>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the registry");
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let mut metadata = std::collections::HashMap::new();
    let mut tarballs = std::collections::HashMap::new();
    for (name, releases) in packages {
        let encoded = name.replace('/', "%2F");
        let mut versions = serde_json::Map::new();
        for release in releases {
            let tarball = pack(&release.files);
            let path = format!("/{encoded}/-/{}.tgz", release.version);
            versions.insert(
                release.version.to_string(),
                serde_json::json!({
                    "engines": { "esdev": release.esdev },
                    "dist": { "tarball": format!("{origin}{path}"), "integrity": sri(&tarball) }
                }),
            );
            tarballs.insert(path, tarball);
        }
        metadata.insert(
            format!("/{encoded}"),
            serde_json::json!({ "versions": versions }).to_string(),
        );
    }
    let metadata = std::sync::Arc::new(metadata);
    let tarballs = std::sync::Arc::new(tarballs);
    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else { continue };
            let (metadata, tarballs) = (metadata.clone(), tarballs.clone());
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
                let body: Vec<u8> = if let Some(tarball) = tarballs.get(&path) {
                    tarball.clone()
                } else if let Some(json) = metadata.get(&path) {
                    json.clone().into_bytes()
                } else {
                    br#"{"dist-tags":{"latest":"1.2.3"},"versions":{"1.2.3":{}}}"#.to_vec()
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            });
        }
    });
    origin
}

/// A template directory's files under `dir`: a manifest and a package.json.
fn template_files(dir: &str, name: &str, group: Option<&str>) -> Vec<(String, String)> {
    let group = group.map_or(String::new(), |g| format!(r#", "group": "{g}""#));
    vec![
        (
            format!("{dir}/{name}/template.json"),
            format!(r#"{{ "label": "{name}", "description": "the {name} template"{group} }}"#),
        ),
        (
            format!("{dir}/{name}/package.json"),
            "{\n  \"name\": \"{{name}}\"\n}\n".to_string(),
        ),
    ]
}

/// The root package: `api`, and a catalog listing `listed`.
fn root_package(listed: &[&str], template: &str) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "package.json".to_string(),
            r#"{ "name": "@opentf/esdev-templates", "esdev": { "templates": "templates" } }"#
                .to_string(),
        ),
        (
            "templates/catalog.json".to_string(),
            serde_json::json!({ "packages": listed }).to_string(),
        ),
    ];
    files.extend(template_files("templates", template, None));
    files
}

/// A framework's package, templates under `esdev/templates`, in a group.
fn framework_package(template: &str) -> Vec<(String, String)> {
    let mut files = vec![
        (
            "package.json".to_string(),
            r#"{ "name": "@opentf/create-afw", "esdev": { "templates": "esdev/templates" } }"#
                .to_string(),
        ),
        (
            "esdev/templates/catalog.json".to_string(),
            serde_json::json!({ "groups": [ { "name": "AFW", "label": "AFW",
                "description": "AFW starters", "question": "Which?", "templates": [template] } ] })
            .to_string(),
        ),
    ];
    files.extend(template_files("esdev/templates", template, Some("AFW")));
    files
}

/// The root's catalog lists a second package: both are fetched and checked,
/// their templates are one menu, and either writes a project.
#[test]
fn a_package_the_root_lists_is_read_beside_it() {
    let registry = registry_of(vec![
        (
            "@opentf/esdev-templates",
            vec![Published {
                version: "0.2.0",
                esdev: ">=0.0.1",
                files: root_package(&["@opentf/create-afw"], "api"),
            }],
        ),
        (
            "@opentf/create-afw",
            vec![Published {
                version: "0.6.1",
                esdev: ">=0.0.1",
                files: framework_package("app"),
            }],
        ),
    ]);
    let (out, app) = create("two-packages", &registry, &["--template=app"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("Template packages: @opentf/esdev-templates@0.2.0, @opentf/create-afw@0.6.1"),
        "{said}"
    );
    assert!(app.join("package.json").is_file());
    let (out, api) = create("two-packages-root", &registry, &["--template=api"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(api.join("package.json").is_file());
}

/// A template both packages define is refused: which package writes a
/// project never depends on the order they were read in.
#[test]
fn a_template_two_packages_define_is_refused() {
    let registry = registry_of(vec![
        (
            "@opentf/esdev-templates",
            vec![Published {
                version: "0.2.0",
                esdev: ">=0.0.1",
                files: root_package(&["@opentf/create-afw"], "app"),
            }],
        ),
        (
            "@opentf/create-afw",
            vec![Published {
                version: "0.6.1",
                esdev: ">=0.0.1",
                files: framework_package("app"),
            }],
        ),
    ]);
    let (out, app) = create("clash", &registry, &["--template=app"]);
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("the app template is in both"), "{said}");
    assert!(!app.exists());
}

/// A listed package with no release this esdev reads is left out, saying
/// so; the root's templates still write.
#[test]
fn a_listed_package_this_esdev_cannot_read_is_left_out() {
    let registry = registry_of(vec![
        (
            "@opentf/esdev-templates",
            vec![Published {
                version: "0.2.0",
                esdev: ">=0.0.1",
                files: root_package(&["@opentf/create-afw"], "api"),
            }],
        ),
        (
            "@opentf/create-afw",
            vec![Published {
                version: "0.6.1",
                esdev: ">=99.0.0",
                files: framework_package("app"),
            }],
        ),
    ]);
    let (out, api) = create("left-out", &registry, &["--template=api"]);
    let said = text(&out);
    assert!(out.status.success(), "{said}");
    assert!(
        said.contains("the templates in @opentf/create-afw are left out"),
        "{said}"
    );
    assert!(said.contains(">=99.0.0"), "{said}");
    assert!(api.join("package.json").is_file());
    let (out, _) = create("left-out-app", &registry, &["--template=app"]);
    assert!(
        !out.status.success(),
        "the app template came from a package that was left out"
    );
}

/// `--from` twice reads both directories as one menu, and refuses a name
/// both define.
#[test]
fn from_twice_reads_both_directories() {
    let write = |label: &str, files: Vec<(String, String)>| {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("dir-{label}"));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in files {
            let at = root.join(path);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, text).unwrap();
        }
        format!("--from={}", root.display())
    };
    let ours = write("ours", root_package(&[], "api"));
    let theirs = write("theirs", framework_package("app"));
    let registry = registry_of(vec![]);
    let (out, app) = create("from-twice", &registry, &[&ours, &theirs, "--template=app"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(app.join("package.json").is_file());
    let clash = write("clash", framework_package("api"));
    let (out, _) = create("from-clash", &registry, &[&ours, &clash, "--template=api"]);
    let said = text(&out);
    assert!(!out.status.success(), "{said}");
    assert!(said.contains("the api template is in both"), "{said}");
}
