//! `esdev create` (D65).

use super::*;

/// Templates from the repository's own package, read as `--from` reads it:
/// the tests never reach a registry for them (DECISIONS D220).
const TEMPLATES: &str = concat!(
    "--from=",
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/esdev-templates"
);

/// A registry whose `latest` is a release candidate, as elysia's was in
/// November 2026, beside an older stable release and a newer deprecated one.
fn prerelease_registry() -> &'static str {
    static REGISTRY: OnceLock<String> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind prerelease registry");
            let address = listener.local_addr().expect("prerelease registry address");
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
                        let body = r#"{"dist-tags":{"latest":"2.0.0-rc.4"},"versions":{"1.4.30":{},"1.4.31":{},"1.5.0":{"deprecated":"oops"},"2.0.0-rc.4":{}}}"#;
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

/// The first request fails transiently; later requests use the normal fixture
/// response so the caller can assert that create reports and retries it.
fn retry_registry() -> (&'static str, Arc<AtomicUsize>) {
    static REGISTRY: OnceLock<(String, Arc<AtomicUsize>)> = OnceLock::new();
    let (url, attempts) = REGISTRY.get_or_init(|| {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind retry registry");
        let address = listener.local_addr().expect("retry registry address");
        let attempts = Arc::new(AtomicUsize::new(0));
        let server_attempts = Arc::clone(&attempts);
        std::thread::spawn(move || {
            for incoming in listener.incoming() {
                let Ok(mut stream) = incoming else { continue };
                let attempts = Arc::clone(&server_attempts);
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
                    let first = attempts.fetch_add(1, Ordering::SeqCst) == 0;
                    let (status, body) = if first {
                        ("503 Service Unavailable", "")
                    } else {
                        ("200 OK", r#"{"dist-tags":{"latest":"1.2.3"}}"#)
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes());
                });
            }
        });
        (format!("http://{address}"), attempts)
    });
    (url.as_str(), Arc::clone(attempts))
}

/// A React project declares `@opentf/esdev-plugin-react`, a package, which a
/// test with no registry cannot install. Fast Refresh does nothing in a test,
/// so a plugin that does nothing stands in for it.
fn stand_in_for_the_react_plugin(dir: &Path) {
    let plugin = dir.join("node_modules/@opentf/esdev-plugin-react");
    std::fs::create_dir_all(&plugin).expect("mkdir the plugin stand-in");
    write_in(
        &plugin,
        "package.json",
        r#"{"name":"@opentf/esdev-plugin-react","version":"1.2.3","type":"module","exports":"./index.js"}"#,
    );
    write_in(
        &plugin,
        "index.js",
        "export default { name: 'react-refresh' };\n",
    );
}

#[test]
fn create_writes_a_project_that_builds_and_runs() {
    let parent = watch_dir("c_project");
    let dir = parent.join("weather-app");

    let out = esdev_in(&parent)
        .args(["create", TEMPLATES, "weather-app", "--template=react"])
        .output()
        .expect("spawn esdev create");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("npm install"), "{}", stdout(&out));

    // The name comes from the directory, into the manifest and the document.
    let manifest = std::fs::read_to_string(dir.join("package.json")).expect("read package.json");
    assert!(manifest.contains(r#""name": "weather-app""#), "{manifest}");
    assert!(!manifest.contains("{{name}}"), "a placeholder survived");
    assert!(
        !manifest.contains("{{registry:"),
        "a package version placeholder survived"
    );
    assert!(manifest.contains(r#""react": "1.2.3""#), "{manifest}");
    assert!(
        stderr(&out).contains("react@1.2.3"),
        "registry resolutions were not reported: {}",
        stderr(&out)
    );
    let document = std::fs::read_to_string(dir.join("index.html")).expect("read index.html");
    assert!(
        document.contains("<title>weather-app</title>"),
        "{document}"
    );
    // A static project previews what it built.
    assert!(
        manifest.contains(r#""preview": "esdev preview""#),
        "{manifest}"
    );

    // `_gitignore` is written under the name it has to have — as itself, it
    // would apply to the template in this repository.
    assert!(dir.join(".gitignore").is_file(), "no .gitignore");
    assert!(
        !dir.join("_gitignore").exists(),
        "_gitignore was written as-is"
    );

    // Nothing a local build or install left behind is in the binary.
    assert!(!dir.join("node_modules").exists());
    assert!(!dir.join("dist").exists());

    // The tests it ships pass, which is the smallest end-to-end claim that
    // does not need a package registry. The one plugin it declares is a
    // package, so it is stood in for: Fast Refresh does nothing in a test.
    stand_in_for_the_react_plugin(&dir);
    let tested = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        tested.status.success(),
        "the template's own tests failed:\n{}{}",
        stdout(&tested),
        stderr(&tested)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn create_reports_and_retries_a_transient_registry_failure() {
    let parent = watch_dir("c_registry_retry");
    let (registry, attempts) = retry_registry();
    let created = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "retry-app",
            "--template=api",
            "--no-install",
        ])
        .env("npm_config_registry", registry)
        .env("NPM_CONFIG_REGISTRY", registry)
        .output()
        .expect("spawn esdev create");

    assert!(created.status.success(), "{}", stderr(&created));
    assert!(
        stderr(&created).contains("failed on attempt 1/4")
            && stderr(&created).contains("retrying in 500 ms"),
        "retry progress was not reported: {}",
        stderr(&created)
    );
    assert!(
        attempts.load(Ordering::SeqCst) >= 3,
        "request was not retried"
    );
    let manifest = std::fs::read_to_string(parent.join("retry-app/package.json"))
        .expect("read generated manifest");
    assert!(!manifest.contains("{{registry:"), "{manifest}");

    let _ = std::fs::remove_dir_all(&parent);
}

/// A prerelease tagged `latest` by mistake is not what a new project pins
/// (D215): the newest stable, non-deprecated release is.
#[test]
fn create_pins_the_newest_stable_release_over_a_prerelease_latest() {
    let parent = watch_dir("c_registry_prerelease");
    let registry = prerelease_registry();
    let created = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "rc-app",
            "--template=elysia",
            "--no-install",
        ])
        .env("npm_config_registry", registry)
        .env("NPM_CONFIG_REGISTRY", registry)
        .output()
        .expect("spawn esdev create");

    assert!(created.status.success(), "{}", stderr(&created));
    let manifest = std::fs::read_to_string(parent.join("rc-app/package.json"))
        .expect("read generated manifest");
    assert!(manifest.contains(r#""elysia": "1.4.31""#), "{manifest}");
    assert!(!manifest.contains("rc.4"), "{manifest}");

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn react_compiler_runs_before_jsx_lowering_in_an_esdev_build() {
    let parent = watch_dir("c_react_compiler");
    std::fs::create_dir_all(parent.join("src")).expect("create source directory");
    std::fs::create_dir_all(parent.join("node_modules/react"))
        .expect("create React package directory");
    write_in(
        &parent,
        "esdev.json",
        r#"{"jsx":{"importSource":"react","reactCompiler":true},"build":{"targets":{"app":{"entry":"src/App.tsx","out":"dist/app.js","platform":"browser"}}}}"#,
    );
    write_in(
        &parent,
        "src/App.tsx",
        "export function Greeting({ name }: { name: string }) { return <h1>Hello {name}</h1>; }",
    );
    write_in(
        &parent,
        "node_modules/react/package.json",
        r#"{"name":"react","type":"module","exports":{"./compiler-runtime":"./compiler-runtime.js","./jsx-runtime":"./jsx-runtime.js"}}"#,
    );
    write_in(
        &parent,
        "node_modules/react/compiler-runtime.js",
        "export function c(size) { const cache = new Array(size); cache.__OXC_REACT_COMPILER__ = true; return cache; }",
    );
    write_in(
        &parent,
        "node_modules/react/jsx-runtime.js",
        "export const Fragment = Symbol.for('react.fragment'); export function jsx(type, props) { return { type, props }; } export const jsxs = jsx;",
    );

    let built = esdev_in(&parent)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));
    let output = std::fs::read_to_string(parent.join("dist/app.js")).expect("read bundle");
    assert!(
        output.contains("__OXC_REACT_COMPILER__"),
        "the React Compiler runtime was not included:\n{output}"
    );
    assert!(
        !output.contains("<h1>"),
        "JSX was not lowered after the React Compiler:\n{output}"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// It owns nothing it writes into, so a directory with anything in it is
/// refused — and `--force` means "write among what is there", never over it.
#[test]
fn create_refuses_a_directory_that_holds_something() {
    let parent = watch_dir("c_refuse");
    let dir = parent.join("taken");
    std::fs::create_dir_all(&dir).expect("create dir");
    write_in(&dir, "package.json", "{ \"name\": \"mine\" }\n");

    let refused = esdev_in(&parent)
        .args(["create", TEMPLATES, "taken", "--template=react"])
        .output()
        .expect("spawn esdev create");
    assert!(!refused.status.success());
    assert!(
        stderr(&refused).contains("not empty"),
        "{}",
        stderr(&refused)
    );

    let forced = esdev_in(&parent)
        .args(["create", TEMPLATES, "taken", "--template=react", "--force"])
        .output()
        .expect("spawn esdev create");
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert!(
        stdout(&forced).contains("left alone"),
        "{}",
        stdout(&forced)
    );
    // The file that was there is the file that is there.
    assert_eq!(
        std::fs::read_to_string(dir.join("package.json")).expect("read"),
        "{ \"name\": \"mine\" }\n"
    );
    // …and the rest of the project was written around it.
    assert!(dir.join("src/App.tsx").is_file());

    let _ = std::fs::remove_dir_all(&parent);
}

/// Every template, scaffolded and put through its own test suite.
///
/// The one property that matters for a scaffolder and cannot be checked by
/// looking at the files: that what it writes *works*. A template is a project
/// nobody builds until somebody depends on it, which is exactly the kind of
/// thing that rots quietly.
///
/// `react` is the exception, and deliberately: its tests need `node_modules`,
/// and installing from a registry is not something a unit test should do. It is
/// covered by `create_writes_a_project_that_builds_and_runs` instead.
#[test]
fn every_dependency_free_template_passes_its_own_tests() {
    let parent = watch_dir("c_all");

    for template in ["api", "lib", "vanilla", "micro-ui"] {
        let dir = parent.join(template);
        let created = esdev_in(&parent)
            .args([
                "create",
                TEMPLATES,
                template,
                &format!("--template={template}"),
            ])
            .output()
            .expect("spawn esdev create");
        assert!(
            created.status.success(),
            "{}: {}",
            template,
            stderr(&created)
        );
        let manifest = std::fs::read_to_string(dir.join("package.json")).expect("manifest");
        assert!(
            !manifest.contains("{{registry:"),
            "the {template} manifest has an unresolved package version: {manifest}"
        );

        let tested = esdev_in(&dir)
            .arg("test")
            .output()
            .expect("spawn esdev test");
        assert!(
            tested.status.success(),
            "the {template} template's own tests failed:\n{}{}",
            stdout(&tested),
            stderr(&tested)
        );

        // …and it builds. A template that tests clean and does not build is
        // still a broken starting point.
        let mut build = esdev_in(&dir);
        build.arg("build");
        if template == "lib" {
            // `--lib` is a flag rather than an esdev.json key, so the template
            // carries it in its `build` script rather than its config.
            build.args(["--lib", "src"]);
        }
        let built = build.output().expect("spawn esdev build");
        assert!(
            built.status.success(),
            "the {template} template does not build:\n{}{}",
            stdout(&built),
            stderr(&built)
        );

        // …and the DOM run passes with nothing installed. DOM tests guard
        // on `document`, so this also proves a plain `esdev test` stays
        // green beside them.
        let domed = esdev_in(&dir)
            .args(["test", "--dom"])
            .output()
            .expect("spawn esdev test --dom");
        assert!(
            domed.status.success(),
            "the {template} template fails under --dom:\n{}{}",
            stdout(&domed),
            stderr(&domed)
        );
    }

    let _ = std::fs::remove_dir_all(&parent);
}

/// A library builds with a flag, not a config — so a bare `esdev build`
/// inside one must say so rather than fail cryptically.
#[test]
fn a_bare_build_in_a_lib_project_names_the_flag() {
    let parent = watch_dir("c_lib_hint");
    let dir = parent.join("libbed");

    let created = esdev_in(&parent)
        .args(["create", TEMPLATES, "libbed", "--template=lib"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(created.status.success(), "{}", stderr(&created));

    let built = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!built.status.success(), "a bare build wrote a library");
    assert!(
        stderr(&built).contains("--lib"),
        "the error does not name the flag:\n{}",
        stderr(&built)
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// A mode is the whole project, not a preset on top of one. What you get is the
/// files that mode needs and none of the other's — otherwise a starter ships a
/// server nobody runs and a permission nobody needs, and the person scaffolding
/// is left deleting half of it.
#[test]
fn each_mode_writes_its_own_project_and_none_of_the_other() {
    let parent = watch_dir("c_modes");

    for (mode, mine, theirs) in [
        ("static", "src/prerender.tsx", "src/server.tsx"),
        ("fullstack", "src/server.tsx", "src/prerender.tsx"),
    ] {
        let dir = parent.join(mode);
        let out = esdev_in(&parent)
            .args([
                "create",
                TEMPLATES,
                mode,
                "--template=react",
                &format!("--mode={mode}"),
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn esdev create");
        assert!(out.status.success(), "{mode}: {}", stderr(&out));
        assert!(
            stdout(&out).contains(&format!("react ({mode}) template")),
            "{mode}: {}",
            stdout(&out)
        );

        assert!(dir.join(mine).is_file(), "{mode} has no {mine}");
        assert!(
            !dir.join(theirs).exists(),
            "{mode} was written with {theirs}, which belongs to the other mode"
        );
        // The shared half is in both, rather than duplicated into each.
        assert!(dir.join("src/App.tsx").is_file(), "{mode} has no App");

        // And the tests it ships pass — the react template's own suite needs no
        // node_modules but its one plugin, which is what makes this checkable
        // here at all.
        stand_in_for_the_react_plugin(&dir);
        let tested = esdev_in(&dir)
            .arg("test")
            .output()
            .expect("spawn esdev test");
        assert!(
            tested.status.success(),
            "the react ({mode}) template's own tests failed:\n{}{}",
            stdout(&tested),
            stderr(&tested)
        );
    }

    // A static project has nothing to run in production, so it grants nothing
    // and names no server; a fullstack one does both.
    let statik = std::fs::read_to_string(parent.join("static/esdev.json")).expect("read");
    assert!(!statik.contains("permissions"), "{statik}");
    // The page esdev serves, and nothing it runs.
    let statik_dev: serde_json::Value = serde_json::from_str(&statik).expect("JSON");
    assert_eq!(
        statik_dev["dev"]["servers"],
        serde_json::json!({ "web": { "port": 5173 } }),
        "{statik}"
    );
    let full = std::fs::read_to_string(parent.join("fullstack/esdev.json")).expect("read");
    let full_dev: serde_json::Value = serde_json::from_str(&full).expect("JSON");
    assert_eq!(
        full_dev["dev"]["servers"],
        serde_json::json!({ "server": { "port": 3000 } }),
        "{full}"
    );

    // Both ways of getting a static build are scripts on the project, so the
    // SSG/SPA choice is made per deploy rather than at scaffold time.
    let scripts = std::fs::read_to_string(parent.join("static/package.json")).expect("read");
    assert!(
        scripts.contains("\"build\": \"esdev build --minify\""),
        "{scripts}"
    );
    assert!(scripts.contains("build:spa"), "{scripts}");

    let _ = std::fs::remove_dir_all(&parent);
}

/// A mode that is not one is refused rather than ignored, and so is a mode on a
/// template that has only one shape — a flag that silently does nothing is one
/// somebody keeps passing, and keeps believing.
#[test]
fn a_mode_that_does_not_exist_is_refused() {
    let parent = watch_dir("c_bad_mode");

    let unknown = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "nope",
            "--template=react",
            "--mode=ssr",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success(), "{}", stdout(&unknown));
    // The message names the modes there are.
    assert!(stderr(&unknown).contains("static"), "{}", stderr(&unknown));
    assert!(
        stderr(&unknown).contains("fullstack"),
        "{}",
        stderr(&unknown)
    );
    assert!(!parent.join("nope").exists(), "it wrote a project anyway");

    let modeless = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "nope",
            "--template=api",
            "--mode=static",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!modeless.status.success(), "{}", stdout(&modeless));
    assert!(
        stderr(&modeless).contains("has no mode"),
        "{}",
        stderr(&modeless)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// A prompt in a script is a script that hangs, which is the whole reason the
/// interactive path is gated. These run with stdin closed — the shape every CI
/// job has — and must answer without asking anything.
#[test]
fn create_never_asks_when_nobody_is_there() {
    let parent = watch_dir("c_quiet");

    // No flags at all: there is no default template, so this stops rather
    // than guessing — without asking, and without writing anything.
    let out = esdev_in(&parent)
        .args(["create", TEMPLATES, "quiet"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("no template chosen"),
        "{}",
        stderr(&out)
    );
    assert!(
        !parent.join("quiet").exists(),
        "an undecided run wrote a project"
    );

    // …and nothing was written to the question stream.
    assert!(
        !stderr(&out).contains("Which Template"),
        "it asked anyway:\n{}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// Every question has a flag, so the interactive path is a convenience over the
/// scriptable one rather than the only way to an answer.
#[test]
fn every_question_has_a_flag() {
    let parent = watch_dir("c_flags");

    let out = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "flagged",
            "--template=lib",
            "--no-install",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        parent.join("flagged/src/index.ts").is_file(),
        "not the lib template"
    );

    // The next step is what that template actually has: a library has nothing
    // to `run dev`.
    assert!(stdout(&out).contains("run test"), "{}", stdout(&out));

    // `--yes` is the conventional "take every default", and must not ask
    // either — even where a terminal would have been available. The template
    // has no default, so it still has to be named.
    let yes = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "defaulted",
            "--yes",
            "--template=react",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(yes.status.success(), "{}", stderr(&yes));
    assert!(
        stdout(&yes).contains("react (static) template"),
        "{}",
        stdout(&yes)
    );

    // Without it, `--yes` stops before writing anything rather than
    // guessing a project nobody chose.
    let none = esdev_in(&parent)
        .args(["create", TEMPLATES, "undecided", "--yes"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!none.status.success());
    assert!(
        stderr(&none).contains("no template chosen"),
        "{}",
        stderr(&none)
    );
    assert!(stderr(&none).contains("--template"), "{}", stderr(&none));
    assert!(
        !parent.join("undecided").exists(),
        "nothing was written: {}",
        stderr(&none)
    );

    // A package manager that is not one is named as such, before anything runs.
    let unknown = esdev_in(&parent)
        .args(["create", TEMPLATES, "nope", "--install=cargo"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("npm, bun, pnpm, yarn"),
        "{}",
        stderr(&unknown)
    );

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn create_lists_its_templates_and_names_one_it_does_not_have() {
    let dir = watch_dir("c_list");

    let listed = esdev_in(&dir)
        .args(["create", TEMPLATES, "--list"])
        .output()
        .expect("spawn esdev create");
    assert!(listed.status.success(), "{}", stderr(&listed));
    assert!(stdout(&listed).contains("react"), "{}", stdout(&listed));
    assert!(stdout(&listed).contains("micro-ui"), "{}", stdout(&listed));

    let unknown = esdev_in(&dir)
        .args(["create", TEMPLATES, "app", "--template=svelte"])
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("no svelte template"),
        "{}",
        stderr(&unknown)
    );
    // The error is also the list, so the next command is obvious.
    assert!(stderr(&unknown).contains("react"), "{}", stderr(&unknown));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The docs name the chosen manager: `--package-manager` renders into every
/// usage line, the default is npm, and an unknown name fails before writing.
#[test]
fn create_names_the_chosen_package_manager_in_its_docs() {
    let parent = watch_dir("c_pm");

    // pnpm chosen: the README and the next steps speak pnpm throughout.
    let pnpm = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "shop-pnpm",
            "--template=vanilla",
            "--styling=css",
            "--package-manager=pnpm",
            "--no-install",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(pnpm.status.success(), "{}", stderr(&pnpm));
    let readme = std::fs::read_to_string(parent.join("shop-pnpm").join("README.md")).expect("read");
    assert!(readme.contains("pnpm run dev"), "{readme}");
    assert!(!readme.contains("{{pm}}"), "a placeholder survived");
    assert!(
        !readme.contains("\nnpm "),
        "the default leaked in: {readme}"
    );
    let document =
        std::fs::read_to_string(parent.join("shop-pnpm").join("index.html")).expect("read");
    assert!(document.contains("<title>shop-pnpm</title>"), "{document}");
    assert!(
        stdout(&pnpm).contains("pnpm install"),
        "the next steps name it too: {}",
        stdout(&pnpm)
    );

    // Unsaid and unattended: npm, matching the install default.
    let plain = esdev_in(&parent)
        .args(["create", TEMPLATES, "shop-npm", "--template=api"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(plain.status.success(), "{}", stderr(&plain));
    let readme = std::fs::read_to_string(parent.join("shop-npm").join("README.md")).expect("read");
    assert!(readme.contains("npm run dev"), "{readme}");
    assert!(!readme.contains("{{pm}}"), "a placeholder survived");

    // An unknown name fails before writing, like a bad --install.
    let dir = parent.join("nope");
    let unknown = esdev_in(&parent)
        .args([
            "create",
            TEMPLATES,
            "nope",
            "--template=api",
            "--package-manager=cargo",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn esdev create");
    assert!(!unknown.status.success());
    assert!(
        stderr(&unknown).contains("npm, bun, pnpm, yarn"),
        "{}",
        stderr(&unknown)
    );
    assert!(!dir.exists(), "it wrote a project anyway");

    let _ = std::fs::remove_dir_all(&parent);
}

/// An option flag where it does not apply is refused, like a stray `--mode`
/// — and a value that is not one is refused with the values that are.
#[test]
fn an_option_flag_is_refused_where_it_does_not_apply() {
    let parent = watch_dir("c_option_flags");

    for (template, flag, message) in [
        ("api", "--language=ts", "has no language"),
        ("api", "--blog", "has no blog"),
        ("api", "--styling=tailwind", "has no styling"),
        ("vanilla", "--styling=sass", "no styling sass"),
        ("react", "--mode=spa", "no mode spa"),
    ] {
        let dir = parent.join(format!("refused-{template}"));
        let refused = esdev_in(&parent)
            .args([
                "create",
                TEMPLATES,
                dir.file_name().unwrap().to_str().unwrap(),
                &format!("--template={template}"),
                flag,
            ])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn esdev create");
        assert!(!refused.status.success(), "{template} {flag} was accepted");
        assert!(
            stderr(&refused).contains(message),
            "{template} {flag}: {}",
            stderr(&refused)
        );
        assert!(!dir.exists(), "it wrote a project anyway");
    }

    // The list names the templates, so the error above is actionable.
    let listed = esdev_in(&parent)
        .args(["create", TEMPLATES, "--list"])
        .output()
        .expect("spawn esdev create");
    for template in ["api", "vanilla", "react"] {
        assert!(
            stdout(&listed).contains(template),
            "{template} is not listed:\n{}",
            stdout(&listed)
        );
    }

    let _ = std::fs::remove_dir_all(&parent);
}
