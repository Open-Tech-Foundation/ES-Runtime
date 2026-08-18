//! Nothing lands where it is deployed until all of it worked.
//!
//! # What went wrong without it
//!
//! A build wrote straight into `dist`, and a whole-project release build
//! **emptied `dist` first** — because output names are content-hashed, so
//! yesterday's `app-1a2b.js` would otherwise sit there for ever beside today's.
//! Put together, a build that failed halfway did two bad things at once: it
//! deleted the deployment that was working, and it left the fragments of the one
//! that did not. The react static template shows it exactly — bundles and an
//! `index.html` on disk, and then the prerender step exits non-zero because a
//! dependency is missing. What is in `dist` at that moment is a site whose pages
//! were never rendered, and nothing about it says so. In CI it is what gets
//! uploaded.
//!
//! # What happens instead
//!
//! Every output path is mirrored under one staging directory beside the project
//! (`.esdev-build-<pid>-<n>`), the whole build runs there — bundles, assets, the
//! document, and any `"then": "run"` step, which writes beside its own bundle
//! and so writes into staging too — and only once all of it has succeeded does
//! anything move into place. A failure anywhere removes the staging directory
//! and leaves what was already deployed exactly as it was.
//!
//! Two ways to land, because a build owns some directories and not others:
//!
//! * **Replace** ([`Staging::own`]) — the directory is the build's. It is
//!   removed and the staged one takes its name, so nothing stale survives. This
//!   is what the up-front emptying used to be, moved to the end where a failure
//!   cannot benefit from it. Only a whole-project release build (and `--lib`)
//!   owns its output this way.
//! * **Overlay** — everything else. Each staged file is moved to its own path,
//!   creating directories and replacing files of the same name, and anything
//!   else already there is left alone. A `--target=` build writes one target's
//!   output into a directory another target's output may share; the dev loop
//!   rebuilds one target at a time into a directory a page is being served from.
//!   Neither may delete what it did not write.
//!
//! # The dev loop does not stage
//!
//! `esdev start` is the one build that is not producing a deployment. It
//! rebuilds into a directory a page is being *served from* while that page is
//! running, and the hot updates it computes are written into the same directory
//! for the page to fetch — so moving that directory out from under the page at
//! the end of every build would trade a problem nobody has in development
//! (yesterday's failed build is one keystroke from being replaced) for one
//! everybody would: a patch written where it can no longer be served, and a
//! full page reload for every save. [`Staging::new`] takes that decision as an
//! argument, and the paths work the same either way.
//!
//! Staging sits **inside the project** rather than in a temp directory, because
//! the last step is a rename and a rename is only cheap — only *atomic* —
//! within one filesystem. A cross-device move would be a copy of the whole
//! output, and a copy that fails halfway is the failure this module exists to
//! prevent.

use std::path::{Path, PathBuf};

/// What every staging directory's name starts with.
///
/// The watcher and the test discovery both skip it by this prefix rather than by
/// the whole name, which carries a pid: a build writes JavaScript, and a watcher
/// that saw its own staging directory would rebuild for ever.
pub const PREFIX: &str = ".esdev-build-";

/// A number per staging directory in one process, so the dev loop's rebuilds
/// cannot collide with each other on a name.
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One build's staging directory.
///
/// Dropped without [`Staging::commit`] — an error return, a `?` anywhere in the
/// build — it removes itself, which is what makes "a failed build changes
/// nothing" hold for every failure rather than the ones somebody remembered.
pub struct Staging {
    /// The project root. Every path handed to [`Staging::path`] is relative to
    /// this, and every path committed lands under it.
    root: PathBuf,
    /// The staging directory, or `None` for a build that writes into place —
    /// the dev loop, which is not producing a deployment.
    dir: Option<PathBuf>,
    /// Directories, relative to the root, that the build owns.
    owned: Vec<PathBuf>,
}

impl Staging {
    /// Prepares where a build writes.
    ///
    /// `stage` false is the dev loop: every path resolves to its real place and
    /// [`Staging::commit`] has nothing to move. Everything else about a build
    /// is written the same way, which is the point — there is one set of paths,
    /// not two code paths.
    pub fn new(root: &Path, stage: bool) -> Result<Self, String> {
        let dir = if stage {
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = root.join(format!("{PREFIX}{}-{n}", std::process::id()));
            // A directory left by a build that was killed rather than failed:
            // same process, same counter, and nothing else could have written it.
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
            Some(dir)
        } else {
            None
        };
        Ok(Self {
            root: root.to_path_buf(),
            dir,
            owned: Vec::new(),
        })
    }

    /// Where `path` — relative to the project root — is written while the build
    /// is still deciding whether it worked.
    ///
    /// The mirror keeps the shape of the project, so a bundle staged from
    /// `dist/server.js` still finds `dist/index.html` beside it, and a
    /// `"then": "run"` step that writes beside itself writes into staging
    /// without knowing that is what it is doing.
    pub fn path(&self, path: impl AsRef<Path>) -> PathBuf {
        self.dir.as_ref().unwrap_or(&self.root).join(path)
    }

    /// Marks `dir` — relative to the project root — as one the build owns, so
    /// committing replaces it rather than writing into it.
    pub fn own(&mut self, dir: PathBuf) {
        if !self.owned.contains(&dir) {
            self.owned.push(dir);
        }
    }

    /// `text` with the paths in it written the way a developer would write
    /// them: relative to the project, and naming where a file *will* be rather
    /// than the directory it occupies for the next few milliseconds.
    pub fn reveal(&self, text: &str) -> String {
        // Three spellings, because a build reports paths in whichever one it
        // was handed: the staging directory absolute (a path this module
        // built), the same directory relative to the project (what a step that
        // strips the project root prints), and the project root itself.
        let mut prefixes: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &self.dir {
            prefixes.push(dir.clone());
            prefixes.push(dir.strip_prefix(&self.root).unwrap_or(dir).to_path_buf());
        }
        prefixes.push(self.root.clone());

        let mut revealed = text.to_string();
        for prefix in prefixes {
            revealed = revealed.replace(
                &format!("{}{}", prefix.display(), std::path::MAIN_SEPARATOR),
                "",
            );
        }
        revealed
    }

    /// Moves everything staged into place, and removes the staging directory.
    ///
    /// Owned directories are replaced; everything else is overlaid. Both are
    /// renames within one filesystem, so the window in which a deployment is
    /// neither the old build nor the new one is as short as the platform can
    /// make it.
    pub fn commit(self) -> Result<(), String> {
        let Some(dir) = self.dir.clone() else {
            // The dev loop: it has been writing into place all along.
            return Ok(());
        };
        // Deepest first, so `dist/client` is dealt with before `dist` — after
        // which it no longer exists in staging, and the outer replace carries
        // whatever it left.
        let mut owned = self.owned.clone();
        owned.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for owned in owned {
            let staged = dir.join(&owned);
            if !staged.exists() {
                continue;
            }
            replace(&staged, &self.root.join(&owned))?;
        }
        overlay(&dir, &self.root)?;
        drop(self);
        Ok(())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // Best effort by design: a build that has already failed should report
        // why it failed, not why the cleanup after it also failed.
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Replaces `real` with `staged`.
fn replace(staged: &Path, real: &Path) -> Result<(), String> {
    if real.exists() {
        let removed = if real.is_dir() {
            std::fs::remove_dir_all(real)
        } else {
            std::fs::remove_file(real)
        };
        removed.map_err(|e| format!("cannot clear {}: {e}", real.display()))?;
    }
    if let Some(parent) = real.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::rename(staged, real).map_err(|e| {
        format!(
            "cannot move {} into place at {}: {e}",
            staged.display(),
            real.display()
        )
    })
}

/// Moves every file under `from` to the same path under `into`, leaving
/// whatever else is there alone.
fn overlay(from: &Path, into: &Path) -> Result<(), String> {
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("cannot read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let source = entry.path();
        let destination = into.join(entry.file_name());
        if source.is_dir() {
            std::fs::create_dir_all(&destination)
                .map_err(|e| format!("cannot create {}: {e}", destination.display()))?;
            overlay(&source, &destination)?;
        } else {
            replace(&source, &destination)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-staging-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// The whole point: a build that never commits leaves the last one where it
    /// was, and leaves nothing of its own behind.
    #[test]
    fn a_dropped_staging_changes_nothing() {
        let root = scratch("dropped");
        write(&root.join("dist/index.html"), "the deployed page");

        let staging = Staging::new(&root, true).expect("stage");
        let staged = staging.path("dist/index.html");
        write(&staged, "half a build");
        let dir = staging.path("");
        assert_ne!(dir, root, "a staged build wrote straight into the project");
        drop(staging);

        assert_eq!(read(&root.join("dist/index.html")), "the deployed page");
        assert!(!dir.exists(), "the staging directory outlived the build");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An owned directory is *replaced*, so a hashed filename from an older
    /// build does not accumulate in what gets deployed.
    #[test]
    fn committing_an_owned_directory_leaves_nothing_stale() {
        let root = scratch("owned");
        write(&root.join("dist/app-old.js"), "yesterday");
        write(&root.join("dist/index.html"), "old page");

        let mut staging = Staging::new(&root, true).expect("stage");
        staging.own(PathBuf::from("dist"));
        write(&staging.path("dist/app-new.js"), "today");
        write(&staging.path("dist/index.html"), "new page");
        staging.commit().expect("commit");

        assert!(
            !root.join("dist/app-old.js").exists(),
            "a stale file survived"
        );
        assert_eq!(read(&root.join("dist/app-new.js")), "today");
        assert_eq!(read(&root.join("dist/index.html")), "new page");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A directory the build only writes *into* keeps what the build did not
    /// write: the other target's bundle, and whatever else lives there.
    #[test]
    fn committing_an_unowned_directory_keeps_what_it_did_not_write() {
        let root = scratch("overlay");
        write(&root.join("dist/server.js"), "another target's output");
        write(&root.join("dist/index.html"), "old page");

        let staging = Staging::new(&root, true).expect("stage");
        write(&staging.path("dist/index.html"), "new page");
        staging.commit().expect("commit");

        assert_eq!(
            read(&root.join("dist/server.js")),
            "another target's output"
        );
        assert_eq!(read(&root.join("dist/index.html")), "new page");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A report names the path the developer will find, not the one that exists
    /// for the few milliseconds between the build and the commit.
    #[test]
    fn a_staged_path_is_reported_as_the_real_one() {
        let root = scratch("reveal");
        let staging = Staging::new(&root, true).expect("stage");

        let staged = staging.path("dist/server.js");
        let text = format!("bundled → {} (6.2 KB)", staged.display());
        assert_eq!(staging.reveal(&text), "bundled → dist/server.js (6.2 KB)");

        // The same directory as a build that strips the project root itself
        // reports it — the HTML build's `built → dist/index.html` is this one.
        let relative = staged.strip_prefix(&root).expect("under the root");
        assert_eq!(
            staging.reveal(&format!("built → {}", relative.display())),
            "built → dist/server.js"
        );

        // And a path that was never staged is still reported relative to the
        // project, which is what the dev loop's messages are made of.
        let unstaged = root.join("dist/server.js");
        assert_eq!(
            staging.reveal(&format!("bundled → {}", unstaged.display())),
            "bundled → dist/server.js"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The dev loop writes into place, so every path is the real one and there
    /// is nothing to move at the end.
    #[test]
    fn an_unstaged_build_writes_where_it_means_to() {
        let root = scratch("unstaged");
        let staging = Staging::new(&root, false).expect("stage");

        assert_eq!(staging.path("dist/app.js"), root.join("dist/app.js"));
        write(&staging.path("dist/app.js"), "written in place");
        staging.commit().expect("commit");

        assert_eq!(read(&root.join("dist/app.js")), "written in place");
        let _ = std::fs::remove_dir_all(&root);
    }
}
