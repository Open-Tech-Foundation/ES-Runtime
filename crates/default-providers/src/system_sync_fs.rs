//! OS-backed [`SyncFileSystem`] for the standalone embedding — blocking
//! `std::fs` I/O confined to the same **root jail** as [`SystemFileSystem`]
//! (DECISIONS D25).
//!
//! This exists for WASI, whose syscalls are synchronous (see the trait docs).
//! Reads are gated on `Capability::FileRead` and mutations on
//! `Capability::FileWrite` by `runtime` before any method here runs, so a WASI
//! guest reaching a file goes through exactly the same two checks — the
//! capability, then the jail — as `runtime:fs` does.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use es_runtime_providers::{
    DirEntry, FileStat, ProviderError, SyncFd, SyncFileSystem, SyncOpenOptions, SyncWhence,
};

use crate::path;
use crate::system_fs::{confine, file_stat};

/// What a handle refers to. A directory is kept as a path rather than an OS
/// handle: WASI uses directory fds only as anchors for path resolution, and
/// opening a directory as a file is not portable (it fails on Windows).
enum Handle {
    File(std::fs::File),
    Dir(PathBuf),
}

/// A [`SyncFileSystem`] over the real OS, jailed to `root`. Relative paths
/// resolve against `base` (the runtime's working directory).
pub struct SystemSyncFileSystem {
    base: PathBuf,
    root: PathBuf,
    /// Open handles. A `Mutex` because the trait is `Sync`; contention is nil in
    /// practice (the runtime drives one thread).
    open: Mutex<HashMap<SyncFd, Handle>>,
    next_fd: Mutex<SyncFd>,
}

impl SystemSyncFileSystem {
    /// Builds a jailed synchronous filesystem: relative paths resolve under
    /// `base`, every access is confined to the canonicalized `root`.
    pub fn new(base: impl AsRef<Path>, root: impl AsRef<Path>) -> Self {
        let root =
            path::canonicalize(root.as_ref()).unwrap_or_else(|_| root.as_ref().to_path_buf());
        SystemSyncFileSystem {
            base: base.as_ref().to_path_buf(),
            root,
            open: Mutex::new(HashMap::new()),
            next_fd: Mutex::new(1),
        }
    }

    /// Resolves `p` against `base` and confines it to `root`. Re-canonicalizes on
    /// every call, exactly as the async jail does: caching a validated path would
    /// let a later symlink swap escape it.
    fn jailed(&self, p: &str) -> Result<PathBuf, ProviderError> {
        let raw = Path::new(p);
        let abs = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.base.join(raw)
        };
        confine(&abs, &self.root)
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
        let resolved = self.jailed(path)?;
        let display = resolved.display().to_string();

        let handle = if options.directory {
            let meta = std::fs::metadata(&resolved).map_err(|e| io(&display, e))?;
            if !meta.is_dir() {
                return Err(ProviderError::Other(format!(
                    "{display} is not a directory"
                )));
            }
            Handle::Dir(resolved)
        } else {
            let file = std::fs::OpenOptions::new()
                .read(options.read)
                .write(options.write)
                .append(options.append)
                .truncate(options.truncate)
                .create(options.create)
                .create_new(options.create_new)
                .open(&resolved)
                .map_err(|e| io(&display, e))?;
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
        let resolved = self.jailed(path)?;
        let meta =
            std::fs::metadata(&resolved).map_err(|e| io(&resolved.display().to_string(), e))?;
        Ok(file_stat(&meta))
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, ProviderError> {
        let resolved = self.jailed(path)?;
        let display = resolved.display().to_string();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&resolved).map_err(|e| io(&display, e))? {
            let entry = entry.map_err(|e| io(&display, e))?;
            let file_type = entry.file_type().map_err(|e| io(&display, e))?;
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
        let resolved = self.jailed(path)?;
        std::fs::create_dir(&resolved).map_err(|e| io(&resolved.display().to_string(), e))
    }

    fn remove_file(&self, path: &str) -> Result<(), ProviderError> {
        let resolved = self.jailed(path)?;
        std::fs::remove_file(&resolved).map_err(|e| io(&resolved.display().to_string(), e))
    }

    fn remove_dir(&self, path: &str) -> Result<(), ProviderError> {
        let resolved = self.jailed(path)?;
        std::fs::remove_dir(&resolved).map_err(|e| io(&resolved.display().to_string(), e))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), ProviderError> {
        let from = self.jailed(from)?;
        let to = self.jailed(to)?;
        std::fs::rename(&from, &to).map_err(|e| io(&from.display().to_string(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs_in(name: &str) -> (SystemSyncFileSystem, PathBuf) {
        let dir = std::env::temp_dir().join(format!("esrt-syncfs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fs = SystemSyncFileSystem::new(&dir, &dir);
        (fs, dir)
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
}
