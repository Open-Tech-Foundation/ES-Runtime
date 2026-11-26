//! The import policy a release build applies, and subresource integrity (D193).

use super::*;

/// A project whose server and browser builds import three packages: `left-pad`
/// (unlisted), `@acme/ui` (a scope) and `aws-sdk`, whose one export is a
/// constant the bundler folds into its importer — so the module is gone from
/// every output chunk, and only the graph still shows it was read (D193).
fn policy_project(name: &str, policy: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    for (pkg, code) in [
        ("left-pad", "export default (s) => ' ' + s;"),
        ("aws-sdk", "export const S3 = 1;"),
        ("@acme/ui", "export const button = 'b';"),
    ] {
        let pkg_dir = dir.join("node_modules").join(pkg);
        std::fs::create_dir_all(&pkg_dir).unwrap();
        std::fs::write(
            pkg_dir.join("package.json"),
            format!(r#"{{"name":"{pkg}","version":"1.0.0","type":"module","main":"index.js"}}"#),
        )
        .unwrap();
        std::fs::write(pkg_dir.join("index.js"), code).unwrap();
    }
    std::fs::write(
        dir.join("src/server.js"),
        "import pad from 'left-pad';\nimport { button } from '@acme/ui';\nimport { S3 } from 'aws-sdk';\nconsole.log(pad(button), S3);\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/page.js"),
        "import { button } from '@acme/ui';\nimport { S3 } from 'aws-sdk';\ndocument.title = button + S3;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><script type=\"module\" src=\"./src/page.js\"></script></head><body></body></html>\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("esdev.json"),
        r#"{"build":{"importPolicy":"./import-policy.json","targets":{
            "server":{"entry":"src/server.js","out":"dist/server.js"},
            "web":{"entry":"index.html","outDir":"dist/web"}}}}"#,
    )
    .unwrap();
    std::fs::write(dir.join("import-policy.json"), policy).unwrap();
    dir
}

fn build_in(dir: &Path, target: &str) -> std::process::Output {
    esdev_in(dir)
        .args(["build", &format!("--target={target}")])
        .output()
        .unwrap()
}

#[test]
fn a_strict_import_policy_refuses_every_unapproved_package_a_build_ships() {
    let dir = policy_project(
        "policy_strict",
        r#"{ "allow": ["./src", "@acme/*"], "deny": ["aws-sdk"] }"#,
    );
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("the import policy (./import-policy.json) refuses what this build ships"),
        "{err}"
    );
    // Every refusal at once; `aws-sdk` was folded away and is still caught.
    assert!(err.contains("denied: aws-sdk"), "{err}");
    assert!(err.contains("not in the allow list: left-pad"), "{err}");
    assert!(
        !err.contains("@acme"),
        "a scope entry approves its packages: {err}"
    );
    assert!(!dir.join("dist/server.js").exists(), "nothing is written");

    // A browser target ships code too, and is judged the same way.
    let out = build_in(&dir, "web");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(err.contains("denied: aws-sdk"), "{err}");
}

#[test]
fn a_report_mode_policy_ships_and_lists_what_it_does_not_name_but_never_a_denied_package() {
    let dir = policy_project(
        "policy_report_build",
        r#"{ "allow": ["./src", "@acme/*"], "unlisted": "warn" }"#,
    );
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    assert!(
        err.contains("left-pad is not in the import policy's allow list"),
        "{err}"
    );
    assert!(
        err.contains("aws-sdk is not in the import policy's allow list"),
        "{err}"
    );
    assert!(dir.join("dist/server.js").exists());

    std::fs::write(
        dir.join("import-policy.json"),
        r#"{ "allow": ["./src", "@acme/*"], "deny": ["aws-sdk"], "unlisted": "warn" }"#,
    )
    .unwrap();
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "a deny never only warns");
    assert!(err.contains("denied: aws-sdk"), "{err}");
}

#[test]
fn a_policy_that_approves_everything_shipped_builds_and_a_bad_one_is_an_error() {
    let dir = policy_project(
        "policy_approved",
        r#"{ "allow": ["./src", "@acme/*", "left-pad", "aws-sdk"] }"#,
    );
    let out = build_in(&dir, "server");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    std::fs::write(
        dir.join("import-policy.json"),
        r#"{ "allow": ["x"], "unlisted": "maybe" }"#,
    )
    .unwrap();
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("\"unlisted\" is \"error\" or \"warn\""),
        "{err}"
    );

    std::fs::write(
        dir.join("esdev.json"),
        r#"{"build":{"importPolicy":true,"targets":{"server":{"entry":"src/server.js","out":"dist/server.js"}}}}"#,
    )
    .unwrap();
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("`build.importPolicy` is the path of an import policy file"),
        "{err}"
    );
}

#[test]
fn a_pinned_package_must_be_the_approved_version_and_files_in_every_mode() {
    let base = r#""./src", "@acme/*", "aws-sdk""#;
    let dir = policy_project(
        "policy_pinned",
        &format!(
            r#"{{ "allow": [{base}, {{ "package": "left-pad", "version": "1.0.0", "reason": "padding" }}] }}"#
        ),
    );
    let out = build_in(&dir, "server");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Another version is not the one approved, even in report mode.
    std::fs::write(
        dir.join("import-policy.json"),
        format!(r#"{{ "allow": [{base}, {{ "package": "left-pad", "version": "2.0.0" }}], "unlisted": "warn" }}"#),
    )
    .unwrap();
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("not the approved copy: left-pad@1.0.0 (approved: 2.0.0)"),
        "{err}"
    );

    // The right version with other files is not the approved copy either.
    std::fs::write(
        dir.join("import-policy.json"),
        format!(
            r#"{{ "allow": [{base}, {{ "package": "left-pad", "version": "1.0.0",
                "integrity": "h1:MqlKs+Ul5e3twLZxAsDHxdUENULXjG0nDnnk4B/VuZA=" }}] }}"#
        ),
    )
    .unwrap();
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("left-pad@1.0.0 (its files do not match the approved integrity)"),
        "{err}"
    );
}

#[test]
fn an_inventory_is_a_policy_that_approves_exactly_what_shipped() {
    let dir = policy_project("policy_inventory", r#"{ "allow": ["./src"] }"#);
    // The draft is written even though this policy refuses the build.
    let out = esdev_in(&dir)
        .args(["build", "--target=server", "--inventory=draft.json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let draft: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("draft.json")).unwrap()).unwrap();
    assert_eq!(draft["unlisted"], "error");
    let allow = draft["allow"].as_array().unwrap();
    assert_eq!(allow[0], "./src");
    let packages: Vec<&str> = allow[1..]
        .iter()
        .map(|e| e["package"].as_str().unwrap())
        .collect();
    assert_eq!(packages, ["@acme/ui", "aws-sdk", "left-pad"]);
    for entry in &allow[1..] {
        assert_eq!(entry["version"], "1.0.0");
        assert!(
            entry["integrity"].as_str().unwrap().starts_with("h1:"),
            "{entry}"
        );
        assert_eq!(entry["reason"], "");
    }

    // Approved as it stands, it builds.
    std::fs::copy(dir.join("draft.json"), dir.join("import-policy.json")).unwrap();
    let out = build_in(&dir, "server");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // One byte of a package changed after the review is not the approved copy.
    std::fs::write(
        dir.join("node_modules/left-pad/index.js"),
        "export default (s) => s;",
    )
    .unwrap();
    let out = build_in(&dir, "server");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("left-pad@1.0.0 (its files do not match the approved integrity)"),
        "{err}"
    );

    // The reviewed file is never the draft's destination.
    let out = esdev_in(&dir)
        .args(["build", "--inventory=import-policy.json"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(
        err.contains("is the import policy this project names"),
        "{err}"
    );
}

#[test]
fn subresource_integrity_pins_every_script_and_stylesheet_a_release_html_build_writes() {
    use base64::Engine as _;
    use sha2::{Digest, Sha384};

    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sri_build");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/page.js"), "document.title = 'hi';\n").unwrap();
    std::fs::write(dir.join("styles.css"), "body { color: red }\n").unwrap();
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><head><link rel=\"stylesheet\" href=\"./styles.css\">\
         <script type=\"module\" src=\"./src/page.js\"></script>\
         <script src=\"https://cdn.example.com/x.js\"></script></head><body></body></html>\n",
    )
    .unwrap();
    let config = |sri: &str| {
        std::fs::write(
            dir.join("esdev.json"),
            format!(
                r#"{{"build":{{{sri}"targets":{{"web":{{"entry":"index.html","outDir":"dist"}}}}}}}}"#
            ),
        )
        .unwrap();
    };

    config(r#""subresourceIntegrity":true,"#);
    let out = build_in(&dir, "web");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let html = std::fs::read_to_string(dir.join("dist/index.html")).unwrap();
    let mut pinned = 0;
    for (attr, rest) in html
        .split(" integrity=\"")
        .skip(1)
        .zip(html.split(" integrity=\""))
    {
        let digest = attr.split('"').next().unwrap();
        // The URL is the attribute just before this one.
        let url = rest.rsplit("=\"/").next().unwrap().trim_end_matches('"');
        let bytes = std::fs::read(dir.join("dist").join(url)).unwrap();
        let want = base64::engine::general_purpose::STANDARD.encode(Sha384::digest(&bytes));
        assert_eq!(digest, format!("sha384-{want}"), "{url}");
        pinned += 1;
    }
    assert_eq!(pinned, 2, "the script and the stylesheet: {html}");
    assert!(
        html.contains("<script src=\"https://cdn.example.com/x.js\"></script>"),
        "another origin is not ours to hash: {html}"
    );

    // Off unless asked for.
    config("");
    let out = build_in(&dir, "web");
    assert!(out.status.success());
    assert!(
        !std::fs::read_to_string(dir.join("dist/index.html"))
            .unwrap()
            .contains("integrity=")
    );

    config(r#""subresourceIntegrity":"yes","#);
    let out = build_in(&dir, "web");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("`build.subresourceIntegrity` is true or false")
    );
}
