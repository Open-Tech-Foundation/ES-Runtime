//! `runtime:watch`: file events in guest JS (the esdev-only module).

use super::*;

/// The whole point of the module: the program **stays up** across the change
/// and is told what changed, rather than being restarted like `--watch` does.
#[test]
fn runtime_watch_delivers_changes_to_the_program() {
    let dir = watch_dir("w_events");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
import { write } from "runtime:fs";

const changes = watch(["."], { recursive: true });
setTimeout(() => write("new.txt", "hello"), 300);

for await (const change of changes) {
  if (change.path.endsWith("new.txt")) {
    console.log(change.kind, "seen");
    break;
  }
}
console.log("still running");
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    // "created", not "modified": a save of a new file is a create followed by a
    // write, and the burst has to add up to the first of those.
    assert!(stdout(&out).contains("created seen"), "{}", stdout(&out));
    assert!(stdout(&out).contains("still running"), "{}", stdout(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// The watch set grows while the watcher runs — the case a dev server needs,
/// because which files a bundle depends on is known only after it is built.
#[test]
fn runtime_watch_takes_new_paths_while_it_runs() {
    let dir = watch_dir("w_add");
    std::fs::create_dir_all(dir.join("lib")).expect("create lib");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
import { write } from "runtime:fs";

// Opened on the app directory only; `lib` is not watched yet.
const changes = watch(["."]);
await changes.add("lib");
setTimeout(() => write("lib/dep.js", "export const x = 1;"), 300);

for await (const change of changes) {
  if (change.path.endsWith("dep.js")) {
    console.log("saw the added path");
    break;
  }
}
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("saw the added path"),
        "{}",
        stdout(&out)
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Watching is scoped by the same `--allow-read` list as reading, because it
/// answers the same questions: which files exist, and when they change.
#[test]
fn runtime_watch_is_bounded_by_allow_read() {
    let dir = watch_dir("w_scope");
    std::fs::create_dir_all(dir.join("app")).expect("create app");
    std::fs::create_dir_all(dir.join("secrets")).expect("create secrets");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
try {
  const changes = watch(["secrets"]);
  await changes.next();
  console.log("watched it");
} catch (err) {
  console.log("refused:", err.name);
}
"#,
    );

    let out = esdev_in(&dir)
        .args(["--deny-all", "--allow-read=app", "app.mjs"])
        .output()
        .expect("spawn esdev");
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).starts_with("refused:"), "{}", stdout(&out));

    let _ = std::fs::remove_dir_all(&dir);
}

/// `add()` refuses a tree that is already being watched — **including one a
/// recursive watch already covers**, which is the spelling that happens: a dev
/// server watches `app/`, then adds the package `app/` lives in as a
/// dependency. Comparing paths for equality caught the exact repeat and let the
/// overlap through, and an overlap is every event delivered twice on the
/// backends that allow it.
#[test]
fn runtime_watch_does_not_watch_one_tree_twice() {
    let dir = watch_dir("w_overlap");
    std::fs::create_dir_all(dir.join("pkg/src/app/deeper")).expect("create tree");
    write_in(
        &dir,
        "app.mjs",
        r#"
import { watch } from "runtime:watch";
const w = await watch(["pkg/src/app"], { recursive: true });
// Inside what is already watched, and the same path again.
console.log("inside:", await w.add("pkg/src/app/deeper"));
console.log("same:", await w.add("pkg/src/app"));
// Around it: the new watch covers the old one, which stops being its own.
console.log("around:", await w.add("pkg"));
// And now everything under it is covered by that.
console.log("inside again:", await w.add("pkg/src"));
await w.close();
"#,
    );

    let out = esdev_in(&dir).arg("app.mjs").output().expect("spawn esdev");
    let printed = stdout(&out);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(printed.contains("inside: false"), "{printed}");
    assert!(printed.contains("same: false"), "{printed}");
    assert!(printed.contains("around: true"), "{printed}");
    assert!(printed.contains("inside again: false"), "{printed}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// And it is `esdev`'s, not the runtime's: the same program under `esrun` must
/// fail at the import rather than run with a watcher that never fires.
#[test]
fn runtime_watch_does_not_exist_under_esrun() {
    let Some(esrun) = sibling_binary("esrun") else {
        eprintln!("skipping: esrun is not built in this target dir");
        return;
    };
    let dir = watch_dir("w_esrun");
    write_in(&dir, "app.mjs", "import 'runtime:watch';\n");

    // From the project, as a program is run: the root is the working
    // directory's project (D79), and an entry outside it is refused.
    let out = Command::new(esrun)
        .current_dir(&dir)
        .arg("app.mjs")
        .output()
        .expect("spawn esrun");
    assert!(!out.status.success(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("unknown built-in module"),
        "{}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&dir);
}
