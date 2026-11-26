//! Assets a module imports.

use super::*;

/// `import logo from "./logo.png"` used to stop the build — the bundler read
/// the image as source and reported that it was not valid UTF-8. The `assets`
/// copy could not answer it either: it cannot hash a name, and it does not know
/// the file was referenced at all.
#[test]
fn an_imported_asset_is_emitted_hashed_and_referenced_by_url() {
    let dir = build_dir("b_asset_import");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\nfirst").expect("write png");
    write_in(
        &dir,
        "src/main.ts",
        "import logo from './logo.png';\n\
         document.body.innerHTML = `<img src=\"${logo}\">`;\n",
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

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("1 asset"), "{}", stdout(&out));

    let emitted: Vec<_> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".png"))
        .collect();
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    let name = &emitted[0];
    assert!(
        name.starts_with("logo-") && name.len() > "logo-.png".len(),
        "{name}"
    );

    // The module exports the URL the file is served from, and it is the name
    // that was written.
    let bundle = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|e| e == "js"))
        .expect("a bundle");
    let text = std::fs::read_to_string(bundle).expect("read the bundle");
    assert!(text.contains(&format!("/assets/{name}")), "{text}");

    // Changed bytes, changed name — which is the whole reason the copy was not
    // enough: a cache cannot serve last week's image under this week's URL.
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\nsecond").expect("write png");
    let again = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(again.status.success(), "{}", stderr(&again));
    let second: Vec<_> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".png"))
        .collect();
    assert_eq!(second.len(), 1, "{second:?}");
    assert_ne!(&second[0], name, "the URL did not change with the file");
}

/// A server bundle and a browser bundle contain the same component, so they
/// have to agree about the URL: the markup one sends names the file the other
/// fetches.
#[test]
fn a_server_build_emits_the_same_asset_url() {
    let dir = build_dir("b_asset_server");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\n").expect("write png");
    write_in(
        &dir,
        "src/server.ts",
        "import logo from './logo.png';\nexport default `<img src=\"${logo}\">`;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "src/server.ts", "--out=dist/server.js"])
        .output()
        .expect("spawn esdev build");
    assert!(out.status.success(), "{}", stderr(&out));

    let bundle = std::fs::read_to_string(dir.join("dist/server.js")).expect("read the bundle");
    let emitted: Vec<_> = std::fs::read_dir(dir.join("dist/assets"))
        .expect("read assets")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(emitted.len(), 1, "{emitted:?}");
    assert!(
        bundle.contains(&format!("/assets/{}", emitted[0])),
        "{bundle}"
    );
}

/// A library is an input to a build that has not run yet, and where that build
/// serves a file from is its own decision — so the import is refused, with the
/// reason, rather than emitting a URL that is only correct here.
#[test]
fn a_library_refuses_an_asset_import_and_says_why() {
    let dir = build_dir("b_asset_lib");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src/logo.png"), b"\x89PNG\r\n\x1a\n").expect("write png");
    write_in(
        &dir,
        "src/index.ts",
        "import logo from './logo.png';\nexport const url: string = logo;\n",
    );

    let out = esdev_in(&dir)
        .args(["build", "--lib", "src"])
        .output()
        .expect("spawn esdev build --lib");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    // The asset is named, with the pass that refused it, and the explanation
    // says what to do.
    assert!(message.contains("[esdev:assets] src/logo.png"), "{message}");
    assert!(message.contains("--lib cannot know one"), "{message}");
    assert!(message.contains("data:"), "{message}");
}

/// A `?raw` import used to die in the resolver naming a file that is really
/// there. The suffix is refused by name instead — after every other plugin,
/// so one that claims it keeps it.
#[test]
fn a_raw_suffix_is_refused_by_name() {
    let dir = build_dir("b_raw_suffix");
    write_in(&dir, "data.txt", "hello\n");
    write_in(
        &dir,
        "app.mjs",
        "import text from './data.txt?raw';\nconsole.log(text);\n",
    );
    write_in(
        &dir,
        "esdev.json",
        "{ \"build\": { \"targets\": { \"app\": { \"entry\": \"app.mjs\", \"out\": \"dist/app.js\" } } } }",
    );

    let out = esdev_in(&dir)
        .arg("build")
        .output()
        .expect("spawn esdev build");
    assert!(!out.status.success(), "{}", stdout(&out));
    let message = slash_paths(&stderr(&out));
    assert!(message.contains("?raw"), "{message}");
    assert!(message.contains("does not support yet"), "{message}");
    assert!(!message.contains("No such file"), "{message}");
}
