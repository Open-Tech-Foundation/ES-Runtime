//! `esdev.json`: what a project builds, in a file.

use super::*;

/// A project with all three kinds of target: one file, one directory, one that
/// runs when it is built. No dependencies — what is under test is the config,
/// not the bundler.
fn project_dir(name: &str) -> PathBuf {
    let dir = build_dir(name);
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::create_dir_all(dir.join("public/nested")).expect("create public");
    write_in(&dir, "src/server.mjs", "console.log('server');\n");
    write_in(&dir, "src/client.mjs", "console.log('client');\n");
    write_in(
        &dir,
        "src/prerender.mjs",
        "import { write } from 'runtime:fs';\n\
         await write('about.html', '<h1>about</h1>');\n",
    );
    write_in(&dir, "index.html", "<!doctype html><div id=root></div>\n");
    write_in(&dir, "public/styles.css", "body{color:red}\n");
    write_in(&dir, "public/nested/deep.txt", "deep\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{
          "build": { "targets": {
            "server":    { "entry": "src/server.mjs", "out": "dist/server.js",
                           "assets": ["index.html", "public"] },
            "browser":   { "entry": "src/client.mjs", "outDir": "dist/client",
                           "platform": "browser" },
            "prerender": { "entry": "src/prerender.mjs", "out": "dist/prerender.js",
                           "then": "run" }
          } },
          "dev": { "servers": { "server": {} }, "watch": { "targets": ["server", "browser"] } }
        }"#,
    );
    dir
}

#[test]
fn build_with_no_entry_builds_every_target_in_the_project() {
    let dir = project_dir("p_all");
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    assert!(dir.join("dist/server.js").exists(), "{}", stdout(&out));
    assert!(
        dir.join("dist/client/client.js").exists(),
        "{}",
        stdout(&out)
    );

    // `then: run` executed the bundle it just built, and the file that step
    // wrote landed beside it — the runtime resolves a relative path against the
    // entry module's directory, which is what makes `dist/` the deployment.
    assert!(dir.join("dist/about.html").exists(), "{}", stdout(&out));
    assert!(stdout(&out).contains("ran → "), "{}", stdout(&out));
    // The step itself is not deployed: it is the tool that made the page.
    assert!(!dir.join("dist/prerender.js").exists(), "{}", stdout(&out));
}

/// A `"then": "run"` step is part of making a build, not part of what it
/// made: its bundle and source map are gone once it has run — from an `out`
/// inside the deployed directory, and from a directory of its own, which is
/// not left behind empty. What it wrote stays, and a plugin's `finish` still
/// sees the step among the targets.
#[test]
fn a_build_step_is_not_left_in_the_output() {
    let dir = build_dir("b_step_gone");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/app.ts", "console.log('app');\n");
    write_in(
        &dir,
        "src/prerender.ts",
        "import { write } from 'runtime:fs';\nawait write('page.html', '<h1>page</h1>');\n",
    );
    write_in(
        &dir,
        "src/report.ts",
        "import { write } from 'runtime:fs';\nawait write('../dist/report.txt', 'reported');\n",
    );
    write_in(
        &dir,
        "seen.mjs",
        "import { write } from 'runtime:fs';\n\
         export default { name: 'seen', finish: { async handler(targets) {\n\
           await write(targets.web.outDir + '/seen.txt', Object.keys(targets).sort().join(','));\n\
         } } };\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"plugins": ["./seen.mjs"],
            "build": {"targets": {
              "web": {"entry": "src/app.ts", "outDir": "dist"},
              "prerender": {"entry": "src/prerender.ts", "out": "dist/prerender.js", "then": "run", "sourcemap": true},
              "report": {"entry": "src/report.ts", "out": ".steps/report.js", "then": "run"}
            }}}"#,
    );
    // What a build before this one left, as every build before this fix did.
    std::fs::create_dir_all(dir.join(".steps")).expect("create .steps");
    write_in(&dir, ".steps/report.js", "stale\n");

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    assert_eq!(
        std::fs::read_to_string(dir.join("dist/page.html")).expect("the page"),
        "<h1>page</h1>"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/report.txt")).expect("the report"),
        "reported"
    );
    for step in [
        "dist/prerender.js",
        "dist/prerender.js.map",
        ".steps/report.js",
    ] {
        assert!(!dir.join(step).exists(), "{step} was left in place");
    }
    assert!(
        !dir.join(".steps").exists(),
        "an empty step directory was left"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("dist/seen.txt")).expect("finish ran"),
        "prerender,report,web"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file is copied by name and a directory by its *contents*, so
/// `public/styles.css` is served at `/styles.css` without anything having to
/// rewrite an href.
#[test]
fn assets_are_copied_by_name_and_directories_by_their_contents() {
    let dir = project_dir("p_assets");
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    assert!(dir.join("dist/index.html").exists(), "the named file");
    assert!(
        dir.join("dist/styles.css").exists(),
        "the directory's contents"
    );
    assert!(dir.join("dist/nested/deep.txt").exists(), "recursively");
    assert!(
        !dir.join("dist/public").exists(),
        "the directory itself was copied, so every href would need to know it"
    );
}

/// The condition that decides which build of a dependency a client bundle gets.
/// Conditions match in the order the *package author* wrote them, so `worker`
/// being asserted at all is enough to win — and the failure is not here, it is
/// in somebody's browser.
#[test]
fn a_browser_target_takes_the_browser_build_of_a_dependency() {
    let dir = build_dir("p_platform");
    let package = dir.join("node_modules/dual");
    std::fs::create_dir_all(&package).expect("create package");
    std::fs::write(
        package.join("package.json"),
        r#"{ "name": "dual", "version": "1.0.0", "type": "module",
             "exports": { ".": { "worker": "./worker.js", "browser": "./browser.js",
                                 "default": "./default.js" } } }"#,
    )
    .expect("write manifest");
    for build in ["worker", "browser", "default"] {
        std::fs::write(
            package.join(format!("{build}.js")),
            format!("export const who = '{}_BUILD';\n", build.to_uppercase()),
        )
        .expect("write build");
    }
    write_in(
        &dir,
        "app.mjs",
        "import { who } from 'dual';\nconsole.log(who);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"web": {"entry": "app.mjs", "outDir": "out/web", "platform": "browser"}, "srv": {"entry": "app.mjs", "outDir": "out/srv"}}}}"#,
    );

    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));

    let web = std::fs::read_to_string(dir.join("out/web/app.js")).expect("read web bundle");
    let srv = std::fs::read_to_string(dir.join("out/srv/app.js")).expect("read server bundle");
    assert!(web.contains("BROWSER_BUILD"), "{web}");
    assert!(srv.contains("WORKER_BUILD"), "{srv}");
}

#[test]
fn target_builds_one_of_them_and_names_the_others_when_it_is_not_there() {
    let dir = project_dir("p_target");
    let one = esdev_in(&dir)
        .args(["build", "--target=browser"])
        .output()
        .expect("spawn esdev");
    assert!(one.status.success(), "{}{}", stdout(&one), stderr(&one));
    assert!(dir.join("dist/client/client.js").exists());
    assert!(
        !dir.join("dist/server.js").exists(),
        "--target built more than the one it named"
    );

    let missing = esdev_in(&dir)
        .args(["build", "--target=brower"])
        .output()
        .expect("spawn esdev");
    assert!(!missing.status.success());
    assert!(
        stderr(&missing).contains("is not a target"),
        "{}",
        stderr(&missing)
    );
    assert!(stderr(&missing).contains("browser"), "{}", stderr(&missing));
}

/// Naming an entry ignores the file entirely — a project that has a config can
/// still build a scratch entry — but asking for both leaves no answer to which
/// one named the entry.
#[test]
fn an_entry_on_the_command_line_and_a_target_together_are_refused() {
    let dir = project_dir("p_conflict");

    let scratch = esdev_in(&dir)
        .args(["build", "src/client.mjs", "--out=scratch.js"])
        .output()
        .expect("spawn esdev");
    assert!(scratch.status.success(), "{}", stderr(&scratch));
    assert!(dir.join("scratch.js").exists());
    assert!(
        !dir.join("dist/server.js").exists(),
        "naming an entry built the project's targets as well"
    );

    let both = esdev_in(&dir)
        .args(["build", "src/client.mjs", "--target=browser"])
        .output()
        .expect("spawn esdev");
    assert!(!both.status.success());
    assert!(
        stderr(&both).contains("--target=browser"),
        "{}",
        stderr(&both)
    );

    // `--out` names one file, and a project build writes what its targets say.
    let out = esdev_in(&dir)
        .args(["build", "--out=everything.js"])
        .output()
        .expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("--out=everything.js"),
        "{}",
        stderr(&out)
    );
}

/// A mistyped key is otherwise a setting that silently does nothing, which for
/// `platform` is the wrong build of a dependency.
#[test]
fn a_config_error_names_the_key_and_the_one_it_was_nearly() {
    let dir = build_dir("p_typo");
    write_in(&dir, "app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"build": {"targets": {"app": {"entry": "app.mjs", "outDri": "dist"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("unknown key `outDri`"),
        "{}",
        stderr(&out)
    );
    assert!(stderr(&out).contains("`outDir`"), "{}", stderr(&out));
}

/// Project permissions do not belong in the development config. `dev` uses the
/// development grant; production grants are passed to `esrun`.
#[test]
fn permissions_are_not_accepted_in_project_config() {
    let dir = build_dir("p_perms");
    write_in(&dir, "app.mjs", "console.log(1);\n");
    write_in(
        &dir,
        "esdev.json",
        r#"{"permissions": {"deny": ["all"], "allow": {"filesystem": true}}, "build": {"targets": {"app": {"entry": "app.mjs"}}}}"#,
    );
    let out = esdev_in(&dir).arg("build").output().expect("spawn esdev");
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("not an esdev setting"),
        "{}",
        stderr(&out)
    );
}

/// `--config` points at a file elsewhere, and every path in it is relative to
/// *that* file rather than to the working directory.
#[test]
fn a_config_elsewhere_resolves_its_paths_against_itself() {
    let dir = project_dir("p_elsewhere");
    let outside = build_dir("p_elsewhere_cwd");
    let config = dir.join("esdev.json");

    let out = esdev_in(&outside)
        .args([
            "build",
            &format!("--config={}", config.display()),
            "--target=server",
        ])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    assert!(
        dir.join("dist/server.js").exists(),
        "built beside the config"
    );
    assert!(!outside.join("dist").exists(), "built beside the caller");
}
