//! `runtime:build`: the bundler, from guest JS (the other esdev-only module).

use super::*;

/// A plugin can see what the build **produced**. `end` fires when the module
/// graph is finished, before there are chunks at all — which is why it is
/// handed `null` — so route-level modulepreload had no seam to hang on unless
/// the program itself called `build()`.
#[test]
fn runtime_build_hands_a_plugin_the_chunks_it_produced() {
    let dir = build_dir("rb_bundle");
    write_in(&dir, "dep.js", "export const x = 1;\n");
    write_in(
        &dir,
        "main.js",
        "import { x } from './dep.js';\nglobalThis.lazy = () => import('./dep.js');\nconsole.log(x);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
let seen = null;
const plugin = {
  name: "preload",
  bundle: { handler: (output) => { seen = output; } },
};
const bundle = await build({ input: "main.js", plugins: [plugin] });
await bundle.generate({ format: "esm" });
const entry = seen.find((o) => o.type === "chunk" && o.isEntry);
console.log(
  seen.length > 0,
  entry.fileName,
  entry.facadeModuleId.endsWith("main.js"),
  entry.moduleIds.some((id) => id.endsWith("dep.js")),
  entry.code === undefined,
);
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("true main.js true true true"), "{text}");
}

/// `build()`'s `plugins` reads a list the way an `esdev.json` entry does
/// (D214): nested lists flatten, a promise is awaited, `null`, `undefined`
/// and `false` are skipped, in order. A `0` is not skipped — it is a mistake.
#[test]
fn runtime_build_flattens_its_plugin_list() {
    let dir = build_dir("rb_flatten");
    write_in(&dir, "main.js", "console.log(\"trail\");\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const step = (name) => ({
  name,
  transform: {
    filter: { id: /main\.js$/ },
    handler: (code) => ({ code: code.replace(/"trail([^"]*)"/, `"trail$1>${name}"`) }),
  },
});
const bundle = await build({
  input: "main.js",
  plugins: [[step("a"), [undefined, step("b")]], false, null, Promise.resolve(step("c"))],
});
const { output } = await bundle.generate({ format: "esm" });
console.log(output[0].code.includes("trail>a>b>c"));
try {
  await build({ input: "main.js", plugins: [step("a"), 0] });
  console.log("NO ERROR");
} catch (err) {
  console.log(err.message);
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("true\n"), "{text}");
    assert!(
        text.contains("build: plugin 1 is number, not a plugin object"),
        "{text}"
    );
}

/// A failed build says **where**. It used to be a string — the module id, a
/// colon and "Unexpected token" — so an overlay could name the file and then
/// had to stop; the bundler computed the line, the column and the frame all
/// along for its own terminal output.
#[test]
fn runtime_build_reports_where_a_build_failed() {
    let dir = build_dir("rb_failure");
    write_in(&dir, "broken.js", "export const a = 1;\nconst b = ;\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "broken.js" });
try {
  await bundle.generate({ format: "esm" });
  console.log("NO ERROR");
} catch (err) {
  const [first] = err.errors;
  console.log(err.name, err.errors.length, first.kind, first.line, first.column);
  console.log(first.id.endsWith("broken.js"), first.frame.includes("const b = ;"));
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(out.status.success(), "{text}");
    assert!(text.contains("BuildError 1 PARSE_ERROR 2 10"), "{text}");
    assert!(text.contains("true true"), "{text}");
}

/// The whole feature in one test: a plugin serving a **virtual module**
/// (`resolveId` + `load`), a `transform`, an `external` **predicate**, output
/// held in memory, and `watchFiles` covering both what was imported and what a
/// plugin declared. None of that is reachable through a subprocess protocol,
/// which is why the bridge exists.
#[test]
fn runtime_build_bundles_with_a_js_plugin() {
    let dir = build_dir("rb_plugin");
    write_in(&dir, "dep.js", "export const answer = 42;\n");
    write_in(
        &dir,
        "main.js",
        "import { answer } from './dep.js';\nimport hello from 'virtual:greeting';\nconsole.log(hello, answer);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

let transformCalls = 0;

const plugin = {
  name: "greeting",
  // A module that exists on no disk. `virtual: true` rather than the NUL-byte
  // prefix every bundler inherited from rollup.
  resolve: {
    filter: { id: "virtual:greeting" },
    handler: () => ({ id: "virtual:greeting", virtual: true }),
  },
  load: {
    filter: { id: "virtual:greeting" },
    // A dependency the graph cannot discover: nothing imports dep.js *from
    // here*, but this module is built from it. Returned, not declared by a
    // call that can be forgotten.
    handler: () => ({ code: 'export default "hello";', dependsOn: ["dep.js"] }),
  },
  transform: {
    // The filter is matched on the host side, so this handler is entered once
    // — not once per module in the graph.
    filter: { id: /dep\.js$/ },
    handler(code, id, ctx) {
      transformCalls++;
      return { code: code.replace("42", "43") };
    },
  },
};

const bundle = await build({
  input: "main.js",
  plugins: [plugin],
  external: (id) => id.startsWith("runtime:"),
});

const { output, watchFiles } = await bundle.generate({ format: "esm", codeSplitting: false });
console.log("chunks", output.length);
console.log("crossings", transformCalls);
console.log(output[0].code.includes('console.log("hello", 43)') ? "transformed" : output[0].code);
console.log("watched", watchFiles.some((f) => f.endsWith("dep.js")));
await bundle.close();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(printed.contains("chunks 1"), "{printed}");
    // The filter is the difference between one crossing into the isolate and
    // one per module in the graph.
    assert!(printed.contains("crossings 1"), "{printed}");
    assert!(printed.contains("transformed"), "{printed}");
    assert!(printed.contains("watched true"), "{printed}");
}

/// A hook's `this` is the bundler's own context, mid-build: `this.resolve()`
/// asks its resolver, `this.emitFile()` adds to a build already running, and
/// `this.warn()` reaches the caller instead of vanishing into a worker thread.
#[test]
fn runtime_build_hooks_get_the_bundlers_context() {
    let dir = build_dir("rb_context");
    write_in(&dir, "dep.js", "export const answer = 42;\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

// Arrow functions throughout: the context is the last argument, not `this`,
// so an arrow cannot silently lose it.
const plugin = {
  name: "probe",
  resolve: {
    filter: { id: "virtual:entry" },
    handler: async (source, importer, ctx) => {
      const found = await ctx.resolve("./dep.js", importer ?? undefined);
      console.log("resolved", found !== null && found.id.endsWith("dep.js"));
      ctx.warn("a warning from the plugin");
      return { id: "virtual:entry", virtual: true };
    },
  },
  load: {
    filter: { id: "virtual:entry" },
    handler: (id, ctx) => {
      const ref = ctx.emit({ type: "asset", name: "meta.json", source: '{"ok":true}' });
      console.log("emitted", typeof ref === "string");
      return { code: "export default 1;" };
    },
  },
};

const bundle = await build({ input: "virtual:entry", plugins: [plugin] });
const { output, warnings } = await bundle.generate({});
console.log("assets", output.some((o) => o.type === "asset"));
console.log("warned", warnings.some((w) => w.includes("a warning from the plugin")));
await bundle.close();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    for expected in [
        "resolved true",
        "emitted true",
        "assets true",
        "warned true",
    ] {
        assert!(
            printed.contains(expected),
            "{expected} missing from {printed}"
        );
    }
}

/// The declaration is checked where it is written. There is **one** way to
/// declare a hook, and rollup's bare-function shorthand is refused rather than
/// quietly accepted — accepting it would make the filter, the order and the
/// context argument optional extras on somebody else's design.
#[test]
fn runtime_build_refuses_a_malformed_plugin() {
    let dir = build_dir("rb_strict");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const refuse = async (plugin, what) => {
  try {
    await build({ input: "main.js", plugins: [plugin] });
    console.log("NOT REFUSED", what);
  } catch (err) {
    console.log(what, "|", err.message);
  }
};

await refuse({ name: "legacy", transform(code, id) {} }, "bare");
await refuse({ name: "typo", tranform: { handler() {} } }, "typo");
await refuse({ name: "wide", start: { filter: { id: /x/ }, handler() {} } }, "filtered-start");
await refuse({ name: "codef", load: { filter: { code: /x/ }, handler() {} } }, "code-on-load");
await refuse({ name: "ord", transform: { order: "first", handler() {} } }, "order");
await refuse({ name: "none", transform: { filter: { id: /x/ } } }, "no-handler");
// A filter with no key this contract knows would be handed the whole graph,
// so it is refused where it was written rather than honoured as a catch-all.
await refuse({ name: "bare", transform: { filter: /x/, handler() {} } }, "bare-pattern");
await refuse({ name: "typo", transform: { filter: { ID: /x/ }, handler() {} } }, "filter-typo");
await refuse(
  { name: "rollup", transform: { filter: { include: /x/ }, handler() {} } },
  "rollup-key",
);
await refuse({ name: "empty", transform: { filter: {}, handler() {} } }, "empty-filter");
await refuse({ name: "none", transform: { filter: { id: [] }, handler() {} } }, "empty-list");
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    assert!(!printed.contains("NOT REFUSED"), "{printed}");
    // Each rejection says what to write instead, and a misspelling names the
    // hook it was nearly.
    assert!(
        printed.contains("a hook is an object, not a function"),
        "{printed}"
    );
    assert!(printed.contains(r#"Did you mean "transform""#), "{printed}");
    assert!(printed.contains("cannot be filtered"), "{printed}");
    assert!(
        printed.contains("only transform can filter on code"),
        "{printed}"
    );
    assert!(printed.contains(r#""pre" or "post""#), "{printed}");
    assert!(printed.contains("handler must be a function"), "{printed}");
    // The 0.4 spelling names the field it is missing, rather than becoming a
    // filter that matches everything.
    assert!(printed.contains("write { id: /x/ }"), "{printed}");
    assert!(
        printed.contains(r#"unknown key "ID". Did you mean "id"?"#),
        "{printed}"
    );
    assert!(printed.contains(r#"unknown key "include""#), "{printed}");
    assert!(printed.contains("must say what it matches"), "{printed}");
    assert!(printed.contains("empty list"), "{printed}");
}

/// `order` decides which plugin sees a module first — the thing a framework
/// needs when one pass has to run before another.
#[test]
fn runtime_build_runs_hooks_in_the_order_they_asked_for() {
    let dir = build_dir("rb_order");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const seen = [];
const at = (name, order) => ({
  name,
  transform: {
    filter: { id: /main\.js$/ },
    order,
    handler: () => { seen.push(name); return null; },
  },
});

const bundle = await build({
  input: "main.js",
  plugins: [at("normal"), at("pre", "pre"), at("post", "post")],
});
await bundle.generate({});
console.log("order", seen.join(","));
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("order pre,normal,post"),
        "{}",
        stdout(&out)
    );
}

/// A `dependsOn` written the way every other path in a run is written —
/// relative — has to land in `watchFiles` as the same absolute path the graph
/// reports for it. Otherwise the same file appears twice, once as the graph
/// found it and once as the plugin spelled it, and a consumer matching a change
/// against its dependency set misses half the time.
#[test]
fn runtime_build_resolves_a_relative_dependency() {
    let dir = build_dir("rb_deps");
    write_in(&dir, "dep.js", "export const x = 1;\n");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const bundle = await build({
  input: "main.js",
  plugins: [{
    name: "deps",
    transform: {
      filter: { id: /main\.js$/ },
      handler: (code) => ({ code, dependsOn: ["dep.js"] }),
    },
  }],
});
const { watchFiles } = await bundle.generate({});
const dep = watchFiles.filter((f) => f.endsWith("dep.js"));
console.log("absolute", dep.length === 1 && /^(?:[A-Za-z]:[\\/]|\/)/.test(dep[0]));
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("absolute true"), "{}", stdout(&out));
}

/// The passes this toolchain owns are installed in a guest build too, and this
/// is the regression test for the day they were not: `esdev build` scoped
/// `styles.button` and `runtime:build` did not, so the same project produced
/// markup that did not match its own stylesheet depending on which path built
/// it. Both must arrive at the identical scoped name.
#[test]
fn runtime_build_runs_the_same_owned_passes_as_the_subcommand() {
    let dir = build_dir("rb_css");
    write_in(&dir, "s.module.css", ".button { color: red; }\n");
    write_in(
        &dir,
        "app-entry.js",
        "import styles from './s.module.css';\nconsole.log(styles.button);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js" });
const { output } = await bundle.generate({});
const name = /button_[a-f0-9]+/.exec(output[0].code);
console.log("scoped", name === null ? "none" : name[0]);
"#,
    );

    let from_module = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(from_module.status.success(), "{}", stderr(&from_module));

    let from_subcommand = esdev_in(&dir)
        .args(["build", "app-entry.js", "--out=out/x.js"])
        .output()
        .expect("spawn esdev build");
    assert!(
        from_subcommand.status.success(),
        "{}",
        stderr(&from_subcommand)
    );

    let written = std::fs::read_to_string(dir.join("out/x.js")).expect("read the bundle");
    let subcommand_name = regex_find(&written);
    let module_name = stdout(&from_module)
        .lines()
        .find_map(|l| l.strip_prefix("scoped ").map(str::to_string))
        .expect("the module build printed a name");
    assert_eq!(
        module_name, subcommand_name,
        "the two build paths disagree about the scoped name"
    );
}

/// The first `button_<hash>` in some text. Written by hand rather than with a
/// regex crate: this file drives a binary, and a test dependency to find eight
/// hex digits is not worth the graph.
fn regex_find(text: &str) -> String {
    let at = text.find("button_").expect("a scoped name in the bundle");
    let rest = &text[at..];
    let end = rest
        .char_indices()
        .find(|(i, c)| *i > 7 && !c.is_ascii_hexdigit())
        .map_or(rest.len(), |(i, _)| i);
    rest[..end].to_string()
}

/// `runtime:build`'s `resolve(specifier, from)` resolves as an `import` written
/// at `from` would: a tool finds the project's copy of a package rather than
/// its own nested one. `import.meta.resolve` stays the standard one-argument
/// form, and the production binary serves neither.
#[test]
fn runtime_build_resolves_from_the_project() {
    let dir = build_dir("rb_resolve");
    for pkg in ["node_modules/pkg", "tool/node_modules/pkg"] {
        std::fs::create_dir_all(dir.join(pkg)).expect("mkdir");
        write_in(
            &dir.join(pkg),
            "package.json",
            r#"{ "name": "pkg", "type": "module", "exports": "./i.js" }"#,
        );
        write_in(&dir.join(pkg), "i.js", "export default 1;\n");
    }
    write_in(&dir, "package.json", r#"{ "name": "project" }"#);
    write_in(
        &dir.join("tool"),
        "cli.js",
        "import { resolve } from 'runtime:build';\n\
         const root = new URL('../', import.meta.url);\n\
         console.log(import.meta.resolve('pkg').includes('/tool/node_modules/'));\n\
         console.log(!resolve('pkg', root).includes('/tool/'));\n\
         console.log(resolve('pkg', root.href) === resolve('pkg', root));\n\
         console.log(import.meta.resolve.length);\n\
         try { resolve('pkg', './relative/'); } catch (e) { console.log(e instanceof TypeError); }\n",
    );
    let out = esdev_in(&dir)
        .arg("tool/cli.js")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "true\ntrue\ntrue\n1\ntrue\n");

    let denied = esdev_in(&dir)
        .args(["--deny-all", "tool/cli.js"])
        .output()
        .expect("spawn esdev");
    assert!(!denied.status.success());
    assert!(stderr(&denied).contains("imports"), "{}", stderr(&denied));
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A guest build asserts what the subcommand asserts.** The two used to
/// disagree: `esdev build` names the `worker` condition and the `module`/`main`
/// fields, and `runtime:build` named neither unless the caller did — so the
/// same project resolved to a package's `node:` build one way and its Web build
/// the other. Nothing fails at build time when that happens; the bundle is
/// produced and dies later on an import this runtime does not have.
#[test]
fn runtime_build_asserts_the_same_conditions_as_the_subcommand() {
    let dir = build_dir("rb_conditions");
    write_resolution_fixture(&dir);
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js" });
const { output } = await bundle.generate({});
console.log("code", JSON.stringify(output[0].code));
"#,
    );

    let from_module = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(from_module.status.success(), "{}", stderr(&from_module));
    let from_module = stdout(&from_module);

    let from_subcommand = esdev_in(&dir)
        .args(["build", "app-entry.js", "--out=out/x.js"])
        .output()
        .expect("spawn esdev build");
    assert!(
        from_subcommand.status.success(),
        "{}",
        stderr(&from_subcommand)
    );
    let written = std::fs::read_to_string(dir.join("out/x.js")).expect("read the bundle");

    for (what, code) in [("runtime:build", &from_module), ("esdev build", &written)] {
        assert!(
            code.contains("worker-build"),
            "{what} resolved `dual` to the wrong build: {code}"
        );
        assert!(
            !code.contains("node-build"),
            "{what} took the package's Node build: {code}"
        );
        assert!(
            code.contains("legacy-esm"),
            "{what} could not resolve a package with no `exports` map: {code}"
        );
    }
}

/// Naming a condition adds to what we assert rather than replacing it. A caller
/// that wants `development` should not lose the condition that decides which
/// half of React it gets.
#[test]
fn runtime_build_appends_the_callers_conditions() {
    let dir = build_dir("rb_conditions_extra");
    write_resolution_fixture(&dir);
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({
  input: "app-entry.js",
  resolve: { conditionNames: ["development"] },
});
const { output } = await bundle.generate({});
console.log("code", JSON.stringify(output[0].code));
"#,
    );

    let run = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(run.status.success(), "{}", stderr(&run));
    let code = stdout(&run);
    assert!(code.contains("worker-build"), "{code}");
}

/// **This toolchain's own passes go through the same contract.** The CSS
/// Modules pass used to be written against the bundler's trait, which meant the
/// contract had one implementation and no way to check that it was a contract
/// at all. It is a `Pass` now, and this is the observable consequence: it
/// returns the stylesheets it read through `dependsOn`, so they arrive in the
/// guest's `watchFiles` beside everything the module graph found by itself.
///
/// Nothing imports an `@import`ed stylesheet or a `composes … from` target —
/// the reference is inside the CSS — so before this a save to either rebuilt
/// nothing and the page kept the rules it had.
#[test]
fn runtime_build_watches_the_stylesheets_a_css_module_read() {
    let dir = build_dir("rb_css_deps");
    write_in(&dir, "base.css", ".shared { padding: 4px; }\n");
    write_in(
        &dir,
        "shared.module.css",
        ".pill { border-radius: 999px; }\n",
    );
    write_in(
        &dir,
        "s.module.css",
        "@import \"./base.css\";\n.button { composes: pill from \"./shared.module.css\"; color: red; }\n",
    );
    write_in(
        &dir,
        "app-entry.js",
        "import styles from './s.module.css';\nconsole.log(styles.button);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "app-entry.js" });
const { watchFiles } = await bundle.generate({});
for (const file of watchFiles) console.log("watch", file);
"#,
    );

    let run = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(run.status.success(), "{}", stderr(&run));
    let watched = stdout(&run);
    assert!(
        watched.contains("base.css"),
        "the @import'ed stylesheet is not watched: {watched}"
    );
    assert!(
        watched.contains("shared.module.css"),
        "the composed module is not watched: {watched}"
    );
}

/// A plugin that throws fails the build **with its own message**. The hook ran
/// on a different thread from the bundler; an error that arrived as "build
/// failed" would be the worst possible outcome of that.
#[test]
fn runtime_build_reports_what_a_plugin_threw() {
    let dir = build_dir("rb_throw");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({
  input: "main.js",
  plugins: [
    { name: "boom", transform: { handler() { throw new Error("plugin exploded"); } } },
  ],
});
try {
  await bundle.generate({});
  console.log("NOT REACHED");
} catch (err) {
  console.log(String(err.message).includes("plugin exploded") ? "reported" : err.message);
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("reported"), "{}", stdout(&out));
}

/// A plugin failure says **where**: the module the hook was called about, and
/// the plugin that reported it.
///
/// The backend carries neither. A hook returns an `anyhow::Error` — that is the
/// whole of the error type its signatures have — and what comes back out is a
/// diagnostic with `id: null`, `plugin: null`, a message that is the anyhow
/// chain (`plugin `x` threw an error / Caused by: …`) around a stack through
/// `runtime:build`'s own dispatcher, and a "frame" that is the same sentence a
/// second time behind a banner. An editor overlay had nothing to point at.
#[test]
fn runtime_build_says_which_module_a_plugin_failed_in() {
    let dir = build_dir("rb_blame");
    write_in(&dir, "main.js", "import \"./dep.js\";\n");
    write_in(&dir, "dep.js", "export const x = 1;\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";

const reported = {
  name: "afw",
  transform: {
    filter: { id: /dep\.js$/ },
    handler(code, id, ctx) {
      ctx.error("cannot compile this route");
    },
  },
};
const crashed = {
  name: "afw",
  transform: { filter: { id: /dep\.js$/ }, handler() { return nope.missing; } },
};

for (const [what, plugin] of [["reported", reported], ["crashed", crashed]]) {
  const bundle = await build({ input: "main.js", plugins: [plugin] });
  try {
    await bundle.generate({});
    console.log("NOT REACHED");
  } catch (err) {
    for (const e of err.errors) {
      const first = e.message.split(String.fromCharCode(10))[0];
      console.log(what, "|", e.plugin, "|", (e.id ?? "").split(/[\\\\/]/).pop(), "|", e.frame, "|", first);
    }
  }
  await bundle.close();
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    // What ctx.error() said, attributed — and nothing else. Not a stack through
    // the dispatcher that carried it, and not a frame that is the message again.
    assert!(
        printed.contains("reported | afw | dep.js | null | cannot compile this route"),
        "{printed}"
    );
    // A plugin that *crashed* is a different failure and keeps its stack: the
    // first frame is the line in the plugin, which is the whole of what helps.
    assert!(
        printed.contains("crashed | afw | dep.js | null | ReferenceError: nope is not defined"),
        "{printed}"
    );
    assert!(!printed.contains("Caused by"), "{printed}");
}

/// Building reads, so it needs `FileRead`; writing the result out needs
/// `FileWrite` as well, and refusing one must not refuse the other.
#[test]
fn runtime_build_is_gated_on_the_filesystem_capabilities() {
    let dir = build_dir("rb_caps");
    write_in(&dir, "main.js", "console.log(1);\n");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
const bundle = await build({ input: "main.js" });
try {
  await bundle.write({ dir: "out" });
  console.log("wrote");
} catch (err) {
  console.log("refused", err.name);
}
const { output } = await bundle.generate({});
console.log("generated", output.length === 1);
"#,
    );

    let denied = esdev_in(&dir)
        .args(["--deny-write", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(denied.status.success(), "{}", stderr(&denied));
    assert!(
        stdout(&denied).contains("refused NotAllowedError"),
        "{}",
        stdout(&denied)
    );
    // The same run still builds: the two grants are separate.
    assert!(
        stdout(&denied).contains("generated true"),
        "{}",
        stdout(&denied)
    );

    let all_denied = esdev_in(&dir)
        .args(["--deny-all", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(!all_denied.status.success(), "{}", stdout(&all_denied));
    assert!(
        stderr(&all_denied).contains("capability denied: FileRead"),
        "{}",
        stderr(&all_denied)
    );
}

/// And, like the watcher, it is `esdev`'s: a production binary that could
/// bundle would have to carry a bundler.
#[test]
fn runtime_build_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = build_dir("rb_esrun");
    let app = write_in(&dir, "app.mjs", "import 'runtime:build';\n");

    let out = Command::new(esrun)
        .current_dir(app.parent().expect("fixture dir"))
        .arg(app.file_name().expect("entry"))
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );
}

/// The two modules are one feature used from two sides, and this is the shape
/// they exist for: a server that bundles a route on demand, keeps the chunk,
/// serves it, and on a save drops **only** the routes that used the changed
/// file — all while staying up. Every part of that is happening at once here,
/// which is the integration the design is really making a claim about: the
/// bundler's hooks run in the same isolate that is answering the requests.
#[test]
fn a_dev_server_can_bundle_watch_and_serve_at_once() {
    let dir = watch_dir("rb_devserver");
    write_in(&dir, "dep.js", "export const answer = 42;\n");
    write_in(
        &dir,
        "main.js",
        "import { answer } from './dep.js';\nconsole.log(answer);\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
import { watch } from "runtime:watch";
import { serve } from "runtime:http";
import { write } from "runtime:fs";

const cache = new Map();

// A hook that awaits: the bundler must wait for this isolate, and this isolate
// must go on answering requests while it does.
const slow = {
  name: "slow",
  transform: {
    filter: { id: /\.js$/ },
    handler: async () => {
      await new Promise((r) => setTimeout(r, 5));
      return null;
    },
  },
};

async function bundleRoute(route) {
  const bundle = await build({ input: route, plugins: [slow] });
  const { output, watchFiles } = await bundle.generate({ codeSplitting: false });
  await bundle.close();
  const entry = { code: output[0].code, deps: new Set(watchFiles) };
  cache.set(route, entry);
  return entry;
}

const server = serve({ port: 0 }, async (req) => {
  const route = new URL(req.url).pathname === "/dep" ? "dep.js" : "main.js";
  const entry = cache.get(route) ?? (await bundleRoute(route));
  return new Response(entry.code);
});
const { port } = await server.addr;

const changes = watch(["."], { recursive: true });
(async () => {
  for await (const { path } of changes) {
    for (const [route, entry] of cache) {
      if (entry.deps.has(path)) cache.delete(route);
    }
  }
})();

const one = await (await fetch(`http://127.0.0.1:${port}/`)).text();
const two = await (await fetch(`http://127.0.0.1:${port}/dep`)).text();
console.log("served", one.includes("console.log") && two.includes("answer"));
console.log("cached", cache.size === 2);

await write("dep.js", "export const answer = 44;\n");
for (let i = 0; i < 40 && cache.size === 2; i++) {
  await new Promise((r) => setTimeout(r, 100));
}
// Only what depended on dep.js was dropped — dep.js's own route and main.js's
// both did, but nothing cleared the map wholesale.
console.log("invalidated", cache.size < 2);

const rebuilt = await (await fetch(`http://127.0.0.1:${port}/`)).text();
console.log("rebuilt", rebuilt.includes("44"));

await changes.close();
await server.stop();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    let printed = stdout(&out);
    for expected in [
        "served true",
        "cached true",
        "invalidated true",
        "rebuilt true",
    ] {
        assert!(
            printed.contains(expected),
            "{expected} missing from {printed}"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
