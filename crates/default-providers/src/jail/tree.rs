//! A directory pinned by descriptor and written to by relative path
//! (DECISIONS D180) — the [`Tree`] behind [`FileSystem::open_tree`].
//!
//! The jail promises that a path stays inside the project. Writing a tree of
//! files needs more than that: a symlink that stays inside the project is
//! allowed by the jail, and an archive entry written through one lands
//! somewhere the archive did not name. So beneath the pinned directory nothing
//! is followed. Each component is opened from the previous descriptor with
//! `O_NOFOLLOW`, the final name is opened, created, renamed or removed with
//! the `*at` call for it, and a link anywhere along the way — present before,
//! or swapped in during — is refused rather than followed.
//!
//! Windows has no `*at` family; there, as in [`anchor`](super::anchor), each
//! component is checked with `symlink_metadata` and then used by path. The
//! window between the two stays open, on the record rather than by omission.
//!
//! [`FileSystem::open_tree`]: es_runtime_providers::FileSystem::open_tree

use std::sync::Arc;

use es_runtime_common::ErrorCode;
use es_runtime_providers::{BoxFuture, ProviderError, Tree};

use super::Anchored;

/// The pinned directory. Cheap to clone into a blocking task.
pub(crate) struct PinnedTree {
    inner: Arc<Inner>,
}

struct Inner {
    #[cfg(unix)]
    dir: std::os::fd::OwnedFd,
    #[cfg(not(unix))]
    dir: std::path::PathBuf,
    /// The path as the program wrote it, for messages (D141).
    shown: String,
}

impl PinnedTree {
    /// Pins the directory `anchored` names. The jail admitted it; this opens
    /// it without following a link at its name.
    pub(super) fn pin(anchored: &Anchored) -> Result<PinnedTree, ProviderError> {
        #[cfg(unix)]
        let dir = anchored.open_dir()?;
        #[cfg(not(unix))]
        let dir = {
            let dir = anchored.dir_path();
            refuse_link(&dir, anchored.shown())?;
            dir
        };
        Ok(PinnedTree {
            inner: Arc::new(Inner {
                dir,
                shown: anchored.shown().to_string(),
            }),
        })
    }

    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Inner) -> Result<T, ProviderError> + Send + 'static,
    ) -> BoxFuture<Result<T, ProviderError>> {
        let inner = self.inner.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || work(&inner))
                .await
                .map_err(|e| {
                    ProviderError::Other(format!("tree operation did not complete: {e}"))
                })?
        })
    }
}

impl Tree for PinnedTree {
    fn check(&self, path: String) -> BoxFuture<Result<(), ProviderError>> {
        self.run(move |tree| tree.check(&path))
    }

    fn mkdir(&self, path: String) -> BoxFuture<Result<(), ProviderError>> {
        self.run(move |tree| {
            let parts = components(&tree.shown, &path)?;
            tree.dirs(&path, &parts, true).map(drop)
        })
    }

    fn write(
        &self,
        path: String,
        data: Vec<u8>,
        append: bool,
    ) -> BoxFuture<Result<(), ProviderError>> {
        self.run(move |tree| tree.write(&path, &data, append))
    }

    fn rename(&self, from: String, to: String) -> BoxFuture<Result<(), ProviderError>> {
        self.run(move |tree| tree.rename(&from, &to))
    }

    fn remove(&self, path: String) -> BoxFuture<Result<(), ProviderError>> {
        self.run(move |tree| tree.remove(&path))
    }

    fn make_temp_dir(&self, prefix: String) -> BoxFuture<Result<String, ProviderError>> {
        self.run(move |tree| tree.make_temp_dir(&prefix))
    }
}

/// The components of a path relative to the pinned directory. Refuses what
/// could name anything but a descendant: empty, absolute, `.`, `..`, an empty
/// component, a backslash (a separator on Windows), and NUL.
fn components<'p>(shown: &str, path: &'p str) -> Result<Vec<&'p str>, ProviderError> {
    let invalid = |why: &str| ProviderError::Coded {
        code: ErrorCode::InvalidPath,
        message: format!("{shown}: {path:?} {why}"),
    };
    if path.is_empty() {
        return Err(invalid("is empty"));
    }
    if path.starts_with('/') {
        return Err(invalid("is absolute; a tree takes paths relative to it"));
    }
    if path.contains('\\') || path.contains('\0') {
        return Err(invalid("contains a backslash or NUL"));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(invalid("has an empty, . or .. component"));
    }
    Ok(parts)
}

/// A link found where the tree needed a directory or a file.
fn link_refused(shown: &str, path: &str) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::JailEscape,
        message: format!(
            "{shown}: {path:?} passes through a symbolic link; a tree is written without \
             following links"
        ),
    }
}

/// An unpredictable directory name: `prefix` and 16 hex digits.
fn temp_name(prefix: &str) -> Result<String, ProviderError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| ProviderError::Entropy(e.to_string()))?;
    let suffix: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!("{prefix}{suffix}"))
}

#[cfg(unix)]
impl Inner {
    /// Opens (creating with `create`) each directory in `parts`, beneath the
    /// pinned one, never following a link. Resolves to the last.
    fn dirs(
        &self,
        path: &str,
        parts: &[&str],
        create: bool,
    ) -> Result<std::os::fd::OwnedFd, ProviderError> {
        use rustix::fs::{Mode, OFlags, mkdirat, openat};
        use rustix::io::Errno;

        let mut dir = openat(
            &self.dir,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| self.errno(path, e))?;
        for part in parts {
            if create {
                match mkdirat(&dir, *part, Mode::from(0o777)) {
                    Ok(()) | Err(Errno::EXIST) => {}
                    Err(e) => return Err(self.errno(path, e)),
                }
            }
            dir = openat(
                &dir,
                *part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| self.refusal(&dir, part, path, e))?;
        }
        Ok(dir)
    }

    /// Walks `path` without creating anything, refusing a link at any part
    /// that exists. Stops quietly at the first part that does not.
    fn check(&self, path: &str) -> Result<(), ProviderError> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat};
        use rustix::io::Errno;

        let parts = components(&self.shown, path)?;
        let mut dir = openat(
            &self.dir,
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| self.errno(path, e))?;
        for (index, part) in parts.iter().enumerate() {
            let kind = match statat(&dir, *part, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => FileType::from_raw_mode(stat.st_mode),
                Err(Errno::NOENT) => return Ok(()),
                Err(e) => return Err(self.errno(path, e)),
            };
            if kind == FileType::Symlink {
                return Err(link_refused(&self.shown, path));
            }
            if index + 1 == parts.len() || kind != FileType::Directory {
                return Ok(());
            }
            dir = openat(
                &dir,
                *part,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| self.refusal(&dir, part, path, e))?;
        }
        Ok(())
    }

    fn write(&self, path: &str, data: &[u8], append: bool) -> Result<(), ProviderError> {
        use rustix::fs::{Mode, OFlags, openat};
        use std::io::Write;

        let parts = components(&self.shown, path)?;
        let (name, parents) = parts.split_last().expect("components is never empty");
        let dir = self.dirs(path, parents, true)?;
        let mut flags = OFlags::WRONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        flags |= if append {
            OFlags::APPEND
        } else {
            OFlags::TRUNC
        };
        let fd = openat(&dir, *name, flags, Mode::from(0o666))
            .map_err(|e| self.refusal(&dir, name, path, e))?;
        std::fs::File::from(fd)
            .write_all(data)
            .map_err(|e| self.io(path, e))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), ProviderError> {
        let source = components(&self.shown, from)?;
        let target = components(&self.shown, to)?;
        let (from_name, from_parents) = source.split_last().expect("components is never empty");
        let (to_name, to_parents) = target.split_last().expect("components is never empty");
        let from_dir = self.dirs(from, from_parents, false)?;
        let to_dir = self.dirs(to, to_parents, true)?;
        // `renameat` acts on names: a link at `from` is moved as a link, and
        // one at `to` is replaced rather than written through.
        rustix::fs::renameat(&from_dir, *from_name, &to_dir, *to_name)
            .map_err(|e| self.errno(to, e))
    }

    fn remove(&self, path: &str) -> Result<(), ProviderError> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat, statat, unlinkat};

        let parts = components(&self.shown, path)?;
        let (name, parents) = parts.split_last().expect("components is never empty");
        let dir = self.dirs(path, parents, false)?;
        let stat =
            statat(&dir, *name, AtFlags::SYMLINK_NOFOLLOW).map_err(|e| self.errno(path, e))?;
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            let child = openat(
                &dir,
                *name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| self.refusal(&dir, name, path, e))?;
            super::anchor::empty(&child, &self.shown, 0)?;
            drop(child);
            unlinkat(&dir, *name, AtFlags::REMOVEDIR).map_err(|e| self.errno(path, e))
        } else {
            unlinkat(&dir, *name, AtFlags::empty()).map_err(|e| self.errno(path, e))
        }
    }

    fn make_temp_dir(&self, prefix: &str) -> Result<String, ProviderError> {
        use rustix::fs::{Mode, mkdirat};
        use rustix::io::Errno;

        if prefix.contains('/') || prefix.contains('\\') || prefix.contains('\0') {
            return Err(ProviderError::Coded {
                code: ErrorCode::InvalidPath,
                message: format!(
                    "{}: a temp directory prefix cannot contain a separator",
                    self.shown
                ),
            });
        }
        for _ in 0..16 {
            let name = temp_name(prefix)?;
            match mkdirat(&self.dir, name.as_str(), Mode::from(0o700)) {
                Ok(()) => return Ok(name),
                Err(Errno::EXIST) => continue,
                Err(e) => return Err(self.errno(&name, e)),
            }
        }
        Err(ProviderError::Other(format!(
            "{}: could not find an unused temp directory name",
            self.shown
        )))
    }

    /// A failed `openat` of `part` beneath `dir`: a link there is the refusal.
    fn refusal(
        &self,
        dir: &std::os::fd::OwnedFd,
        part: &str,
        path: &str,
        err: rustix::io::Errno,
    ) -> ProviderError {
        use rustix::fs::{AtFlags, FileType, statat};
        use rustix::io::Errno;
        if matches!(err, Errno::LOOP | Errno::MLINK | Errno::NOTDIR)
            && statat(dir, part, AtFlags::SYMLINK_NOFOLLOW)
                .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Symlink)
        {
            return link_refused(&self.shown, path);
        }
        self.errno(path, err)
    }

    fn errno(&self, path: &str, err: rustix::io::Errno) -> ProviderError {
        match err {
            rustix::io::Errno::LOOP | rustix::io::Errno::MLINK => link_refused(&self.shown, path),
            other => self.io(path, other.into()),
        }
    }

    fn io(&self, path: &str, err: std::io::Error) -> ProviderError {
        ProviderError::from_io(format!("{}/{path}", self.shown.trim_end_matches('/')), &err)
    }
}

/// Windows: the same operations by path, each component checked first.
#[cfg(not(unix))]
fn refuse_link(path: &std::path::Path, shown: &str) -> Result<(), ProviderError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(link_refused(shown, &path.to_string_lossy()))
        }
        _ => Ok(()),
    }
}

#[cfg(not(unix))]
impl Inner {
    /// Each prefix of `parts` beneath the pinned directory, checked for a link
    /// and created with `create`. Resolves to the last.
    fn dirs(
        &self,
        path: &str,
        parts: &[&str],
        create: bool,
    ) -> Result<std::path::PathBuf, ProviderError> {
        let mut at = self.dir.clone();
        for part in parts {
            at.push(part);
            self.refuse(&at, path)?;
            if create {
                match std::fs::create_dir(&at) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(self.io(path, e)),
                }
            }
        }
        Ok(at)
    }

    fn refuse(&self, at: &std::path::Path, path: &str) -> Result<(), ProviderError> {
        match std::fs::symlink_metadata(at) {
            Ok(meta) if meta.file_type().is_symlink() => Err(link_refused(&self.shown, path)),
            _ => Ok(()),
        }
    }

    fn check(&self, path: &str) -> Result<(), ProviderError> {
        let parts = components(&self.shown, path)?;
        let mut at = self.dir.clone();
        for part in parts {
            at.push(part);
            match std::fs::symlink_metadata(&at) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(link_refused(&self.shown, path));
                }
                Ok(meta) if meta.is_dir() => {}
                _ => return Ok(()),
            }
        }
        Ok(())
    }

    fn write(&self, path: &str, data: &[u8], append: bool) -> Result<(), ProviderError> {
        use std::io::Write;
        let parts = components(&self.shown, path)?;
        let (name, parents) = parts.split_last().expect("components is never empty");
        let target = self.dirs(path, parents, true)?.join(name);
        self.refuse(&target, path)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(append)
            .truncate(!append)
            .open(&target)
            .map_err(|e| self.io(path, e))?;
        file.write_all(data).map_err(|e| self.io(path, e))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), ProviderError> {
        let source = components(&self.shown, from)?;
        let target = components(&self.shown, to)?;
        let (from_name, from_parents) = source.split_last().expect("components is never empty");
        let (to_name, to_parents) = target.split_last().expect("components is never empty");
        let from_path = self.dirs(from, from_parents, false)?.join(from_name);
        let to_path = self.dirs(to, to_parents, true)?.join(to_name);
        std::fs::rename(&from_path, &to_path).map_err(|e| self.io(to, e))
    }

    fn remove(&self, path: &str) -> Result<(), ProviderError> {
        let parts = components(&self.shown, path)?;
        let (name, parents) = parts.split_last().expect("components is never empty");
        let target = self.dirs(path, parents, false)?.join(name);
        let meta = std::fs::symlink_metadata(&target).map_err(|e| self.io(path, e))?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&target).map_err(|e| self.io(path, e))
        } else {
            std::fs::remove_file(&target).map_err(|e| self.io(path, e))
        }
    }

    fn make_temp_dir(&self, prefix: &str) -> Result<String, ProviderError> {
        if prefix.contains('/') || prefix.contains('\\') || prefix.contains('\0') {
            return Err(ProviderError::Coded {
                code: ErrorCode::InvalidPath,
                message: format!(
                    "{}: a temp directory prefix cannot contain a separator",
                    self.shown
                ),
            });
        }
        for _ in 0..16 {
            let name = temp_name(prefix)?;
            match std::fs::create_dir(self.dir.join(&name)) {
                Ok(()) => return Ok(name),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(self.io(&name, e)),
            }
        }
        Err(ProviderError::Other(format!(
            "{}: could not find an unused temp directory name",
            self.shown
        )))
    }

    fn io(&self, path: &str, err: std::io::Error) -> ProviderError {
        ProviderError::from_io(format!("{}/{path}", self.shown.trim_end_matches('/')), &err)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::jail::Jail;
    use crate::path_allowlist::Access;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("esrun-tree-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::path::canonicalize(&dir).unwrap()
    }

    fn tree(root: &std::path::Path, dir: &str) -> PinnedTree {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        Jail::new(root, root).open_tree(dir, Access::Write).unwrap()
    }

    fn block<T>(future: BoxFuture<T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn writes_creates_renames_and_removes_beneath_the_directory() {
        let root = scratch("ops");
        let t = tree(&root, "out");
        block(t.write("a/b/c.txt".into(), b"one".to_vec(), false)).unwrap();
        block(t.write("a/b/c.txt".into(), b"+two".to_vec(), true)).unwrap();
        assert_eq!(
            std::fs::read(root.join("out/a/b/c.txt")).unwrap(),
            b"one+two"
        );

        block(t.mkdir("d/e".into())).unwrap();
        block(t.mkdir("d/e".into())).unwrap(); // existing is fine
        block(t.rename("a/b/c.txt".into(), "d/e/f.txt".into())).unwrap();
        assert!(root.join("out/d/e/f.txt").is_file());
        assert!(!root.join("out/a/b/c.txt").exists());

        let temp = block(t.make_temp_dir(".stage-".into())).unwrap();
        assert!(temp.starts_with(".stage-") && root.join("out").join(&temp).is_dir());

        block(t.remove("d".into())).unwrap();
        assert!(!root.join("out/d").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_beneath_the_directory_is_refused_not_followed() {
        let root = scratch("links");
        let t = tree(&root, "out");
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("victim"), b"keep").unwrap();
        std::os::unix::fs::symlink(&elsewhere, root.join("out/dir")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("victim"), root.join("out/file")).unwrap();

        for result in [
            block(t.write("dir/x".into(), b"no".to_vec(), false)),
            block(t.write("file".into(), b"no".to_vec(), false)),
            block(t.mkdir("dir/sub".into())),
        ] {
            let error = result.expect_err("a link was followed");
            assert_eq!(error.code(), Some(ErrorCode::JailEscape), "{error:?}");
        }
        assert_eq!(std::fs::read(elsewhere.join("victim")).unwrap(), b"keep");
        assert!(!elsewhere.join("x").exists() && !elsewhere.join("sub").exists());

        // check() refuses a link at any existing part, before anything is done.
        for path in ["dir", "dir/x", "file", "file/x"] {
            let error = block(t.check(path.into())).expect_err(path);
            assert_eq!(error.code(), Some(ErrorCode::JailEscape), "{path:?}");
        }
        block(t.check("missing/x".into())).unwrap();

        // Removing a link removes the link; renaming onto one replaces it.
        block(t.write("new".into(), b"mine".to_vec(), false)).unwrap();
        block(t.rename("new".into(), "file".into())).unwrap();
        assert!(!root.join("out/file").is_symlink());
        assert_eq!(std::fs::read(elsewhere.join("victim")).unwrap(), b"keep");
        block(t.remove("dir".into())).unwrap();
        assert!(elsewhere.is_dir());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_as_the_directory_itself_is_not_pinned() {
        let root = scratch("pin-link");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("out")).unwrap();
        let error = Jail::new(&root, &root)
            .open_tree("out", Access::Write)
            .err()
            .expect("a link was pinned");
        assert_eq!(error.code(), Some(ErrorCode::JailEscape), "{error:?}");
        // The root itself may be pinned: writing beneath it is not mutating it.
        Jail::new(&root, &root)
            .open_tree(".", Access::Write)
            .unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn paths_that_could_leave_the_directory_are_invalid() {
        let root = scratch("paths");
        let t = tree(&root, "out");
        for path in ["", "/etc/x", "../x", "a/../../x", "./x", "a//b", "a\\b"] {
            let error = block(t.write(path.into(), b"no".to_vec(), false)).expect_err(path);
            assert_eq!(error.code(), Some(ErrorCode::InvalidPath), "{path:?}");
        }
        std::fs::remove_dir_all(&root).ok();
    }
}
