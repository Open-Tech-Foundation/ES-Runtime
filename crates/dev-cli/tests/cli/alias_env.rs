//! `alias` and `import.meta.env`.

use super::*;

/// A project that makes every build flag visible in what it builds: an alias
/// with two places to point, a define it prints, and a package whose export
/// depends on a condition.
fn flag_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    for sub in ["src/a", "src/b", "node_modules/pick"] {
        std::fs::create_dir_all(dir.join(sub)).expect("mkdir");
    }
    write_in(&dir, "package.json", r#"{"name":"flags"}"#);
    write_in(&dir, "src/a/x.ts", "export default 'from a';\n");
    write_in(&dir, "src/b/x.ts", "export default 'from b';\n");
    write_in(
        &dir,
        "node_modules/pick/package.json",
        r#"{ "name": "pick", "type": "module",
             "exports": { ".": { "custom": "./custom.js", "default": "./default.js" } } }"#,
    );
    write_in(
        &dir,
        "node_modules/pick/custom.js",
        "export default 'custom';\n",
    );
    write_in(
        &dir,
        "node_modules/pick/default.js",
        "export default 'default';\n",
    );
    write_in(
        &dir,
        "src/app.ts",
        "import x from '@/x.ts';\nimport which from 'pick';\n\
         declare const GIVEN: string;\n\
         console.log(x, typeof GIVEN === 'undefined' ? 'no define' : GIVEN, which);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{ "resolve": { "alias": { "@": "./src/a" } },
             "build": { "targets": { "app": { "entry": "src/app.ts", "out": "dist/app.js" } } } }"#,
    );
    dir
}

/// **Every flag `esdev build --help` lists is either applied to a project
/// build or refused by it — never accepted and dropped** (DECISIONS D125).
/// Five were, until a project build carried only three of them. The table is
/// checked against the help text, so a flag added without a row fails here.
#[test]
fn every_build_flag_is_applied_or_refused_in_a_project_build() {
    enum Expect {
        /// Built, and running it prints this.
        Prints(&'static str),
        /// Built, and the bundle is checked by the function.
        Bundle(fn(&Path) -> bool),
        /// Refused, naming the flag.
        Refused,
        /// Not a build flag: the help text names it for another reason.
        NotAFlag,
    }
    let table: Vec<(&str, Vec<&str>, Expect)> = vec![
        (
            "--alias",
            vec!["--alias=@=./src/b"],
            Expect::Prints("from b no define default"),
        ),
        (
            "--define",
            vec!["--define=GIVEN=\"yes\""],
            Expect::Prints("from a yes default"),
        ),
        (
            "--conditions",
            vec!["--conditions=custom"],
            Expect::Prints("from a no define custom"),
        ),
        (
            "--target",
            vec!["--target=app"],
            Expect::Prints("from a no define default"),
        ),
        (
            "--config",
            vec!["--config=esdev.json"],
            Expect::Prints("from a no define default"),
        ),
        (
            "--minify",
            vec!["--minify"],
            Expect::Bundle(|dir| {
                let text = std::fs::read_to_string(dir.join("dist/app.js")).unwrap_or_default();
                text.trim_end().lines().count() <= 2
            }),
        ),
        (
            "--sourcemap",
            vec!["--sourcemap"],
            Expect::Bundle(|dir| dir.join("dist/app.js.map").is_file()),
        ),
        (
            "--inventory",
            vec!["--inventory=inventory.json"],
            Expect::Bundle(|dir| {
                std::fs::read_to_string(dir.join("inventory.json"))
                    .is_ok_and(|text| text.contains("\"allow\""))
            }),
        ),
        ("--out", vec!["--out=other.js"], Expect::Refused),
        ("--lib", vec!["--lib"], Expect::Refused),
        ("--format", vec!["--format=cjs"], Expect::Refused),
        ("--no-types", vec!["--no-types"], Expect::Refused),
        ("--dts-bundle", vec!["--dts-bundle"], Expect::Refused),
        ("--css-urls", vec!["--css-urls=inline"], Expect::Refused),
        ("--help", Vec::new(), Expect::NotAFlag),
        // Named in the help's deploy line, which is esrun's.
        ("--allow-imports", Vec::new(), Expect::NotAFlag),
    ];

    let help = esdev()
        .args(["build", "--help"])
        .output()
        .expect("spawn esdev");
    let listed: std::collections::BTreeSet<String> = stdout(&help)
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|word| word.starts_with("--") && word.len() > 2)
        .map(str::to_string)
        .collect();
    for flag in &listed {
        assert!(
            table.iter().any(|(name, _, _)| name == flag),
            "{flag} is in `esdev build --help` and has no row: say what a project build does with it"
        );
    }

    let dir = flag_project("b_flag_table");
    for (flag, args, expect) in &table {
        if matches!(expect, Expect::NotAFlag) {
            continue;
        }
        let _ = std::fs::remove_dir_all(dir.join("dist"));
        let out = esdev_in(&dir)
            .arg("build")
            .args(args)
            .output()
            .expect("spawn esdev build");
        match expect {
            Expect::Refused => {
                assert!(
                    !out.status.success(),
                    "{flag} was accepted by a project build"
                );
                assert!(stderr(&out).contains(flag), "{flag}: {}", stderr(&out));
            }
            Expect::Prints(text) => {
                assert!(out.status.success(), "{flag}: {}", stderr(&out));
                let ran = esdev_in(&dir)
                    .arg("dist/app.js")
                    .output()
                    .expect("spawn esdev");
                assert_eq!(stdout(&ran).trim(), *text, "{flag}: {}", stderr(&ran));
            }
            Expect::Bundle(check) => {
                assert!(out.status.success(), "{flag}: {}", stderr(&out));
                assert!(check(&dir), "{flag} was accepted and did nothing");
            }
            Expect::NotAFlag => unreachable!(),
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every flag `esdev test --help` and `esdev start --help` advertise is one
/// the command's parser takes. Each is given, then a flag nothing takes, so
/// the parser stops before anything runs: the refusal names the second flag
/// when the first was understood. What the file's `test` section says is held
/// by the compiler instead — `resolve_test` names every key it has.
#[test]
fn every_advertised_test_and_start_flag_is_understood() {
    // An option row: indented, the flag (after a short alias, if any), then a
    // gap of two or more spaces before its description. Prose that merely
    // starts with a flag has no such gap.
    let row = regex::Regex::new(r"^ {4}(?:-\w, )?(--[^\s]+) {2,}\S").expect("pattern");
    for command in ["test", "start"] {
        let help = esdev()
            .args([command, "--help"])
            .output()
            .expect("spawn esdev");
        let text = stdout(&help);
        let mut checked = 0;
        for line in text.lines() {
            let Some(found) = row.captures(line) else {
                continue;
            };
            let flag = found.get(1).map_or("", |m| m.as_str());
            if flag == "--help" {
                continue;
            }
            let (name, rest) = match flag.find(['=', '[']) {
                Some(at) => flag.split_at(at),
                None => (flag, ""),
            };
            // A family written as a placeholder — `--allow-<name>` — is the
            // permission grammar's, checked by its own tests.
            if name.contains('<') {
                continue;
            }
            // A required value is given one; an optional one is left out.
            let given = if rest.starts_with('=') {
                format!("{name}=1")
            } else {
                name.to_string()
            };
            let out = esdev()
                .args([command, &given, "--no-such-flag-at-all"])
                .output()
                .expect("spawn esdev");
            let err = stderr(&out);
            assert!(
                !err.contains(&format!("unknown option: {name}")),
                "esdev {command} --help lists {name}, and the parser does not take it:\n{err}"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "no flags found in esdev {command} --help:\n{text}"
        );
    }
}

/// A one-off `esdev build <entry>` is still in the project: its `alias`
/// applies, and a flag replaces the one of the same name.
#[test]
fn a_one_off_build_reads_the_projects_aliases() {
    let dir = flag_project("b_one_off_alias");
    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=one/app.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let ran = esdev_in(&dir)
        .arg("one/app.js")
        .output()
        .expect("spawn esdev");
    assert_eq!(stdout(&ran).trim(), "from a no define default");

    let out = esdev_in(&dir)
        .args([
            "build",
            "src/app.ts",
            "--out=two/app.js",
            "--alias=@=./src/b",
        ])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let ran = esdev_in(&dir)
        .arg("two/app.js")
        .output()
        .expect("spawn esdev");
    assert_eq!(stdout(&ran).trim(), "from b no define default");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A library target keeps the specifier its source wrote, whatever alias the
/// project has — the rule `--lib` states, now held for a target too.
#[test]
fn a_library_target_is_built_without_the_projects_alias() {
    let dir = flag_project("b_lib_target_alias");
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"pkg": {"entry": "src", "outDir": "libout", "lib": true, "types": false}}}, "resolve": {"alias": {"@": "./src/a"}}}"#,
    );
    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    // Where the target said: a library target's `outDir` used to be dropped
    // on the way to the bundler, which wrote `./dist` instead.
    assert!(!dir.join("dist").exists());
    let built = std::fs::read_to_string(dir.join("libout/app.js")).expect("the library's app.js");
    assert!(built.contains("@/x"), "{built}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev <file>` reads source the way the project says, as a test and a build
/// of it do: its `jsx`, and its plugins.
#[test]
fn a_module_run_unbundled_reads_the_projects_jsx_and_plugins() {
    let dir = framework_project("b_run_jsx_plugins");
    write_in(
        &dir,
        "run.mjs",
        "import { greeting } from './greeting.jsx';\nconsole.log(greeting);\n",
    );
    let out = esdev_in(&dir).arg("run.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "hi from the plugin");

    let jsx = build_dir("b_run_jsx");
    write_in(&jsx, "esdev.json", r#"{ "jsx": { "factory": "h" } }"#);
    write_in(
        &jsx,
        "app.jsx",
        "const h = (tag) => tag;\nconsole.log(<section />);\n",
    );
    let out = esdev_in(&jsx).arg("app.jsx").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "section");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&jsx);
}

/// A project whose imports go through both kinds of alias: `esdev.json`'s
/// and `tsconfig.json`'s `paths`, the second one through `extends`.
fn aliased_project(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src/lib")).expect("create src");
    std::fs::create_dir_all(dir.join("src/db")).expect("create src");
    write_in(&dir, "package.json", r#"{"name":"aliased"}"#);
    write_in(
        &dir,
        "src/lib/util.ts",
        "export const help = (): string => 'helped';\n",
    );
    write_in(
        &dir,
        "src/db/index.ts",
        "export const db = (): string => 'the db';\n",
    );
    write_in(
        &dir,
        "tsconfig.base.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "~/*": ["src/*"] } } }"#,
    );
    write_in(
        &dir,
        "tsconfig.json",
        r#"{ "extends": "./tsconfig.base.json" }"#,
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"resolve": {"alias": {"@db": "./src/db"}}}"#,
    );
    dir
}

/// A JavaScript project states its `paths` in `jsconfig.json`, as
/// TypeScript's own tools read it: honoured by the build, a run and a test
/// alike, when there is no `tsconfig.json` beside it.
#[test]
fn a_jsconfig_names_the_paths_of_a_javascript_project() {
    let dir = build_dir("b_jsconfig");
    std::fs::create_dir_all(dir.join("src/lib")).expect("mkdir");
    write_in(&dir, "package.json", r#"{"name":"js"}"#);
    write_in(
        &dir,
        "jsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "~/*": ["src/*"] } } }"#,
    );
    write_in(&dir, "src/lib/two.js", "export const two = () => 2;\n");
    write_in(
        &dir,
        "src/app.js",
        "import { two } from '~/lib/two.js';\nconsole.log(two());\n",
    );
    write_in(
        &dir,
        "src/two.test.js",
        "import { test, expect } from 'runtime:test';\nimport { two } from '~/lib/two.js';\n\
         test('two', () => expect(two()).toBe(2));\n",
    );

    let ran = esdev_in(&dir)
        .arg("src/app.js")
        .output()
        .expect("spawn esdev");
    assert_eq!(stdout(&ran).trim(), "2", "{}", stderr(&ran));
    let tested = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(
        tested.status.success(),
        "{}{}",
        stdout(&tested),
        stderr(&tested)
    );
    let built = esdev_in(&dir)
        .args(["build", "src/app.js", "--out=dist/app.js"])
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("the bundle");
    assert!(
        !bundle.contains("~/lib"),
        "the alias was left for the runtime: {bundle}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// One resolution rule for every importer (D196): a project inside a folder
/// whose own `tsconfig.json` maps the same package. A `.jsx` page (no
/// `allowJs`) and an `.mdx` page a plugin compiles both take the project's
/// `paths`, never the parent's; and `esdev.json`'s `alias` comes before
/// `paths` in the bundle as it does in `esdev <file>`. The report this comes
/// from had two copies of one package in one page.
#[test]
fn every_importer_resolves_through_the_projects_tsconfig_and_alias() {
    let root = build_dir("b_one_rule");
    for sub in [
        "packages/pkg",
        "app/node_modules/pkg",
        "app/vendor",
        "app/src",
    ] {
        std::fs::create_dir_all(root.join(sub)).expect("mkdir");
    }
    write_in(
        &root,
        "tsconfig.json",
        r#"{ "compilerOptions": { "allowJs": true, "paths": { "pkg": ["./packages/pkg/index.js"] } } }"#,
    );
    write_in(
        &root,
        "packages/pkg/index.js",
        "export const who = 'parent';\n",
    );
    let dir = root.join("app");
    write_in(&dir, "package.json", r#"{"name":"app","type":"module"}"#);
    write_in(
        &dir,
        "tsconfig.json",
        r#"{ "compilerOptions": { "paths": { "pkg": ["./node_modules/pkg/index.js"] } } }"#,
    );
    write_in(
        &dir,
        "node_modules/pkg/package.json",
        r#"{"name":"pkg","main":"index.js"}"#,
    );
    write_in(
        &dir,
        "node_modules/pkg/index.js",
        "export const who = 'own';\n",
    );
    write_in(&dir, "vendor/pkg.js", "export const who = 'alias';\n");
    write_in(
        &dir,
        "mdx.js",
        "export default { name: 'mdx', transform: { filter: { id: /\\.mdx$/ }, handler() {\n\
           return { code: \"import { who } from 'pkg'; export const page = 'mdx:' + who;\", type: 'js' };\n\
         } } };\n",
    );
    write_in(
        &dir,
        "src/a.jsx",
        "import { who } from 'pkg';\nexport const page = 'jsx:' + who;\n",
    );
    write_in(&dir, "src/b.mdx", "# b\n");
    write_in(
        &dir,
        "src/main.js",
        "import { page as a } from './a.jsx';\nimport { page as b } from './b.mdx';\n\
         import { who } from 'pkg';\nconsole.log(a, b, 'js:' + who);\n",
    );
    let built = |config: &str| {
        write_in(&dir, "esdev.json", config);
        let out = esdev_in(&dir)
            .args(["build", "src/main.js", "--out=dist/main.js"])
            .output()
            .expect("spawn esdev build");
        assert!(out.status.success(), "{}", stderr(&out));
        std::fs::read_to_string(dir.join("dist/main.js")).expect("the bundle")
    };

    let bundle = built(r#"{ "plugins": ["./mdx.js"] }"#);
    assert!(
        bundle.contains(r#""jsx:own", "mdx:own", "js:own""#),
        "every importer takes the project's paths: {bundle}"
    );
    assert!(!bundle.contains("parent"), "{bundle}");

    let bundle = built(
        r#"{ "plugins": ["./mdx.js"], "resolve": { "alias": { "pkg": "./vendor/pkg.js" } } }"#,
    );
    // Every importer reads the aliased file and none the package `paths`
    // names. Whether the bundler folds the constant into each string is its
    // business, so the module is what is looked for, not the folded text.
    assert!(
        bundle.contains("// vendor/pkg.js") && bundle.contains(r#""alias""#),
        "the alias comes before paths: {bundle}"
    );
    assert!(
        !bundle.contains(r#""own""#) && !bundle.contains("own\""),
        "{bundle}"
    );
    for importer in ["jsx:", "mdx:", "js:"] {
        assert!(bundle.contains(importer), "{importer} is missing: {bundle}");
    }
    write_in(
        &dir,
        "src/run.js",
        "import { who } from 'pkg';\nconsole.log(who);\n",
    );
    let ran = esdev_in(&dir)
        .arg("src/run.js")
        .output()
        .expect("spawn esdev");
    assert_eq!(stdout(&ran).trim(), "alias", "{}", stderr(&ran));

    // An alias that names nothing is an error, not the package as written.
    write_in(
        &dir,
        "esdev.json",
        r#"{ "plugins": ["./mdx.js"], "resolve": { "alias": { "pkg": "./nowhere/pkg.js" } } }"#,
    );
    let out = esdev_in(&dir)
        .args(["build", "src/main.js", "--out=dist/main.js"])
        .output()
        .expect("spawn esdev build");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(r#"the alias "pkg" points it at"#),
        "{}",
        stderr(&out)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// `esdev test` resolves an import the way the build does: through
/// `tsconfig.json`'s `paths` and `esdev.json`'s `alias` — and so does
/// `mock.module`, which has to replace the module the import reaches.
#[test]
fn test_resolves_the_projects_aliases() {
    let dir = aliased_project("b_alias_test");
    write_in(
        &dir,
        "src/app.test.ts",
        "import { test, expect } from 'runtime:test';\n\
         import { help } from '~/lib/util.js';\n\
         import { db } from '@db';\n\
         test('aliases', () => {\n\
           expect(help()).toBe('helped');\n\
           expect(db()).toBe('the db');\n\
         });\n",
    );
    write_in(
        &dir,
        "src/mocked.test.ts",
        "import { test, expect, mock } from 'runtime:test';\n\
         mock.module('@db', () => ({ db: () => 'mocked' }));\n\
         import { db } from '~/db/index.ts';\n\
         test('the alias names the module the mock replaces', () => expect(db()).toBe('mocked'));\n",
    );
    let out = esdev_in(&dir)
        .arg("test")
        .output()
        .expect("spawn esdev test");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(stdout(&out).contains("2 files passed"), "{}", stdout(&out));
    let _ = std::fs::remove_dir_all(&dir);
}

/// `esdev <file>` too — and `esrun` still does not: what it runs is a bundle,
/// whose specifiers the build already settled.
#[test]
fn a_module_run_unbundled_resolves_the_projects_aliases() {
    let dir = aliased_project("b_alias_run");
    write_in(
        &dir,
        "src/app.ts",
        "import { help } from '~/lib/util';\nimport { db } from '@db';\n\
         console.log(help(), db());\n",
    );
    let out = esdev_in(&dir)
        .arg("src/app.ts")
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "helped the db\n");

    let built = esdev_in(&dir)
        // A one-off build reads no project, so it is told the alias.
        .args([
            "build",
            "src/app.ts",
            "--out=dist/app.js",
            "--alias=@db=./src/db",
        ])
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));
    let Some(esrun) = sibling_binary("esrun") else {
        return;
    };
    let bundled = Command::new(&esrun)
        .current_dir(&dir)
        .arg("dist/app.js")
        .output()
        .expect("spawn esrun");
    assert_eq!(stdout(&bundled), "helped the db\n", "{}", stderr(&bundled));
    let unbundled = Command::new(&esrun)
        .current_dir(&dir)
        .args(["--allow-imports", "src/app.ts"])
        .output()
        .expect("spawn esrun");
    assert!(!unbundled.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `@/db` is how most of the ecosystem's source is written, and without this it
/// was an unresolved import.
#[test]
fn an_alias_is_rewritten_before_it_is_resolved() {
    let dir = build_dir("b_alias");
    std::fs::create_dir_all(dir.join("src/lib")).expect("create src");
    write_in(
        &dir,
        "src/lib/util.ts",
        "export const help = (): string => 'helped';\n",
    );
    write_in(
        &dir,
        "src/server.ts",
        "import { help } from '@/lib/util.js';\nexport default help();\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"resolve\": { \"alias\": { \"@\": \"./src\" } },\n  \
         \"build\": { \"targets\": { \"server\": { \"entry\": \"src/server.ts\", \
         \"out\": \"dist/server.js\" } } } }\n",
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/server.js")).expect("read bundle");
    assert!(bundle.contains("helped"), "{bundle}");

    // …and the same rewrite off the command line, for an entry with no project
    // around it.
    let flagged = esdev_in(&dir)
        .args([
            "build",
            "src/server.ts",
            "--out=flag/server.js",
            "--alias=@=./src",
        ])
        .output()
        .expect("spawn esdev build");
    assert!(flagged.status.success(), "{}", stderr(&flagged));
    assert!(
        std::fs::read_to_string(dir.join("flag/server.js"))
            .expect("read bundle")
            .contains("helped")
    );

    // A published library keeps the specifier its source wrote — the consuming
    // build resolves it, and it has never heard of this alias.
    let lib = esdev_in(&dir)
        .args(["build", "--lib", "src", "--out=libout", "--alias=@=./src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!lib.status.success(), "{}", stdout(&lib));
    assert!(
        stderr(&lib).contains("--lib does not bundle"),
        "{}",
        stderr(&lib)
    );
}

/// A browser bundle cannot read the environment at run time, so what it is
/// configured with has to be compiled in — and what is compiled in is public by
/// definition, which is what the prefix says out loud.
#[test]
fn import_meta_env_carries_the_public_variables_and_no_others() {
    let dir = build_dir("b_env");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        ".env",
        "PUBLIC_API=https://api.example.com\nSECRET_KEY=do-not-ship\n",
    );
    write_in(
        &dir,
        "src/app.ts",
        "export const api = import.meta.env.PUBLIC_API;\n\
         export const mode = import.meta.env.MODE;\n\
         export const dev = import.meta.env.DEV;\n\
         export const all = import.meta.env;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=dist/app.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");

    assert!(bundle.contains("https://api.example.com"), "{bundle}");
    assert!(bundle.contains("\"production\""), "{bundle}");
    // A boolean, not the string "false": `if (import.meta.env.DEV)` on a string
    // is a branch that always runs.
    assert!(bundle.contains("dev = false"), "{bundle}");
    // The object itself is replaced too, so destructuring it works.
    assert!(bundle.contains("\"PROD\": true"), "{bundle}");
    // The one that matters: what was not marked public did not travel.
    assert!(!bundle.contains("do-not-ship"), "{bundle}");
    assert!(!bundle.contains("SECRET_KEY"), "{bundle}");
}

/// The file is what the machine has checked out; the environment is what that
/// machine was told. The second wins.
#[test]
fn the_environment_beats_the_env_file() {
    let dir = build_dir("b_env_order");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, ".env", "PUBLIC_API=from-the-file\n");
    write_in(
        &dir,
        "src/app.ts",
        "export const api = import.meta.env.PUBLIC_API;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/app.ts", "--out=dist/app.js"])
        .env("PUBLIC_API", "from-the-environment")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    let bundle = std::fs::read_to_string(dir.join("dist/app.js")).expect("read bundle");
    assert!(bundle.contains("from-the-environment"), "{bundle}");
    assert!(!bundle.contains("from-the-file"), "{bundle}");
}

/// A library is an input to somebody else's build, and which environment the
/// code runs in is that build's to decide — the same rule that keeps `--lib`
/// from defining `NODE_ENV`.
#[test]
fn a_library_compiles_in_no_environment() {
    let dir = build_dir("b_env_lib");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, ".env", "PUBLIC_API=https://api.example.com\n");
    write_in(
        &dir,
        "src/index.ts",
        "export const api: unknown = import.meta.env.PUBLIC_API;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(out.status.success(), "{}", stderr(&out));
    let module = std::fs::read_to_string(dir.join("dist/index.js")).expect("read module");
    assert!(module.contains("import.meta.env.PUBLIC_API"), "{module}");
    assert!(!module.contains("api.example.com"), "{module}");
}

/// A `tsconfig.json` above the project is not the project's: a monorepo's
/// `paths` would reroute the project's imports to the monorepo's source, and
/// a package the project installed would load twice. A project's own
/// tsconfig still decides `paths` (DECISIONS D222).
#[test]
fn a_tsconfig_above_the_project_does_not_reroute_its_imports() {
    let parent = build_dir("tsconfig_boundary");
    let app = parent.join("app");
    for sub in ["app/node_modules/shared", "app/src", "source"] {
        std::fs::create_dir_all(parent.join(sub)).expect("mkdir");
    }
    // The enclosing repository: its tsconfig maps `shared` to its own source.
    write_in(
        &parent,
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "shared": ["./source/shared.js"] } } }"#,
    );
    write_in(
        &parent,
        "source/shared.js",
        "export const id = \"MONOREPO-SOURCE\";\n",
    );
    write_in(
        &app,
        "node_modules/shared/package.json",
        r#"{ "name": "shared", "type": "module", "exports": "./index.js" }"#,
    );
    write_in(
        &app,
        "node_modules/shared/index.js",
        "export const id = \"INSTALLED\";\n",
    );
    write_in(
        &app,
        "src/main.js",
        "import { id } from \"shared\";\nconsole.log(id);\n",
    );
    write_in(
        &app,
        "package.json",
        r#"{ "name": "app", "type": "module" }"#,
    );
    write_in(
        &app,
        "esdev.json",
        r#"{ "build": { "targets": { "app": { "entry": "src/main.js", "out": "dist/main.js" } } } }"#,
    );
    let built = esdev_in(&app)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));
    let bundle = std::fs::read_to_string(app.join("dist/main.js")).expect("the bundle");
    assert!(bundle.contains("INSTALLED"), "{bundle}");
    assert!(
        !bundle.contains("MONOREPO-SOURCE"),
        "the enclosing tsconfig rerouted it:\n{bundle}"
    );

    // With a tsconfig of its own, the project's `paths` apply.
    write_in(
        &app,
        "tsconfig.json",
        r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "shared": ["./src/local.js"] } } }"#,
    );
    write_in(
        &app,
        "src/local.js",
        "export const id = \"PROJECT-PATH\";\n",
    );
    let built = esdev_in(&app)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));
    let bundle = std::fs::read_to_string(app.join("dist/main.js")).expect("the bundle");
    assert!(
        bundle.contains("PROJECT-PATH"),
        "the project's own paths: {bundle}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// `build({ mode })` compiles `import.meta.env` as `esdev build` does — the
/// mode, `.env`, then the environment, `PUBLIC_*` only, the program's own
/// `define` winning — and reads both sources as the program (D223).
#[test]
fn runtime_build_with_a_mode_compiles_import_meta_env_as_the_program_reads_it() {
    let dir = build_dir("rb_env");
    write_in(
        &dir,
        ".env",
        "PUBLIC_FILE=from-file\nPUBLIC_BOTH=file\nSECRET_KEY=never\n",
    );
    write_in(
        &dir,
        "main.js",
        "console.log(JSON.stringify([import.meta.env.MODE, import.meta.env.DEV, import.meta.env.PUBLIC_FILE, import.meta.env.PUBLIC_BOTH, import.meta.env.PUBLIC_SHELL, import.meta.env.PUBLIC_MINE, import.meta.env.SECRET_KEY ?? null]));\n",
    );
    write_in(
        &dir,
        "app.mjs",
        r#"
import { build } from "runtime:build";
import { args } from "runtime:process";
import { write } from "runtime:fs";
const options = { input: "main.js", define: { "import.meta.env.PUBLIC_MINE": '"own"' } };
if (args[0] !== "none") options.mode = args[0];
try {
  const bundle = await build(options);
  const { output } = await bundle.generate({});
  const code = output.find((c) => c.type === "chunk").code;
  console.log("CODE", code.includes("import.meta.env.PUBLIC") ? "unreplaced" : "replaced", code.includes("never") ? "LEAK" : "clean");
  await write("out.mjs", code);
  await import(new URL("./out.mjs", import.meta.url).href + "?" + args[0]);
} catch (err) {
  console.log("REFUSED", err.name, err.message);
}
"#,
    );
    let run = |flags: &[&str], mode: &str, extra: &[(&str, &str)]| {
        let mut command = esdev_in(&dir);
        command
            .args(flags)
            .arg("app.mjs")
            .arg(mode)
            .env("PUBLIC_SHELL", "from-shell")
            .env("PUBLIC_BOTH", "shell");
        for (name, value) in extra {
            command.env(name, value);
        }
        let out = command.output().expect("spawn esdev");
        assert!(out.status.success(), "{}", stderr(&out));
        stdout(&out)
    };

    let production = run(&[], "production", &[]);
    assert!(production.contains("CODE replaced clean"), "{production}");
    assert!(
        production.contains(r#"["production",false,"from-file","shell","from-shell","own",null]"#),
        "{production}"
    );
    let development = run(&[], "development", &[]);
    assert!(
        development.contains(r#"["development",true,"#),
        "{development}"
    );

    // No mode: nothing is read, and nothing but the program's define compiled in.
    let none = run(&[], "none", &[]);
    assert!(none.contains("CODE unreplaced clean"), "{none}");

    // A scoped grant compiles in only the names it lists.
    let scoped = run(
        &[
            "--deny-all",
            "--allow-read",
            "--allow-write",
            "--allow-imports",
            "--allow-env=PUBLIC_SHELL",
        ],
        "production",
        &[],
    );
    assert!(
        scoped.contains(r#"["production",false,"from-file","file","from-shell","own",null]"#),
        "{scoped}"
    );
    // No env grant: refused, naming the grant — not the environment handed over.
    let denied = run(
        &[
            "--deny-all",
            "--allow-read",
            "--allow-write",
            "--allow-imports",
        ],
        "production",
        &[],
    );
    assert!(denied.contains("REFUSED NotAllowedError"), "{denied}");
    assert!(denied.contains("--allow-env"), "{denied}");
    // `PUBLIC_` says the value is public: a name the runtime would mask as a
    // secret is compiled in like any other, not refused.
    let secret = run(&[], "production", &[("PUBLIC_API_TOKEN", "hunter2")]);
    assert!(secret.contains("CODE replaced clean"), "{secret}");
    assert!(!secret.contains("REFUSED"), "{secret}");
    // A mode is one of two words.
    let wrong = run(&[], "staging", &[]);
    assert!(
        wrong.contains("REFUSED TypeError build: mode is"),
        "{wrong}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Under `esdev test`, `import.meta.env` is what a bundle has compiled in —
/// `PUBLIC_*` from `.env`, then the shell — with `MODE` `"test"`, in the test
/// and in the source it imports, `.js` and `.ts` alike (D226).
#[test]
fn esdev_test_gives_import_meta_env_the_values_a_bundle_has() {
    let dir = build_dir("test_import_meta_env");
    std::fs::create_dir_all(dir.join("src")).expect("mkdir");
    write_in(
        &dir,
        "package.json",
        r#"{ "name": "app", "type": "module" }"#,
    );
    write_in(
        &dir,
        ".env",
        "PUBLIC_API_URL=https://api.example\nPUBLIC_BOTH=file\nSECRET=never\n",
    );
    write_in(
        &dir,
        "src/config.js",
        "export const api = import.meta.env.PUBLIC_API_URL;\nexport const { MODE } = import.meta.env;\n",
    );
    write_in(
        &dir,
        "src/flags.ts",
        "export const both: string | undefined = import.meta.env.PUBLIC_BOTH;\n",
    );
    write_in(
        &dir,
        "src/config.test.ts",
        "import { test, assertEquals } from 'runtime:test';\n\
         import { api, MODE } from './config.js';\n\
         import { both } from './flags.ts';\n\
         test('env', () => {\n\
           assertEquals([api, MODE, both, import.meta.env.PUBLIC_SHELL], ['https://api.example', 'test', 'shell', 'from-shell']);\n\
           assertEquals([import.meta.env.DEV, import.meta.env.PROD, import.meta.env.SECRET], [true, false, undefined]);\n\
         });\n",
    );
    for flags in [&[][..], &["--isolation=none"][..]] {
        let ran = esdev_in(&dir)
            .arg("test")
            .args(flags)
            .env("PUBLIC_SHELL", "from-shell")
            .env("PUBLIC_BOTH", "shell")
            .output()
            .expect("spawn esdev test");
        assert!(
            ran.status.success(),
            "{flags:?}\n{}{}",
            stdout(&ran),
            stderr(&ran)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
