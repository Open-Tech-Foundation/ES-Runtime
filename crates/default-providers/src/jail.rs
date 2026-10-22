//! The root jail (DECISIONS D25, D180): resolving a path the program wrote to
//! the real location it may reach, and pinning that answer so the syscall acts
//! where the check looked.
//!
//! Every door onto the filesystem — the async [`SystemFileSystem`]
//! (`runtime:fs`), the synchronous [`SystemSyncFileSystem`] (`runtime:wasi`,
//! archive sources), the embedded database's VFS — holds an `Arc<Jail>` and asks
//! it. None of them carries a copy of the policy, so none of them can drift
//! from it, and a new door starts with the whole of it rather than the parts
//! somebody remembered to copy.
//!
//! [`anchor`] is a private submodule: the only way to get an [`Anchored`] is
//! through a `Jail` method, so nothing can pin a path the jail did not admit.
//!
//! [`SystemFileSystem`]: crate::SystemFileSystem
//! [`SystemSyncFileSystem`]: crate::SystemSyncFileSystem

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;

use crate::path;
use crate::path_allowlist::{Access, PathAllowlist};
use crate::refusal::Refusal;

mod anchor;
mod tree;

pub(crate) use anchor::{Anchored, OpenMode};
pub(crate) use tree::PinnedTree;

/// The one answer to "may this path be reached, and where is it?" for every
/// door onto the filesystem (DECISIONS D180): `runtime:fs`, `runtime:wasi`, the
/// embedded database's files and archive extraction all resolve through one
/// `Jail`, built once per run. Relative paths resolve against `base`; every
/// access is confined to the canonicalized `root`, any subtree the command line
/// added (D54), and the scope lists (D38).
pub struct Jail {
    base: PathBuf,
    root: PathBuf,
    /// Paths reads may touch (`--allow-read=<paths>`). `None` ⇒ anywhere inside
    /// the root jail.
    allow_read: Option<Arc<PathAllowlist>>,
    /// Paths writes may touch (`--allow-write=<paths>`). `None` ⇒ anywhere
    /// inside the root jail.
    allow_write: Option<Arc<PathAllowlist>>,
    /// The roots a **read** may resolve under: the jail, then any path the
    /// allowlist named outside it (D54). Precomputed rather than derived per
    /// call — it changes only when the policy is built.
    read_roots: Vec<PathBuf>,
    /// The same for writes. Separate, because `--allow-read=/etc/certs` must
    /// not make `/etc/certs` writable: the two flags are two grants.
    write_roots: Vec<PathBuf>,
}

impl Jail {
    /// Builds a jail: relative paths resolve under `base`, and every access is
    /// confined to the canonicalized `root`.
    pub fn new(base: impl AsRef<Path>, root: impl AsRef<Path>) -> Self {
        let root =
            path::canonicalize(root.as_ref()).unwrap_or_else(|_| root.as_ref().to_path_buf());
        // Spelled the way the root is, so `spelled` can find one in the other
        // where the two differ only by a link (`/tmp` on macOS). Resolution
        // canonicalizes every path it joins onto the base anyway.
        let base =
            path::canonicalize(base.as_ref()).unwrap_or_else(|_| base.as_ref().to_path_buf());
        Jail {
            base,
            read_roots: vec![root.clone()],
            write_roots: vec![root.clone()],
            root,
            allow_read: None,
            allow_write: None,
        }
    }

    /// Scopes reads to `allow` — `esrun --allow-read=<paths>` (D38).
    ///
    /// Inside the jail the list **narrows**: only the named subtrees are
    /// readable. An entry *outside* the jail **adds** that subtree (D54), which
    /// is how a server reaches a TLS certificate in `/etc/letsencrypt` — a
    /// location no project root contains and no renewal writes inside one. Only
    /// a path typed on the command line can do that; guest code cannot move the
    /// boundary, which is what the jail is for.
    #[must_use]
    pub fn with_read_allowlist(mut self, allow: PathAllowlist) -> Self {
        self.read_roots = roots_with(&self.root, &allow);
        self.allow_read = Some(Arc::new(allow));
        self
    }

    /// Scopes writes to `allow` — `esrun --allow-write=<paths>` (D38). Same
    /// reading as [`with_read_allowlist`](Self::with_read_allowlist), against
    /// its own grant: reading a path never implies writing it.
    #[must_use]
    pub fn with_write_allowlist(mut self, allow: PathAllowlist) -> Self {
        self.write_roots = roots_with(&self.root, &allow);
        self.allow_write = Some(Arc::new(allow));
        self
    }

    /// The path as an error names it: what the program wrote, and — when that
    /// was relative to a base below the root — where it lands in the project.
    ///
    /// A bundle at `dist/server.js` that reads `public/index.html` reaches
    /// `dist/public/index.html`, and a refusal naming only `public/index.html`
    /// reads as though `--allow-read=public` had failed to cover it. The second
    /// spelling is the one a grant has to name, because grants and the root are
    /// both the working directory (D79).
    ///
    /// Lexical, not resolved: the base and the program's own spelling are all
    /// it is made of, both of which the program already knows, so it says
    /// nothing about where a link leads (D141). A path whose join leaves the
    /// root is shown as written, as D141 shows every escape.
    pub(crate) fn spelled(&self, p: &str) -> String {
        let raw = Path::new(p);
        if raw.is_absolute() || self.base == self.root {
            return p.to_string();
        }
        let mut joined = self.base.clone();
        for part in raw.components() {
            match part {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    joined.pop();
                }
                other => joined.push(other),
            }
        }
        match joined.strip_prefix(&self.root) {
            Ok(rel) if !rel.as_os_str().is_empty() && rel != raw => {
                format!("{p} ({} in the project)", path::to_slash(rel))
            }
            _ => p.to_string(),
        }
    }

    /// The roots an access of this kind may resolve under.
    pub(crate) fn roots(&self, access: Access) -> &[PathBuf] {
        match access {
            Access::Read => &self.read_roots,
            Access::Write => &self.write_roots,
        }
    }

    /// The read scope list, if one was set — for a walk that has to re-check
    /// each entry it finds.
    pub(crate) fn allow_read(&self) -> Option<Arc<PathAllowlist>> {
        self.allow_read.clone()
    }

    /// The list for `access`, if one was set.
    pub(crate) fn allowlist(&self, access: Access) -> Option<&PathAllowlist> {
        match access {
            Access::Read => self.allow_read.as_deref(),
            Access::Write => self.allow_write.as_deref(),
        }
    }

    /// Applies the scope list to an **already-resolved** path.
    ///
    /// After canonicalization and never before: a symlink is a name for a file
    /// elsewhere, so judging the name the guest wrote would let
    /// `--allow-read=./data` admit `./data/link-to-etc/passwd` — the hole the
    /// root jail exists to close, reopened one level in.
    pub(crate) fn scoped(&self, real: PathBuf, access: Access) -> Result<PathBuf, Refusal> {
        match self.allowlist(access) {
            Some(allow) => allow.check(&real, access).map(|()| real),
            None => Ok(real),
        }
    }

    /// The jailed directory a temp entry goes in: `dir`, or the base directory
    /// when empty. Deliberately *not* the OS temp directory — that lives outside
    /// the root jail, so writing there would be the one filesystem call that
    /// escapes it.
    pub(crate) fn temp_base(&self, dir: &str, access: Access) -> Result<PathBuf, ProviderError> {
        if dir.is_empty() {
            confine(&self.base, self.roots(access))
                .and_then(|real| self.scoped(real, access))
                .map_err(|refused| refused.named("."))
        } else {
            self.jailed(dir, access)
        }
    }

    /// The three refusals every resolution passes, in order — the jail, the
    /// root-mutation guard, the scope list — on an absolute path.
    ///
    /// Nothing here names a path: a [`Refusal`] is turned into an error by the
    /// caller, which is the one that knows what the program wrote (D141).
    pub(crate) fn admit(&self, abs: &Path, access: Access) -> Result<PathBuf, Refusal> {
        let resolved = confine(abs, self.roots(access))?;
        self.scoped(
            reject_root_mutation(resolved, self.roots(access), access)?,
            access,
        )
    }

    /// Like [`jailed`](Self::jailed) but without resolving the **final**
    /// component: the parent chain is canonicalized and confined, then the last
    /// name is reattached literally.
    ///
    /// `read_link` needs this. Resolving the whole path follows the very link it
    /// is being asked about, so it would read the target's target — or, for a
    /// link to a regular file, fail with `EINVAL`. The parent is still fully
    /// resolved and jailed, so the link being read is provably inside the root.
    ///
    /// A refusal names `shown` rather than `p`: they differ only for a path
    /// this provider built from the one the program wrote, such as one level
    /// of a recursive `mkdir` (D141).
    pub(crate) fn jailed_nofollow_as(
        &self,
        p: &str,
        shown: &str,
        access: Access,
    ) -> Result<PathBuf, ProviderError> {
        let raw = reject_empty(p)?;
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        // A path that names a **root itself** — `.`, `./`, `data/..`, or the
        // root's own absolute path — has no parent this may resolve: the
        // directory above the jail is not somewhere it is allowed to look, and
        // holding back the last component would send it there. Resolved whole
        // instead, which is also where the root-mutation guard lives.
        //
        // It only began to matter when mutations moved onto this path. `remove`
        // and `chmod` must not resolve their final component (removing a link
        // removes the link), and `remove(".")` then arrived here for the first
        // time — and was reported as an escape from `/tmp` rather than as the
        // refusal to destroy the sandbox that it is.
        if let Ok(real) = path::canonicalize(&abs)
            && self.roots(access).contains(&real)
        {
            return reject_root_mutation(real, self.roots(access), access)
                .and_then(|real| self.scoped(real, access))
                .map_err(|refused| refused.named(&self.spelled(shown)));
        }
        let (parent, name) = match (abs.parent(), abs.file_name()) {
            (Some(parent), Some(name)) => (parent.to_path_buf(), name.to_os_string()),
            // No final component to hold back (a bare root); fall through.
            _ => {
                return self
                    .admit(&abs, access)
                    .map_err(|refused| refused.named(&self.spelled(shown)));
            }
        };
        let resolved = confine(&parent, self.roots(access))
            .map(|parent| parent.join(name))
            .and_then(|resolved| reject_root_mutation(resolved, self.roots(access), access))
            .and_then(|resolved| self.scoped(resolved, access))
            .map_err(|refused| refused.named(&self.spelled(shown)))?;
        reject_trailing_slash_on_a_file(p, &resolved)?;
        Ok(resolved)
    }

    /// A jailed path, **pinned to the directory it resolved in**, following a
    /// symlink at the final component as `jailed` does.
    ///
    /// The pair every mutation uses now. [`jailed`](Self::jailed) decides
    /// whether the path is allowed; [`anchor`](crate::anchor) makes the answer
    /// stick, by holding the parent open so the syscall cannot be pointed at a
    /// different directory than the one that was checked.
    pub(crate) fn anchored(&self, p: &str, access: Access) -> Result<Anchored, ProviderError> {
        self.anchored_as(p, p, access)
    }

    /// [`anchored`](Self::anchored), naming `shown`.
    pub(crate) fn anchored_as(
        &self,
        p: &str,
        shown: &str,
        access: Access,
    ) -> Result<Anchored, ProviderError> {
        let real = self.jailed_as(p, shown, access)?;
        anchor::anchor(&real, self.roots(access), &self.spelled(shown))
    }

    /// The same, without resolving the final component — for an operation
    /// about the link rather than about what it points at.
    pub(crate) fn anchored_nofollow(
        &self,
        p: &str,
        access: Access,
    ) -> Result<Anchored, ProviderError> {
        self.anchored_nofollow_as(p, p, access)
    }

    /// [`anchored_nofollow`](Self::anchored_nofollow), naming `shown`.
    pub(crate) fn anchored_nofollow_as(
        &self,
        p: &str,
        shown: &str,
        access: Access,
    ) -> Result<Anchored, ProviderError> {
        let real = self.jailed_nofollow_as(p, shown, access)?;
        anchor::anchor(&real, self.roots(access), &self.spelled(shown))
    }

    /// Pins the directory `p` for writing beneath it (D180). Admitted like any
    /// write — the jail, then the scope list — except that a root itself may be
    /// pinned: the root-mutation guard is about removing or renaming a root,
    /// and a tree cannot do either to the directory it is pinned to.
    pub(crate) fn open_tree(&self, p: &str, access: Access) -> Result<PinnedTree, ProviderError> {
        let raw = reject_empty(p)?;
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        // The directory's own name is not followed: a link there would pin
        // wherever it points, and a tree is written without following links.
        // Its parents are resolved as every jailed path is. A root is taken
        // whole, since it has no parent the jail may look in.
        let real = match path::canonicalize(&abs) {
            Ok(real) if self.roots(access).contains(&real) => Ok(real),
            _ => match (abs.parent(), abs.file_name()) {
                (Some(parent), Some(name)) => {
                    confine(parent, self.roots(access)).map(|parent| parent.join(name))
                }
                _ => confine(&abs, self.roots(access)),
            },
        }
        .and_then(|real| self.scoped(real, access))
        .map_err(|refused| refused.named(&self.spelled(p)))?;
        let anchored = anchor::anchor(&real, self.roots(access), &self.spelled(p))?;
        PinnedTree::pin(&anchored)
    }

    /// Opens `p` through the jail and returns a handle **pinned to what the
    /// check agreed on**.
    ///
    /// `runtime:db`'s VFS needs a raw handle it can hand the engine, not the
    /// provider's own read/write. Resolving and then giving the engine the
    /// *path* would reopen the check-then-use race this crate closes everywhere
    /// else — a directory swapped for a symlink after the check — so the open
    /// happens here, by descriptor, and the caller gets the handle that was
    /// approved.
    #[cfg(unix)]
    pub(crate) fn open_anchored(
        &self,
        p: &str,
        access: Access,
        mode: OpenMode,
    ) -> Result<std::fs::File, ProviderError> {
        self.anchored(p, access)?.open_mode(mode)
    }

    /// Removes `p` through the jail, acting on the link at it rather than what
    /// it points at, with the parent pinned — the same rule
    /// [`remove_file`](crate::SystemFileSystem::remove_file) follows.
    #[cfg(unix)]
    pub(crate) fn remove_anchored(
        &self,
        p: &str,
        access: Access,
        directory: bool,
    ) -> Result<(), ProviderError> {
        self.anchored_nofollow(p, access)?.unlink(directory)
    }

    /// Resolves `p` (relative to `base`) and confines it to `root`, returning the
    /// real, jailed path. Existing paths are canonicalized; for a not-yet-created
    /// path, the deepest existing ancestor is canonicalized and checked, then the
    /// remaining (literal, `..`-free) components are reattached.
    ///
    /// This re-canonicalizes on every call by design: the jail's safety against
    /// symlink swaps depends on it, so the result must never be cached across
    /// calls (the filesystem is mutable, and a path validated once can later
    /// become a symlink escape).
    pub(crate) fn jailed(&self, p: &str, access: Access) -> Result<PathBuf, ProviderError> {
        self.jailed_as(p, p, access)
    }

    /// [`jailed`](Self::jailed), naming `shown` in any refusal instead of `p`.
    pub(crate) fn jailed_as(
        &self,
        p: &str,
        shown: &str,
        access: Access,
    ) -> Result<PathBuf, ProviderError> {
        let raw = reject_empty(p)?;
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        // The trailing-separator check stats the target, so it runs after the
        // scope list: whether an unlisted path is a directory is not something
        // a refusal should answer.
        let resolved = self
            .admit(&abs, access)
            .map_err(|refused| refused.named(&self.spelled(shown)))?;
        reject_trailing_slash_on_a_file(p, &resolved)?;
        Ok(resolved)
    }
}

/// The jail plus whatever `allow` named outside it — the roots one access kind
/// may resolve under, jail first so it is the one an error names.
fn roots_with(root: &Path, allow: &PathAllowlist) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    roots.extend(allow.roots_outside(root));
    roots
}

/// Rejects the empty path before it can be joined onto the base directory.
///
/// `Path::new("").is_absolute()` is false and `base.join("")` is `base`, so an
/// empty argument silently *becomes the root jail* — which is how
/// `remove("", { recursive: true })` came to delete the whole project. There is
/// no operation for which an empty path is the intended target, and Node's
/// `fs` rejects it too (`ENOENT` on `""`), so it fails here rather than
/// resolving to something the caller never named.
pub(crate) fn reject_empty(p: &str) -> Result<&Path, ProviderError> {
    if p.is_empty() {
        return Err(ProviderError::Coded {
            code: ErrorCode::InvalidPath,
            message: "path is empty (an empty path names no file; it is not the current directory)"
                .into(),
        });
    }
    Ok(Path::new(p))
}

/// Refuses a **mutation** whose resolved target is the root jail itself.
///
/// The empty-path guard above closes the sharpest spelling, but not the others:
/// `.`, `./`, `data/..` and the root's own absolute path all legitimately
/// resolve to the root, and reads of them (`stat(".")`, `readDir(".")`) are
/// ordinary and must keep working. What must not happen is a *write* landing
/// there — removing, renaming, truncating or chmod'ing the root is never a
/// coherent request from inside the jail, and every one of them destroys the
/// sandbox the guest is running in.
///
/// Writes *below* the root are unaffected, and so is creating an entry directly
/// in it: this compares the resolved target, and a new child resolves to
/// `root/<name>`, not to `root`.
pub(crate) fn reject_root_mutation(
    resolved: PathBuf,
    roots: &[PathBuf],
    access: Access,
) -> Result<PathBuf, Refusal> {
    // Every root, not just the jail: a directory the command line granted is a
    // boundary of this run in exactly the same way, and removing it out from
    // under the grant is the same self-destruction one level along.
    if access == Access::Write && roots.contains(&resolved) {
        return Err(Refusal::Root);
    }
    Ok(resolved)
}

/// Outside every root. Says whether the command line added any — a run with
/// `--allow-read=/etc/certs` that still cannot reach a path should see that the
/// grant did not cover it — without naming them (D141).
fn escape(roots: &[PathBuf]) -> Refusal {
    Refusal::Escape {
        granted: roots.len() > 1,
    }
}

pub(crate) fn confine(abs: &Path, roots: &[PathBuf]) -> Result<PathBuf, Refusal> {
    let mut existing = abs.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = path::canonicalize(&existing) {
            // Any of them: `roots[0]` is the project jail and the rest are
            // subtrees the command line added (D54). One `within_root` per root
            // rather than a cleverer structure — the list is the jail plus a
            // handful of explicitly typed paths, never more.
            let Some(root) = roots.iter().find(|root| path::within_root(&real, root)) else {
                return Err(escape(roots));
            };
            let mut out = real;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            // Belt and braces: the reattached path must still be under the root
            // that admitted it.
            if !out.starts_with(root) {
                return Err(escape(roots));
            }
            return Ok(out);
        }
        // Not present yet — climb to the parent, remembering the literal tail.
        // A `..`/empty tail component has no `file_name`, so it is rejected here
        // (no climbing out of the jail through a non-existent `..`).
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                existing = existing
                    .parent()
                    .map(Path::to_path_buf)
                    .ok_or_else(|| escape(roots))?;
            }
            None => return Err(escape(roots)),
        }
    }
}

/// Whether the path the guest wrote ends in a separator, which POSIX reads as
/// "this name must be a directory".
fn has_trailing_separator(p: &str) -> bool {
    // Windows accepts either separator; Unix treats a backslash as an ordinary
    // filename character, so only `/` counts there.
    p.ends_with('/') || (cfg!(windows) && p.ends_with('\\'))
}

/// Rejects `file.txt/` — a trailing separator on something that is not a
/// directory.
///
/// Resolution canonicalizes, and canonicalizing drops the trailing separator, so
/// `file.txt/` arrived at the operation indistinguishable from `file.txt` and
/// read, wrote and stat'd the file as though the slash had never been written.
/// The kernel would have refused it (`ENOTDIR`), which is what Node and Bun
/// surface because they hand the path over untouched; only `readDir` failed
/// here, because it is the one call whose syscall re-derives the requirement.
///
/// Checked against the resolved target rather than re-attaching the separator
/// for the kernel to judge: every operation downstream compares and joins these
/// paths, and a trailing separator would change what those comparisons mean.
///
/// A target that does not exist is left alone — the operation reports that
/// itself, and `mkdir("newdir/")` is a legitimate request to create one.
pub(crate) fn reject_trailing_slash_on_a_file(
    input: &str,
    resolved: &Path,
) -> Result<(), ProviderError> {
    if !has_trailing_separator(input) {
        return Ok(());
    }
    match std::fs::metadata(resolved) {
        Ok(md) if !md.is_dir() => Err(ProviderError::Coded {
            code: ErrorCode::NotDirectory,
            message: format!("{input}: not a directory (the trailing separator requires one)"),
        }),
        _ => Ok(()),
    }
}
