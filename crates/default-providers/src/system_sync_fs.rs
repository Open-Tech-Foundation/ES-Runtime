//! OS-backed [`SyncFileSystem`] for the standalone embedding — blocking
//! `std::fs` I/O confined to the same **root jail** as [`SystemFileSystem`]
//! (DECISIONS D25).
//!
//! This exists for WASI, whose syscalls are synchronous (see the trait docs).
//! Reads are gated on `Capability::FileRead` and mutations on
//! `Capability::FileWrite` by `runtime` before any method here runs, so a WASI
//! guest reaching a file goes through exactly the same two checks — the
//! capability, then the jail — as `runtime:fs` does.
//!
//! The jail is the async one's — the same [`Jail`] value, not a copy of it
//! (D180): every content-bearing operation resolves through it and then
//! **pins** the parent directory open, acting by descriptor. A path checked and then used by name is two lookups the
//! filesystem can change between, so this door follows none of them: a
//! component swapped for a symlink after the check is refused, never followed.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use es_runtime_providers::{
    DirEntry, FileStat, ProviderError, SyncFd, SyncFileSystem, SyncOpenOptions, SyncWhence,
};

use crate::jail::{Jail, OpenMode};
use crate::path_allowlist::Access;
use crate::system_fs::file_stat;

/// What a handle refers to. A directory is kept as a path rather than an OS
/// handle: WASI uses directory fds only as anchors for path resolution, and
/// opening a directory as a file is not portable (it fails on Windows).
enum Handle {
    File(std::fs::File),
    Dir(PathBuf),
}

/// A [`SyncFileSystem`] over the real OS, reaching only what its [`Jail`]
/// admits — the same jail as the async filesystem's, not a copy of it.
pub struct SystemSyncFileSystem {
    jail: Arc<Jail>,
    /// Open handles. A `Mutex` because the trait is `Sync`; contention is nil in
    /// practice (the runtime drives one thread).
    open: Mutex<HashMap<SyncFd, Handle>>,
    next_fd: Mutex<SyncFd>,
}

impl SystemSyncFileSystem {
    /// A synchronous filesystem over a fresh jail: relative paths resolve under
    /// `base`, every access is confined to `root`. For scope lists, build the
    /// [`Jail`] and use [`with_jail`](Self::with_jail).
    pub fn new(base: impl AsRef<Path>, root: impl AsRef<Path>) -> Self {
        Self::with_jail(Arc::new(Jail::new(base, root)))
    }

    /// A synchronous filesystem over `jail` — pass the async filesystem's, so
    /// `runtime:fs` and `runtime:wasi` are two doors onto one policy.
    pub fn with_jail(jail: Arc<Jail>) -> Self {
        SystemSyncFileSystem {
            jail,
            open: Mutex::new(HashMap::new()),
            next_fd: Mutex::new(1),
        }
    }

    /// Runs `f` against the handle for `fd`, holding the table lock for the call.
    fn with_handle<T>(
        &self,
        fd: SyncFd,
        f: impl FnOnce(&mut Handle) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        match open.get_mut(&fd) {
            Some(handle) => f(handle),
            None => Err(ProviderError::Other(format!("bad file handle {fd}"))),
        }
    }

    /// Like [`with_handle`](Self::with_handle) but rejects a directory, which has
    /// no byte stream to operate on.
    fn with_file<T>(
        &self,
        fd: SyncFd,
        f: impl FnOnce(&mut std::fs::File) -> Result<T, ProviderError>,
    ) -> Result<T, ProviderError> {
        self.with_handle(fd, |handle| match handle {
            Handle::File(file) => f(file),
            Handle::Dir(_) => Err(ProviderError::Other(
                "handle refers to a directory, not a file".into(),
            )),
        })
    }
}

fn io(p: &str, e: std::io::Error) -> ProviderError {
    ProviderError::from_io(p, &e)
}

impl SyncFileSystem for SystemSyncFileSystem {
    fn open(&self, path: &str, options: SyncOpenOptions) -> Result<SyncFd, ProviderError> {
        // The access an open needs is the access it asks for, and a handle
        // opened for both needs both — the later `read`/`write` calls have only
        // an fd to go on, so this is the one place the path is still known.
        let writes = options.write
            || options.append
            || options.truncate
            || options.create
            || options.create_new;
        let access = if writes { Access::Write } else { Access::Read };
        // Resolved, then pinned to the directory it resolved in: the open below
        // is `openat` against that descriptor with `O_NOFOLLOW` at the final
        // name, so a component swapped for a symlink between the check and the
        // call cannot point the open somewhere the check did not agree to.
        let anchored = self.jail.anchored(path, access)?;
        if writes && options.read {
            // A handle that also reads needs the read scope too, and the
            // fd-based `read` below has no path left to re-check it against.
            self.jail
                .scoped(anchored.path().to_path_buf(), Access::Read)
                .map_err(|refused| refused.named(path))?;
        }

        let handle = if options.directory {
            // A directory fd is an anchor, not a stream: record the path and
            // check it really is a directory, without holding a host handle.
            if !anchored.is_dir()? {
                return Err(ProviderError::Other(format!("{path} is not a directory")));
            }
            Handle::Dir(anchored.path().to_path_buf())
        } else {
            let file = anchored.open_mode(OpenMode {
                read: options.read,
                write: options.write,
                append: options.append,
                truncate: options.truncate,
                create: options.create,
                create_new: options.create_new,
            })?;
            Handle::File(file)
        };

        let mut next = self.next_fd.lock().unwrap_or_else(|e| e.into_inner());
        let fd = *next;
        *next += 1;
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(fd, handle);
        Ok(fd)
    }

    fn read(&self, fd: SyncFd, buf: &mut [u8]) -> Result<usize, ProviderError> {
        self.with_file(fd, |file| file.read(buf).map_err(|e| io("<open file>", e)))
    }

    fn write(&self, fd: SyncFd, data: &[u8]) -> Result<usize, ProviderError> {
        self.with_file(fd, |file| {
            file.write(data).map_err(|e| io("<open file>", e))
        })
    }

    fn seek(&self, fd: SyncFd, offset: i64, whence: SyncWhence) -> Result<u64, ProviderError> {
        let pos = match whence {
            SyncWhence::Start => SeekFrom::Start(offset.max(0) as u64),
            SyncWhence::Current => SeekFrom::Current(offset),
            SyncWhence::End => SeekFrom::End(offset),
        };
        self.with_file(fd, |file| file.seek(pos).map_err(|e| io("<open file>", e)))
    }

    fn close(&self, fd: SyncFd) -> Result<(), ProviderError> {
        match self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&fd)
        {
            // Dropping the handle closes the OS file.
            Some(_) => Ok(()),
            None => Err(ProviderError::Other(format!("bad file handle {fd}"))),
        }
    }

    fn fstat(&self, fd: SyncFd) -> Result<FileStat, ProviderError> {
        self.with_handle(fd, |handle| {
            let meta = match handle {
                Handle::File(file) => file.metadata(),
                Handle::Dir(path) => std::fs::metadata(path),
            }
            .map_err(|e| io("<open handle>", e))?;
            Ok(file_stat(&meta))
        })
    }

    fn stat(&self, path: &str) -> Result<FileStat, ProviderError> {
        let resolved = self.jail.jailed(path, Access::Read)?;
        let meta = std::fs::metadata(&resolved).map_err(|e| io(path, e))?;
        Ok(file_stat(&meta))
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, ProviderError> {
        let resolved = self.jail.jailed(path, Access::Read)?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&resolved).map_err(|e| io(path, e))? {
            let entry = entry.map_err(|e| io(path, e))?;
            let file_type = entry.file_type().map_err(|e| io(path, e))?;
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_file: file_type.is_file(),
                is_dir: file_type.is_dir(),
                is_symlink: file_type.is_symlink(),
            });
        }
        Ok(out)
    }

    fn mkdir(&self, path: &str) -> Result<(), ProviderError> {
        self.jail.anchored_nofollow(path, Access::Write)?.mkdir()
    }

    fn remove_file(&self, path: &str) -> Result<(), ProviderError> {
        // Not `jailed`: removing a symlink removes the link, so the final
        // component must not be resolved — otherwise this would be a decision
        // about the file at the other end of it.
        self.jail
            .anchored_nofollow(path, Access::Write)?
            .unlink(false)
    }

    fn remove_dir(&self, path: &str) -> Result<(), ProviderError> {
        self.jail
            .anchored_nofollow(path, Access::Write)?
            .unlink(true)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), ProviderError> {
        // Both ends pinned, and neither final component resolved: renaming a
        // symlink moves the link.
        let from = self.jail.anchored_nofollow(from, Access::Write)?;
        let to = self.jail.anchored_nofollow(to, Access::Write)?;
        from.rename_to(&to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::path_allowlist::PathAllowlist;
    use es_runtime_common::ErrorCode;

    fn fs_in(name: &str) -> (SystemSyncFileSystem, PathBuf) {
        let dir = std::env::temp_dir().join(format!("esrt-syncfs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fs = SystemSyncFileSystem::new(&dir, &dir);
        (fs, dir)
    }

    /// The trailing-separator rule holds on this door too: `runtime:wasi` reaches
    /// the same jail, and `file.txt/` must not open the file there either.
    #[test]
    fn a_trailing_separator_on_a_file_is_refused_for_wasi_too() {
        let (fs, dir) = fs_in("trailing-slash");
        std::fs::write(dir.join("file.txt"), b"contents").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let err = fs.stat("file.txt/").err().expect("stat must refuse it");
        assert_eq!(err.code(), Some(ErrorCode::NotDirectory), "{err}");
        let err = fs
            .open("file.txt/", SyncOpenOptions::default())
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::NotDirectory), "{err}");

        // A real directory keeps its conventional trailing separator.
        assert!(fs.stat("sub/").expect("stat sub/").is_dir);
    }

    /// The same root guard the async jail applies — `runtime:wasi` is the other
    /// door onto this filesystem, so an empty path must not resolve to the root
    /// here either, and the root must not be removable through it.
    #[test]
    fn the_root_guard_holds_for_wasi_too() {
        let (fs, dir) = fs_in("root-guard");
        std::fs::write(dir.join("keep.txt"), b"data").unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        for err in [
            fs.remove_dir("").unwrap_err(),
            fs.remove_file("").unwrap_err(),
            fs.mkdir("").unwrap_err(),
            fs.rename("", "").unwrap_err(),
        ] {
            assert_eq!(err.code(), Some(ErrorCode::InvalidPath), "{err}");
        }
        for spelling in [".", "./", "data/.."] {
            let err = fs.remove_dir(spelling).unwrap_err();
            assert_eq!(
                err.code(),
                Some(ErrorCode::InvalidPath),
                "{spelling}: {err}"
            );
        }
        // Reads of the root, and writes to entries inside it, are unaffected.
        assert!(fs.stat(".").unwrap().is_dir);
        assert_eq!(fs.read_dir(".").unwrap().len(), 2);
        fs.mkdir("sub").unwrap();
        assert!(dir.join("keep.txt").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_scope_lists_hold_for_wasi_too() {
        // `runtime:fs` and `runtime:wasi` are two doors onto one filesystem, so
        // the same lists apply — a WASI program is not a way around --allow-read.
        let (_, dir) = fs_in("scoped");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data/ok.txt"), b"fine").unwrap();
        std::fs::write(dir.join("secrets.env"), b"TOKEN=1").unwrap();
        let fs = SystemSyncFileSystem::with_jail(Arc::new(
            Jail::new(&dir, &dir)
                .with_read_allowlist(PathAllowlist::parse(["data"], &dir).unwrap())
                .with_write_allowlist(PathAllowlist::parse(["out"], &dir).unwrap()),
        ));

        let opts = SyncOpenOptions {
            read: true,
            ..Default::default()
        };
        assert!(fs.open("data/ok.txt", opts).is_ok());
        let err = fs.open("secrets.env", opts).unwrap_err();
        assert_eq!(
            err.code(),
            Some(es_runtime_common::ErrorCode::PermissionDenied)
        );

        // An open that creates is a write, wherever it points.
        let creating = SyncOpenOptions {
            write: true,
            create: true,
            ..Default::default()
        };
        assert!(fs.open("data/new.txt", creating).is_err());
        assert!(fs.stat("secrets.env").is_err());
        assert!(fs.mkdir("data/sub").is_err());
    }

    #[test]
    fn writes_then_reads_a_file_through_handles() {
        let (fs, dir) = fs_in("roundtrip");

        let fd = fs
            .open(
                "out.txt",
                SyncOpenOptions {
                    write: true,
                    create: true,
                    truncate: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(fs.write(fd, b"hello sync").unwrap(), 10);
        fs.close(fd).unwrap();

        let fd = fs
            .open(
                "out.txt",
                SyncOpenOptions {
                    read: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let mut buf = [0u8; 32];
        let n = fs.read(fd, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello sync");
        // Reading at end of file is zero bytes, not an error.
        assert_eq!(fs.read(fd, &mut buf).unwrap(), 0);
        fs.close(fd).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn seeks_from_each_anchor() {
        let (fs, dir) = fs_in("seek");
        std::fs::write(dir.join("d.txt"), b"0123456789").unwrap();

        let fd = fs
            .open(
                "d.txt",
                SyncOpenOptions {
                    read: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(fs.seek(fd, 4, SyncWhence::Start).unwrap(), 4);
        let mut buf = [0u8; 2];
        fs.read(fd, &mut buf).unwrap();
        assert_eq!(&buf, b"45");
        assert_eq!(fs.seek(fd, -1, SyncWhence::Current).unwrap(), 5);
        assert_eq!(fs.seek(fd, -2, SyncWhence::End).unwrap(), 8);
        fs.close(fd).unwrap();

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_closed_or_unknown_handle_errors_rather_than_panicking() {
        let (fs, dir) = fs_in("badfd");
        let mut buf = [0u8; 4];
        assert!(fs.read(9999, &mut buf).is_err());
        assert!(fs.close(9999).is_err());

        let fd = fs
            .open(
                "x.txt",
                SyncOpenOptions {
                    write: true,
                    create: true,
                    ..Default::default()
                },
            )
            .unwrap();
        fs.close(fd).unwrap();
        assert!(fs.write(fd, b"after close").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn directory_handles_are_anchors_not_byte_streams() {
        let (fs, dir) = fs_in("dirfd");
        std::fs::create_dir(dir.join("sub")).unwrap();

        let fd = fs
            .open(
                "sub",
                SyncOpenOptions {
                    read: true,
                    directory: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(fs.fstat(fd).unwrap().is_dir);
        // A directory has no byte stream.
        let mut buf = [0u8; 4];
        assert!(fs.read(fd, &mut buf).is_err());
        fs.close(fd).unwrap();

        // Opening a non-directory as one is rejected.
        std::fs::write(dir.join("f.txt"), b"x").unwrap();
        assert!(
            fs.open(
                "f.txt",
                SyncOpenOptions {
                    directory: true,
                    ..Default::default()
                }
            )
            .is_err()
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_root_jail_holds_for_every_path_method() {
        let (fs, dir) = fs_in("jail");
        let opts = SyncOpenOptions {
            read: true,
            ..Default::default()
        };
        // `..` cannot climb out of the root.
        assert!(fs.open("../outside.txt", opts).is_err());
        assert!(fs.stat("../outside.txt").is_err());
        assert!(fs.read_dir("..").is_err());
        assert!(fs.mkdir("../nope").is_err());
        assert!(fs.remove_file("../nope").is_err());
        assert!(fs.rename("a.txt", "../nope.txt").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lists_creates_and_removes_directory_entries() {
        let (fs, dir) = fs_in("entries");
        fs.mkdir("d").unwrap();
        std::fs::write(dir.join("d/a.txt"), b"a").unwrap();

        let entries = fs.read_dir("d").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.txt");
        assert!(entries[0].is_file);

        fs.rename("d/a.txt", "d/b.txt").unwrap();
        assert!(fs.stat("d/b.txt").unwrap().is_file);

        fs.remove_file("d/b.txt").unwrap();
        fs.remove_dir("d").unwrap();
        assert!(fs.stat("d").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The WASI door names what the program wrote too, not the canonical path
    /// the jail decided on (D141).
    #[test]
    fn a_refusal_names_the_path_as_written_for_wasi_too() {
        let (_, dir) = fs_in("spelling");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        let root = crate::path::canonicalize(&dir).unwrap();
        let fs = SystemSyncFileSystem::with_jail(Arc::new(
            Jail::new(&root, &root)
                .with_read_allowlist(PathAllowlist::parse(["data"], &root).unwrap()),
        ));
        for (written, err) in [
            ("../x", fs.stat("../x").err().unwrap()),
            ("secrets.env", fs.stat("secrets.env").err().unwrap()),
            ("data/missing", fs.stat("data/missing").err().unwrap()),
            (".", fs.remove_dir(".").unwrap_err()),
        ] {
            let message = err.to_string();
            assert!(message.contains(written), "{written}: {message}");
            assert!(
                !message.contains(root.to_str().unwrap()),
                "{written}: {message}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The link may be *written* pointing out of the jail — it is data — and it
    /// still cannot be *followed* out: following is an access, and the jail is
    /// where accesses are judged. Same rule as the async door.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_jail_is_not_followed_for_wasi_too() {
        let (fs, dir) = fs_in("link-outside");
        let outside = dir
            .parent()
            .unwrap()
            .join(format!("esrt-syncfs-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"top secret").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("out")).unwrap();

        let opts = SyncOpenOptions {
            read: true,
            ..Default::default()
        };
        let err = fs.open("out/secret", opts).unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::JailEscape), "{err}");
        assert!(fs.stat("out/secret").is_err());

        let _ = std::fs::remove_dir_all(&outside);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Following a link that was already there is not the attack: the jail
    /// resolves and confines the target, so it is checked, and the write lands
    /// through the link rather than replacing it.
    #[cfg(unix)]
    #[test]
    fn a_write_follows_a_link_that_was_already_there_for_wasi_too() {
        let (fs, dir) = fs_in("write-through-link");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/config.json"), b"old").unwrap();
        std::os::unix::fs::symlink("real/config.json", dir.join("config.json")).unwrap();

        let fd = fs
            .open(
                "config.json",
                SyncOpenOptions {
                    write: true,
                    truncate: true,
                    ..Default::default()
                },
            )
            .unwrap();
        fs.write(fd, b"new").unwrap();
        fs.close(fd).unwrap();
        assert_eq!(std::fs::read(dir.join("real/config.json")).unwrap(), b"new");
        assert!(dir.join("config.json").is_symlink());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `remove_file` and `rename` are about the **link**, not what it points
    /// at. Resolving the final component first deleted the file at the other
    /// end of it — or refused a link that pointed out of the jail rather than
    /// removing the link — neither of which is what the async door does.
    #[cfg(unix)]
    #[test]
    fn removing_a_link_removes_the_link_for_wasi_too() {
        let (fs, dir) = fs_in("remove-link");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        std::fs::write(dir.join("real/kept.txt"), b"keep me").unwrap();
        std::os::unix::fs::symlink("real/kept.txt", dir.join("link.txt")).unwrap();

        fs.remove_file("link.txt").unwrap();
        assert!(!dir.join("link.txt").exists());
        assert_eq!(
            std::fs::read(dir.join("real/kept.txt")).unwrap(),
            b"keep me",
            "the file the link pointed at must survive"
        );

        std::os::unix::fs::symlink("real/kept.txt", dir.join("moved.txt")).unwrap();
        fs.rename("moved.txt", "elsewhere.txt").unwrap();
        assert!(dir.join("elsewhere.txt").is_symlink());
        assert!(std::fs::read(dir.join("real/kept.txt")).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A link that pointed out of the jail can be removed — removing it is
    /// about the link, not the file it names — where resolving it first would
    /// have refused.
    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_jail_can_still_be_removed_for_wasi_too() {
        let (fs, dir) = fs_in("remove-link-outside");
        let outside = dir
            .parent()
            .unwrap()
            .join(format!("esrt-syncfs-rm-outside-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), b"top secret").unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("out")).unwrap();

        fs.remove_file("out").unwrap();
        assert!(!dir.join("out").exists());
        assert_eq!(
            std::fs::read(outside.join("secret")).unwrap(),
            b"top secret",
            "the file the link pointed at must survive"
        );

        let _ = std::fs::remove_dir_all(&outside);
        std::fs::remove_dir_all(&dir).ok();
    }
}
