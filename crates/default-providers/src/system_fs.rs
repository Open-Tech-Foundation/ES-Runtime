//! OS-backed [`FileSystem`] for the standalone embedding — tokio file I/O
//! confined to a **root jail** (DECISIONS D25). Every path is resolved against a
//! base directory, then its real location is checked to be inside the
//! canonicalized root; an escape (via `..` or a symlink) is rejected. Reads are
//! gated on `Capability::FileRead` and mutations on `Capability::FileWrite` by
//! `runtime` before any method here runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use es_runtime_providers::{
    BoxFuture, DirEntry, FileStat, FileSystem, GlobScanOptions, ProviderError,
};
use globset::GlobBuilder;
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

use crate::path;

/// Compiles a glob pattern into a matcher plus a "negated" flag, covering the
/// full conventional set: `?`, `*` (not crossing `/`), `**` (crossing), `[ab]`,
/// `[a-z]`, `[!abc]` **and** `[^abc]`, `{a,b}`, `\` escaping, and a leading `!`
/// that negates the whole pattern.
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
    fn jailed(&self, p: &str) -> Result<PathBuf, ProviderError> {
        let raw = Path::new(p);
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        confine(&abs, &self.root)
    }
}

fn escape(p: &Path, root: &Path) -> ProviderError {
    ProviderError::Other(format!(
        "path {} escapes the filesystem root jail {} (access outside the root is not permitted)",
        p.display(),
        root.display()
    ))
}

fn confine(abs: &Path, root: &Path) -> Result<PathBuf, ProviderError> {
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
    ProviderError::Other(format!("{p}: {e}"))
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
        let resolved = self.jailed(&path);
        if let Ok(p) = &resolved
            && let Ok(md) = std::fs::metadata(p)
                && md.len() < 64 * 1024 {
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
        let resolved = self.jailed(&path);
        let len = data.len() as u64;

        if let Ok(p) = &resolved
            && len < 64 * 1024 {
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
            Ok(len)
        })
    }

    fn stat(&self, path: String) -> BoxFuture<Result<FileStat, ProviderError>> {
        let resolved = self.jailed(&path);
        if let Ok(p) = &resolved
            && let Ok(md) = std::fs::metadata(p) {
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
        let resolved = self.jailed(&path);
        if let Ok(p) = &resolved {
            return Box::pin(std::future::ready(
                p.try_exists().map_err(|e| other(&path, e)),
            ));
        }
        Box::pin(std::future::ready(Ok(false)))
    }

    fn read_dir(&self, path: String) -> BoxFuture<Result<Vec<DirEntry>, ProviderError>> {
        let p = match self.jailed(&path) {
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
        let resolved = self.jailed(&path);
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
        let resolved = self.jailed(&path);
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
        let from_r = self.jailed(&from);
        let to_r = self.jailed(&to);
        Box::pin(async move {
            let (a, b) = (from_r?, to_r?);
            tokio::fs::rename(&a, &b).await.map_err(|e| other(&from, e))
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
        let resolved = self.jailed(&base);
        let root = self.root.clone();
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
                        .map(|real| !path::within_root(&real, &root))
                        .unwrap_or(false)
                {
                    continue; // a followed link left the jail
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
        let first = fs.jailed("link");
        assert!(first.is_ok(), "should resolve before the symlink exists");

        // Now "link" becomes a symlink pointing outside the jail.
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();

        // A second resolution must re-canonicalize and reject the escape.
        let second = fs.jailed("link");
        assert!(
            second.is_err(),
            "symlink escape must be re-checked, got {second:?}"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
