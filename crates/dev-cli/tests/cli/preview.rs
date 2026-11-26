//! `esdev start` and `esdev preview`: the release build, before it is a deployment.

use super::*;

/// The dev loop serves a build that is not the one that ships — NODE_ENV is
/// "development" and nothing is hashed — so the last look before deploying has
/// to be at the release output itself.
#[test]
fn preview_serves_the_build_with_a_route_fallback() {
    let dir = build_dir("b_preview");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(
        &dir,
        "src/main.ts",
        "document.body.innerHTML = `<h1>${import.meta.env.MODE}</h1>`;\n",
    );
    write_in(
        &dir,
        "index.html",
        "<!doctype html><html><body>\
         <script type=\"module\" src=\"./src/main.ts\"></script></body></html>\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"build\": { \"targets\": { \"web\": { \"entry\": \"index.html\", \"outDir\": \"dist\" } } } }\n",
    );

    // Nothing built yet: a preview serves what a build wrote, and says so
    // rather than opening a port onto an empty directory.
    let unbuilt = esdev_in(&dir)
        .args(["preview", "--port=0"])
        .output()
        .expect("spawn esdev preview");
    assert!(!unbuilt.status.success(), "{}", stdout(&unbuilt));
    assert!(
        stderr(&unbuilt).contains("esdev build"),
        "{}",
        stderr(&unbuilt)
    );

    let built = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(built.status.success(), "{}", stderr(&built));

    let port = test_port("preview");
    let mut command = esdev_in(&dir);
    command
        .args(["preview", "--spa", &format!("--port={port}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let _server = Supervisor(Some(command.spawn().expect("spawn esdev preview")));

    let index = wait_for_http(port, "/", |response| response.contains("<!doctype html"));
    assert!(index.contains("<script"), "{index}");
    // The document points at the hashed bundle a release build wrote, which is
    // the half `esdev start` cannot show.
    assert!(index.contains("/assets/main-"), "{index}");

    // With --spa, a reload on a route the router owns has to reach the router.
    let route = http_get(port, "/about").expect("a response");
    assert!(route.contains("200 OK"), "{route}");
    assert!(route.contains("<script"), "{route}");

    // …and a missing file that is not a route is still missing.
    let missing = http_get(port, "/nope.js").expect("a response");
    assert!(missing.contains("404"), "{missing}");
}

/// `--spa` is a switch: a value would read as a fallback file it does not take.
#[test]
fn preview_spa_takes_no_value() {
    let out = esdev()
        .args(["preview", "--spa=shell.html", "--dir=."])
        .output()
        .expect("spawn esdev preview");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(stderr(&out).contains("--spa"), "{}", stderr(&out));
}

/// A server bundle is run, not served — and by the binary that will run it in
/// production.
#[test]
fn preview_says_a_server_project_is_run_rather_than_served() {
    let dir = build_dir("b_preview_server");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    write_in(&dir, "src/api.ts", "export default 1;\n");
    write_in(
        &dir,
        "esdev.json",
        "{ \"build\": { \"targets\": { \"api\": { \"entry\": \"src/api.ts\", \
         \"out\": \"dist/api.js\" } } } }\n",
    );

    let out = esdev_in(&dir)
        .args(["preview", "--port=0"])
        .output()
        .expect("spawn esdev preview");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(stderr(&out).contains("esrun"), "{}", stderr(&out));
}
