//! `esdev build --lib` (D59).

use super::*;

/// The layout `tsc` gives and a package's `exports` map is written against.
#[test]
fn lib_mirrors_the_source_tree_rather_than_bundling_it() {
    let dir = lib_project("l_tree");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    assert!(dir.join("dist/index.js").exists(), "{}", stdout(&out));
    assert!(
        dir.join("dist/protocol/codec.js").exists(),
        "{}",
        stdout(&out)
    );
    assert!(dir.join("dist/index.d.ts").exists(), "{}", stdout(&out));
    assert!(
        dir.join("dist/protocol/codec.d.ts").exists(),
        "{}",
        stdout(&out)
    );

    // The module boundary survives: `index.js` imports `codec.js` instead of
    // containing it, which is what makes a subpath export a real file.
    let index = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index");
    assert!(index.contains("./protocol/codec.js"), "inlined:\n{index}");
}

/// The one found by building this repository's own Redis driver: shaking took
/// an export that only a *future* caller uses, and the failure surfaced as a
/// SyntaxError in the consumer rather than anything the build said.
#[test]
fn lib_keeps_an_export_no_other_module_uses() {
    let dir = lib_project("l_exports");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let codec = std::fs::read_to_string(dir.join("dist/protocol/codec.js")).expect("read codec");
    assert!(
        codec.contains("UNUSED_BY_THE_ENTRY"),
        "an export was shaken out of a published module:\n{codec}"
    );

    // And it is genuinely importable, not merely present in the text.
    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    write_in(
        &dir,
        "consumer.mjs",
        "import { UNUSED_BY_THE_ENTRY } from './dist/protocol/codec.js';\n\
         console.log(UNUSED_BY_THE_ENTRY.join(','));\n",
    );
    let ran = Command::new(esrun)
        .current_dir(&dir)
        .arg("--allow-imports")
        .arg("consumer.mjs")
        .output()
        .expect("spawn esrun");
    assert!(ran.status.success(), "{}", stderr(&ran));
    assert_eq!(stdout(&ran).trim(), "a,b");
}

/// Inlining a dependency publishes a private copy of it that no consumer can
/// dedupe, override or patch.
#[test]
fn lib_leaves_dependencies_external() {
    let dir = lib_project("l_external");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    // It resolves nothing, so a dependency that is not even installed is fine.
    assert!(out.status.success(), "{}", stderr(&out));

    let index = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index");
    assert!(index.contains("some-dependency"), "{index}");
}

/// `NODE_ENV` and `worker` are the consuming build's decisions. Baking either
/// one in freezes somebody else's environment into your package.
#[test]
fn lib_defines_nothing_and_asserts_no_condition() {
    let dir = build_dir("l_neutral");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/index.js",
        "export const mode = process.env.NODE_ENV;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));

    let index = std::fs::read_to_string(dir.join("dist/index.js")).expect("read index");
    assert!(
        index.contains("process.env.NODE_ENV"),
        "the consumer's decision was made for them:\n{index}"
    );
    assert!(!index.contains("production"), "{index}");
}

/// A `.d.ts` is what makes the package a typed contract, and it is derived from
/// the annotations the source already carries.
#[test]
fn lib_emits_declarations_and_no_types_skips_them() {
    let dir = lib_project("l_types");

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));
    let declaration = std::fs::read_to_string(dir.join("dist/index.d.ts")).expect("read d.ts");
    assert!(
        declaration.contains("declare function greet"),
        "{declaration}"
    );
    assert!(declaration.contains("string"), "{declaration}");
    // The contract, not the implementation.
    assert!(!declaration.contains("encode(name)"), "{declaration}");
    assert!(stdout(&out).contains("declaration"), "{}", stdout(&out));

    let skipped = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=nodts", "--no-types"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(skipped.status.success(), "{}", stderr(&skipped));
    assert!(dir.join("nodts/index.js").exists());
    assert!(!dir.join("nodts/index.d.ts").exists());
}
