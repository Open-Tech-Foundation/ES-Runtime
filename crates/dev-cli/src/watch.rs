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

/// Extensions a *build* answers to, over and above the ones a run does.
///
/// The difference is what the two consume. A run's inputs are modules; a
/// build's include the document that names them, the stylesheet that document
/// links, and whatever is sitting in `public` — none of which would restart a
/// server, and all of which change what the browser gets.
const ASSET_EXTENSIONS: &[&str] = &[
    "html",
    "htm",
    "css",
    "svg",
    "png",
    "jpg",
    "jpeg",
    "webp",
    "avif",
    "gif",
    "ico",
    "woff",
    "woff2",
    "ttf",
    "otf",
    "txt",
    "webmanifest",
];

/// How long the filesystem must be **quiet** before a change is acted on.
///
/// One editor save is several events — a truncate, a write, sometimes a rename
/// over the top — and acting on the first would build against a half-written
/// file. So the window restarts on every event and only a genuine lull ends it.
///
/// It is short because it is a *lull*, not a delay: the events of one save land
/// within a millisecond or two of each other, so 30 ms clears them with an order
/// of magnitude to spare, and the 120 ms this used to be was 120 ms added to
/// every save a developer makes — a third of the whole rebuild cycle spent
/// deliberately waiting. An editor that somehow straggles past the window costs
/// one wasted rebuild, and a build that fails changes nothing.
const SETTLE: Duration = Duration::from_millis(30);

/// The longest a burst may hold a rebuild off.
///
/// Without it the window is extendable without limit, so anything producing a
/// steady stream of events — `git checkout` across a large tree, an install
/// writing into a watched directory, a formatter walking the project — keeps
/// resetting the lull and the rebuild never happens. The bound turns that into
/// *rebuild now, and again when the stream ends*, which is a wasted build rather
/// than a dev loop that appears to have stopped working.
const MAX_HOLD: Duration = Duration::from_millis(500);

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
    let watch_scope = root.clone();
    // The watcher runs on its own thread and must outlive this scope, so it is
    // held here for the duration.
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && is_change(&event.kind)
            && event.paths.iter().any(|p| is_interesting(p, &watch_scope))
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
                match coalesce(&mut rx).await {
                    Some(_) => true,
                    None => return Ok(()),
                }
            }
            _ = coalesce(&mut rx) => {
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
/// Returns **everything the burst carried**, in the order it arrived, because
/// what changed decides what the page is told: a stylesheet can be swapped in
/// place, and anything else is a reload. A caller that only needs to know
/// *that* something changed can ignore the list.
///
/// `None` means the watcher is gone, which can only happen at shutdown.
pub async fn coalesce<T>(rx: &mut mpsc::UnboundedReceiver<T>) -> Option<Vec<T>> {
    let mut burst = vec![rx.recv().await?];
    // The cap starts with the burst, not with each event in it, so a stream of
    // changes cannot push it back for ever.
    let hold_until = tokio::time::Instant::now() + MAX_HOLD;

    // Keep draining until the filesystem has been quiet for a beat — or until
    // waiting for that beat has itself become the delay.
    loop {
        let quiet = tokio::time::timeout(SETTLE, rx.recv());
        match tokio::time::timeout_at(hold_until, quiet).await {
            // Another event: the save is still landing, so the lull restarts.
            Ok(Ok(Some(change))) => burst.push(change),
            // A lull, or the watcher stopped — either way the burst is over.
            Ok(Ok(None) | Err(_)) => return Some(burst),
            // Still arriving, and the cap is up. Build what is there.
            Err(_) => return Some(burst),
        }
    }
}

/// Ends a child, giving it the chance to drain first.
///
/// The signal is the one the runtime already handles gracefully; the kill is
/// the backstop for a program that will not take the hint.
pub async fn stop(child: &mut Child, grace: Duration) {
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

/// The directory to watch: the working directory, which is the root the child
/// runs under (D79) — the supervisor shares it with the process it starts.
///
/// The entry's own directory is the fallback for the one case the cwd cannot
/// answer: a working directory that no longer exists.
fn watch_root(entry: &Path) -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| {
        entry
            .canonicalize()
            .unwrap_or_else(|_| entry.to_path_buf())
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    })
}

/// Whether an event means the file actually *changed*.
///
/// This is not a nicety. `inotify` reports reads: the child process opening the
/// entry to load it raises `Access(Open)` on a watched file, so treating every
/// event as a change makes the watcher restart because it just restarted —
/// forever, with no edit involved. Metadata is excluded for the same reason,
/// since an access-time update is not an edit either.
pub fn is_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Remove(_) => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Name(_) | ModifyKind::Any) => true,
        // Reads, permission/atime changes, and the catch-alls a backend emits
        // when it cannot say what happened.
        EventKind::Access(_) | EventKind::Modify(_) | EventKind::Any | EventKind::Other => false,
    }
}

/// Whether a changed path is one a restart should answer to.
pub fn is_interesting(path: &Path, root: &Path) -> bool {
    is_watchable(path, root, WATCHED_EXTENSIONS)
}

/// Whether a changed path is one a *build* should answer to, beyond the
/// modules [`is_interesting`] covers.
pub fn is_asset(path: &Path, root: &Path) -> bool {
    is_watchable(path, root, ASSET_EXTENSIONS)
}

/// Whether a change is inside the project, outside the machine-written
/// directories, and in a file of a kind worth acting on.
///
/// **The ignored names are matched below the watch root, not anywhere in the
/// path.** Matching the whole path looks equivalent and is not: a project that
/// happens to live in `~/work/target/app` — or in a test's own
/// `target/tmp/fixture` — would have every one of its files ignored, and the
/// symptom is a watcher that runs and reports and never reacts to anything.
/// Found exactly that way.
fn is_watchable(path: &Path, root: &Path, extensions: &[&str]) -> bool {
    let ignored: HashSet<&str> = IGNORED_DIRS.iter().copied().collect();
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative.components().any(|c| {
        let name = c.as_os_str().to_string_lossy();
        // The staging directory a build writes into carries a pid, so it is
        // matched by its prefix. Without this the dev loop watches its own
        // half-finished output and rebuilds for ever.
        ignored.contains(name.as_ref()) || name.starts_with(crate::staging::PREFIX)
    }) {
        return false;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| extensions.contains(&ext.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lull is what ends a burst, so the events of one save — which land
    /// within a millisecond or two of each other — become one rebuild.
    #[tokio::test]
    async fn one_save_is_one_rebuild() {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        // A truncate, a write, a rename: what an editor actually does.
        for _ in 0..3 {
            tx.send(()).expect("send");
        }

        let started = std::time::Instant::now();
        let burst = coalesce(&mut rx).await.expect("a burst");
        // One answer for the three, carrying all three, and the wait was the
        // lull rather than a fixed delay per event.
        assert_eq!(burst.len(), 3);
        assert!(rx.try_recv().is_err(), "events were left unclaimed");
        assert!(
            started.elapsed() < MAX_HOLD,
            "a settled burst waited out the cap: {:?}",
            started.elapsed()
        );
    }

    /// A stream that never stops — `git checkout` across a tree, an install
    /// writing into a watched directory — must not hold the rebuild off for
    /// ever. Without the cap the lull restarts on every event and the dev loop
    /// looks like it has stopped working.
    #[tokio::test]
    async fn a_burst_that_never_ends_is_still_built() {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        // Faster than the lull, so it can never be reached.
        let flood = tokio::spawn(async move {
            loop {
                if tx.send(()).is_err() {
                    return;
                }
                tokio::time::sleep(SETTLE / 3).await;
            }
        });

        let started = std::time::Instant::now();
        assert!(coalesce(&mut rx).await.is_some());
        let waited = started.elapsed();
        flood.abort();

        assert!(waited >= MAX_HOLD, "it gave up before the cap: {waited:?}");
        // Bounded by the cap rather than by the stream, with room for a loaded
        // machine's scheduling.
        assert!(
            waited < MAX_HOLD * 4,
            "the cap did not bound it: {waited:?}"
        );
    }

    /// A watcher that has gone away is not a change, and waiting on one for
    /// ever is how a shutdown hangs.
    #[tokio::test]
    async fn a_dropped_watcher_ends_the_wait() {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        drop(tx);
        assert!(coalesce(&mut rx).await.is_none());
    }

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
        let root = Path::new("/p");
        assert!(is_interesting(Path::new("/p/src/app.ts"), root));
        assert!(is_interesting(Path::new("/p/server.mjs"), root));
        assert!(is_interesting(Path::new("/p/package.json"), root));
        assert!(!is_interesting(Path::new("/p/README.md"), root));
        assert!(!is_interesting(Path::new("/p/logo.png"), root));
        assert!(!is_interesting(Path::new("/p/no-extension"), root));

        // A build has more inputs than a run does.
        assert!(is_asset(Path::new("/p/index.html"), root));
        assert!(is_asset(Path::new("/p/public/styles.css"), root));
        assert!(!is_asset(Path::new("/p/src/app.ts"), root));
    }

    /// `dist` is the one that would otherwise loop: `esdev build` writes there,
    /// and a watcher that restarted on its own output would never settle.
    #[test]
    fn machine_written_directories_are_ignored() {
        let root = Path::new("/p");
        assert!(!is_interesting(
            Path::new("/p/node_modules/x/index.js"),
            root
        ));
        assert!(!is_interesting(Path::new("/p/dist/server.js"), root));
        assert!(!is_interesting(Path::new("/p/target/debug/build.js"), root));
        assert!(!is_interesting(
            Path::new("/p/.git/hooks/pre-commit.js"),
            root
        ));
    }

    /// The names are ignored *below the root*, not anywhere in the path — a
    /// project living in a directory called `target` is still a project, and
    /// the symptom of getting this wrong is a watcher that never reacts.
    #[test]
    fn a_project_inside_an_ignored_name_is_still_watched() {
        let root = Path::new("/home/me/work/target/app");
        assert!(is_interesting(
            Path::new("/home/me/work/target/app/src/x.ts"),
            root
        ));
        assert!(is_asset(
            Path::new("/home/me/work/target/app/index.html"),
            root
        ));
        // …and its own build output is still ignored.
        assert!(!is_interesting(
            Path::new("/home/me/work/target/app/dist/x.js"),
            root
        ));
    }
}
