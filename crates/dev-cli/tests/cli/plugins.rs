//! `esdev.json`'s `plugins`: a project's own passes, in the subcommand's build.

use super::*;

/// The plugin the tests below load: a virtual module whose contents come from
/// the options the config passed to the factory. A factory rather than a plain
/// object on purpose — the call is the thing a JSON config cannot make for
/// itself.
fn banner_plugin(dir: &Path) {
    write_in(
        dir,
        "plugin.mjs",
        r#"
export default function banner(options) {
  const text = options?.text ?? "NO OPTIONS";
  return {
    name: "banner",
    resolve: {
      filter: { id: "virtual:banner" },
      handler: () => ({ id: "virtual:banner", virtual: true }),
    },
    load: {
      filter: { id: "virtual:banner" },
      handler: () => ({ code: `export default ${JSON.stringify(text)};` }),
    },
  };
}
"#,
    );
}

/// The whole feature: the config names a module, esdev imports it, calls the
/// factory with the options the file carried, and the plugin serves a module
/// that exists on no disk — for **every** target, because a project's plugins
/// are the project's.
#[test]
fn build_loads_the_plugins_the_project_config_names() {
    let dir = build_dir("p_plugins");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    banner_plugin(&dir);
    write_in(
        &dir,
        "src/server.mjs",
        "import banner from 'virtual:banner';\nconsole.log('server', banner);\n",
    );
    write_in(
        &dir,
        "src/client.mjs",
        "import banner from 'virtual:banner';\nconsole.log('client', banner);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": [{"module": "./plugin.mjs", "options": {"text": "FROM THE CONFIG"}}], "build": {"targets": {"server": {"entry": "src/server.mjs", "out": "dist/server.js"}, "browser": {"entry": "src/client.mjs", "outDir": "dist/client", "platform": "browser"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let server = std::fs::read_to_string(dir.join("dist/server.js")).expect("the server bundle");
    assert!(server.contains("FROM THE CONFIG"), "{server}");
    let client =
        std::fs::read_to_string(dir.join("dist/client/client.js")).expect("the browser bundle");
    assert!(client.contains("FROM THE CONFIG"), "{client}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// One `plugins` entry may be several plugins (D214): a package offers one
/// factory and returns the plugins it is made of. Each keeps its own hooks,
/// and together they take the entry's place — after the entry before it and
/// before the one after. `null` and `false` are skipped, nested lists
/// flatten, and an entry that gives no plugins at all is not an error.
#[test]
fn a_plugin_entry_may_return_several_plugins() {
    let dir = build_dir("p_plugins_several");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "trail.mjs",
        r#"
const step = (name) => ({
  name,
  transform: {
    filter: { id: /app\.mjs$/ },
    handler: (code) => ({ code: code.replace(/"trail([^"]*)"/, `"trail$1>${name}"`) }),
  },
});
export default function preset({ extra }) {
  return [step("a"), [null, false, step("b")], extra && step("x")];
}
export const one = step("c");
export const none = () => [];
"#,
    );
    write_in(&dir, "src/app.mjs", "console.log(\"trail\");\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": [{"module": "./trail.mjs", "options": {"extra": false}}, {"module": "./trail.mjs", "export": "none"}, {"module": "./trail.mjs", "export": "one"}], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    assert!(bundle.contains("trail>a>b>c"), "{bundle}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// What an entry gives must be a plugin, a list of them or nothing. Anything
/// else is named against the entry that gave it, not the list it landed in.
#[test]
fn a_plugin_entry_that_gives_no_plugin_is_named() {
    let dir = build_dir("p_plugins_not_one");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "odd.mjs", "export default () => \"mdx\";\n");
    write_in(&dir, "src/app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./odd.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains(
            "./odd.mjs: its default export gave string, not a plugin or a list of plugins"
        ),
        "{text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Where a build runs, as a plugin reads it (D148): one declaration, and each
/// target's build tells the plugin its command, platform and name. This is how
/// a framework compiles for the browser and the server from one `plugins`
/// entry without esdev knowing what either means to it.
fn site_plugin(dir: &Path) {
    write_in(
        dir,
        "site.mjs",
        r#"
export default {
  name: "site",
  resolve: {
    filter: { id: "virtual:site" },
    handler: () => ({ id: "virtual:site", virtual: true }),
  },
  load: {
    filter: { id: "virtual:site" },
    handler: (id, ctx) => ({
      code: `export default ${JSON.stringify(`${ctx.command}/${ctx.platform}/${ctx.target}/${ctx.hot}`)};`,
    }),
  },
};
"#,
    );
}

#[test]
fn a_plugin_reads_where_each_build_runs() {
    let dir = build_dir("p_plugins_site");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    site_plugin(&dir);
    write_in(
        &dir,
        "src/app.mjs",
        "import site from 'virtual:site';\nconsole.log(site);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./site.mjs"], "build": {"targets": {"api": {"entry": "src/app.mjs", "out": "dist/api.js"}, "web": {"entry": "src/app.mjs", "outDir": "dist/web", "platform": "browser"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let api = std::fs::read_to_string(dir.join("dist/api.js")).expect("api");
    assert!(api.contains("build/server/api/false"), "{api}");
    let web = std::fs::read_to_string(dir.join("dist/web/app.js")).expect("web");
    assert!(web.contains("build/browser/web/false"), "{web}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for a Sass plugin: it compiles `.scss` to CSS (variables only)
/// and says so with `type: "css"`, which is all a stylesheet language has to
/// do for esdev to bundle, scope and link what it made (D149).
fn fake_sass_plugin(dir: &Path) {
    write_in(
        dir,
        "sass.mjs",
        r#"
export default {
  name: "fake-sass",
  transform: {
    filter: { id: /\.scss$/ },
    handler(code) {
      const vars = {};
      const body = code.replace(/^\$(\w+):\s*([^;]+);\s*$/gm, (_, name, value) => {
        vars[name] = value;
        return "";
      });
      return { code: body.replace(/\$(\w+)/g, (_, name) => vars[name]), type: "css" };
    },
  },
};
"#,
    );
}

#[test]
fn a_plugin_compiled_stylesheet_is_bundled_scoped_and_linked() {
    let dir = build_dir("p_css_sass");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    fake_sass_plugin(&dir);
    write_in(
        &dir,
        "src/card.module.scss",
        "$accent: rebeccapurple;\n.card { color: $accent }\n",
    );
    write_in(
        &dir,
        "src/theme.scss",
        "$page: papayawhip;\nbody { background: $page }\n",
    );
    write_in(
        &dir,
        "src/main.js",
        "import styles from './card.module.scss';\ndocument.body.className = styles.card;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <link rel=\"stylesheet\" href=\"./src/theme.scss\">\
         <script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./sass.mjs"], "build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let written: Vec<PathBuf> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("assets")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    let css: String = written
        .iter()
        .filter(|path| path.extension().is_some_and(|e| e == "css"))
        .map(|path| std::fs::read_to_string(path).expect("css"))
        .collect();
    assert!(css.contains("papayawhip"), "the linked sheet: {css}");
    assert!(css.contains("rebeccapurple"), "the imported module: {css}");
    assert!(!css.contains('$'), "uncompiled: {css}");
    assert!(
        !written
            .iter()
            .any(|path| path.extension().is_some_and(|e| e == "scss")),
        "a .scss was copied as it was: {written:?}"
    );
    let bundle: String = written
        .iter()
        .filter(|path| path.extension().is_some_and(|e| e == "js"))
        .map(|path| std::fs::read_to_string(path).expect("js"))
        .collect();
    assert!(bundle.contains("card_"), "no scoped name: {bundle}");
    let html = std::fs::read_to_string(dir.join("dist/index.html")).expect("html");
    assert!(html.contains(".css\""), "the link: {html}");

    // And under test, where the same plugin compiles the same module.
    write_in(
        &dir,
        "src/card.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         import styles from './card.module.scss';\n\
         test('scoped', () => assert(styles.card.startsWith('card_'), styles.card));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// An edit to a plugin — here a module the plugin imports — reloads the
/// plugins before the rebuild. The host is kept across ordinary saves for the
/// state it holds, and so used to go on applying the transform that had just
/// been replaced.
#[test]
fn the_dev_loop_reloads_an_edited_plugin() {
    let dir = watch_dir("s_plugin_edit");
    let port = test_port("s_plugin_edit");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "mark.mjs", "export const mark = 'FIRST-MARK';\n");
    write_in(
        &dir,
        "plugin.mjs",
        r#"import { mark } from "./mark.mjs";
export default {
  name: "mark",
  transform: {
    filter: { id: /main\.js$/ },
    handler: (code) => ({ code: code.replace("__MARK__", mark) }),
  },
};
"#,
    );
    write_in(&dir, "src/main.js", "console.log('__MARK__');\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{"plugins": ["./plugin.mjs"], "build": {{"targets": {{"web": {{"entry": "index.html", "outDir": "dist"}}}}}}, "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }}}}"#
        ),
    );
    let _supervisor = start_in(&dir);
    let first = wait_for_http(port, "/assets/main.js", |body| body.contains("FIRST-MARK"));
    assert!(first.contains("FIRST-MARK"), "{first}");
    write_in(&dir, "mark.mjs", "export const mark = 'SECOND-MARK';\n");
    let second = wait_for_http(port, "/assets/main.js", |body| body.contains("SECOND-MARK"));
    assert!(
        second.contains("SECOND-MARK"),
        "the rebuild used the plugin as it was loaded: {second}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The dev loop rebuilds for a file a build read, whatever its extension: a
/// `.scss` a plugin compiles is an input, as a `.ts` is, and a change to one
/// is a stylesheet swap rather than a reload.
#[test]
fn the_dev_loop_rebuilds_for_a_file_a_plugin_compiles() {
    let dir = watch_dir("s_plugin_stylesheet");
    let port = test_port("s_plugin_stylesheet");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    fake_sass_plugin(&dir);
    write_in(
        &dir,
        "src/theme.scss",
        "$page: papayawhip;\nbody { background: $page }\n",
    );
    write_in(
        &dir,
        "src/card.module.scss",
        "$accent: rebeccapurple;\n.card { color: $accent }\n",
    );
    write_in(
        &dir,
        "src/main.js",
        "import styles from './card.module.scss';\ndocument.body.className = styles.card;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <link rel=\"stylesheet\" href=\"./src/theme.scss\">\
         <script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{"plugins": ["./sass.mjs"], "build": {{"targets": {{"web": {{"entry": "index.html", "outDir": "dist"}}}}}}, "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }}}}"#
        ),
    );
    let css = |dir: &Path| -> String {
        std::fs::read_dir(dir.join(".dev/dist/assets"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|e| e == "css"))
                    .map(|path| std::fs::read_to_string(path).unwrap_or_default())
                    .collect()
            })
            .unwrap_or_default()
    };
    let wait = |want: &str| {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            if css(&dir).contains(want) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        panic!("never built {want}: {}", css(&dir));
    };

    let (_supervisor, log) = start_in_logging(&dir, &[]);
    wait("papayawhip");
    wait("rebeccapurple");
    write_in(
        &dir,
        "src/theme.scss",
        "$page: honeydew;\nbody { background: $page }\n",
    );
    wait("honeydew");
    write_in(
        &dir,
        "src/card.module.scss",
        "$accent: teal;\n.card { color: $accent }\n",
    );
    wait("teal");
    let printed = wait_for_file(&log, Duration::from_secs(10), |text| {
        text.matches("swapped stylesheet").count() >= 2
    });
    assert!(
        printed.matches("swapped stylesheet").count() >= 2,
        "a .scss change reloaded instead of swapping:\n{printed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A web project for the reload tests: one module, `public/` copied into the
/// output, served on `port`.
fn reload_project(name: &str) -> PathBuf {
    let dir = watch_dir(name);
    let port = test_port(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("public")).expect("create public");
    write_in(&dir, "src/main.mjs", "document.title = 'ONE';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{"build": {{"targets": {{"web": {{"entry": "index.html", "outDir": "dist", "assets": ["public"]}}}}}}, "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }}}}"#
        ),
    );
    dir
}

/// Creating a file nothing loads — a module nothing imports, a note beside
/// the project — rebuilds to the same output, and the page is not reloaded
/// for it (D166). Only the edit after it is.
#[test]
fn a_new_file_nothing_loads_does_not_reload_the_page() {
    let dir = reload_project("s_newfile_quiet");
    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let ready = wait_for_file(&log, Duration::from_secs(30), |t| t.contains("ready in"));
    assert!(ready.contains("ready in"), "{ready}");
    let builds = |text: &str| text.matches("built →").count();

    write_in(&dir, "src/unused.mjs", "export const unused = 1;\n");
    let after = wait_for_file(&log, Duration::from_secs(30), |t| builds(t) >= 2);
    write_in(&dir, "notes.txt", "remember the milk\n");
    let after = wait_for_file(&log, Duration::from_secs(30), |t| {
        builds(t) > builds(&after)
    });
    assert!(
        !after.contains('✓'),
        "a file nothing loads reloaded the page:\n{after}"
    );

    write_in(&dir, "src/main.mjs", "document.title = 'TWO';\n");
    let printed = wait_for_file(&log, Duration::from_secs(30), |t| t.contains('✓'));
    assert_eq!(printed.matches('✓').count(), 1, "{printed}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A new file that does reach the output — here one under `public/`, which
/// the target copies — reloads the page, as any change to what it loads does.
#[test]
fn a_new_file_the_output_picks_up_reloads_the_page() {
    let dir = reload_project("s_newfile_copied");
    let (_supervisor, log) = start_in_logging(&dir, &[]);
    let ready = wait_for_file(&log, Duration::from_secs(30), |t| t.contains("ready in"));
    assert!(ready.contains("ready in"), "{ready}");

    write_in(&dir, "public/robots.txt", "User-agent: *\n");
    let printed = wait_for_file(&log, Duration::from_secs(30), |t| t.contains("✓ reloaded"));
    assert!(printed.contains("✓ reloaded"), "{printed}");
    assert!(dir.join(".dev/dist/robots.txt").is_file());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin that rewrites CSS and keeps it CSS — PostCSS's shape — runs
/// `pre`, and esdev bundles what it wrote rather than reading the file again.
#[test]
fn a_pre_plugin_rewrites_css_that_esdev_then_bundles() {
    let dir = build_dir("p_css_postcss");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "postcss.mjs",
        r#"
export default {
  name: "fake-postcss",
  transform: {
    order: "pre",
    filter: { id: /\.css$/ },
    handler: (code) => ({ code: code.replaceAll("OLD", "NEW"), type: "css" }),
  },
};
"#,
    );
    write_in(&dir, "src/linked.css", ".linked { content: 'OLD' }\n");
    write_in(&dir, "src/imported.css", ".imported { content: 'OLD' }\n");
    write_in(&dir, "src/main.js", "import './imported.css';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <link rel=\"stylesheet\" href=\"./src/linked.css\">\
         <script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./postcss.mjs"], "build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let css: String = std::fs::read_dir(dir.join("dist/assets"))
        .expect("assets")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "css"))
        .map(|path| std::fs::read_to_string(path).expect("css"))
        .collect();
    assert!(css.contains(".linked"), "{css}");
    assert!(css.contains(".imported"), "{css}");
    assert!(
        !css.contains("OLD"),
        "the plugin's output was dropped: {css}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// What a plugin says a module now is decides how it is compiled, unbundled as
/// in a build: a `.component` a plugin turns into JSX is compiled as JSX, not
/// passed to the engine as the JavaScript its name does not claim to be.
#[test]
fn an_unbundled_run_compiles_a_module_as_the_type_a_plugin_gave_it() {
    let dir = build_dir("p_plugins_type");
    write_in(
        &dir,
        "component.mjs",
        r#"
export default {
  name: "component",
  transform: {
    filter: { id: /\.component$/ },
    handler: (code) => ({ code: `export default () => ${code.trim()};`, type: "jsx" }),
  },
};
"#,
    );
    write_in(&dir, "widget.component", "<b>hi</b>\n");
    // The factory is a global, set before the component module is loaded, so
    // the compiled module needs no import of its own.
    write_in(
        &dir,
        "jsx.js",
        "globalThis.h = (tag, props, ...children) => ({ tag, children });\n",
    );
    write_in(
        &dir,
        "main.js",
        "import './jsx.js';\n\
         const { default: widget } = await import('./widget.component');\n\
         console.log(JSON.stringify(widget()));\n",
    );
    write_in(
        &dir,
        "widget.test.js",
        "import { test, assertEquals } from 'runtime:test';\n\
         import './jsx.js';\n\
         const { default: widget } = await import('./widget.component');\n\
         test('compiled as JSX', () => assertEquals(widget().tag, 'b'));\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./component.mjs"], "jsx": {"factory": "h"}}"#,
    );

    let run = esdev_in(&dir).arg("main.js").output().expect("spawn esdev");
    assert!(run.status.success(), "{}{}", stdout(&run), stderr(&run));
    assert!(
        stdout(&run).contains(r#"{"tag":"b","children":["hi"]}"#),
        "{}",
        stdout(&run)
    );

    let test = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(test.status.success(), "{}{}", stdout(&test), stderr(&test));
    assert!(stdout(&test).contains("1 passed"), "{}", stdout(&test));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin shapes the document (D151). A `pre` hook's script is an input, so
/// it is bundled and hashed like one the author wrote; a later hook sees the
/// written document and what was built, and preloads the chunks the entries
/// import.
#[test]
fn a_plugin_shapes_the_html_document() {
    let dir = build_dir("p_plugins_html");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "boot.mjs",
        r#"
export default {
  name: "boot",
  html: {
    order: "pre",
    handler: (html, id, ctx) => ({
      html: html.replace("<title>app</title>", "<title>framework app</title>"),
      tags: [
        { tag: "script", attrs: { type: "module", src: "./src/boot.js" }, injectTo: "body" },
        { tag: "meta", attrs: { name: "built-by", content: `${ctx.command}/${ctx.platform}/${ctx.target}` } },
      ],
    }),
  },
};
"#,
    );
    write_in(
        &dir,
        "preload.mjs",
        r#"
export default {
  name: "preload",
  html: {
    filter: { id: /index\.html$/ },
    handler(html, id, ctx) {
      const shared = ctx.bundle
        .filter((file) => file.type === "chunk" && file.isEntry)
        .flatMap((file) => file.imports);
      return {
        tags: [...new Set(shared)].map((file) => ({
          tag: "link",
          attrs: { rel: "modulepreload", href: `/${file}` },
          injectTo: "head-prepend",
        })),
      };
    },
  },
};
"#,
    );
    write_in(
        &dir,
        "src/shared.js",
        "export const shared = () => 'SHARED';\n",
    );
    write_in(
        &dir,
        "src/main.js",
        "import { shared } from './shared.js';\nconsole.log('MAIN', shared());\n",
    );
    write_in(
        &dir,
        "src/boot.js",
        "import { shared } from './shared.js';\nconsole.log('BOOT', shared());\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head><title>app</title>\
         <script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./boot.mjs", "./preload.mjs"], "build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let html = std::fs::read_to_string(dir.join("dist/index.html")).expect("html");
    assert!(html.contains("<title>framework app</title>"), "{html}");
    assert!(
        html.contains(r#"<meta name="built-by" content="build/browser/web">"#),
        "{html}"
    );
    // The injected script was built: its reference points at a hashed bundle.
    assert!(!html.contains("./src/boot.js"), "{html}");
    let boot = regex_like_find(&html, "/assets/boot-", ".js").expect("the boot bundle");
    assert!(dir.join("dist").join(&boot[1..]).is_file(), "{boot}");
    // And the preload names the shared chunk, which is on disk.
    let preload = html
        .split("<link rel=\"modulepreload\" href=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap_or_else(|| panic!("no preload: {html}"))
        .to_string();
    assert!(preload.starts_with("/assets/"), "{preload}");
    assert!(dir.join("dist").join(&preload[1..]).is_file(), "{preload}");
    assert!(
        html.find("modulepreload").unwrap() < html.find("<title>").unwrap(),
        "head-prepend: {html}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The first `start…end` span in `text` that begins with `start` and runs to
/// the next `end`.
fn regex_like_find(text: &str, start: &str, end: &str) -> Option<String> {
    let at = text.find(start)?;
    let rest = &text[at..];
    let stop = rest.find(end)? + end.len();
    Some(rest[..stop].to_string())
}

/// A `load` that throws is reported with what the plugin said. The bundler
/// turns any other error from a `load` into "Could not load x — plugin `p`
/// threw an error" and drops the cause, which is the only part that says what
/// went wrong.
/// A handler with more parameters than its hook passes has another hook's
/// signature — `load(code, id, ctx)` copied from `transform` — and is refused
/// when the plugin is declared, naming the signature, rather than failing later
/// as `ctx` being undefined. `ctx.log()` names the methods that exist.
#[test]
fn a_plugin_with_the_wrong_signature_or_ctx_log_is_told_which() {
    let dir = build_dir("p_plugins_signature");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugin.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    write_in(
        &dir,
        "plugin.mjs",
        "export default { name: \"slip\", load: { handler(code, id, ctx) { return null; } } };\n",
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("slip.load: load takes (id, ctx), but the handler declares 3 parameters"),
        "{text}"
    );

    // Fewer parameters than the hook passes are fine.
    write_in(
        &dir,
        "plugin.mjs",
        "export default { name: \"ok\", transform: { handler(code) { return null; } } };\n",
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    write_in(
        &dir,
        "plugin.mjs",
        "export default { name: \"logger\", transform: { handler(code, id, ctx) { ctx.log(\"hi\"); return null; } } };\n",
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(
        text.contains("ctx has no log() — call ctx.warn(), ctx.info() or ctx.debug()"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin's `ctx.warn()` and `ctx.info()` read the same under `esdev build`
/// and `esdev test`, and `ctx.debug()` is shown by neither.
#[test]
fn a_plugin_says_the_same_thing_under_build_and_test() {
    let dir = build_dir("p_plugins_logs");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "plugin.mjs",
        r#"export default {
  name: "talk",
  transform: {
    filter: { id: /src\/x\.js$/ },
    handler(code, id, ctx) {
      ctx.warn("careful here");
      ctx.info("compiled x");
      ctx.debug("internal detail");
      return null;
    },
  },
};
"#,
    );
    write_in(&dir, "src/x.js", "export const x = 1;\n");
    write_in(
        &dir,
        "src/x.test.js",
        "import { test, expect } from \"runtime:test\";\nimport { x } from \"./x.js\";\ntest(\"x\", () => expect(x).toBe(1));\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugin.mjs"], "build": {"targets": {"app": {"entry": "src/x.js", "out": "dist/x.js"}}}}"#,
    );
    for command in ["build", "test"] {
        let out = esdev_in(&dir).arg(command).output().expect("spawn esdev");
        let text = format!("{}{}", stdout(&out), stderr(&out));
        assert!(out.status.success(), "{command}: {text}");
        assert!(
            text.contains("warning: talk: careful here"),
            "{command}: {text}"
        );
        assert!(text.contains("info: talk: compiled x"), "{command}: {text}");
        assert!(!text.contains("internal detail"), "{command}: {text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failing_load_reports_the_plugins_own_message() {
    let dir = build_dir("p_plugins_load_fails");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
export default {
  name: "pages",
  resolve: {
    filter: { id: "virtual:pages" },
    handler: () => ({ id: "virtual:pages", virtual: true }),
  },
  load: {
    filter: { id: "virtual:pages" },
    handler() { throw new Error("the app directory is missing"); },
  },
};
"#,
    );
    write_in(
        &dir,
        "src/app.mjs",
        "import pages from 'virtual:pages';\nconsole.log(pages);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugin.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("the app directory is missing"), "{text}");
    assert!(text.contains("[pages] virtual:pages"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A test resolves and loads through the plugins as a build does, so a
/// virtual module exists under test too — and the plugin is told it is a test,
/// against a DOM or not.
#[test]
fn a_test_loads_a_plugins_virtual_module() {
    let dir = build_dir("p_plugins_test_virtual");
    site_plugin(&dir);
    write_in(
        &dir,
        "site.test.mjs",
        "import { test, assertEquals } from 'runtime:test';\n\
         import site from 'virtual:site';\n\
         const expected = typeof document === 'undefined' ? 'test/server/undefined/false' : 'test/browser/undefined/false';\n\
         test('the plugin served it', () => assertEquals(site, expected));\n",
    );
    write_in(&dir, "esdev.json", r#"{"plugins": ["./site.mjs"]}"#);
    for args in [
        vec!["test"],
        vec!["test", "--dom"],
        vec!["test", "--isolation=none"],
    ] {
        let out = esdev_in(&dir)
            .args(&args)
            .output()
            .expect("spawn esdev test");
        assert!(
            out.status.success(),
            "{args:?}\n{}{}",
            stdout(&out),
            stderr(&out)
        );
        assert!(
            stdout(&out).contains("1 passed"),
            "{args:?}\n{}",
            stdout(&out)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A component imports its stylesheet and its images, and a test imports the
/// component: esdev's own passes answer under test as in the build. A CSS
/// Module gives its scoped names, a plain stylesheet nothing, an image its URL.
#[test]
fn a_test_imports_stylesheets_and_assets_as_the_build_does() {
    let dir = build_dir("p_test_css_assets");
    write_in(&dir, "card.module.css", ".card { color: red }\n");
    write_in(&dir, "global.css", "body { margin: 0 }\n");
    std::fs::write(dir.join("logo.png"), b"\x89PNG\r\n\x1a\n\xff\xfe").expect("write png");
    write_in(
        &dir,
        "card.test.mjs",
        "import { test, assert, assertEquals } from 'runtime:test';\n\
         import styles from './card.module.css';\n\
         import './global.css';\n\
         import logo from './logo.png';\n\
         test('scoped names', () => assert(styles.card.startsWith('card_')));\n\
         test('an asset URL', () => assert(/^\\/assets\\/logo-[0-9a-f]+\\.png$/.test(logo), logo));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("2 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev test` compiles what a test imports through the project's plugins,
/// as `esdev build` does.
#[test]
fn test_compiles_modules_through_the_projects_plugins() {
    let dir = framework_project("p_plugins_test");
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev test --config=<file>` reads that file, as `build` and `start` do.
/// It was parsed and then ignored, so the run read `./esdev.json` — here, a
/// file that does not exist, which left the plugin out and the JSX unknown.
#[test]
fn test_reads_the_project_file_config_names() {
    let dir = framework_project("p_plugins_test_config");
    std::fs::rename(dir.join("esdev.json"), dir.join("esdev.other.json")).expect("rename");
    let out = esdev_in(&dir)
        .args(["test", "--config=esdev.other.json"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin decides what a test's modules compile to, so `--related` counts a
/// change to it — or to anything it imports — as reaching every test. It
/// selected none: nothing a test imports imports the plugin.
#[test]
fn a_changed_plugin_is_related_to_every_test() {
    let dir = framework_project("p_plugins_related");
    write_in(&dir, "greeting.helper.mjs", "export const unused = 1;\n");
    let framework = std::fs::read_to_string(dir.join("framework.mjs")).expect("framework");
    write_in(
        &dir,
        "framework.mjs",
        &format!("import './greeting.helper.mjs';\n{framework}"),
    );
    for named in ["framework.mjs", "greeting.helper.mjs"] {
        let out = esdev_in(&dir)
            .args(["test", "--related", named])
            .output()
            .expect("spawn esdev test");
        assert!(
            out.status.success(),
            "{named}: {}{}",
            stdout(&out),
            stderr(&out)
        );
        assert!(
            stderr(&out).contains("1 of 1 test files reach it"),
            "{named}: {}",
            stderr(&out)
        );
        assert!(
            stdout(&out).contains("1 passed"),
            "{named}: {}",
            stdout(&out)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same with every file in one process.
#[test]
fn test_compiles_through_the_plugins_without_isolation() {
    let dir = framework_project("p_plugins_test_shared");
    let out = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Without isolation, JSX still compiles the way the project's `jsx` says —
/// the shared process used to compile it with nothing said at all.
#[test]
fn test_without_isolation_compiles_jsx_with_the_projects_settings() {
    let dir = build_dir("p_jsx_test_shared");
    write_in(
        &dir,
        "view.test.jsx",
        "import { test, assertEquals } from 'runtime:test';\n\
         const h = (tag) => tag;\n\
         test('classic JSX calls h', () => assertEquals(<section />, 'section'));\n",
    );
    write_in(&dir, "esdev.json", r#"{ "jsx": { "factory": "h" } }"#);
    let out = esdev_in(&dir)
        .args(["test", "--isolation=none"])
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("1 passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A test run chains the map a plugin returns under the compiler's, so a
/// failure in a module a plugin rewrote names the line that was written, not
/// the line the plugin produced. The plugin here moves every line down three.
#[test]
fn test_maps_a_failure_through_a_plugins_source_map() {
    let dir = build_dir("p_plugins_test_map");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
export default {
  name: "shift",
  transform: {
    filter: { id: /\.shift\.ts$/ },
    handler(code, id) {
      const lines = code.split("\n");
      const mappings = ";;;" + lines.map((_, i) => (i === 0 ? "AAAA" : "AACA")).join(";");
      return { code: "\n\n\n" + code, map: { version: 3, sources: [id], names: [], mappings } };
    },
  },
};
"#,
    );
    write_in(
        &dir,
        "boom.shift.ts",
        "export function boom(n: number): number {\n  \
         if (n > 1) throw new Error('too big');\n  \
         return n;\n\
         }\n",
    );
    write_in(
        &dir,
        "boom.test.ts",
        "import { test } from 'runtime:test';\n\
         import { boom } from './boom.shift.ts';\n\
         test('throws', () => { boom(5); });\n",
    );
    write_in(&dir, "esdev.json", r#"{"plugins": ["./plugin.mjs"]}"#);
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("boom.shift.ts:2:"), "unmapped:\n{text}");
    assert!(
        !text.contains("boom.shift.ts:5:"),
        "the plugin's line:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin that throws fails the file, naming the plugin and the module.
#[test]
fn test_reports_a_plugin_that_fails_to_compile_a_module() {
    let dir = framework_project("p_plugins_test_fails");
    write_in(&dir, "greeting.jsx", "export const greeting = BROKEN;\n");
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(!out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("framework"), "{text}");
    assert!(text.contains("cannot compile greeting.jsx"), "{text}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// **`order: "pre"` is ordered against the built-in passes, not only against
/// the other plugins.** A Tailwind compiler claims `.css` so that
/// `@import "tailwindcss"` never reaches `esdev:css-modules`, which would treat
/// it as a file to fetch and fail with "which is not there".
///
/// Two things have to hold for that, and this is a regression test for both:
/// the plugin's transform runs first, and the built-in pass then **steps
/// aside** — it filters on the id, which is still `.css`, and would otherwise
/// re-read the stylesheet off disk and undo the whole thing.
#[test]
fn a_pre_ordered_plugin_claims_css_ahead_of_the_built_in_pass() {
    let dir = build_dir("p_plugins_pre");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
export default {
  name: "tailwind-ish",
  transform: {
    order: "pre",
    filter: { id: /\.css$/ },
    handler: (code, id, ctx) => ({
      code: `export default { button: "COMPILED-${ctx.type}" };`,
      type: "js",
    }),
  },
};
"#,
    );
    // The `@import` names a package, which is exactly what the built-in CSS
    // pass cannot resolve — so if it ever sees this file, the build fails.
    write_in(
        &dir,
        "src/ui.css",
        "@import \"tailwindcss\";\n.button { color: red }\n",
    );
    write_in(
        &dir,
        "src/app.mjs",
        "import styles from './ui.css';\nconsole.log(styles.button);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugin.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    // `ctx.type` is what the module was when the hook was called — the extension
    // said `.css`, and nothing had changed it yet.
    assert!(bundle.contains("COMPILED-css"), "{bundle}");
    assert!(
        !bundle.contains("color: red"),
        "the built-in pass re-read the stylesheet: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A stylesheet nobody claimed is still the built-in pass's, so stepping aside
/// for a plugin did not quietly turn CSS Modules off.
#[test]
fn css_modules_still_scopes_a_stylesheet_no_plugin_claimed() {
    let dir = build_dir("p_plugins_css_intact");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    banner_plugin(&dir);
    write_in(&dir, "src/ui.module.css", ".button { color: red }\n");
    write_in(
        &dir,
        "src/app.mjs",
        "import banner from 'virtual:banner';\nimport styles from './ui.module.css';\n\
         console.log(banner, styles.button);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": [{"module": "./plugin.mjs", "options": {"text": "B"}}], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    // The scoped name the CSS Modules pass mints — the source class mapped to a
    // name derived from the file's path, which is neither the class alone nor
    // anything a plugin here produced.
    assert!(
        bundle.contains("\"button\": \"button_"),
        "the class was not scoped: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A plugin that cannot be loaded fails the build **where it was named**, not
/// as a module-not-found inside a program nobody typed.
#[test]
fn a_plugin_that_cannot_be_loaded_is_reported_against_the_config() {
    let dir = build_dir("p_plugins_missing");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.mjs", "console.log('hi');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugins/not-here.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("./plugins/not-here.mjs"), "{text}");
    assert!(!dir.join("dist/app.js").exists(), "a failed build wrote");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A module that exports no plugin is named too — the module, and which export
/// was looked for.
#[test]
fn a_plugin_module_with_no_export_says_so() {
    let dir = build_dir("p_plugins_noexport");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "plugin.mjs", "export const other = 1;\n");
    write_in(&dir, "src/app.mjs", "console.log('hi');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugin.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("no default export"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The reusable React package resolves from node_modules, initializes once,
/// and adds refresh code only to the hot browser build.
#[test]
fn the_react_plugin_package_bootstraps_hot_builds_only() {
    let dir = build_dir("p_react_package");
    let package = "node_modules/@opentf/esdev-plugin-react";
    for (name, contents) in [
        (
            "package.json",
            include_str!("../../../../packages/esdev-plugin-react/package.json"),
        ),
        (
            "src/index.js",
            include_str!("../../../../packages/esdev-plugin-react/src/index.js"),
        ),
        (
            "src/runtime.js",
            include_str!("../../../../packages/esdev-plugin-react/src/runtime.js"),
        ),
    ] {
        let path = dir.join(package).join(name);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("package directory");
        std::fs::write(path, contents).expect("package file");
    }
    std::fs::create_dir_all(dir.join("node_modules/react-refresh")).expect("peer directory");
    write_in(
        &dir,
        "node_modules/react-refresh/package.json",
        r#"{"name":"react-refresh","type":"module","exports":{"./runtime":"./runtime.js"}}"#,
    );
    write_in(
        &dir,
        "node_modules/react-refresh/runtime.js",
        r#"
export function injectIntoGlobalHook(target) {
  target.__reactPluginInjected = (target.__reactPluginInjected ?? 0) + 1;
  console.log('REACT-PLUGIN-INJECTED');
}
export function register() {}
export function createSignatureFunctionForTransform() { return () => {}; }
export function performReactRefresh() { console.log('REACT-PLUGIN-REFRESH'); }
"#,
    );
    write_in(
        &dir,
        "bootstrap.test.js",
        r#"
import '@opentf/esdev-plugin-react/runtime';
import '@opentf/esdev-plugin-react/runtime';
import { test, assertEquals } from 'runtime:test';
test('initializes once', () => {
  assertEquals(globalThis.__reactPluginInjected, 1);
  assertEquals(typeof globalThis.$RefreshReg$, 'function');
  assertEquals(typeof globalThis.$RefreshSig$, 'function');
});
"#,
    );
    write_in(&dir, "app.jsx", "console.log('REACT-PACKAGE-APP');\n");
    write_in(
        &dir,
        "index.html",
        "<html><head><script type=\"module\" src=\"./app.jsx\"></script></head><body></body></html>",
    );
    let port = test_port("p_react_package");
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{
      "plugins": ["@opentf/esdev-plugin-react"],
      "build": {{"targets": {{"web": {{"entry":"index.html","outDir":"dist"}}}}}},
      "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }}
    }}"#
        ),
    );
    let tested = esdev_in(&dir)
        .args(["test", "--dom"])
        .output()
        .expect("test bootstrap");
    assert!(
        tested.status.success(),
        "{}{}",
        stdout(&tested),
        stderr(&tested)
    );
    let built = esdev_in(&dir).arg("build").output().expect("release build");
    assert!(
        built.status.success(),
        "{}{}",
        stdout(&built),
        stderr(&built)
    );
    let release: String = std::fs::read_dir(dir.join("dist/assets"))
        .expect("assets")
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "js"))
        .map(|entry| std::fs::read_to_string(entry.path()).expect("chunk"))
        .collect();
    assert!(!release.contains("REACT-PLUGIN-"), "{release}");
    {
        let _supervisor = start_in(&dir);
        let hot = wait_for_http(port, "/assets/app.js", |code| {
            code.contains("REACT-PACKAGE-APP")
        });
        assert!(hot.contains("REACT-PLUGIN-INJECTED"), "{hot}");
        assert!(hot.contains("REACT-PLUGIN-REFRESH"), "{hot}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A project with no `plugins` starts no isolate and builds exactly as it
/// always did — the cost of the feature is paid only by projects that use it.
#[test]
fn a_project_without_plugins_is_unchanged() {
    let dir = build_dir("p_plugins_none");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.mjs", "console.log('plain');\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    assert!(bundle.contains("plain"), "{bundle}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The case the whole feature was asked for: a configured plugin reading the
/// **chunk graph** and writing a route manifest. `end` fires before there are
/// chunks, and until `esdev.json` had a plugin slot the only way to reach the
/// graph was for the framework to call `build()` itself.
#[test]
fn a_configured_plugin_reads_the_chunk_graph_and_writes_a_manifest() {
    let dir = build_dir("p_plugins_bundle");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/shared.mjs", "export const shared = 1;\n");
    write_in(
        &dir,
        "src/app.mjs",
        "import { shared } from './shared.mjs';\nconsole.log(shared);\n",
    );
    write_in(
        &dir,
        "plugin.mjs",
        r#"
import { write } from "runtime:fs";
export default {
  name: "manifest",
  bundle: {
    handler: async (output) => {
      const entry = output.find((o) => o.type === "chunk" && o.isEntry);
      // `facadeModuleId` is the module the chunk *is*, and `imports` are the
      // chunks a preload would have to fetch beside it.
      await write(
        "manifest.json",
        JSON.stringify({
          file: entry.fileName,
          module: entry.facadeModuleId.endsWith("app.mjs"),
          modules: entry.moduleIds.length,
          imports: entry.imports,
        }),
      );
    },
  },
};
"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./plugin.mjs"], "build": {"targets": {"app": {"entry": "src/app.mjs", "out": "dist/app.js"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).expect("the manifest");
    assert!(manifest.contains("\"module\":true"), "{manifest}");
    assert!(manifest.contains("\"file\":\"app.js\""), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// `host()` is esdev's seam, not an API a program that bundles should reach
/// for. Called with nothing driving a build from outside, it says which one is
/// which rather than waiting for ever on a build nobody started.
#[test]
fn runtime_build_host_is_refused_when_nothing_is_driving_a_build() {
    let dir = build_dir("rb_host_alone");
    write_in(
        &dir,
        "app.mjs",
        "import { host } from 'runtime:build';\n\
         try { await host([]); console.log('RESOLVED'); }\n\
         catch (err) { console.log('refused:', err.message); }\n",
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("refused:"), "{text}");
    assert!(text.contains("build() instead"), "{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A hot-reload scheme, written the way esdev expects one: **as a plugin**.
///
/// esdev implements none and knows the name of none. It provides the generic
/// half — `import.meta.hot`, the update channel, and the compiler's component
/// registrations on request — and a framework's scheme is this.
fn refresh_plugin(dir: &Path) {
    std::fs::create_dir_all(dir.join("plugins")).expect("create plugins");
    write_in(
        dir,
        "plugins/refresh.mjs",
        r#"
export default {
  name: "afw-refresh",
  // The half a plugin cannot do for itself: finding the components to register
  // needs the syntax tree the compiler already has.
  jsx: { refresh: true },
  transform: {
    filter: { id: /\.[jt]sx?$/ },
    handler(code, id, ctx) {
      // A wrapper that makes every module a hot boundary is exactly wrong in
      // anything shipped, so it goes in only where the loop is running this
      // build hot.
      if (!ctx.hot) return null;
      return { code: `globalThis.__afw_hot = ${JSON.stringify(ctx.target)};\n${code}` };
    },
  },
};
"#,
    );
}

fn refresh_project(dir: &Path, port: Option<u16>) {
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    refresh_plugin(dir);
    // JSX, so the compiler's half has a component to register.
    write_in(
        dir,
        "src/Home.jsx",
        "export function Home() {\n  return <h1>hi</h1>;\n}\n",
    );
    write_in(
        dir,
        "src/main.jsx",
        "import { Home } from './Home.jsx';\nglobalThis.Home = Home;\n",
    );
    write_in(
        dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.jsx\"></script></head>\
         <body><div id=root></div></body></html>\n",
    );
    let start = port.map_or_else(String::new, |port| {
        format!(r#", "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }}"#)
    });
    write_in(
        dir,
        "esdev.json",
        &format!(
            r#"{{ "jsx": {{ "factory": "h" }}, "plugins": ["./plugins/refresh.mjs"],
                 "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outDir": "dist" }} }} }}{start} }}"#
        ),
    );
}

/// A release build is not hot, so the wrapper is not installed. Without this
/// half, a plugin would have to inject its wrapper into everything, including
/// what you ship.
#[test]
fn a_release_build_tells_a_plugin_it_is_not_hot() {
    let dir = build_dir("p_refresh_release");
    refresh_project(&dir, None);

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("the assets")
        .filter_map(Result::ok)
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap_or_default())
        .collect::<String>();
    assert!(bundle.contains("Home"), "nothing was built");
    assert!(
        !bundle.contains("__afw_hot"),
        "a release build installed a hot-reload wrapper: {bundle}"
    );
    // The compiler's half is gated on the same thing, and has to be: the
    // registrations reach globals only a hot loop installs, so shipping them
    // would ship calls to something undefined.
    assert!(
        !bundle.contains("$RefreshReg$"),
        "a release build got the compiler's registrations: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **A hot-reload scheme is a plugin, and esdev knows nothing about any of
/// them.** `"react"` was for a while the only name the config would take *and*
/// the only implementation, both inside esdev — so every other framework took a
/// full page reload on each edit. Both halves a scheme needs are generic now:
/// the plugin is told the build is hot (`ctx.hot`) and the compiler is asked
/// for the component registrations by the plugin's own declaration.
#[test]
fn the_hot_dev_loop_tells_a_plugin_its_build_is_hot() {
    let dir = watch_dir("s_refresh");
    let port = test_port("s_refresh");
    refresh_project(&dir, Some(port));

    let _supervisor = start_in(&dir);

    let bundle = wait_for_http(port, "/assets/main.js", |body| body.contains("__afw_hot"));
    // The plugin's half: it was told this build is hot, so it installed its
    // wrapper.
    assert!(
        bundle.contains(r#"__afw_hot = "web""#),
        "the hot build never reached the plugin: {bundle}"
    );
    // The compiler's half: `jsx: { refresh: true }` on the plugin's own
    // declaration asked for the registrations, and nothing in esdev knows what
    // framework asked or why.
    assert!(
        bundle.contains("$RefreshReg$(_c,"),
        "the compiler never inserted the registrations: {bundle}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The dev loop, with a plugin. Two things are under test that a single build
/// cannot show:
///
/// * the plugin still applies on a **rebuild** — the isolate holding it is
///   started once and kept, because evaluating a plugin's module (and whatever
///   it initialises) on every save would be a startup cost per keystroke;
/// * a plugin holding **state across builds** works, which is what every
///   incremental compiler is. The counter here stands in for one: it is the
///   plugin's own module scope, and a fresh isolate per build would reset it.
#[test]
fn start_keeps_the_projects_plugins_across_rebuilds() {
    let dir = watch_dir("s_plugins");
    let port = test_port("s_plugins");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
// Module scope: it survives for as long as the isolate does, and a host that
// reloaded the plugin per build would hand back 1 every time.
let builds = 0;
export default {
  name: "counter",
  resolve: {
    filter: { id: "virtual:builds" },
    handler: () => ({ id: "virtual:builds", virtual: true }),
  },
  load: {
    filter: { id: "virtual:builds" },
    handler: () => ({ code: `export default ${++builds};` }),
  },
};
"#,
    );
    write_in(
        &dir,
        "src/main.mjs",
        "import builds from 'virtual:builds';\ndocument.title = 'FIRST build ' + builds;\n",
    );
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
        &format!(
            r#"{{ "plugins": ["./plugin.mjs"],
                 "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outDir": "dist" }} }} }},
                 "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);

    let bundle = wait_for_http(port, "/assets/main.js", |body| body.contains("FIRST build"));
    assert!(bundle.contains("FIRST build"), "{bundle}");
    assert!(bundle.contains("build 1"), "the plugin never ran: {bundle}");

    // A save rebuilds. The plugin applies again — and its counter has moved,
    // which it could not have done if the isolate holding it were new.
    write_in(
        &dir,
        "src/main.mjs",
        "import builds from 'virtual:builds';\ndocument.title = 'SECOND build ' + builds;\n",
    );
    let rebuilt = wait_for_http(port, "/assets/main.js", |body| {
        body.contains("SECOND build")
    });
    assert!(rebuilt.contains("SECOND build"), "the change never landed");
    assert!(
        !rebuilt.contains("build 1"),
        "the plugin was reloaded for the rebuild: {rebuilt}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// **Every name a release build hashes is spelled one way** — `<stem>-<8 hex>`
/// — for a chunk the bundler names and a stylesheet or file esdev names alike,
/// and `esdev preview` serves each as production would: cached for good, while
/// the document is revalidated.
#[test]
fn a_release_build_hashes_one_way_and_preview_caches_it() {
    let dir = watch_dir("b_one_hash");
    let port = test_port("b_one_hash");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.js", "document.title = 'main';\n");
    write_in(
        &dir,
        "src/styles.css",
        ".a { background: url(./icon.svg) }\n",
    );
    write_in(
        &dir,
        "src/icon.svg",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./src/styles.css\"><script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "index.html", "outDir": "dist"}}}, "dev": {"servers": {"web": {}}}}"#,
    );
    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let names: Vec<String> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    // A chunk the bundler named, a stylesheet and a file esdev named.
    assert!(names.len() >= 3, "{names:?}");
    for name in &names {
        let stem = name.split('.').next().unwrap_or("");
        let hash = stem.rsplit_once('-').map(|(_, hash)| hash).unwrap_or("");
        assert!(
            hash.len() == 8 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "{name} is not <stem>-<8 hex>: {names:?}"
        );
    }

    let mut preview = esdev_in(&dir)
        .args(["preview", &format!("--port={port}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn esdev preview");
    let asset = format!(
        "/assets/{}",
        names.iter().find(|n| n.ends_with(".js")).expect("a chunk")
    );
    let cached = wait_for_http(port, &asset, |response| {
        response.starts_with("HTTP/1.1 200")
    });
    let page = http_get(port, "/").unwrap_or_default();
    let _ = preview.kill();
    let _ = preview.wait();
    assert!(
        cached
            .to_ascii_lowercase()
            .contains("cache-control: public, max-age=31536000, immutable"),
        "{cached}"
    );
    assert!(
        page.to_ascii_lowercase()
            .contains("cache-control: no-cache"),
        "{page}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A proxied request waits for a server that is restarting** (D235). The web
/// entry is the team's gateway in front of its API; a save restarts the API,
/// and a request in that moment used to be refused and answered 502. It waits
/// for the server to listen again instead: across several saves under steady
/// traffic, every request is answered by the API.
#[test]
fn the_proxy_waits_for_a_server_that_is_restarting() {
    let dir = watch_dir("s_proxy_wait");
    let web = test_port("s_proxy_wait_web");
    let api = test_port("s_proxy_wait_api");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    let server = |word: &str| {
        format!(
            "import {{ serve }} from 'runtime:http';\n\
             import {{ env, unmask }} from 'runtime:process';\n\
             serve({{ port: Number(unmask(env.PORT)) }}, () => new Response('{word}'));\n"
        )
    };
    write_in(&dir, "src/api.mjs", &server("V1"));
    write_in(&dir, "src/main.mjs", "document.title = 'page';\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head><script type=\"module\" src=\"./src/main.mjs\"></script></head><body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "build": {{ "targets": {{
                   "web": {{ "entry": "index.html", "outDir": "dist" }},
                   "api": {{ "entry": "src/api.mjs", "out": "dist/api.js" }} }} }},
                 "dev": {{ "servers": {{
                   "web": {{ "port": {web}, "proxy": {{ "/api": "api" }} }},
                   "api": {{ "port": {api} }} }} }} }}"#
        ),
    );
    let _supervisor = start_in(&dir);
    let first = wait_for_http(web, "/api/x", |body| body.contains("V1"));
    assert!(first.contains("V1"), "{first}");

    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let traffic = {
        let done = done.clone();
        std::thread::spawn(move || {
            let mut answers = Vec::new();
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                answers.push(http_get(web, "/api/x"));
            }
            answers
        })
    };
    for word in ["V2", "V3", "V4"] {
        std::thread::sleep(Duration::from_millis(700));
        write_in(&dir, "src/api.mjs", &server(word));
    }
    let last = wait_for_http(web, "/api/x", |body| body.contains("V4"));
    std::thread::sleep(Duration::from_millis(300));
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let answers = traffic.join().expect("the traffic thread");
    assert!(last.contains("V4"), "the last save never landed: {last}");
    assert!(
        answers.len() > 20,
        "too little traffic to mean anything: {}",
        answers.len()
    );
    let failed: Vec<String> = answers
        .iter()
        .filter(|answer| {
            !answer
                .as_deref()
                .is_some_and(|a| a.starts_with("HTTP/1.1 200"))
        })
        .map(|answer| {
            answer
                .clone()
                .unwrap_or_else(|| "no answer".to_string())
                .lines()
                .next()
                .unwrap_or("")
                .to_string()
        })
        .collect();
    assert!(
        failed.is_empty(),
        "{} of {} requests were not answered by the API: {:?}",
        failed.len(),
        answers.len(),
        &failed[..failed.len().min(5)]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// **An edit to a linked package rebuilds** (D234): what the project links
/// from outside it is watched with it, so developing the package against the
/// app is a save, not a restart.
#[cfg(unix)]
#[test]
fn start_rebuilds_for_an_edit_to_a_linked_package() {
    let base = watch_dir("s_linked");
    let app = base.join("app");
    let lib = base.join("lib");
    let port = test_port("s_linked");
    std::fs::create_dir_all(app.join("src")).expect("mkdir app");
    std::fs::create_dir_all(app.join("node_modules")).expect("mkdir node_modules");
    std::fs::create_dir_all(&lib).expect("mkdir lib");
    write_in(
        &lib,
        "package.json",
        r#"{"name":"lib","version":"1.0.0","type":"module","exports":"./index.js"}"#,
    );
    write_in(&lib, "index.js", "export const word = 'FIRST';\n");
    std::os::unix::fs::symlink(&lib, app.join("node_modules/lib")).expect("link lib");
    write_in(
        &app,
        "package.json",
        r#"{"name":"app","type":"module","dependencies":{"lib":"link:../lib"}}"#,
    );
    write_in(&app, ".gitignore", "node_modules\n");
    write_in(
        &app,
        "src/main.js",
        "import { word } from 'lib';\ndocument.title = word + ' word';\n",
    );
    write_in(
        &app,
        "index.html",
        "<!doctype html><html><head><script type=\"module\" src=\"./src/main.js\"></script></head><body></body></html>\n",
    );
    write_in(
        &app,
        "esdev.json",
        &format!(
            r#"{{ "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outDir": "dist" }} }} }},
                 "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }} }}"#
        ),
    );
    let _supervisor = start_in(&app);
    let first = wait_for_http(port, "/assets/main.js", |body| body.contains("FIRST"));
    assert!(first.contains("FIRST"), "{first}");
    write_in(&lib, "index.js", "export const word = 'SECOND';\n");
    let second = wait_for_http(port, "/assets/main.js", |body| body.contains("SECOND"));
    assert!(
        second.contains("SECOND"),
        "an edit to the linked package never rebuilt: {second}"
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// **What the project ignores is not watched, and a new directory is.** A
/// write under a `.gitignore`d directory never rebuilds — it is not watched at
/// all — and a file saved into a directory made after the loop started does.
#[test]
fn start_watches_new_directories_and_not_ignored_ones() {
    let dir = watch_dir("s_tree");
    let port = test_port("s_tree");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("generated")).expect("create generated");
    write_in(&dir, ".gitignore", "generated/\n");
    write_in(
        &dir,
        "plugin.mjs",
        r#"
let builds = 0;
export default {
  name: "counter",
  resolve: {
    filter: { id: "virtual:builds" },
    handler: () => ({ id: "virtual:builds", virtual: true }),
  },
  load: {
    filter: { id: "virtual:builds" },
    handler: () => ({ code: `export default ${++builds};` }),
  },
};
"#,
    );
    write_in(
        &dir,
        "src/main.mjs",
        "import builds from 'virtual:builds';\ndocument.title = 'build ' + builds;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head>\
         <script type=\"module\" src=\"./src/main.mjs\"></script></head>\
         <body></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        &format!(
            r#"{{ "plugins": ["./plugin.mjs"],
                 "build": {{ "targets": {{ "web": {{ "entry": "index.html", "outDir": "dist" }} }} }},
                 "dev": {{ "servers": {{ "web": {{ "port": {port} }} }} }} }}"#
        ),
    );

    let _supervisor = start_in(&dir);
    let first = wait_for_http(port, "/assets/main.js", |body| body.contains("build 1"));
    assert!(first.contains("build 1"), "{first}");

    write_in(&dir, "generated/types.mjs", "export const x = 1;\n");
    std::thread::sleep(Duration::from_millis(1500));
    let still = wait_for_http(port, "/assets/main.js", |body| body.contains("build 1"));
    assert!(
        still.contains("build 1"),
        "a write under an ignored directory rebuilt: {still}"
    );

    std::fs::create_dir_all(dir.join("src/feature/deep")).expect("mkdir -p");
    write_in(&dir, "src/feature/deep/x.mjs", "export const y = 2;\n");
    let rebuilt = wait_for_http(port, "/assets/main.js", |body| body.contains("build 2"));
    assert!(
        rebuilt.contains("build 2"),
        "a file in a new directory never rebuilt: {rebuilt}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A test that never settles is a **failure**, not a hang and not an omission.
/// The old epilogue awaited every pending promise, so this file hung forever;
/// with the tally in the host, the case is simply never finished, and a run
/// that reported "1 passed" and exited zero would be lying about the other one.
#[test]
fn a_test_that_never_finishes_fails_the_run() {
    let dir = build_dir("t_unfinished");
    write_in(
        &dir,
        "hangs.test.mjs",
        "import { test, assert } from 'runtime:test';\n\
         test('finishes', () => assert(true));\n\
         test('never finishes', async () => { await new Promise(() => {}); });\n",
    );

    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(!out.status.success(), "{text}");
    assert!(text.contains("FAIL never finishes"), "{text}");
    assert!(text.contains("never finished"), "{text}");
    assert!(text.contains("1 passed, 1 failed"), "{text}");
}

/// The project the `finish` tests share: an HTML target and a server target,
/// and a plugin whose `finish` reports what it was shown into the web output.
fn finish_project(name: &str, plugin: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/main.js", "console.log('MAIN');\n");
    write_in(&dir, "src/server.js", "console.log('SERVER');\n");
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><head><script type=\"module\" src=\"./src/main.js\"></script>\
         </head><body></body></html>\n",
    );
    write_in(&dir, "finish.mjs", plugin);
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./finish.mjs"], "build": {"targets": {
            "web": {"entry": "index.html", "outDir": "dist"},
            "server": {"entry": "src/server.js", "out": "server/server.js"}}}}"#,
    );
    dir
}

/// A plugin finishes a release build (D153): once, after every target, with
/// each target's staged directory and files, in `pre`/unordered/`post` order,
/// and what it writes there is deployed with the rest.
#[test]
fn a_plugin_finishes_a_release_build() {
    let dir = finish_project(
        "p_plugins_finish",
        r#"
import { file, write } from "runtime:fs";

export default {
  name: "report",
  finish: {
    async handler(targets, ctx) {
      const pre = await file(`${targets.web.outDir}/pre.txt`).text();
      const shown = Object.fromEntries(
        Object.entries(targets).map(([name, t]) => [
          name,
          {
            platform: t.platform,
            staged: t.outDir.includes(".esdev-build-"),
            files: t.files.map((f) => `${f.type}:${f.fileName}`),
          },
        ]),
      );
      await write(
        `${targets.web.outDir}/report.json`,
        JSON.stringify({ pre, command: ctx.command, platform: ctx.platform ?? null, shown }),
      );
    },
  },
};
"#,
    );
    // A second plugin ordered `pre`, which the one above reads after.
    write_in(
        &dir,
        "pre.mjs",
        r#"
import { write } from "runtime:fs";
export default {
  name: "pre",
  finish: { order: "pre", handler: (targets) => write(`${targets.web.outDir}/pre.txt`, "first") },
};
"#,
    );
    let config = std::fs::read_to_string(dir.join("esdev.json")).expect("config");
    write_in(
        &dir,
        "esdev.json",
        &config.replace(r#"["./finish.mjs"]"#, r#"["./finish.mjs", "./pre.mjs"]"#),
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let report = std::fs::read_to_string(dir.join("dist/report.json")).expect("deployed");
    assert!(report.contains(r#""pre":"first""#), "{report}");
    assert!(report.contains(r#""command":"build""#), "{report}");
    assert!(report.contains(r#""platform":null"#), "{report}");
    assert!(
        report.contains(r#""web":{"platform":"browser","staged":true"#),
        "{report}"
    );
    assert!(
        report
            .contains(r#""server":{"platform":"server","staged":true,"files":["chunk:server.js"]"#),
        "{report}"
    );
    assert!(report.contains(r#""asset:index.html""#), "{report}");
    assert!(report.contains(r#""chunk:assets/main-"#), "{report}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `finish` that throws fails the build, and what was deployed before is
/// what is still deployed.
#[test]
fn a_failing_finish_deploys_nothing() {
    let dir = finish_project(
        "p_plugins_finish_fails",
        r#"
import { write } from "runtime:fs";
export default {
  name: "prerender",
  finish: {
    async handler(targets) {
      await write(`${targets.web.outDir}/half.html`, "half");
      throw new Error("route /about failed to render");
    },
  },
};
"#,
    );
    std::fs::create_dir_all(dir.join("dist")).expect("dist");
    write_in(&dir, "dist/index.html", "LAST GOOD");

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success(), "{}", stdout(&out));
    let printed = stderr(&out);
    assert!(printed.contains("[plugin prerender] finish"), "{printed}");
    assert!(
        printed.contains("route /about failed to render"),
        "{printed}"
    );
    assert!(!printed.contains(".esdev-build-"), "{printed}");
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/index.html")).expect("kept"),
        "LAST GOOD"
    );
    assert!(!dir.join("dist/half.html").exists());
    assert!(!dir.join("server/server.js").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `--target` builds part of the project, and `finish` is shown that part.
#[test]
fn finish_is_shown_only_the_selected_targets() {
    let dir = finish_project(
        "p_plugins_finish_selected",
        r#"
import { write } from "runtime:fs";
export default {
  name: "names",
  finish: {
    handler: (targets) =>
      write(`${Object.values(targets)[0].outDir}/names.txt`, Object.keys(targets).join(",")),
  },
};
"#,
    );
    let out = esdev_in(&dir)
        .args(["build", "--target=server"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert_eq!(
        std::fs::read_to_string(dir.join("server/names.txt")).expect("written"),
        "server"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
