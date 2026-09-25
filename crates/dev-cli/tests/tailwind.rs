//! End-to-end tests for Tailwind CSS in `esdev build`.
//!
//! CI installs no npm packages, so the project each test writes carries a
//! **stand-in `tailwindcss`**: a package with the real one's manifest shape
//! (`exports["."]` with `style` and `import`) and its `compile()` contract —
//! `loadStylesheet`, `loadModule`, `sources`, `root`, `features` and
//! `build(candidates)`. What is under test is esdev's half: finding the
//! package, loading what the compiler asks for, scanning the project, and
//! putting the result through the CSS pipeline. The real compiler's behaviour
//! is the real compiler's.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn project(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("tailwind-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create project");
    dir
}

fn write(dir: &Path, name: &str, contents: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
    std::fs::write(path, contents).expect("write file");
}

fn esdev(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esdev"));
    command.current_dir(dir);
    command
}

fn output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The one stylesheet a build wrote under `dir`'s `assets/`.
fn built_css(dir: &Path) -> String {
    let assets = dir.join("assets");
    let entry = std::fs::read_dir(&assets)
        .unwrap_or_else(|e| panic!("{}: {e}", assets.display()))
        .flatten()
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "css"))
        .expect("a stylesheet was written");
    std::fs::read_to_string(entry.path()).expect("read the stylesheet")
}

/// The stand-in compiler.
///
/// It resolves `@import "tailwindcss…"` through `loadStylesheet` (so the
/// output shows which file the package's `style` export named), loads each
/// `@plugin` through `loadModule`, reports `@source` lines as the real one
/// does, and generates one rule per candidate starting `tw-` — so a test can
/// read off exactly which candidates reached `build()`.
const STAND_IN: &str = r#"
export async function compile(css, { base, loadStylesheet, loadModule }) {
  let imported = "";
  for (const match of css.matchAll(/@import "(tailwindcss[^"]*)"[^;]*;/g)) {
    imported += (await loadStylesheet(match[1], base)).content;
  }
  const plugins = [];
  for (const match of css.matchAll(/@plugin "([^"]+)";/g)) {
    plugins.push((await loadModule(match[1], base, "plugin")).module);
  }
  if (css.includes("@apply broken")) {
    throw new Error("Cannot apply unknown utility class `broken`");
  }
  const sources = [...css.matchAll(/@source (not )?"([^"]+)";/g)].map((match) => ({
    base,
    pattern: match[2],
    negated: match[1] !== undefined,
  }));
  const rest = css
    .replace(/@import "tailwindcss[^"]*"[^;]*;/g, "")
    .replace(/@(source|plugin)[^;]*;/g, "");
  return {
    sources,
    root: css.includes("source(none)") ? "none" : null,
    features: css.includes("@import \"tailwindcss") ? 16 : 1,
    build(candidates) {
      const generated = candidates
        .filter((name) => name.startsWith("tw-"))
        .map((name) => `.${name} { --generated: 1 }`);
      for (const plugin of plugins) generated.push(plugin());
      return [imported, rest, ...generated].join("\n");
    },
  };
}
"#;

/// Installs the stand-in as `node_modules/tailwindcss`.
fn install_tailwind(dir: &Path, version: &str) {
    write(
        dir,
        "node_modules/tailwindcss/package.json",
        &format!(
            r#"{{
  "name": "tailwindcss",
  "version": "{version}",
  "exports": {{
    ".": {{ "types": "./lib.d.mts", "style": "./index.css", "import": "./lib.mjs" }},
    "./theme.css": "./theme.css"
  }}
}}"#
        ),
    );
    write(dir, "node_modules/tailwindcss/lib.mjs", STAND_IN);
    write(
        dir,
        "node_modules/tailwindcss/index.css",
        "/* the package's style export */\n",
    );
}

/// A document target over `index.html`, which links `app.css`.
fn html_project(dir: &Path, css: &str) {
    write(dir, "package.json", r#"{ "name": "app", "private": true }"#);
    write(
        dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
    );
    write(
        dir,
        "index.html",
        "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./app.css\"></head>\
         <body class=\"tw-from-html\"><script type=\"module\" src=\"./src/main.js\"></script>\
         </body></html>\n",
    );
    write(
        dir,
        "src/main.js",
        "document.body.className = [\"tw-from-js\", \"tw-in-array\"].join(\" \");\n",
    );
    write(dir, "app.css", css);
}

/// The whole path: the package found, its `style` export loaded for
/// `@import "tailwindcss"`, the project scanned (skipping what `.gitignore`
/// skips), and the result through the CSS pipeline — `url()` included.
#[test]
fn a_linked_stylesheet_is_compiled_with_the_projects_tailwind() {
    let dir = project("linked");
    install_tailwind(&dir, "4.3.3");
    html_project(
        &dir,
        "@import \"tailwindcss\";\n.card { background: url(\"./bg.png\") }\n",
    );
    write(&dir, "bg.png", "png");
    write(&dir, ".gitignore", "generated/\n");
    write(&dir, "generated/out.js", "\"tw-from-ignored\"");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", output(&out));
    let css = built_css(&dir.join("dist"));

    assert!(css.contains("the package's style export"), "{css}");
    for used in ["tw-from-html", "tw-from-js", "tw-in-array"] {
        assert!(css.contains(&format!(".{used} ")), "{used} missing:\n{css}");
    }
    assert!(!css.contains("tw-from-ignored"), "{css}");
    assert!(css.contains(".card"), "{css}");
    assert!(css.contains("url(\"/assets/bg-"), "{css}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `@source` reaches a path detection skips — a library in `node_modules` —
/// and `@source not` takes one away. Both are written in an `@import`ed file,
/// and resolve against *that* file.
#[test]
fn source_directives_add_and_remove_paths() {
    let dir = project("sources");
    install_tailwind(&dir, "4.3.3");
    html_project(
        &dir,
        "@import \"tailwindcss\";\n@import \"./styles/sources.css\";\n",
    );
    write(
        &dir,
        "styles/sources.css",
        "@source \"../node_modules/ui-kit\";\n@source not \"../src/legacy\";\n",
    );
    write(&dir, "node_modules/ui-kit/button.js", "\"tw-from-library\"");
    write(&dir, "src/legacy/old.js", "\"tw-from-legacy\"");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", output(&out));
    let css = built_css(&dir.join("dist"));
    assert!(css.contains(".tw-from-library "), "{css}");
    assert!(!css.contains("tw-from-legacy"), "{css}");
    assert!(css.contains(".tw-from-js "), "{css}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `source(none)` turns detection off: only what `@source` names is read.
#[test]
fn source_none_scans_only_what_is_named() {
    let dir = project("source-none");
    install_tailwind(&dir, "4.3.3");
    html_project(
        &dir,
        "@import \"tailwindcss\" source(none);\n@source \"./src/only\";\n",
    );
    write(&dir, "src/only/a.js", "\"tw-named\"");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", output(&out));
    let css = built_css(&dir.join("dist"));
    assert!(css.contains(".tw-named "), "{css}");
    assert!(!css.contains("tw-from-js"), "{css}");
    assert!(!css.contains("tw-from-html"), "{css}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stylesheet a module imports goes through the same compiler, and a
/// `@plugin` beside it is loaded relative to the file that named it.
#[test]
fn an_imported_stylesheet_and_its_plugin_are_compiled() {
    let dir = project("imported");
    install_tailwind(&dir, "4.3.3");
    write(
        &dir,
        "package.json",
        r#"{ "name": "app", "private": true }"#,
    );
    write(
        &dir,
        "esdev.json",
        r#"{ "targets": { "web": { "entry": "web/index.html", "outdir": "dist" } } }"#,
    );
    write(
        &dir,
        "web/index.html",
        "<!doctype html><html><head></head><body>\
         <script type=\"module\" src=\"./main.js\"></script></body></html>\n",
    );
    write(
        &dir,
        "web/main.js",
        "import \"./styles/global.css\";\ndocument.body.className = \"tw-imported\";\n",
    );
    write(
        &dir,
        "web/styles/global.css",
        "@import \"tailwindcss\";\n@plugin \"./tab.js\";\n",
    );
    write(
        &dir,
        "web/styles/tab.js",
        "export default () => \".tab-4 { tab-size: 4 }\";\n",
    );

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", output(&out));
    let css = built_css(&dir.join("dist"));
    assert!(css.contains(".tw-imported "), "{css}");
    assert!(css.contains(".tab-4 { tab-size: 4 }"), "{css}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stylesheet named on the command line is an entry, and Tailwind's too.
#[test]
fn a_stylesheet_entry_is_compiled() {
    let dir = project("entry");
    install_tailwind(&dir, "4.3.3");
    html_project(&dir, "@import \"tailwindcss\";\n");

    let out = esdev(&dir)
        .args(["build", "app.css", "--out=out/app.css"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", output(&out));
    let css = std::fs::read_to_string(dir.join("out/app.css")).expect("the entry's output");
    assert!(css.contains(".tw-from-js "), "{css}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A compile error is the compiler's message against the stylesheet, and the
/// build writes nothing.
#[test]
fn a_compile_error_fails_the_build_with_the_compilers_message() {
    let dir = project("error");
    install_tailwind(&dir, "4.3.3");
    html_project(&dir, "@import \"tailwindcss\";\n.x { @apply broken; }\n");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    let text = output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("cannot compile app.css with Tailwind"),
        "{text}"
    );
    assert!(
        text.contains("Cannot apply unknown utility class `broken`"),
        "{text}"
    );
    // Only the message: no stack through esdev's plumbing.
    assert!(!text.contains("host.js"), "{text}");
    assert!(!dir.join("dist").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stylesheet that uses Tailwind in a project that has not installed it is
/// told what to install, rather than shipping `@import "tailwindcss"` to a
/// browser that would fetch it as a URL.
#[test]
fn a_project_without_tailwind_is_told_to_install_it() {
    let dir = project("missing");
    html_project(&dir, "@import \"tailwindcss\";\n");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    let text = output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("`tailwindcss` is not installed"), "{text}");
    assert!(text.contains("npm install -D tailwindcss"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tailwind v3 is refused by name: its configuration is a JavaScript file run
/// through PostCSS, which esdev does not run.
#[test]
fn tailwind_v3_is_refused() {
    let dir = project("v3");
    install_tailwind(&dir, "3.4.17");
    html_project(&dir, "@tailwind utilities;\n");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    let text = output(&out);
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("3.4.17"), "{text}");
    assert!(text.contains("tailwindcss@4"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plain stylesheet does not start the compiler, and does not need
/// `tailwindcss` to be installed.
#[test]
fn plain_css_needs_no_tailwind() {
    let dir = project("plain");
    html_project(&dir, "body { color: red }\n");

    let out = esdev(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", output(&out));
    let css = built_css(&dir.join("dist"));
    assert!(css.contains("color: red"), "{css}");
    let _ = std::fs::remove_dir_all(&dir);
}
