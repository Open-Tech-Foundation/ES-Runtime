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

/// Compiles a glob pattern with `literal_separator` so `*` does not cross `/`
/// while `**` does (the conventional shell/Bun semantics).
fn glob_matcher(pattern: &str) -> Result<globset::GlobMatcher, ProviderError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| ProviderError::Other(format!("invalid glob pattern {pattern:?}: {e}")))
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
        Box::pin(async move {
            let p = resolved?;
            let len = data.len() as u64;
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
        Box::pin(async move {
            match resolved {
                Ok(p) => tokio::fs::try_exists(&p).await.map_err(|e| other(&path, e)),
                // A path that cannot be resolved within the jail simply does not
                // exist from the guest's point of view.
                Err(_) => Ok(false),
            }
        })
    }

    fn read_dir(&self, path: String) -> BoxFuture<Result<Vec<DirEntry>, ProviderError>> {
        let resolved = self.jailed(&path);
        Box::pin(async move {
            let p = resolved?;
            let mut rd = tokio::fs::read_dir(&p).await.map_err(|e| other(&path, e))?;
            let mut out = Vec::new();
            while let Some(entry) = rd.next_entry().await.map_err(|e| other(&path, e))? {
                let ft = entry.file_type().await.map_err(|e| other(&path, e))?;
                out.push(DirEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    is_file: ft.is_file(),
                    is_dir: ft.is_dir(),
                    is_symlink: ft.is_symlink(),
                });
            }
            Ok(out)
        })
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
        Ok(glob_matcher(pattern)?.is_match(path))
    }

    fn glob_scan(
        &self,
        base: String,
        pattern: String,
        opts: GlobScanOptions,
    ) -> BoxFuture<Result<Vec<String>, ProviderError>> {
        let resolved = self.jailed(&base);
        Box::pin(async move {
            let base_real = resolved?;
            let matcher = glob_matcher(&pattern)?;
            let mut out = Vec::new();
            // follow_links(false) keeps the walk from leaving the jail via a
            // symlink; the base is already confined to root.
            for entry in WalkDir::new(&base_real).follow_links(false) {
                let entry = entry.map_err(|e| ProviderError::Other(format!("glob scan: {e}")))?;
                let path = entry.path();
                if path == base_real {
                    continue; // skip the base itself
                }
                let rel = path.strip_prefix(&base_real).unwrap_or(path);
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                if !opts.dot && rel_str.split('/').any(|c| c.starts_with('.')) {
                    continue;
                }
                if opts.only_files && !entry.file_type().is_file() {
                    continue;
                }
                if matcher.is_match(&rel_str) {
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
