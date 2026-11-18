//! Watching a project's tree without watching what the project ignores.
//!
//! # Why not one recursive watch
//!
//! On Linux a recursive watch is not one watch. `inotify` watches a single
//! directory, so a recursive watch is a walk of the whole tree at start and one
//! watch per directory found — `node_modules` included. The site in this
//! repository took 1,491 of them, 1,084 under `node_modules` and 192 under
//! `dist`, every one a directory no event from would ever be acted on: the
//! dev loop drops an event under a `.gitignore`d path the moment it arrives. The
//! walk is paid at every start, the watches count against the user's
//! `fs.inotify.max_user_watches`, and an install churns events through a
//! filter that throws them all away.
//!
//! So a directory is watched only when nothing would ignore what happens in it
//! ([`skipped`]), one directory at a time, and a directory that appears later —
//! created, or moved in — is watched when it does. What was already inside one
//! by the time its watch is in place (a `mkdir -p` and a write, faster than
//! the watch could be added) is reported as created, so nothing written there
//! is missed.
//!
//! # Where a recursive watch is right
//!
//! macOS (FSEvents) and Windows (`ReadDirectoryChangesW`) watch a tree natively:
//! one stream or handle for all of it, whatever it holds. Watching each
//! directory there would be thousands of handles in place of one. They keep the
//! recursive watch, and the same rule decides which of their events reach the
//! caller.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{CreateKind, EventKind};
use notify::{Event, RecursiveMode, Watcher};

/// Whether this platform's watcher covers a tree natively.
const NATIVE_RECURSIVE: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// What decides whether a directory is skipped. Shared with the watcher's own
/// thread, which asks it of every directory that appears.
pub type Skip = Arc<dyn Fn(&Path) -> bool + Send + Sync>;

/// The watch over a tree. Dropping it stops the watching.
pub struct TreeWatch {
    _watcher: Arc<Mutex<notify::RecommendedWatcher>>,
    adds: std::sync::mpsc::Sender<Add>,
    roots: Vec<PathBuf>,
}

impl TreeWatch {
    /// What asks for the roots to be walked again — after the rules changed,
    /// which a `.gitignore` edit does — and can be handed to the callback.
    pub fn rescanner(&self) -> Rescanner {
        Rescanner {
            adds: self.adds.clone(),
            roots: self.roots.clone(),
        }
    }
}

/// Walks a [`TreeWatch`]'s roots again and watches what is no longer skipped.
#[derive(Clone)]
pub struct Rescanner {
    adds: std::sync::mpsc::Sender<Add>,
    roots: Vec<PathBuf>,
}

impl Rescanner {
    pub fn rescan(&self) {
        for root in &self.roots {
            let _ = self.adds.send(Add::Rescan(root.clone()));
        }
    }
}

enum Add {
    /// A directory that appeared: watch it and what is in it, and report what
    /// is in it.
    Created(PathBuf),
    /// A root to walk again for directories not watched yet.
    Rescan(PathBuf),
}

/// Watches `roots` — each a directory, or a single file — and calls `on_event`
/// for every event in a directory `skip` does not exclude. A root itself is
/// never skipped.
pub fn watch_tree(
    roots: &[PathBuf],
    skip: Skip,
    on_event: impl Fn(&Event) + Send + Sync + 'static,
) -> Result<TreeWatch, String> {
    let on_event: Arc<dyn Fn(&Event) + Send + Sync> = Arc::new(on_event);
    let (adds, added) = std::sync::mpsc::channel::<Add>();

    let filter_roots = roots.to_vec();
    let filter_skip = skip.clone();
    let filter_event = on_event.clone();
    let filter_adds = adds.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        // Only a native recursive watch delivers events from a skipped
        // directory; elsewhere none is watched. Asked either way: a file a
        // root names directly is outside every walk.
        let paths: Vec<PathBuf> = event
            .paths
            .iter()
            .filter(|path| !under_skipped(path, &filter_roots, &filter_skip))
            .cloned()
            .collect();
        if paths.is_empty() {
            return;
        }
        if !NATIVE_RECURSIVE && !matches!(event.kind, EventKind::Remove(_) | EventKind::Access(_)) {
            for path in &paths {
                if path.is_dir() && !filter_skip(path) {
                    let _ = filter_adds.send(Add::Created(path.clone()));
                }
            }
        }
        let mut kept = event.clone();
        kept.paths = paths;
        filter_event(&kept);
    })
    .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    let watcher = Arc::new(Mutex::new(watcher));

    {
        let mut held = watcher
            .lock()
            .map_err(|_| "the file watcher stopped".to_string())?;
        for root in roots {
            if !root.is_dir() {
                held.watch(root, RecursiveMode::NonRecursive)
                    .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
            } else if NATIVE_RECURSIVE {
                held.watch(root, RecursiveMode::Recursive)
                    .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;
            } else {
                for dir in directories(root, &skip) {
                    watch_one(&mut *held, &dir, &dir == root)?;
                }
            }
        }
    }

    // Adds the watches for directories that appear. On a thread of its own:
    // the watcher's callback runs on the watcher's thread, and adding a watch
    // from there would wait on the thread that is doing the waiting.
    if !NATIVE_RECURSIVE {
        let thread_watcher = watcher.clone();
        let thread_skip = skip.clone();
        std::thread::Builder::new()
            .name("esdev-watch-tree".to_string())
            .spawn(move || {
                while let Ok(add) = added.recv() {
                    let (dir, created) = match add {
                        Add::Created(dir) => (dir, true),
                        Add::Rescan(dir) => (dir, false),
                    };
                    let found = directories(&dir, &thread_skip);
                    if let Ok(mut held) = thread_watcher.lock() {
                        for each in &found {
                            // A directory that cannot be watched now — gone
                            // again, or past the system's limit — is left: the
                            // watch already running is worth more than one
                            // directory's events.
                            let _ = watch_one(&mut *held, each, false);
                        }
                    }
                    if created {
                        for file in found.iter().flat_map(|each| files_in(each)) {
                            on_event(
                                &Event::new(EventKind::Create(CreateKind::File)).add_path(file),
                            );
                        }
                    }
                }
            })
            .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    }

    Ok(TreeWatch {
        _watcher: watcher,
        adds,
        roots: roots.to_vec(),
    })
}

/// Watches one directory, by itself. Running out of watches is an error worth
/// its own words; any other failure below a root — a directory that went away
/// while the walk was in it, one this user cannot read — is not a reason to
/// stop watching everything else.
fn watch_one(watcher: &mut dyn Watcher, dir: &Path, is_root: bool) -> Result<(), String> {
    match watcher.watch(dir, RecursiveMode::NonRecursive) {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) => Err(format!(
            "cannot watch {}: the system's limit on watched directories is reached.\n\n\
             On Linux it is fs.inotify.max_user_watches. Add the directories this \
             project does not edit to its .gitignore, or raise the limit:\n\n  \
             sudo sysctl fs.inotify.max_user_watches=524288",
            dir.display()
        )),
        Err(e) if is_root => Err(format!("cannot watch {}: {e}", dir.display())),
        Err(_) => Ok(()),
    }
}

/// `root` and every directory below it that `skip` keeps, parents first. A
/// symbolic link is not followed: it is how a tree reaches somewhere else,
/// and a link back up it would be walked for ever.
fn directories(root: &Path, skip: &Skip) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    let mut next = 0;
    while next < found.len() {
        let dir = found[next].clone();
        next += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
            let path = entry.path();
            if is_dir && !skip(&path) {
                found.push(path);
            }
        }
    }
    found
}

/// The files directly in `dir`.
fn files_in(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `path` is inside a directory `skip` excludes, between the root it is
/// under and itself.
fn under_skipped(path: &Path, roots: &[PathBuf], skip: &Skip) -> bool {
    let Some(root) = roots
        .iter()
        .filter(|root| path.starts_with(root))
        .max_by_key(|root| root.components().count())
    else {
        return false;
    };
    path.ancestors()
        .skip(1)
        .take_while(|dir| *dir != root.as_path() && dir.starts_with(root))
        .any(|dir| skip(dir))
}

/// The project's `.gitignore`, kept current while the loop runs.
///
/// Read once, the rules were whatever the file said when `esdev start` began:
/// a directory added to it afterwards — screenshots, a cache, a browser profile
/// a script writes into — kept triggering rebuilds, and every rebuild reloads
/// the page, until the loop was restarted. The watcher already sees the file
/// change, so that is when the rules are read again.
pub struct IgnoreRules {
    root: PathBuf,
    file: PathBuf,
    rules: std::sync::RwLock<Option<Gitignore>>,
}

impl IgnoreRules {
    pub fn load(root: &Path) -> Result<Self, String> {
        Ok(Self {
            root: root.to_path_buf(),
            file: root.join(".gitignore"),
            rules: std::sync::RwLock::new(load_gitignore(root)?),
        })
    }

    /// Whether `path` is the `.gitignore` itself, re-reading it if so. The
    /// file is not a build input, so its own change is never a rebuild. A file
    /// that no longer parses leaves the rules it had; the mistake is the
    /// user's to fix, and dropping every rule would rebuild on everything.
    pub fn refresh(&self, path: &Path) -> bool {
        if path != self.file {
            return false;
        }
        if let Ok(rules) = load_gitignore(&self.root)
            && let Ok(mut held) = self.rules.write()
        {
            *held = rules;
        }
        true
    }

    pub fn current(&self) -> std::sync::RwLockReadGuard<'_, Option<Gitignore>> {
        self.rules
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// Watches a project: [`watch_tree`] over `roots`, skipping what
/// [`skipped`] does for the project at `root` — its `.gitignore`, kept current
/// while the watch runs, and `outputs`. An edit to the `.gitignore` re-reads
/// it and walks the roots again for what it no longer leaves out; the file
/// itself is never passed on, since nothing builds from it.
///
/// `on_event` is given each event with the rules as they are now, for
/// anything it decides by them.
pub fn watch_project(
    roots: &[PathBuf],
    root: &Path,
    outputs: Vec<PathBuf>,
    on_event: impl Fn(&Event, Option<&Gitignore>) + Send + Sync + 'static,
) -> Result<TreeWatch, String> {
    let rules = Arc::new(IgnoreRules::load(root)?);
    let skip: Skip = {
        let rules = rules.clone();
        Arc::new(move |dir: &Path| skipped(dir, &outputs, rules.current().as_ref()))
    };
    let rescanner = Arc::new(std::sync::OnceLock::<Rescanner>::new());
    let callback_rescanner = rescanner.clone();
    let watch = watch_tree(roots, skip, move |event| {
        let mut changed_rules = false;
        let paths: Vec<PathBuf> = event
            .paths
            .iter()
            .filter(|path| {
                let is_rules = rules.refresh(path);
                changed_rules |= is_rules;
                !is_rules
            })
            .cloned()
            .collect();
        if changed_rules && let Some(rescanner) = callback_rescanner.get() {
            rescanner.rescan();
        }
        if paths.is_empty() {
            return;
        }
        let mut kept = event.clone();
        kept.paths = paths;
        on_event(&kept, rules.current().as_ref());
    })?;
    let _ = rescanner.set(watch.rescanner());
    Ok(watch)
}

/// The directories a project's watch skips: git's own, esdev's (a build's
/// staging directory and `.esdev`), `outputs` — what the watching loop writes
/// itself — and whatever the project's `.gitignore` names.
///
/// No directory name is excluded for being one, `node_modules` included: a
/// project says what it does not edit in `.gitignore`, where every other tool
/// it uses reads it too. `.git` is the exception because it is never a source
/// and is not listed in the file that lives inside it.
pub fn skipped(dir: &Path, outputs: &[PathBuf], gitignore: Option<&Gitignore>) -> bool {
    let name = dir
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    if name == ".git" || name == ".esdev" || name.starts_with(crate::staging::PREFIX) {
        return true;
    }
    if outputs.iter().any(|output| dir.starts_with(output)) {
        return true;
    }
    // A path outside the rules' root is not theirs to judge — and asking
    // `ignore` about one panics.
    gitignore.is_some_and(|rules| {
        dir.starts_with(rules.path())
            && matches!(
                rules.matched_path_or_any_parents(dir, true),
                Match::Ignore(_)
            )
    })
}

/// The `.gitignore` at `root`, if there is one.
pub fn load_gitignore(root: &Path) -> Result<Option<Gitignore>, String> {
    let path = root.join(".gitignore");
    if !path.is_file() {
        return Ok(None);
    }
    let mut builder = GitignoreBuilder::new(root);
    if let Some(err) = builder.add(&path) {
        return Err(format!("cannot read {}: {err}", path.display()));
    }
    builder
        .build()
        .map(Some)
        .map_err(|err| format!("cannot parse {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tree(name: &str, files: &[&str]) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("esdev-watch-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
            std::fs::write(&path, "x").expect("write");
        }
        es_runtime_default_providers::path::canonicalize(&dir).expect("canonical")
    }

    fn project_skip(root: &Path, outputs: Vec<PathBuf>) -> Skip {
        let rules = load_gitignore(root).expect("rules");
        Arc::new(move |dir: &Path| skipped(dir, &outputs, rules.as_ref()))
    }

    /// What `.gitignore` names, git's own directory, esdev's, and the loop's
    /// output are never walked; everything else is.
    #[test]
    fn the_walk_leaves_out_what_the_project_ignores() {
        let root = tree(
            "walk",
            &[
                ".gitignore",
                "src/app/page.ts",
                "node_modules/react/index.js",
                "node_modules/react/cjs/react.js",
                "dist/assets/app.js",
                ".git/objects/ab/cd",
                ".dev/dist/server.js",
                ".esdev-build-1-0/dist/x.js",
                "public/logo.svg",
            ],
        );
        std::fs::write(root.join(".gitignore"), "node_modules\n/dist\n").expect("rules");
        let skip = project_skip(&root, vec![root.join(".dev")]);
        let mut found: Vec<String> = directories(&root, &skip)
            .iter()
            .map(|dir| {
                dir.strip_prefix(&root)
                    .expect("under the root")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        found.sort();
        assert_eq!(found, ["", "public", "src", "src/app"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// No name is skipped for being one: a project with no `.gitignore` has
    /// its `node_modules` watched, as it asked.
    #[test]
    fn no_directory_name_is_skipped_by_itself() {
        let root = tree("names", &["node_modules/x/index.js", "dist/a.js"]);
        let skip = project_skip(&root, Vec::new());
        assert!(!skip(&root.join("node_modules")));
        assert!(!skip(&root.join("dist")));
        assert!(skip(&root.join(".git")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A rule never judges a path outside its own root.
    #[test]
    fn a_path_outside_the_rules_is_not_skipped_by_them() {
        let root = tree("outside", &[".gitignore"]);
        std::fs::write(root.join(".gitignore"), "*\n").expect("rules");
        let rules = load_gitignore(&root).expect("rules");
        assert!(!skipped(
            Path::new("/somewhere/else/src"),
            &[],
            rules.as_ref()
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn wait_for(rx: &std::sync::mpsc::Receiver<PathBuf>, wanted: &Path, within: Duration) -> bool {
        let deadline = std::time::Instant::now() + within;
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(path) if path == wanted => return true,
                Ok(_) => {}
                Err(_) => return false,
            }
        }
        false
    }

    /// A directory made after the watch began is watched, and a file already
    /// in it by the time its watch is in place is reported — `mkdir -p` and a
    /// write are faster than a watch can be added. One under an ignored
    /// directory is reported by neither.
    #[test]
    fn a_new_directory_is_watched_and_what_it_holds_is_reported() {
        let root = tree("new-dirs", &[".gitignore", "src/a.ts"]);
        std::fs::write(root.join(".gitignore"), "node_modules\n").expect("rules");
        let (tx, rx) = std::sync::mpsc::channel::<PathBuf>();
        let tx = Mutex::new(tx);
        let watch = watch_tree(
            std::slice::from_ref(&root),
            project_skip(&root, Vec::new()),
            move |event| {
                if let Ok(tx) = tx.lock() {
                    for path in &event.paths {
                        let _ = tx.send(path.clone());
                    }
                }
            },
        )
        .expect("watch");

        let made = root.join("src/feature/deep");
        std::fs::create_dir_all(&made).expect("mkdir -p");
        let early = made.join("early.ts");
        std::fs::write(&early, "x").expect("write");
        assert!(
            wait_for(&rx, &early, Duration::from_secs(5)),
            "the file written with the directory"
        );

        // Watched now: a later write is an event of its own.
        std::thread::sleep(Duration::from_millis(200));
        let later = made.join("later.ts");
        std::fs::write(&later, "x").expect("write");
        assert!(
            wait_for(&rx, &later, Duration::from_secs(5)),
            "a file written into the new directory"
        );

        std::fs::create_dir_all(root.join("node_modules/pkg")).expect("mkdir");
        let ignored = root.join("node_modules/pkg/index.js");
        std::fs::write(&ignored, "x").expect("write");
        assert!(
            !wait_for(&rx, &ignored, Duration::from_millis(750)),
            "an ignored directory was reported"
        );
        drop(watch);
        let _ = std::fs::remove_dir_all(&root);
    }
}
