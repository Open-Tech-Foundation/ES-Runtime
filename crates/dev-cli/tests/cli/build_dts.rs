//! `esdev build --lib --dts-bundle` (D59).

use super::*;

/// A library whose declarations only link correctly if collisions, cycles,
/// re-exports and externals are all handled.
fn dts_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    // Two modules, one name. Only one can keep it.
    write_in(
        &dir,
        "src/a.ts",
        "/** A's own Options. */\n\
         export interface Options {\n\ta: string;\n}\n\
         export interface Wrap {\n\to: Options;\n}\n",
    );
    write_in(
        &dir,
        "src/b.ts",
        "export interface Options {\n\tb: number;\n}\n\
         export type Boxed = {\n\tinner: Options;\n\tlist: Options[];\n};\n",
    );
    // A type cycle, which is ordinary in a tree structure and must not recurse
    // for ever.
    write_in(
        &dir,
        "src/tree.ts",
        "import type { Leaf } from './leaf.js';\n\
         export interface Tree {\n\tchildren: Leaf[];\n}\n",
    );
    write_in(
        &dir,
        "src/leaf.ts",
        "import type { Tree } from './tree.js';\n\
         export interface Leaf {\n\tparent: Tree | null;\n}\n",
    );
    // Reachable only *through* a public type — it has to be inlined, and it
    // must not become part of the package's surface.
    write_in(
        &dir,
        "src/internal.ts",
        "export interface Hidden {\n\th: boolean;\n}\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "import type { Options as AOptions, Wrap } from './a.js';\n\
         import type { Boxed } from './b.js';\n\
         import type { Tree } from './tree.js';\n\
         import type { Hidden } from './internal.js';\n\
         import type { Outside } from 'a-package';\n\
         export type { Wrap, Boxed, Tree };\n\
         export interface Everything {\n\ta: AOptions;\n\tboxed: Boxed;\n\t\
         tree: Tree;\n\thidden: Hidden;\n\toutside: Outside;\n}\n",
    );
    dir
}

/// `export default function f(): string;` carries no `declare` — the export
/// modifier is what made it a declaration — so inlining it as
/// `function f(): string;` is TS1046, a top-level declaration in a `.d.ts` with
/// neither modifier. Found by bundling `@opentf/std`, where every utility is a
/// default export: 317 of them in one file, and `tsc` rejecting the package.
#[test]
fn dts_bundle_keeps_declare_on_an_inlined_default_export() {
    let dir = build_dir("l_dts_declare");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/splice.ts",
        "export default function splice(s: string): string {\n  return s;\n}\n",
    );
    write_in(
        &dir,
        "src/box.ts",
        "export default class Box {\n  readonly size: number = 1;\n}\n",
    );
    write_in(
        &dir,
        "src/index.ts",
        "export { default as splice } from './splice.js';\n\
         export { default as Box } from './box.js';\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    assert!(text.contains("declare function splice"), "{text}");
    assert!(text.contains("declare class Box"), "{text}");
    // The modifier is added, not doubled: a named export already has one.
    assert!(!text.contains("declare declare"), "{text}");
}

/// An inline `import("./x")` is a reference to another module of the library,
/// and linking resolves import *statements* — so it would survive into a single
/// `index.d.ts` with nothing beside it to resolve to (TS2307, in the consumer's
/// editor, from a build that said it succeeded). It is refused instead, which is
/// what this linker does with every construct it cannot link.
#[test]
fn dts_bundle_refuses_an_import_type_it_cannot_link() {
    let dir = build_dir("l_dts_import_type");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/clock.ts", "export default class Clock {}\n");
    write_in(
        &dir,
        "src/index.ts",
        "export interface Options {\n  \
         readonly at?: import('./clock.js').default;\n}\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("import(\"./clock.js\")"), "{message}");
    // The module is named as it is spelled, not as a specifier joined onto a
    // directory: `src/./index.ts` is what that produces.
    assert!(message.contains("src/index.ts"), "{message}");
    assert!(!message.contains("/./"), "{message}");
    // Surface-neutral: the same refusal reaches a reader who wrote
    // `--dts-bundle` and one who wrote `"dtsBundle": true`.
    assert!(message.contains("declaration bundle"), "{message}");

    // A package is not this library's to link, and is left alone.
    write_in(
        &dir,
        "src/index.ts",
        "export interface Options {\n  \
         readonly at?: import('a-package').Clock;\n}\n",
    );
    let external = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(external.status.success(), "{}", stderr(&external));
    assert!(
        bundled(&dir).contains("import(\"a-package\")"),
        "{}",
        bundled(&dir)
    );
}

#[test]
fn dts_bundle_writes_one_declaration_instead_of_a_tree_of_them() {
    let dir = dts_project("d_one");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev build --lib --dts-bundle");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.d.ts").exists(), "{}", stdout(&out));
    // The per-module declarations are what it replaces, not what it joins.
    assert!(!dir.join("dist/a.d.ts").exists());
    assert!(!dir.join("dist/b.d.ts").exists());
    // The JavaScript tree is untouched: only the declarations were linked.
    assert!(dir.join("dist/a.js").exists());
    assert!(dir.join("dist/index.js").exists());

    let text = bundled(&dir);
    // Nothing relative survives — a bundle that still imported `./a.js` would
    // be a declaration file pointing at declarations that are no longer there.
    assert!(!text.contains("./a.js"), "{text}");
    assert!(!text.contains("./b.js"), "{text}");
}

/// The pass that is easy to get subtly wrong. A missed site leaves a name
/// bound to the wrong declaration, in a file no test of the library runs.
#[test]
fn dts_bundle_renames_a_collision_and_rewrites_every_site_of_it() {
    let dir = dts_project("d_collide");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    // One `Options` keeps the name and the other is suffixed…
    assert!(text.contains("interface Options {"), "{text}");
    assert!(text.contains("interface Options$1 {"), "{text}");
    // …and B's type refers to the renamed one in *both* of its positions, not
    // just the first.
    assert!(text.contains("inner: Options$1;"), "{text}");
    assert!(text.contains("list: Options$1[];"), "{text}");
    // A's `Wrap` still names A's `Options`, unrenamed.
    assert!(text.contains("o: Options;"), "{text}");
}

/// A type only reachable through a public one has to be present, or the public
/// type means nothing — but exporting it would widen the package's surface past
/// what its author wrote.
#[test]
fn dts_bundle_inlines_what_is_reachable_and_exports_only_what_the_entry_did() {
    let dir = dts_project("d_surface");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    assert!(text.contains("interface Hidden {"), "inlined:\n{text}");

    let exports = text
        .lines()
        .find(|line| line.starts_with("export {"))
        .unwrap_or_default();
    for public in ["Wrap", "Boxed", "Tree", "Everything"] {
        assert!(exports.contains(public), "{public} missing from {exports}");
    }
    assert!(!exports.contains("Hidden"), "{exports}");
    assert!(!exports.contains("Options"), "{exports}");
}

/// A tree whose nodes point at their parent is ordinary, and a bundler that
/// followed it naively would not terminate.
#[test]
fn dts_bundle_follows_a_cycle_without_recursing_for_ever() {
    let dir = dts_project("d_cycle");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    assert!(text.contains("interface Tree {"), "{text}");
    assert!(text.contains("interface Leaf {"), "{text}");
    assert!(text.contains("children: Leaf[];"), "{text}");
    assert!(text.contains("parent: Tree | null;"), "{text}");
}

/// The same line `--lib` draws for JavaScript: a dependency stays a dependency.
/// Inlining a package's types would publish a private copy of them.
#[test]
fn dts_bundle_leaves_a_package_as_an_import() {
    let dir = dts_project("d_external");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    let text = bundled(&dir);
    // `import type`, because that is how the source asked for it.
    assert!(
        text.contains("import type { Outside } from \"a-package\";"),
        "{text}"
    );
    assert!(text.contains("outside: Outside;"), "{text}");
}

/// The comments in a declaration file are its documentation — an editor shows
/// them on hover. Carrying declarations as text rather than as an AST is what
/// keeps them, and this is the test that says so.
#[test]
fn dts_bundle_keeps_jsdoc_byte_for_byte() {
    let dir = dts_project("d_jsdoc");
    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(
        bundled(&dir).contains("/** A's own Options. */"),
        "{}",
        bundled(&dir)
    );
}

#[test]
fn dts_bundle_rejects_what_it_cannot_be_asked_for() {
    let dir = dts_project("d_args");

    // Without --lib there are no declarations to link.
    let no_lib = esdev_in(&dir)
        .args(["build", "src/index.ts", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(!no_lib.status.success());
    assert!(stderr(&no_lib).contains("--lib"), "{}", stderr(&no_lib));

    // …and with --no-types there are none either.
    let contradiction = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle", "--no-types"])
        .output()
        .expect("spawn esdev");
    assert!(!contradiction.status.success());
    assert!(
        stderr(&contradiction).contains("opposite"),
        "{}",
        stderr(&contradiction)
    );

    // A default entry that is not there names itself rather than failing later.
    let empty = build_dir("d_args_empty");
    std::fs::create_dir_all(empty.join("src")).expect("create src");
    write_in(&empty, "src/other.ts", "export const x: number = 1;\n");
    let missing = esdev_in(&empty)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("no index.ts"),
        "{}",
        stderr(&missing)
    );
}

/// The honest half. Each of these needs a synthesised namespace to mean the
/// same thing in one file, and a `.d.ts` that is wrong is believed.
#[test]
fn dts_bundle_refuses_a_construct_it_cannot_link_rather_than_guessing() {
    let dir = build_dir("d_unsupported");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/dep.ts", "export const value: number = 1;\n");
    write_in(
        &dir,
        "src/index.ts",
        "import * as everything from './dep.js';\n\
         export const re: typeof everything = everything;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src", "--dts-bundle"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = stderr(&out);
    assert!(message.contains("import * as everything"), "{message}");
    // Surface-neutral: the same refusal reaches a reader who wrote
    // `--dts-bundle` and one who wrote `"dtsBundle": true`.
    assert!(message.contains("declaration bundle"), "{message}");

    // …and the per-module build, which the message points at, still works.
    let per_module = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev");
    assert!(per_module.status.success(), "{}", stderr(&per_module));
    assert!(dir.join("dist/index.d.ts").exists());
    assert!(dir.join("dist/dep.d.ts").exists());
}

/// The two shapes are different enough that guessing between them would be
/// worse than saying so: a file to `--lib` would silently drop every module the
/// entry does not import.
#[test]
fn lib_rejects_the_argument_shapes_that_belong_to_the_other_mode() {
    let dir = lib_project("l_args");

    let file = esdev_in(&dir)
        .args(["build", "--lib", "src/index.ts"])
        .output()
        .expect("spawn esdev");
    assert!(!file.status.success());
    assert!(stderr(&file).contains("--lib src"), "{}", stderr(&file));

    let out_file = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=dist/index.js"])
        .output()
        .expect("spawn esdev");
    assert!(!out_file.status.success());
    assert!(
        stderr(&out_file).contains("directory"),
        "{}",
        stderr(&out_file)
    );

    let no_types_alone = esdev_in(&dir)
        .args(["build", "src/index.ts", "--no-types"])
        .output()
        .expect("spawn esdev");
    assert!(!no_types_alone.status.success());
    assert!(
        stderr(&no_types_alone).contains("--lib"),
        "{}",
        stderr(&no_types_alone)
    );
}
