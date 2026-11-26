//! An `index.html` target (D61).

use super::*;

/// A document that references one module, one stylesheet, one image, one CDN
/// URL it does not own, and an inline script nobody should touch.
fn html_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/dep.mjs", "export const answer = 42;\n");
    write_in(
        &dir,
        "src/entry.client.mjs",
        "import { answer } from './dep.mjs';\nconsole.log(answer);\n",
    );
    write_in(&dir, "styles.css", "body{color:red}\n");
    write_in(&dir, "logo.svg", "<svg/>\n");
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<title>My App</title>
<link rel="stylesheet" href="./styles.css">
<link rel="icon" href="./logo.svg">
<script>window.__EARLY__ = 1;</script>
<script type="module" src="./src/entry.client.mjs"></script>
<script src="https://cdn.example.com/analytics.js"></script>
</head><body><div id="root"></div></body></html>
"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );
    dir
}

#[test]
fn an_html_target_builds_what_it_references_and_rewrites_it() {
    let dir = html_project("h_build");
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let document = std::fs::read_to_string(dir.join("dist/index.html")).expect("read the document");

    // The module script became a hashed bundle under /assets, and the document
    // points at it. The name is the bundler's, so it is read back out of the
    // document rather than guessed.
    let script = document
        .split_once(r#"<script type="module" src=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(url, _)| url.to_string())
        .expect("a module script survived");
    assert!(script.starts_with("/assets/entry.client-"), "{script}");
    assert!(script.ends_with(".js"), "{script}");
    // The URL is rooted at the deployment, which is the output directory.
    let bundle = dir.join("dist").join(script.trim_start_matches('/'));
    assert!(bundle.exists(), "{script} was not written");
    let code = std::fs::read_to_string(&bundle).expect("read bundle");
    assert!(!code.contains("./dep.mjs"), "the import survived:\n{code}");

    // The stylesheet and the icon were copied and hashed.
    for (attribute, prefix, suffix) in [
        ("href=\"/assets/styles-", "styles-", ".css"),
        ("href=\"/assets/logo-", "logo-", ".svg"),
    ] {
        assert!(document.contains(attribute), "{document}");
        let copied = std::fs::read_dir(dir.join("dist/assets"))
            .expect("read assets")
            .flatten()
            .any(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.starts_with(prefix) && name.ends_with(suffix)
            });
        assert!(copied, "no {prefix}…{suffix} in dist/assets");
    }

    // Everything else is the author's.
    assert!(document.contains("<title>My App</title>"), "{document}");
    assert!(document.contains("window.__EARLY__ = 1;"), "{document}");
    assert!(document.contains(r#"<html lang="en">"#), "{document}");
    assert!(
        document.contains(r#"src="https://cdn.example.com/analytics.js""#),
        "a URL this build does not own was rewritten:\n{document}"
    );
}

/// The hash follows the content, which is the whole reason it is there — a
/// deployment caches `/assets` immutably, and a file whose name did not change
/// is a file the browser will not fetch again.
#[test]
fn a_changed_file_gets_a_changed_name() {
    let dir = html_project("h_hash");
    let build = || {
        let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
        assert!(out.status.success(), "{}", stderr(&out));
        std::fs::read_to_string(dir.join("dist/index.html")).expect("read the document")
    };

    let before = build();
    write_in(&dir, "styles.css", "body{color:blue}\n");
    let after = build();
    assert_ne!(before, after, "the stylesheet changed and its URL did not");
}

/// A stylesheet the document links **and** the JavaScript imports is one file.
///
/// The link is where the author put it; the import put the same rules in the
/// bundle's collected sheet, and the build wrote both — two content-hashed
/// copies of one stylesheet, both `<link>`ed, so a browser fetched it twice.
#[test]
fn a_stylesheet_that_is_linked_and_imported_is_written_once() {
    let dir = build_dir("h_css_both");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/style.css", "body { color: red }\n");
    write_in(&dir, "src/other.css", ".other { color: blue }\n");
    write_in(
        &dir,
        "src/main.js",
        "import './style.css';\nimport './other.css';\n",
    );
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><link rel="stylesheet" href="./src/style.css"><script type="module" src="./src/main.js"></script></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    // The files whose CSS carries a given rule, by name. A rule in two files is
    // the same stylesheet written twice.
    let assets: Vec<PathBuf> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let carrying = |needle: &str| -> Vec<String> {
        let mut found: Vec<String> = assets
            .iter()
            .filter(|path| path.extension().is_some_and(|e| e == "css"))
            .filter(|path| {
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .contains(needle)
            })
            .map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        found.sort();
        found
    };

    // The linked stylesheet's rules are in exactly one file — the document's
    // own copy, not also a second sheet the bundle's collector produced.
    let linked = carrying("color: red");
    assert_eq!(
        linked.len(),
        1,
        "the linked stylesheet was written {linked:?}"
    );
    // The stylesheet only the JavaScript imports still reaches the output.
    let imported = carrying("color: blue");
    assert_eq!(imported.len(), 1, "imported-only stylesheet is missing");
}

/// A stylesheet is an entry, not a file to copy: what the document ends up
/// pointing at is the whole tree, with every `url()` aimed at where the file it
/// named actually landed.
///
/// The unit tests in `css.rs` cover the bundling; what is worth an end-to-end
/// test is the wiring around it, because both halves fail silently. A
/// placeholder that is never substituted is a stylesheet full of opaque hashes,
/// and a hash computed before substitution is a URL that never changes.
#[test]
fn a_stylesheet_is_bundled_with_what_it_imports_and_references() {
    let dir = build_dir("h_css");
    std::fs::create_dir_all(dir.join("theme")).expect("create theme");
    write_in(
        &dir,
        "styles.css",
        "@import \"./theme/dark.css\";\nbody { color: var(--ink) }\n",
    );
    write_in(
        &dir,
        "theme/dark.css",
        ":root { --ink: #eee }\nbody { background: url(./grain.png) }\n",
    );
    write_in(&dir, "theme/grain.png", "not really a png\n");
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><link rel="stylesheet" href="./styles.css"></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );

    let build = || {
        let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
        assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
        let document =
            std::fs::read_to_string(dir.join("dist/index.html")).expect("read the document");
        let url = document
            .split_once(r#"<link rel="stylesheet" href=""#)
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(url, _)| url.to_string())
            .expect("the stylesheet survived");
        let css = std::fs::read_to_string(dir.join("dist").join(url.trim_start_matches('/')))
            .expect("read the stylesheet");
        (url, css)
    };

    let (url, css) = build();
    assert!(url.starts_with("/assets/styles-"), "{url}");

    // The import is gone because its contents are here.
    assert!(!css.contains("@import"), "the import survived:\n{css}");
    assert!(css.contains("--ink"), "the import was not inlined:\n{css}");

    // The `url()` names where the file landed — rooted, hashed, and written.
    let referenced = css
        .split_once("url(")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(url, _)| url.trim_matches(['"', '\'']).to_string())
        .expect("a url() survived");
    assert!(
        referenced.starts_with("/assets/grain-") && referenced.ends_with(".png"),
        "the placeholder was never substituted: {referenced}"
    );
    assert!(
        dir.join("dist")
            .join(referenced.trim_start_matches('/'))
            .is_file(),
        "{referenced} was not written"
    );

    // The name follows the content of the *bundle*, so editing an imported file
    // — which the entry's own bytes know nothing about — still busts the cache.
    write_in(&dir, "theme/dark.css", ":root { --ink: #111 }\n");
    let (changed, _) = build();
    assert_ne!(url, changed, "an imported file changed and the URL did not");
}

/// A stylesheet can be the command-line entry too. Previously it travelled
/// through the JavaScript CSS-import adapter and `rolldown` faithfully wrote
/// the adapter's empty module as a zero-byte `.js` file.
#[test]
fn a_css_entry_bundles_its_imports_to_a_stylesheet() {
    let dir = build_dir("css_entry");
    write_in(
        &dir,
        "styles.css",
        "@import './theme.css';\nbody { color: #ffffff }",
    );
    write_in(&dir, "theme.css", ":root { margin: 0px 0px 0px 0px }");

    let out = esdev_in(&dir)
        .args(["build", "styles.css", "--minify"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let css = std::fs::read_to_string(dir.join("dist/styles.css")).expect("read stylesheet");
    assert!(!css.contains("@import"), "{css}");
    assert_eq!(css, ":root{margin:0}body{color:#fff}");
    assert!(
        !dir.join("dist/styles.js").exists(),
        "wrote a JavaScript shell"
    );
}

#[test]
fn a_library_keeps_css_at_its_exports_map_path() {
    let dir = build_dir("lib_css");
    std::fs::create_dir_all(dir.join("src")).expect("create source");
    write_in(&dir, "src/index.ts", "export const answer: number = 42;\n");
    write_in(
        &dir,
        "src/styles.css",
        "@import './theme.css';\n.button { color: red }\n",
    );
    write_in(&dir, "src/theme.css", ":root { --brand: #123456 }\n");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/styles.css")).expect("fixed stylesheet"),
        "@import './theme.css';\n.button { color: red }\n"
    );
    assert!(
        dir.join("dist/theme.css").is_file(),
        "keeps imported sibling"
    );
}

/// A component that imports its stylesheet (D156): the library ships the
/// `.css` beside the modules and drops the import from its JavaScript — which
/// would fail anywhere but a CSS-aware bundler — without leaving the empty
/// module the stylesheet became. A `package.json` whose `exports` cannot reach
/// the stylesheets is warned about, naming the line to add.
#[test]
fn a_library_drops_its_css_imports_and_warns_when_exports_miss_them() {
    let dir = build_dir("lib_css_imports");
    std::fs::create_dir_all(dir.join("src/parts")).expect("create source");
    write_in(
        &dir,
        "src/button.js",
        "import './button.css';\nexport const Button = () => 'b';\n",
    );
    write_in(&dir, "src/button.css", ".btn { color: red }\n");
    write_in(
        &dir,
        "src/parts/card.js",
        "import './card.css';\nexport const Card = () => 'c';\n",
    );
    write_in(&dir, "src/parts/card.css", ".card { color: blue }\n");
    write_in(
        &dir,
        "src/index.js",
        "export * from './button.js';\nexport * from './parts/card.js';\n",
    );
    let build = |manifest: &str| {
        write_in(&dir, "package.json", manifest);
        let _ = std::fs::remove_dir_all(dir.join("dist"));
        let out = esdev_in(&dir)
            .args(["build", "--lib", "src", "--format=esm,cjs", "--no-types"])
            .output()
            .expect("spawn esdev");
        assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
        format!("{}{}", stdout(&out), stderr(&out))
    };

    let text =
        build(r#"{ "name": "kit", "type": "module", "exports": { ".": "./dist/index.js" } }"#);
    for module in ["button.js", "button.cjs", "parts/card.js", "parts/card.cjs"] {
        let code = std::fs::read_to_string(dir.join("dist").join(module)).expect("module");
        assert!(
            !code.contains(".css"),
            "{module} still imports its stylesheet:\n{code}"
        );
    }
    let mut files: Vec<String> = Vec::new();
    for entry in walk(&dir.join("dist")) {
        files.push(es_runtime_default_providers::path::to_slash(
            entry.strip_prefix(dir.join("dist")).unwrap(),
        ));
    }
    files.sort();
    assert_eq!(
        files,
        [
            "button.cjs",
            "button.css",
            "button.js",
            "index.cjs",
            "index.js",
            "parts/card.cjs",
            "parts/card.css",
            "parts/card.js",
        ],
        "no stub module for a stylesheet"
    );
    assert!(text.contains("3 modules"), "{text}");
    assert!(
        text.contains(r#"warning: these stylesheets are not in package.json "exports", so a consumer cannot import them: dist/button.css, dist/parts/card.css"#),
        "{text}"
    );
    assert!(
        text.contains(r#"Add to "exports": "./*.css": "./dist/*.css""#),
        "{text}"
    );

    // Exported: quiet. No `exports` at all: everything is importable, quiet.
    let text = build(
        r#"{ "name": "kit", "exports": { ".": "./dist/index.js", "./*.css": "./dist/*.css" } }"#,
    );
    assert!(!text.contains("warning:"), "{text}");
    let text = build(r#"{ "name": "kit" }"#);
    assert!(!text.contains("warning:"), "{text}");
}

/// A library's CSS Modules ship scoped (D157): the class names in the
/// published stylesheet are the ones its JavaScript hands out, a composed
/// module is `@import`ed so its rules come along, and a `url()` names the
/// copied file. A plain stylesheet is still the passthrough copy.
#[test]
fn a_library_ships_its_css_modules_scoped_as_its_javascript_names_them() {
    let dir = build_dir("lib_css_modules");
    std::fs::create_dir_all(dir.join("src/ui")).expect("create source");
    write_in(&dir, "src/ui/base.module.css", ".base { padding: 1px }\n");
    write_in(
        &dir,
        "src/ui/button.module.css",
        ".btn { composes: base from './base.module.css'; color: red; background: url(./icon.png) }\n",
    );
    write_in(&dir, "src/ui/icon.png", "png");
    write_in(&dir, "src/plain.css", ".plain { color: blue }\n");
    write_in(
        &dir,
        "src/ui/button.js",
        "import styles from './button.module.css';\nexport const className = styles.btn;\n",
    );
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--format=esm,cjs", "--no-types"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let js =
        std::fs::read_to_string(dir.join("dist/ui/button.module.css.js")).expect("names module");
    let names = js
        .split("\"btn\": \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no btn mapping in:\n{js}"));
    let (btn, base) = names.split_once(' ').expect("btn composes base");
    let button =
        std::fs::read_to_string(dir.join("dist/ui/button.module.css")).expect("button css");
    let base_css = std::fs::read_to_string(dir.join("dist/ui/base.module.css")).expect("base css");
    assert!(button.contains(&format!(".{btn}")), "{button}");
    assert!(!button.contains(".btn "), "unscoped name left:\n{button}");
    assert!(
        button.starts_with("@import \"./base.module.css\";"),
        "{button}"
    );
    // The image goes where the stylesheets' files go (D158): hashed, in assets/.
    assert!(button.contains("url(\"../assets/icon-"), "{button}");
    assert!(!button.contains("composes"), "{button}");
    assert!(base_css.contains(&format!(".{base}")), "{base_css}");
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/plain.css")).expect("plain"),
        ".plain { color: blue }\n"
    );
}

/// A library stylesheet's `url()`s (D158): copied under a hashed name into
/// `assets/` by default, inlined as `data:` URLs with `--css-urls=inline` or
/// the target's `cssUrls`. The rest of the stylesheet is as written.
#[test]
fn a_library_copies_or_inlines_what_its_stylesheets_url_names() {
    let dir = build_dir("lib_css_urls");
    std::fs::create_dir_all(dir.join("src/ui/img")).expect("create source");
    write_in(&dir, "src/ui/img/bg.png", "png");
    write_in(&dir, "src/index.js", "export const x = 1;\n");
    write_in(
        &dir,
        "src/ui/s.css",
        "@import url(\"../theme.css\");\n.a { background: url(./img/bg.png) }\n",
    );
    write_in(&dir, "src/theme.css", ":root{}\n");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--no-types"])
        .output()
        .expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 file from stylesheets copied"), "{text}");
    let copied: Vec<String> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("dist/assets")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect();
    assert!(
        copied.len() == 1 && copied[0].starts_with("bg-") && copied[0].ends_with(".png"),
        "{copied:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/ui/s.css")).expect("css"),
        format!(
            "@import url(\"../theme.css\");\n.a {{ background: url(\"../assets/{}\") }}\n",
            copied[0]
        )
    );

    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"kit": {"entry": "src", "lib": true, "outDir": "dist", "types": false, "cssUrls": "inline"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("1 file from stylesheets inlined"), "{text}");
    assert!(!dir.join("dist/assets").exists(), "inlined, yet copied");
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/ui/s.css")).expect("css"),
        "@import url(\"../theme.css\");\n.a { background: url(\"data:image/png;base64,cG5n\") }\n"
    );

    let out = esdev_in(&dir)
        .args(["build", "src/index.js", "--css-urls=copy"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--css-urls only means something with --lib"),
        "{}",
        stderr(&out)
    );
}

/// CSS Modules: a stylesheet the *JavaScript* imports, rather than one the
/// document links.
///
/// The end-to-end property is the one the unit tests cannot reach — that the
/// name the bundle uses and the name the stylesheet declares are the same
/// string, and that two files declaring the same class do not collide.
#[test]
fn a_css_module_is_scoped_and_reaches_both_the_bundle_and_a_stylesheet() {
    let dir = build_dir("h_cssmod");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/Button.module.css",
        ".button { color: red }
:global(.no-js) .button { color: grey }
",
    );
    // A second file with the *same* local name: the whole point of scoping.
    write_in(
        &dir,
        "src/Card.module.css",
        ".button { color: blue }
",
    );
    write_in(
        &dir,
        "src/main.js",
        "import button from './Button.module.css';
         import card from './Card.module.css';
         document.body.className = button.button + ' ' + card.button;
",
    );
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><title>t</title><script type="module" src="./src/main.js"></script></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    // The document links a stylesheet nothing in it referenced — the build
    // wrote it from what the JavaScript imported.
    let document = std::fs::read_to_string(dir.join("dist/index.html")).expect("read document");
    let href = document
        .split_once(r#"<link rel="stylesheet" href=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(url, _)| url.to_string())
        .expect("a stylesheet was linked");
    assert!(href.starts_with("/assets/modules-"), "{href}");
    let css = std::fs::read_to_string(dir.join("dist").join(href.trim_start_matches('/')))
        .expect("read the stylesheet");

    // Two files, one local name, two scoped names — and both are in the CSS.
    let unique: std::collections::BTreeSet<&str> = css
        .match_indices(".button_")
        .map(|(at, _)| {
            let from = at + 1;
            let len = css[from..]
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(css.len() - from);
            &css[from..from + len]
        })
        .collect();
    assert_eq!(unique.len(), 2, "expected two scoped names, got {unique:?}");

    // …and the bundle uses exactly those strings, or the markup would name a
    // class the stylesheet never declared.
    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("main") && n.ends_with(".js"))
        })
        .expect("a bundle");
    let code = std::fs::read_to_string(&bundle).expect("read bundle");
    for name in &unique {
        assert!(
            code.contains(name),
            "{name} is not in the bundle:
{code}"
        );
    }

    // `:global()` is a convention of this build, not a selector any browser
    // knows: the wrapper has to be gone and its contents left alone.
    assert!(
        !css.contains(":global"),
        "the wrapper survived:
{css}"
    );
    assert!(css.contains(".no-js"), "{css}");
}

/// `composes`, and a plain stylesheet imported from JavaScript.
///
/// The properties worth an end-to-end test are the two that need the module
/// graph: that a composed module's rules reach the output even though nothing
/// imported it, and that composition is transitive — a class only styles an
/// element that actually carries it, so a chain that stops halfway loses the
/// middle link's styling and nothing reports it.
#[test]
fn composes_is_transitive_and_a_plain_stylesheet_is_imported_whole() {
    let dir = build_dir("h_composes");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("vendor")).expect("create vendor");

    // Nothing imports this module; only `composes` names it.
    write_in(
        &dir,
        "src/base.module.css",
        ".rounded { border-radius: 8px }\n",
    );
    write_in(
        &dir,
        "src/Button.module.css",
        ".button { composes: rounded from \"./base.module.css\"; color: white }\n         .big { composes: button; font-size: 2rem }\n",
    );
    // A third-party stylesheet: its own JS emits these names, so scoping them
    // would rename half of a contract the library has with itself.
    write_in(
        &dir,
        "vendor/lib.css",
        ".lib-widget { outline: 2px solid green }\n",
    );
    write_in(
        &dir,
        "src/main.js",
        "import '../vendor/lib.css';\n         import styles from './Button.module.css';\n         document.body.className = styles.big;\n",
    );
    write_in(
        &dir,
        "index.html",
        r#"<!doctype html><html><head><title>t</title><script type="module" src="./src/main.js"></script></head><body></body></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let document = std::fs::read_to_string(dir.join("dist/index.html")).expect("read document");
    let href = document
        .split_once(r#"<link rel="stylesheet" href=""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(url, _)| url.to_string())
        .expect("a stylesheet was linked");
    let css = std::fs::read_to_string(dir.join("dist").join(href.trim_start_matches('/')))
        .expect("read the stylesheet");

    // The composed module's rules are there even though no JavaScript imported
    // it — without them, `composes` hands out a class name that styles nothing.
    assert!(
        css.contains("border-radius"),
        "the composed module is missing:\n{css}"
    );
    // `composes` is not a property any browser knows; it must be gone.
    assert!(!css.contains("composes"), "{css}");
    // A vendor stylesheet is emitted unscoped, or its own JS stops matching it.
    assert!(css.contains(".lib-widget"), "{css}");

    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("main") && n.ends_with(".js"))
        })
        .expect("a bundle");
    let code = std::fs::read_to_string(&bundle).expect("read bundle");

    // `.big` composes `.button`, which composes `.rounded` — three names, and
    // every one of them declared in the stylesheet.
    let big = code
        .split_once(r#""big": ""#)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.to_string())
        .expect("a mapping for `big`");
    let names: Vec<&str> = big.split(' ').collect();
    assert_eq!(names.len(), 3, "not transitive: {big}");
    for name in names {
        assert!(css.contains(name), "{name} is not declared in:\n{css}");
    }
}

/// A relative path names a file in the project. If it is not there, that is a
/// broken page, and the build is where it should be found — not the browser.
#[test]
fn a_reference_that_is_not_there_stops_the_build() {
    let dir = build_dir("h_missing");
    write_in(
        &dir,
        "index.html",
        r#"<html><head><script type="module" src="./src/gone.mjs"></script></head></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(stderr(&out).contains("./src/gone.mjs"), "{}", stderr(&out));
    assert!(stderr(&out).contains("not there"), "{}", stderr(&out));
}

/// Two entries built to one name is a build that silently ships half of what
/// the page asked for.
#[test]
fn two_module_scripts_that_would_collide_are_refused() {
    let dir = build_dir("h_collide");
    std::fs::create_dir_all(dir.join("a")).expect("create a");
    std::fs::create_dir_all(dir.join("b")).expect("create b");
    write_in(&dir, "a/main.mjs", "console.log('a');\n");
    write_in(&dir, "b/main.mjs", "console.log('b');\n");
    write_in(
        &dir,
        "index.html",
        r#"<html><head>
           <script type="module" src="./a/main.mjs"></script>
           <script type="module" src="./b/main.mjs"></script>
           </head></html>"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("both called main"),
        "{}",
        stderr(&out)
    );
}
