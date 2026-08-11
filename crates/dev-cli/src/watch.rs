//! `esdev --watch` — rerun the program when its source changes.
//!
//! **The program runs in a child process, and a restart is a `SIGTERM`.** That
//! is the whole design, and both halves are deliberate.
//!
//! A child process, because the alternative — tearing down the `Runtime` in
//! place and building another — has to get every piece of host state back:
//! listening sockets, worker threads, the V8 isolate, the signal registry. A
//! leaked handle or a wedged isolate would then poison every subsequent run,
//! and the failure would look like the user's bug. A fresh process cannot carry
//! anything forward, and the prelude snapshot makes starting one cheap. Node's
//! `--watch` and nodemon reach for a child process for the same reason.
//!
//! `SIGTERM` rather than a kill, because the runtime already knows how to stop
//! properly: the Phase 14 shutdown watcher stops accepting, lets in-flight HTTP
//! requests answer, and exits `128+signal`. **A restart therefore drains**, so
//! a request in flight when a file is saved is answered rather than dropped —
//! which is the difference between a watcher you can leave running while you
//! use the app and one you cannot. A child that ignores the signal (a guest
//! that installed its own handler, or a wedged loop) is killed once the grace
//! expires; the grace is `--shutdown-grace`, the same number production uses.
//!
//! What is watched is a **directory tree**, not the module graph. The graph
//! would be more precise, and the runtime knows it — but it knows it in the
//! child, and shipping it back to the supervisor is a protocol between two
//! processes for a gain that only shows on projects big enough for the
//! difference to matter. The tree is the project root when one can be found
//! (the nearest ancestor holding a `package.json`, the same root the loader
//! detects) and the entry's own directory otherwise, minus the directories
//! nobody edits by hand.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::event::{EventKind, ModifyKind};
use notify::{Event, RecursiveMode, Watcher};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

/// Directories never worth watching: machine-written, and large enough that
/// watching them costs real descriptors. `dist` and `target` matter for
/// correctness rather than cost — `esdev build` writes into `dist`, and a
/// watcher that restarted on its own output would never settle.
const IGNORED_DIRS: &[&str] = &["node_modules", ".git", "dist", "target", ".cache"];

/// Extensions a restart is worth. A README or a PNG changing is not a reason to
/// bounce a server.
const WATCHED_EXTENSIONS: &[&str] = &[
    "js", "mjs", "cjs", "ts", "tsx", "jsx", "mts", "cts", "json", "wasm",
];

/// How long to wait for the filesystem to go quiet before restarting.
///
/// One editor save is several events — a truncate, a write, sometimes a rename
/// over the top — and restarting on the first would start a run against a
/// half-written file.
const DEBOUNCE: Duration = Duration::from_millis(120);

/// What `--watch` needs to do its job.
pub struct WatchConfig {
    /// The arguments to hand the child: this command line with `--watch`
    /// removed. Nothing else is rewritten, so the child runs exactly the
    /// program the user described.
    pub child_args: Vec<String>,
    /// The entry file, used to choose what to watch.
    pub entry: PathBuf,
    /// How long a child gets to drain before it is killed.
    pub grace: Duration,
}

/// Runs the program, restarting it whenever a watched file changes.
///
/// Returns only when the user interrupts: a watcher's job is to stay up, so a
/// program that exits — because it finished, or because it threw — leaves the
/// supervisor waiting for the next change rather than exiting with it.
pub async fn supervise(config: WatchConfig) -> Result<(), String> {
    let root = watch_root(&config.entry);
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the esdev binary: {e}"))?;

    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    // The watcher runs on its own thread and must outlive this scope, so it is
    // held here for the duration.
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && is_change(&event.kind)
            && event.paths.iter().any(|p| is_interesting(p))
        {
            // Unbounded and ignore-on-failure: the receiver only ever needs to
            // learn *that* something changed, so a full or closed channel costs
            // nothing worth reporting.
            let _ = tx.send(());
        }
    })
    .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;

    eprintln!("esdev: watching {}", root.display());

    loop {
        let mut child = Command::new(&exe)
            .args(&config.child_args)
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;

        // Wait for whichever comes first: the program finishing, a file
        // changing, or the user interrupting.
        let restart = tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(status) if status.success() => eprintln!("esdev: program exited"),
                    Ok(status) => eprintln!("esdev: program exited ({status})"),
                    Err(e) => eprintln!("esdev: cannot wait for the program: {e}"),
                }
                // It is gone; there is nothing to stop. Hold here until
                // something changes, so the watcher outlives the program.
                match wait_for_change(&mut rx).await {
                    Some(()) => true,
                    None => return Ok(()),
                }
            }
            _ = wait_for_change(&mut rx) => {
                stop(&mut child, config.grace).await;
                true
            }
            _ = tokio::signal::ctrl_c() => {
                // ^C reached the child too (it shares this process group), so
                // this only makes sure it is gone before the terminal returns.
                stop(&mut child, config.grace).await;
                return Ok(());
            }
        };

        if restart {
            eprintln!("esdev: change detected, restarting");
        }
    }
}

/// Blocks until a change arrives, then swallows the burst that follows it.
///
/// `None` means the watcher is gone, which can only happen at shutdown.
async fn wait_for_change(rx: &mut mpsc::UnboundedReceiver<()>) -> Option<()> {
    rx.recv().await?;
    // Coalesce: keep draining until the filesystem has been quiet for a beat.
    loop {
        match tokio::time::timeout(DEBOUNCE, rx.recv()).await {
            Ok(Some(())) => {}
            // Quiet, or the watcher stopped — either way the burst is over.
            Ok(None) | Err(_) => return Some(()),
        }
    }
}

/// Ends a child, giving it the chance to drain first.
///
/// The signal is the one the runtime already handles gracefully; the kill is
/// the backstop for a program that will not take the hint.
async fn stop(child: &mut Child, grace: Duration) {
    let Some(pid) = child.id() else {
        // Already reaped.
        return;
    };
    if !request_termination(pid) {
        let _ = child.kill().await;
        return;
    }
    match tokio::time::timeout(grace, child.wait()).await {
        Ok(_) => {}
        Err(_) => {
            eprintln!("esdev: the program did not stop within the grace period, killing it");
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

/// Asks the process to stop the way production would, reporting whether the
/// request could be made at all.
#[cfg(unix)]
fn request_termination(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    let Some(pid) = rustix::process::Pid::from_raw(pid) else {
        return false;
    };
    rustix::process::kill_process(pid, rustix::process::Signal::TERM).is_ok()
}

/// Windows has no `SIGTERM`, and no console-independent way to ask another
/// process to shut down cleanly. The caller falls back to killing it, which
/// means a restart there does not drain — stated rather than pretended.
#[cfg(not(unix))]
fn request_termination(_pid: u32) -> bool {
    false
}

/// The directory to watch: the project root if there is one, else the entry's
/// own directory.
///
/// "Project root" is the nearest ancestor holding a `package.json` — the same
/// thing the module loader detects (D25), so what is watched and what can be
/// imported agree.
fn watch_root(entry: &Path) -> PathBuf {
    let start = entry
        .canonicalize()
        .unwrap_or_else(|_| entry.to_path_buf())
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut cursor = start.as_path();
    loop {
        if cursor.join("package.json").is_file() {
            return cursor.to_path_buf();
        }
        match cursor.parent() {
            Some(parent) => cursor = parent,
            None => return start,
        }
    }
}

/// Whether an event means the file actually *changed*.
///
/// This is not a nicety. `inotify` reports reads: the child process opening the
/// entry to load it raises `Access(Open)` on a watched file, so treating every
/// event as a change makes the watcher restart because it just restarted —
/// forever, with no edit involved. Metadata is excluded for the same reason,
/// since an access-time update is not an edit either.
fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any) => true,
        // Reads, permission/atime changes, and the catch-alls a backend emits
        // when it cannot say what happened.
        EventKind::Access(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other => false,
    }
}

/// Whether a changed path is one a restart should answer to.
fn is_interesting(path: &Path) -> bool {
    let ignored: HashSet<&str> = IGNORED_DIRS.iter().copied().collect();
    if path
        .components()
        .any(|c| ignored.contains(c.as_os_str().to_string_lossy().as_ref()))
    {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| WATCHED_EXTENSIONS.contains(&ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The loop this cost an afternoon: a child process *reading* the entry
    /// raises `Access(Open)`, so a watcher that restarts on any event restarts
    /// because it restarted.
    #[test]
    fn reads_are_not_changes() {
        use notify::event::{AccessKind, DataChange, MetadataKind, RenameMode};

        assert!(!is_change(&EventKind::Access(AccessKind::Open(
            notify::event::AccessMode::Any
        ))));
        assert!(!is_change(&EventKind::Access(AccessKind::Read)));
        // An access-time bump is not an edit.
        assert!(!is_change(&EventKind::Modify(ModifyKind::Metadata(
            MetadataKind::AccessTime
        ))));

        assert!(is_change(&EventKind::Modify(ModifyKind::Data(
            DataChange::Content
        ))));
        // How an editor that writes a temp file and renames over the top lands.
        assert!(is_change(&EventKind::Modify(ModifyKind::Name(
            RenameMode::To
        ))));
        assert!(is_change(&EventKind::Create(
            notify::event::CreateKind::File
        )));
        assert!(is_change(&EventKind::Remove(
            notify::event::RemoveKind::File
        )));
    }

    #[test]
    fn source_files_are_interesting_and_documents_are_not() {
        assert!(is_interesting(Path::new("/p/src/app.ts")));
        assert!(is_interesting(Path::new("/p/server.mjs")));
        assert!(is_interesting(Path::new("/p/package.json")));
        assert!(!is_interesting(Path::new("/p/README.md")));
        assert!(!is_interesting(Path::new("/p/logo.png")));
        assert!(!is_interesting(Path::new("/p/no-extension")));
    }

    /// `dist` is the one that would otherwise loop: `esdev build` writes there,
    /// and a watcher that restarted on its own output would never settle.
    #[test]
    fn machine_written_directories_are_ignored() {
        assert!(!is_interesting(Path::new("/p/node_modules/x/index.js")));
        assert!(!is_interesting(Path::new("/p/dist/server.js")));
        assert!(!is_interesting(Path::new("/p/target/debug/build.js")));
        assert!(!is_interesting(Path::new("/p/.git/hooks/pre-commit.js")));
    }

    #[test]
    fn the_root_is_the_nearest_package_json() {
        let dir = std::env::temp_dir().join(format!("esdev-watch-root-{}", std::process::id()));
        let nested = dir.join("src/deep");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(dir.join("package.json"), "{}").expect("write");
        let entry = nested.join("app.mjs");
        std::fs::write(&entry, "").expect("write");

        assert_eq!(
            watch_root(&entry),
            dir.canonicalize().expect("canonicalize")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Without a `package.json` anywhere above it, a lone script watches its own
    /// directory rather than walking up to the filesystem root.
    #[test]
    fn a_rootless_entry_watches_its_own_directory() {
        let dir = std::env::temp_dir().join(format!("esdev-watch-lone-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let entry = dir.join("lone.mjs");
        std::fs::write(&entry, "").expect("write");

        let root = watch_root(&entry);
        // Either the entry's directory, or a `package.json`-bearing ancestor of
        // the system temp dir if the machine happens to have one.
        assert!(
            root == dir.canonicalize().expect("canonicalize")
                || root.join("package.json").is_file()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
