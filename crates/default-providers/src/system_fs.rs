//! OS-backed [`FileSystem`] for the standalone embedding — tokio file I/O
//! confined to a **root jail** (DECISIONS D25). Every path is resolved against a
//! base directory, then its real location is checked to be inside the
//! canonicalized root; an escape (via `..` or a symlink) is rejected. Reads are
//! gated on `Capability::FileRead` and mutations on `Capability::FileWrite` by
//! `runtime` before any method here runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use es_runtime_common::ErrorCode;
use es_runtime_providers::{
    BoxFuture, DirEntry, FileStat, FileSystem, GlobScanOptions, ProviderError,
};
use globset::GlobBuilder;
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

use crate::path;
use crate::path_allowlist::{Access, PathAllowlist};

/// Compiles a glob pattern into a matcher plus a "negated" flag, covering the
/// full conventional set: `?`, `*` (not crossing `/`), `**` (crossing), `[ab]`,
/// `[a-z]`, `[!abc]` **and** `[^abc]`, `{a,b}`, `\` escaping, and a leading `!`
/// that negates the whole pattern.
///
/// `\` escaping is the one part that is platform-dependent: globset disables it
/// on Windows, where `\` is the path separator, so `\!x.ts` is a path there
/// rather than a literal `!x.ts`. Node's minimatch makes the same call
/// (`windowsPathsNoEscape`). Forcing it on would make pattern semantics uniform
/// but stop `\` separating components in the real Windows paths this matches
/// against, which is the trade globset's default is choosing.
fn parse_glob(pattern: &str) -> Result<(globset::GlobMatcher, bool), ProviderError> {
    // A leading `!` negates; `\!…` is a literal `!` (globset unescapes it).
    let (negated, body) = match pattern.strip_prefix('!') {
        Some(rest) => (true, rest.to_string()),
        None => (false, pattern.to_string()),
    };
    // Accept the `[^…]` negated-class form (globset spells it `[!…]`).
    let body = body.replace("[^", "[!");
    let matcher = GlobBuilder::new(&body)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| ProviderError::Other(format!("invalid glob pattern {pattern:?}: {e}")))?;
    Ok((matcher, negated))
}

/// A [`FileSystem`] over the real OS, jailed to `root`. Relative paths resolve
/// against `base` (the runtime's working directory).
pub struct SystemFileSystem {
    base: PathBuf,
    root: PathBuf,
    /// Paths reads may touch (`--allow-read=<paths>`). `None` ⇒ anywhere inside
    /// the root jail, which is the grant's own outer bound.
    allow_read: Option<Arc<PathAllowlist>>,
    /// Paths writes may touch (`--allow-write=<paths>`). `None` ⇒ anywhere
    /// inside the root jail.
    allow_write: Option<Arc<PathAllowlist>>,
}

impl SystemFileSystem {
    /// Builds a jailed filesystem: relative paths resolve under `base`, and every
    /// access is confined to the canonicalized `root`.
    pub fn new(base: impl AsRef<Path>, root: impl AsRef<Path>) -> Self {
        let root =
            path::canonicalize(root.as_ref()).unwrap_or_else(|_| root.as_ref().to_path_buf());
        SystemFileSystem {
            base: base.as_ref().to_path_buf(),
            root,
            allow_read: None,
            allow_write: None,
        }
    }

    /// Restricts reads to `allow` — `esrun --allow-read=<paths>` (D38). Narrows
    /// the root jail; it never widens it, so a path outside the root stays
    /// unreachable whatever the list says.
    #[must_use]
    pub fn with_read_allowlist(mut self, allow: PathAllowlist) -> Self {
        self.allow_read = Some(Arc::new(allow));
        self
    }

    /// Restricts writes to `allow` — `esrun --allow-write=<paths>` (D38).
    #[must_use]
    pub fn with_write_allowlist(mut self, allow: PathAllowlist) -> Self {
        self.allow_write = Some(Arc::new(allow));
        self
    }

    /// The list for `access`, if one was set.
    fn allowlist(&self, access: Access) -> Option<&PathAllowlist> {
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
    fn scoped(&self, real: PathBuf, access: Access) -> Result<PathBuf, ProviderError> {
        match self.allowlist(access) {
            Some(allow) => allow.check(&real, access).map(|()| real),
            None => Ok(real),
        }
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
    /// The jailed directory a temp entry goes in: `dir`, or the base directory
    /// when empty. Deliberately *not* the OS temp directory — that lives outside
    /// the root jail, so writing there would be the one filesystem call that
    /// escapes it.
    fn temp_base(&self, dir: &str, access: Access) -> Result<PathBuf, ProviderError> {
        if dir.is_empty() {
            self.scoped(confine(&self.base.clone(), &self.root)?, access)
        } else {
            self.jailed(dir, access)
        }
    }

    /// Like [`jailed`](Self::jailed) but without resolving the **final**
    /// component: the parent chain is canonicalized and confined, then the last
    /// name is reattached literally.
    ///
    /// `read_link` needs this. Resolving the whole path follows the very link it
    /// is being asked about, so it would read the target's target — or, for a
    /// link to a regular file, fail with `EINVAL`. The parent is still fully
    /// resolved and jailed, so the link being read is provably inside the root.
    fn jailed_nofollow(&self, p: &str, access: Access) -> Result<PathBuf, ProviderError> {
        let raw = reject_empty(p)?;
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        let (parent, name) = match (abs.parent(), abs.file_name()) {
            (Some(parent), Some(name)) => (parent.to_path_buf(), name.to_os_string()),
            // No final component to hold back (a bare root); fall through.
            _ => {
                return self.scoped(
                    reject_root_mutation(confine(&abs, &self.root)?, &self.root, access)?,
                    access,
                );
            }
        };
        let resolved = confine(&parent, &self.root)?.join(name);
        self.scoped(reject_root_mutation(resolved, &self.root, access)?, access)
    }

    fn jailed(&self, p: &str, access: Access) -> Result<PathBuf, ProviderError> {
        let raw = reject_empty(p)?;
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        let resolved = confine(&abs, &self.root)?;
        self.scoped(reject_root_mutation(resolved, &self.root, access)?, access)
    }
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
    root: &Path,
    access: Access,
) -> Result<PathBuf, ProviderError> {
    if access == Access::Write && resolved == root {
        return Err(ProviderError::Coded {
            code: ErrorCode::InvalidPath,
            message: format!(
                "refusing to modify the filesystem root jail {} itself (mutating the root would destroy the sandbox; name an entry inside it)",
                root.display()
            ),
        });
    }
    Ok(resolved)
}

fn escape(p: &Path, root: &Path) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::JailEscape,
        message: format!(
            "path {} escapes the filesystem root jail {} (access outside the root is not permitted)",
            p.display(),
            root.display()
        ),
    }
}

/// Builds a [`FileStat`] from already-read metadata. Shared with the
/// synchronous filesystem so both report identically.
///
/// `is_symlink` is always `false` here: this takes metadata that has already
/// followed links. A caller that needs the distinction stats the link itself.
pub(crate) fn file_stat(md: &std::fs::Metadata) -> FileStat {
    FileStat {
        size: md.len(),
        is_file: md.is_file(),
        is_dir: md.is_dir(),
        is_symlink: false,
        mtime_ms: mtime_ms(md),
    }
}

pub(crate) fn confine(abs: &Path, root: &Path) -> Result<PathBuf, ProviderError> {
    let mut existing = abs.to_path_buf();
    let mut tail: Vec<OsString> = Vec::new();
    loop {
        if let Ok(real) = path::canonicalize(&existing) {
            if !path::within_root(&real, root) {
                return Err(escape(abs, root));
            }
            let mut out = real;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            // Belt and braces: the reattached path must still be under root.
            if !out.starts_with(root) {
                return Err(escape(abs, root));
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
                    .ok_or_else(|| escape(abs, root))?;
            }
            None => return Err(escape(abs, root)),
        }
    }
}

fn other(p: &str, e: std::io::Error) -> ProviderError {
    ProviderError::from_io(p, &e)
}

/// Whether two already-jailed paths name the same file on disk.
///
/// Equal paths are the obvious case, and `jailed` has canonicalized both, so
/// `a.txt` and `./a.txt` compare equal here. Identity is not path equality
/// though: two hardlinks to one inode have different names and truncating
/// either destroys the other, so on Unix the device/inode pair decides. A path
/// that cannot be stat'd (the destination usually does not exist yet) is not the
/// same file as anything, and the copy proceeds normally.
#[cfg(unix)]
async fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    if a == b {
        return true;
    }
    // Follows symlinks deliberately: a copy reads and writes through them, so a
    // link and its target are the same file for this purpose.
    let (Ok(ma), Ok(mb)) = (tokio::fs::metadata(a).await, tokio::fs::metadata(b).await) else {
        return false;
    };
    ma.dev() == mb.dev() && ma.ino() == mb.ino()
}

/// Windows exposes no stable inode through `std`, so canonical-path equality is
/// the whole check there and a hardlinked destination still takes the truncating
/// path.
#[cfg(not(unix))]
async fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    a == b
}

fn mtime_ms(md: &std::fs::Metadata) -> Option<f64> {
    md.modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs_f64() * 1000.0)
}

impl FileSystem for SystemFileSystem {
    fn read(&self, path: String) -> BoxFuture<Result<Vec<u8>, ProviderError>> {
        let resolved = self.jailed(&path, Access::Read);
        if let Ok(p) = &resolved
            && let Ok(md) = std::fs::metadata(p)
            && md.len() < 64 * 1024
        {
            return Box::pin(std::future::ready(
                std::fs::read(p).map_err(|e| other(&path, e)),
            ));
        }
        Box::pin(async move {
            let p = resolved?;
            tokio::fs::read(&p).await.map_err(|e| other(&path, e))
        })
    }

    fn write(
        &self,
        path: String,
        data: Vec<u8>,
        append: bool,
    ) -> BoxFuture<Result<u64, ProviderError>> {
        let resolved = self.jailed(&path, Access::Write);
        let len = data.len() as u64;

        if let Ok(p) = &resolved
            && len < 64 * 1024
        {
            let res = (|| -> std::io::Result<()> {
                let mut opts = std::fs::OpenOptions::new();
                opts.write(true).create(true);
                if append {
                    opts.append(true);
                } else {
                    opts.truncate(true);
                }
                use std::io::Write;
                let mut f = opts.open(p)?;
                f.write_all(&data)?;
                Ok(())
            })();
            return Box::pin(std::future::ready(
                res.map(|_| len).map_err(|e| other(&path, e)),
            ));
        }
        Box::pin(async move {
            let p = resolved?;
            let mut opts = tokio::fs::OpenOptions::new();
            opts.write(true).create(true);
            if append {
                opts.append(true);
            } else {
                opts.truncate(true);
            }
            let mut f = opts.open(&p).await.map_err(|e| other(&path, e))?;
            f.write_all(&data).await.map_err(|e| other(&path, e))?;
            // `tokio::fs::File` dispatches writes to the blocking pool and
            // returns before they land, so `write_all` alone leaves the promise
            // resolving over a file that is still empty or half-written — and
            // the `truncate` above has already run, so a reader sees *less* than
            // it would have before the write. Flushing is what makes this
            // method's contract ("resolves to the number of bytes written")
            // true. The sub-64 KiB branch is a synchronous `std::fs` write and
            // was never affected, which is why only large writes tore.
            f.flush().await.map_err(|e| other(&path, e))?;
            Ok(len)
        })
    }

    fn stat(&self, path: String) -> BoxFuture<Result<FileStat, ProviderError>> {
        let resolved = self.jailed(&path, Access::Read);
        if let Ok(p) = &resolved
            && let Ok(md) = std::fs::metadata(p)
        {
            let is_symlink = std::fs::symlink_metadata(p)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            return Box::pin(std::future::ready(Ok(FileStat {
                size: md.len(),
                is_file: md.is_file(),
                is_dir: md.is_dir(),
                is_symlink,
                mtime_ms: mtime_ms(&md),
            })));
        }
        Box::pin(async move {
            let p = resolved?;
            let md = tokio::fs::metadata(&p).await.map_err(|e| other(&path, e))?;
            let is_symlink = tokio::fs::symlink_metadata(&p)
                .await
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false);
            Ok(FileStat {
                size: md.len(),
                is_file: md.is_file(),
                is_dir: md.is_dir(),
                is_symlink,
                mtime_ms: mtime_ms(&md),
            })
        })
    }

    fn exists(&self, path: String) -> BoxFuture<Result<bool, ProviderError>> {
        let resolved = self.jailed(&path, Access::Read);
        if let Ok(p) = &resolved {
            return Box::pin(std::future::ready(
                p.try_exists().map_err(|e| other(&path, e)),
            ));
        }
        Box::pin(std::future::ready(Ok(false)))
    }

    fn read_dir(&self, path: String) -> BoxFuture<Result<Vec<DirEntry>, ProviderError>> {
        let p = match self.jailed(&path, Access::Read) {
            Ok(p) => p,
            // Propagate the jail-escape error, like read/write/stat.
            Err(e) => return Box::pin(std::future::ready(Err(e))),
        };
        let res = (|| -> std::io::Result<Vec<DirEntry>> {
            let mut out = Vec::new();
            for entry in std::fs::read_dir(&p)? {
                let entry = entry?;
                let ft = entry.file_type()?;
                out.push(DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_file: ft.is_file(),
                    is_dir: ft.is_dir(),
                    is_symlink: ft.is_symlink(),
                });
            }
            Ok(out)
        })();
        Box::pin(std::future::ready(res.map_err(|e| other(&path, e))))
    }

    fn mkdir(&self, path: String, recursive: bool) -> BoxFuture<Result<(), ProviderError>> {
        let resolved = self.jailed(&path, Access::Write);
        Box::pin(async move {
            let p = resolved?;
            if recursive {
                tokio::fs::create_dir_all(&p).await
            } else {
                tokio::fs::create_dir(&p).await
            }
            .map_err(|e| other(&path, e))
        })
    }

    fn remove(&self, path: String, recursive: bool) -> BoxFuture<Result<(), ProviderError>> {
        let resolved = self.jailed(&path, Access::Write);
        Box::pin(async move {
            let p = resolved?;
            let md = tokio::fs::symlink_metadata(&p)
                .await
                .map_err(|e| other(&path, e))?;
            if md.is_dir() {
                if recursive {
                    tokio::fs::remove_dir_all(&p).await
                } else {
                    tokio::fs::remove_dir(&p).await
                }
            } else {
                tokio::fs::remove_file(&p).await
            }
            .map_err(|e| other(&path, e))
        })
    }

    fn rename(&self, from: String, to: String) -> BoxFuture<Result<(), ProviderError>> {
        let from_r = self.jailed(&from, Access::Write);
        let to_r = self.jailed(&to, Access::Write);
        Box::pin(async move {
            let (a, b) = (from_r?, to_r?);
            tokio::fs::rename(&a, &b).await.map_err(|e| other(&from, e))
        })
    }

    fn copy(&self, from: String, to: String) -> BoxFuture<Result<u64, ProviderError>> {
        let from_r = self.jailed(&from, Access::Read);
        let to_r = self.jailed(&to, Access::Write);
        Box::pin(async move {
            let (a, b) = (from_r?, to_r?);
            // `fs::copy` opens the destination truncating *before* it reads the
            // source, so copying a file onto itself emptied it and reported 0
            // bytes copied — a silent wipe of the very file being backed up.
            // Deno refuses the same call; Node/libuv treats it as a no-op. It is
            // refused here: nothing was copied, so there is no honest byte count
            // to resolve with, and the call is almost certainly a caller bug.
            if same_file(&a, &b).await {
                return Err(ProviderError::Coded {
                    code: ErrorCode::SameFile,
                    message: format!(
                        "Source and destination paths refer to the same file: copy '{from}' -> '{to}'"
                    ),
                });
            }
            tokio::fs::copy(&a, &b).await.map_err(|e| other(&from, e))
        })
    }

    fn real_path(&self, path: String) -> BoxFuture<Result<String, ProviderError>> {
        let resolved = self.jailed(&path, Access::Read);
        let root = self.root.clone();
        Box::pin(async move {
            let p = resolved?;
            // `jailed` already canonicalizes what exists, but a path whose tail
            // does not exist is reattached literally — so canonicalize again and
            // re-check. Answering with a real location outside the jail would
            // defeat the point of the jail.
            let real = tokio::fs::canonicalize(&p)
                .await
                .map_err(|e| other(&path, e))?;
            let real = confine(&real, &root)?;
            Ok(real.to_string_lossy().into_owned())
        })
    }

    fn read_link(&self, path: String) -> BoxFuture<Result<String, ProviderError>> {
        // Not `jailed`: that would resolve the link being asked about.
        let resolved = self.jailed_nofollow(&path, Access::Read);
        Box::pin(async move {
            let p = resolved?;
            // The stored target, verbatim — it may be relative, and may not
            // exist. Not jailed, because it is data read out of the link rather
            // than a path being accessed; `real_path` is what resolves it, and
            // that is jailed.
            let target = tokio::fs::read_link(&p)
                .await
                .map_err(|e| other(&path, e))?;
            Ok(target.to_string_lossy().into_owned())
        })
    }

    fn truncate(&self, path: String, len: u64) -> BoxFuture<Result<(), ProviderError>> {
        let resolved = self.jailed(&path, Access::Write);
        Box::pin(async move {
            let p = resolved?;
            let file = tokio::fs::OpenOptions::new()
                .write(true)
                .open(&p)
                .await
                .map_err(|e| other(&path, e))?;
            file.set_len(len).await.map_err(|e| other(&path, e))
        })
    }

    fn chmod(&self, path: String, mode: u32) -> BoxFuture<Result<(), ProviderError>> {
        let resolved = self.jailed(&path, Access::Write);
        Box::pin(async move {
            let p = resolved?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                tokio::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode))
                    .await
                    .map_err(|e| other(&path, e))
            }
            #[cfg(not(unix))]
            {
                // Windows has no mode bits — only a read-only flag. Honour the
                // owner-write bit and nothing else, which is the whole of what
                // the platform can represent (the same mapping Node makes).
                let mut perms = tokio::fs::metadata(&p)
                    .await
                    .map_err(|e| other(&path, e))?
                    .permissions();
                perms.set_readonly(mode & 0o200 == 0);
                tokio::fs::set_permissions(&p, perms)
                    .await
                    .map_err(|e| other(&path, e))
            }
        })
    }

    fn make_temp_dir(
        &self,
        dir: String,
        prefix: String,
    ) -> BoxFuture<Result<String, ProviderError>> {
        let resolved = self.temp_base(&dir, Access::Write);
        Box::pin(async move {
            let base = resolved?;
            // Built by tempfile, so the name is unpredictable: a guessable temp
            // name in a shared directory is a symlink-attack invitation, and it
            // is not something each caller should have to get right.
            let made = tempfile::Builder::new()
                .prefix(&prefix)
                .tempdir_in(&base)
                .map_err(|e| other(&dir, e))?;
            // Hand ownership to the guest: it decides when to remove it, exactly
            // as with any other directory it created.
            Ok(made.keep().to_string_lossy().into_owned())
        })
    }

    fn make_temp_file(
        &self,
        dir: String,
        prefix: String,
    ) -> BoxFuture<Result<String, ProviderError>> {
        let resolved = self.temp_base(&dir, Access::Write);
        Box::pin(async move {
            let base = resolved?;
            let made = tempfile::Builder::new()
                .prefix(&prefix)
                .tempfile_in(&base)
                .map_err(|e| other(&dir, e))?;
            let (_file, path) = made.keep().map_err(|e| other(&dir, e.error))?;
            Ok(path.to_string_lossy().into_owned())
        })
    }

    fn glob_match(&self, pattern: &str, path: &str) -> Result<bool, ProviderError> {
        let (matcher, negated) = parse_glob(pattern)?;
        Ok(negated ^ matcher.is_match(path))
    }

    fn glob_scan(
        &self,
        base: String,
        pattern: String,
        opts: GlobScanOptions,
    ) -> BoxFuture<Result<Vec<String>, ProviderError>> {
        let resolved = self.jailed(&base, Access::Read);
        let root = self.root.clone();
        let allow_read = self.allow_read.clone();
        Box::pin(async move {
            let base_real = resolved?;
            let (matcher, negated) = parse_glob(&pattern)?;
            let mut out = Vec::new();
            // Default: don't follow symlinks (can't leave the jail). When the
            // caller opts in, follow them but reject any entry whose real path
            // escapes the root.
            for entry in WalkDir::new(&base_real).follow_links(opts.follow_symlinks) {
                let entry = entry.map_err(|e| ProviderError::Other(format!("glob scan: {e}")))?;
                let path = entry.path();
                if path == base_real {
                    continue; // skip the base itself
                }
                if opts.follow_symlinks
                    && path::canonicalize(path)
                        .map(|real| {
                            // A followed link may leave the jail, or stay inside
                            // it but leave the scope list — listing a name is a
                            // read either way.
                            !path::within_root(&real, &root)
                                || allow_read.as_ref().is_some_and(|a| !a.permits(&real))
                        })
                        .unwrap_or(false)
                {
                    continue; // a followed link left what this run may read
                }
                let rel = path.strip_prefix(&base_real).unwrap_or(path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !opts.dot && rel_str.split('/').any(|c| c.starts_with('.')) {
                    continue;
                }
                if opts.only_files && !entry.file_type().is_file() {
                    continue;
                }
                if negated ^ matcher.is_match(&rel_str) {
                    out.push(if opts.absolute {
                        path.to_string_lossy().into_owned()
                    } else {
                        rel_str
                    });
                }
            }
            Ok(out)
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A jail whose reads are scoped to `data/` and writes to `out/`.
    fn scoped_jail(name: &str) -> (std::path::PathBuf, SystemFileSystem) {
        let (root, _) = jail(name);
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(root.join("out")).unwrap();
        let fs = SystemFileSystem::new(&root, &root)
            .with_read_allowlist(PathAllowlist::parse(["data"], &root).unwrap())
            .with_write_allowlist(PathAllowlist::parse(["out"], &root).unwrap());
        (root, fs)
    }

    /// `base.join("")` is `base`, so an empty path used to resolve to the root
    /// jail — and `remove("", { recursive: true })` deleted the whole project.
    #[tokio::test]
    async fn an_empty_path_is_refused_rather_than_resolving_to_the_root() {
        let (root, fs) = jail("empty-path");
        std::fs::write(root.join("keep.txt"), b"data").unwrap();
        for err in [
            fs.remove(String::new(), true).await.unwrap_err(),
            fs.chmod(String::new(), 0o000).await.unwrap_err(),
            fs.write(String::new(), b"x".to_vec(), false)
                .await
                .unwrap_err(),
            fs.truncate(String::new(), 0).await.unwrap_err(),
            fs.mkdir(String::new(), false).await.unwrap_err(),
            fs.rename(String::new(), String::new()).await.unwrap_err(),
            fs.stat(String::new())
                .await
                .err()
                .expect("empty path must not stat"),
        ] {
            assert_eq!(err.code(), Some(ErrorCode::InvalidPath), "{err}");
        }
        // The root and its contents are untouched.
        assert!(root.join("keep.txt").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    /// `.`, `./` and `data/..` all legitimately resolve to the root. Reading
    /// them is ordinary; mutating the root is never a coherent request from
    /// inside the jail, and destroys the sandbox the guest runs in.
    #[tokio::test]
    async fn mutating_the_root_itself_is_refused_however_it_is_spelled() {
        let (root, fs) = jail("root-mutation");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/db.txt"), b"important").unwrap();
        let root_str = root.to_string_lossy().to_string();
        for spelling in [".", "./", "data/..", root_str.as_str()] {
            let err = fs.remove(spelling.into(), true).await.unwrap_err();
            assert_eq!(
                err.code(),
                Some(ErrorCode::InvalidPath),
                "remove {spelling}: {err}"
            );
            let err = fs.chmod(spelling.into(), 0o000).await.unwrap_err();
            assert_eq!(
                err.code(),
                Some(ErrorCode::InvalidPath),
                "chmod {spelling}: {err}"
            );
            let err = fs
                .rename(spelling.into(), "moved".into())
                .await
                .unwrap_err();
            assert_eq!(
                err.code(),
                Some(ErrorCode::InvalidPath),
                "rename {spelling}: {err}"
            );
        }
        assert_eq!(
            std::fs::read(root.join("data/db.txt")).unwrap(),
            b"important"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The guard is on the resolved *target*, so it must not catch reads of the
    /// root, nor writes to entries directly inside it.
    #[tokio::test]
    async fn reading_the_root_and_writing_inside_it_still_work() {
        let (root, fs) = jail("root-still-usable");
        std::fs::write(root.join("there.txt"), b"x").unwrap();
        assert!(fs.stat(".".into()).await.unwrap().is_dir);
        assert_eq!(fs.read_dir(".".into()).await.unwrap().len(), 1);
        assert_eq!(
            fs.real_path(".".into()).await.unwrap(),
            path::canonicalize(&root).unwrap().to_string_lossy()
        );
        // A new entry in the root resolves to `root/<name>`, not to `root`.
        fs.write("fresh.txt".into(), b"ok".to_vec(), false)
            .await
            .unwrap();
        fs.mkdir("sub".into(), false).await.unwrap();
        fs.remove("fresh.txt".into(), false).await.unwrap();
        // Temp entries default to the base directory and must keep working.
        assert!(
            !fs.make_temp_dir(String::new(), String::new())
                .await
                .unwrap()
                .is_empty()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_read_outside_the_read_list_is_refused() {
        let (root, fs) = scoped_jail("scoped-read");
        std::fs::write(root.join("data/ok.txt"), b"fine").unwrap();
        std::fs::write(root.join("secrets.env"), b"TOKEN=1").unwrap();
        assert_eq!(fs.read("data/ok.txt".into()).await.unwrap(), b"fine");
        let err = fs.read("secrets.env".into()).await.unwrap_err();
        // A scoped denial, not a jail escape: the path is inside the root, it
        // is simply not one this run may read.
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");
    }

    #[tokio::test]
    async fn a_write_outside_the_write_list_is_refused() {
        let (root, fs) = scoped_jail("scoped-write");
        fs.write("out/report.json".into(), b"{}".to_vec(), false)
            .await
            .unwrap();
        let err = fs
            .write("data/ok.txt".into(), b"x".to_vec(), false)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");
        // Nothing was created on the refused path: the check precedes the open.
        assert!(!root.join("data/ok.txt").exists());
    }

    #[tokio::test]
    async fn read_and_write_are_separate_lists() {
        // Being allowed to write somewhere is not being allowed to read it.
        let (root, fs) = scoped_jail("scoped-separate");
        std::fs::write(root.join("out/written.txt"), b"x").unwrap();
        assert!(fs.read("out/written.txt".into()).await.is_err());
        assert!(
            fs.write("data/x.txt".into(), b"x".to_vec(), false)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_symlink_cannot_walk_out_of_the_read_list() {
        // The whole reason the check runs after canonicalization: the name
        // `data/escape/secrets.env` is inside the list, the file is not.
        let (root, fs) = scoped_jail("scoped-symlink");
        std::fs::write(root.join("secrets.env"), b"TOKEN=1").unwrap();
        std::os::unix::fs::symlink(&root, root.join("data/escape")).unwrap();
        let err = fs.read("data/escape/secrets.env".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied), "{err}");
    }

    #[tokio::test]
    async fn a_scope_list_narrows_the_jail_and_never_widens_it() {
        // An entry outside the root is not a way out of it: the jail check runs
        // first, and its refusal is the one reported.
        let (root, _) = jail("scoped-outside");
        let outside = root.parent().unwrap().to_path_buf();
        let fs = SystemFileSystem::new(&root, &root)
            .with_read_allowlist(PathAllowlist::parse([outside.to_string_lossy()], &root).unwrap());
        let err = fs.read("../anything.txt".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::JailEscape), "{err}");
    }

    #[tokio::test]
    async fn an_unscoped_filesystem_reaches_the_whole_jail() {
        let (root, fs) = jail("unscoped");
        std::fs::write(root.join("anywhere.txt"), b"ok").unwrap();
        assert_eq!(fs.read("anywhere.txt".into()).await.unwrap(), b"ok");
    }

    /// A fresh empty jail rooted at its own temp directory, plus a `SystemFileSystem`
    /// based there. Named per test so cases cannot collide.
    fn jail(name: &str) -> (std::path::PathBuf, SystemFileSystem) {
        let root = std::env::temp_dir().join(format!("esrun-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let fs = SystemFileSystem::new(&root, &root);
        (root, fs)
    }

    #[tokio::test]
    async fn a_write_is_readable_in_full_the_moment_it_resolves() {
        // Over 64 KiB `write` takes the async path, which used to resolve while
        // the bytes were still in flight: the read below saw 0 bytes, or a
        // prefix, in most attempts. Repeated because it was a race, and one
        // lucky pass would have hidden it.
        let (root, fs) = jail("write-visibility");
        let data = vec![b'x'; 260_000];
        for attempt in 0..20 {
            let name = format!("big-{attempt}.bin");
            let n = fs.write(name.clone(), data.clone(), false).await.unwrap();
            assert_eq!(n, data.len() as u64);
            assert_eq!(
                fs.read(name.clone()).await.unwrap().len(),
                data.len(),
                "attempt {attempt}: write() resolved over a file that was not written yet",
            );
            // Not just through this provider: the file itself is complete.
            assert_eq!(std::fs::metadata(root.join(&name)).unwrap().len(), 260_000);
        }
    }

    #[tokio::test]
    async fn an_appended_write_is_also_complete_when_it_resolves() {
        let (_root, fs) = jail("append-visibility");
        let chunk = vec![b'y'; 100_000];
        for i in 1..=3 {
            fs.write("log.bin".into(), chunk.clone(), true)
                .await
                .unwrap();
            assert_eq!(fs.read("log.bin".into()).await.unwrap().len(), 100_000 * i);
        }
    }

    #[tokio::test]
    async fn copy_duplicates_a_file_and_reports_its_size() {
        let (root, fs) = jail("copy");
        std::fs::write(root.join("src.txt"), b"hello world").unwrap();
        let n = fs.copy("src.txt".into(), "dst.txt".into()).await.unwrap();
        assert_eq!(n, 11);
        assert_eq!(std::fs::read(root.join("dst.txt")).unwrap(), b"hello world");
    }

    #[tokio::test]
    async fn copy_refuses_a_destination_outside_the_jail() {
        let (root, fs) = jail("copy-escape");
        std::fs::write(root.join("src.txt"), b"secret").unwrap();
        let err = fs
            .copy("src.txt".into(), "../escaped.txt".into())
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::JailEscape), "{err}");
    }

    /// `fs::copy` truncates the destination before reading the source, so a file
    /// copied onto itself came back empty and the call reported success with 0
    /// bytes — the backup wiped the original.
    #[tokio::test]
    async fn copying_a_file_onto_itself_is_refused_rather_than_emptying_it() {
        let (root, fs) = jail("copy-self");
        std::fs::write(root.join("a.txt"), b"hello world").unwrap();

        let err = fs.copy("a.txt".into(), "a.txt".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::SameFile), "{err}");
        assert_eq!(
            std::fs::read(root.join("a.txt")).unwrap(),
            b"hello world",
            "the file must be untouched",
        );

        // Same file reached by a different spelling — `jailed` canonicalizes, so
        // this is the equal-paths case rather than the inode one.
        let err = fs.copy("a.txt".into(), "./a.txt".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::SameFile), "{err}");
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"hello world");
    }

    /// Two hardlinks to one inode have different paths, so path equality alone
    /// would let the copy through — and truncating either empties both.
    #[cfg(unix)]
    #[tokio::test]
    async fn copying_between_hardlinks_to_one_inode_is_refused() {
        let (root, fs) = jail("copy-hardlink");
        std::fs::write(root.join("a.txt"), b"hello world").unwrap();
        std::fs::hard_link(root.join("a.txt"), root.join("b.txt")).unwrap();

        let err = fs.copy("a.txt".into(), "b.txt".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::SameFile), "{err}");
        assert_eq!(std::fs::read(root.join("a.txt")).unwrap(), b"hello world");
    }

    /// The guard must not cost ordinary copies their overwrite behaviour: two
    /// distinct files that merely hold the same bytes are not the same file.
    #[tokio::test]
    async fn copy_still_overwrites_a_distinct_destination() {
        let (root, fs) = jail("copy-overwrite");
        std::fs::write(root.join("src.txt"), b"new").unwrap();
        std::fs::write(root.join("dst.txt"), b"old contents").unwrap();

        let n = fs.copy("src.txt".into(), "dst.txt".into()).await.unwrap();
        assert_eq!(n, 3);
        assert_eq!(std::fs::read(root.join("dst.txt")).unwrap(), b"new");
    }

    #[tokio::test]
    async fn truncate_shortens_and_extends() {
        let (root, fs) = jail("truncate");
        std::fs::write(root.join("f"), b"0123456789").unwrap();
        fs.truncate("f".into(), 4).await.unwrap();
        assert_eq!(std::fs::read(root.join("f")).unwrap(), b"0123");
        // Growing zero-fills rather than leaving the old bytes behind.
        fs.truncate("f".into(), 6).await.unwrap();
        assert_eq!(std::fs::read(root.join("f")).unwrap(), b"0123\0\0");
    }

    /// A temp name must be unpredictable and land inside the jail — the OS temp
    /// directory is outside it, and writing there would be the one filesystem
    /// call that escapes.
    #[tokio::test]
    async fn temp_entries_are_created_inside_the_jail() {
        let (root, fs) = jail("temp");
        let dir = fs.make_temp_dir(String::new(), "d-".into()).await.unwrap();
        let file = fs.make_temp_file(String::new(), "f-".into()).await.unwrap();
        for made in [&dir, &file] {
            let real = std::fs::canonicalize(made).unwrap();
            assert!(
                real.starts_with(std::fs::canonicalize(&root).unwrap()),
                "{made} must be inside the jail"
            );
        }
        assert!(std::path::Path::new(&dir).is_dir());
        assert!(std::path::Path::new(&file).is_file());

        // Two calls must not collide, or "temp" means nothing.
        let again = fs.make_temp_dir(String::new(), "d-".into()).await.unwrap();
        assert_ne!(dir, again);
    }

    #[tokio::test]
    async fn real_path_resolves_dot_segments() {
        let (root, fs) = jail("realpath");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::write(root.join("a/b/f"), b"x").unwrap();
        let real = fs.real_path("a/../a/b/f".into()).await.unwrap();
        assert!(real.ends_with("f"), "{real}");
        assert!(!real.contains(".."), "{real}");
    }

    #[tokio::test]
    async fn real_path_errors_on_a_missing_target() {
        // It answers "where does this really point?", and there is no honest
        // answer for something that is not there.
        let (_root, fs) = jail("realpath-missing");
        assert!(fs.real_path("nope".into()).await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn read_link_returns_the_stored_target_and_real_path_follows_it() {
        let (root, fs) = jail("readlink");
        std::fs::write(root.join("target.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("target.txt", root.join("link")).unwrap();

        // Verbatim: the relative target as stored, not a resolved path.
        assert_eq!(fs.read_link("link".into()).await.unwrap(), "target.txt");
        assert!(
            fs.real_path("link".into())
                .await
                .unwrap()
                .ends_with("target.txt")
        );
    }

    /// A link out of the jail must not be followed. `read_link` reads data out
    /// of the link, but `real_path` is an access, and it has to refuse.
    #[cfg(unix)]
    #[tokio::test]
    async fn real_path_refuses_a_link_that_escapes_the_jail() {
        let (root, fs) = jail("readlink-escape");
        let outside = root.parent().unwrap().join("esrun-fs-outside-target");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("out")).unwrap();
        let err = fs.real_path("out".into()).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::JailEscape), "{err}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn chmod_sets_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let (root, fs) = jail("chmod");
        std::fs::write(root.join("k"), b"secret").unwrap();
        fs.chmod("k".into(), 0o600).await.unwrap();
        let mode = std::fs::metadata(root.join("k"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "got {mode:o}");
    }

    /// Resolution must re-canonicalize on every call: a path that resolves
    /// safely while it does not yet exist must be rejected once it becomes a
    /// symlink that escapes the jail. (Guards against caching resolved paths,
    /// which would silently defeat the symlink re-check.)
    #[test]
    fn jailed_rechecks_symlink_escape_on_every_call() {
        let tmp = std::env::temp_dir().join(format!("esrun-fsjail-{}", std::process::id()));
        let root = tmp.join("root");
        let outside = tmp.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let fs = SystemFileSystem::new(&root, &root);

        // "link" does not exist yet -> resolves under the (existing) root.
        let first = fs.jailed("link", Access::Read);
        assert!(first.is_ok(), "should resolve before the symlink exists");

        // Now "link" becomes a symlink pointing outside the jail.
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // A second resolution must re-canonicalize and reject the escape,
        // carrying the stable jail-escape code (SPEC §6 Phase 13).
        let second = fs.jailed("link", Access::Read);
        assert!(
            matches!(
                second,
                Err(ProviderError::Coded {
                    code: ErrorCode::JailEscape,
                    ..
                })
            ),
            "symlink escape must be re-checked with ERR_JAIL_ESCAPE, got {second:?}"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    // Provider io failures carry their stable classification (SPEC §6 Phase 13).
    #[tokio::test]
    async fn missing_file_read_carries_the_not_found_code() {
        let tmp = std::env::temp_dir().join(format!("esrun-fscode-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let fs = SystemFileSystem::new(&tmp, &tmp);
        let err = fs.read("does-not-exist.txt".into()).await.unwrap_err();
        assert!(
            matches!(
                err,
                ProviderError::Coded {
                    code: ErrorCode::NotFound,
                    ..
                }
            ),
            "expected ERR_NOT_FOUND, got {err:?}"
        );
        std::fs::remove_dir_all(&tmp).ok();
    }
}
