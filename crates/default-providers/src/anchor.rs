//! Resolving a path to the **directory it lives in, held open** — and acting
//! there by descriptor rather than by name.
//!
//! # The window this closes
//!
//! Every jailed operation used to be two steps that named the same file twice:
//!
//! ```text
//!   confine("data/notes.txt")  ->  /project/data/notes.txt      (checked)
//!   std::fs::write("/project/data/notes.txt", …)                (used)
//! ```
//!
//! Between them the filesystem is mutable, and a *name* is not a file. A guest
//! running two operations at once can replace `data` with a symlink to `/etc`
//! after the check and before the use, and the second step follows the new
//! `data` — the check having been about the old one. The jail reports success
//! and the bytes land outside it.
//!
//! It was never nanoseconds, either: [`jailed`](crate::system_fs) resolves
//! eagerly, before the future is constructed, and the syscall then happens a
//! poll and a blocking-pool handoff later. The window spans a scheduling
//! round-trip, which is an eternity to a thread that is trying to win it.
//!
//! # Why a descriptor closes it and a better check cannot
//!
//! No amount of re-checking helps, because the *last* thing to happen is always
//! the kernel resolving the path again for the syscall. The only fix is to stop
//! handing the kernel a path.
//!
//! A file descriptor refers to an **inode**, not to a name. Once this module
//! holds one for `/project/data`, renaming `data`, deleting it, or replacing it
//! with a symlink changes nothing about where the descriptor points — so an
//! `openat(fd, "notes.txt", …)` writes into the directory that was checked, or
//! it fails. There is no third outcome for an attacker to steer towards.
//!
//! # How the walk itself stays honest
//!
//! The path handed here is already canonical: [`confine`](crate::system_fs)
//! resolved every symlink in it and proved the result sits under a root. So no
//! component of it *should* be a symlink — and the walk opens each one with
//! `O_NOFOLLOW`, which turns "should not" into "cannot". A component that has
//! become a symlink since the check is the attack in progress, and it is
//! reported as a jail escape rather than followed.
//!
//! Each directory is opened from the previous descriptor, never from a path, so
//! the chain from the root down is pinned link by link. What the caller ends up
//! holding is the parent, and the final name to use inside it.
//!
//! # What this does not cover
//!
//! **Windows.** There is no `*at` family there; the equivalent is `NtCreateFile`
//! with `RootDirectory` in its `OBJECT_ATTRIBUTES`, which is ntdll surface that
//! neither `std` nor `rustix` exposes. The fallback below is the old
//! path-based behaviour, and is marked as such rather than quietly pretending.
//!
//! **Hard links.** `O_NOFOLLOW` sees a symlink; nothing sees a hard link,
//! because a hard link *is* the file. A guest that could hard-link an outside
//! file to a name inside the jail would defeat the final `openat` — which is
//! why there is no `link()` operation in `runtime:fs`, and why adding one would
//! need more than an afternoon's thought.
//!
//! **Mount points.** A bind mount appearing mid-walk would be followed. Placing
//! one needs privileges no guest of this runtime has.

use std::ffi::OsString;
#[cfg(unix)]
use std::path::Component;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;

/// A checked path, pinned to where it was checked: the parent directory as an
/// open descriptor, and the final name to act on inside it.
///
/// The `path` it came from is kept for **diagnostics only**. Nothing in this
/// module reaches a file through it — that is the whole point of the type.
#[derive(Debug)]
pub struct Anchored {
    #[cfg(unix)]
    parent: std::os::fd::OwnedFd,
    /// Windows has no `*at`: the parent is a path, and the race stays open.
    #[cfg(not(unix))]
    parent: PathBuf,
    name: OsString,
    path: PathBuf,
}

impl Anchored {
    /// What it resolved to. For messages — never to open anything with.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A component of the path was replaced while it was being used.
///
/// Reported as an escape rather than as an I/O error because that is what it
/// is: something arranged for this path to lead somewhere the check did not
/// agree to, and the only reason it did not is that this walk refused to follow
/// it.
#[cfg(unix)]
fn swapped(path: &Path) -> ProviderError {
    ProviderError::Coded {
        code: ErrorCode::JailEscape,
        message: format!(
            "{}: a directory in this path was replaced while the path was being used, \
             so the operation was refused rather than followed to wherever it now leads",
            path.display()
        ),
    }
}

/// An ordinary I/O failure, mapped the way the rest of the provider maps them
/// so a caller sees the same `ERR_*` whichever path reached the syscall.
fn failed(path: &Path, err: std::io::Error) -> ProviderError {
    ProviderError::from_io(path.to_string_lossy().as_ref(), &err)
}

/// Pins `real` — a path [`confine`](crate::system_fs) has already resolved and
/// admitted — to the directory it resolved in.
///
/// `roots` is the set it was admitted under; the walk starts at whichever of
/// them contains it, so only the part of the path the guest had any say over is
/// walked component by component.
#[cfg(unix)]
pub fn anchor(real: &Path, roots: &[PathBuf]) -> Result<Anchored, ProviderError> {
    use rustix::fs::{CWD, Mode, OFlags, openat};

    let Some(root) = roots.iter().find(|root| real.starts_with(root)) else {
        // `confine` admitted it, so this cannot happen — and if it ever does,
        // refusing is the only safe reading of "I do not know which root this
        // is under".
        return Err(swapped(real));
    };

    // The root is configuration, not guest input: it is canonicalized once when
    // the provider is built, and following a symlink *to* it is how `/tmp`
    // works on macOS. Everything below it is walked with `NOFOLLOW`.
    let mut dir = openat(
        CWD,
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| failed(root, e.into()))?;

    let inside = real.strip_prefix(root).map_err(|_| swapped(real))?;
    let mut components = inside.components().peekable();
    // The root itself: nothing to walk, and nothing to name inside it. Callers
    // that mutate have already refused this case; a read of it is `.`.
    let mut name = OsString::from(".");

    while let Some(component) = components.next() {
        let Component::Normal(part) = component else {
            // `confine` returns a canonical path, so `.`, `..`, a prefix or a
            // second root cannot appear. One that does means the assumption
            // this walk rests on is wrong, and the answer to that is to stop.
            return Err(swapped(real));
        };
        if components.peek().is_none() {
            name = part.to_os_string();
            break;
        }
        dir = openat(
            &dir,
            part,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| refusal(&dir, part, real, e))?;
    }

    Ok(Anchored {
        parent: dir,
        name,
        path: real.to_path_buf(),
    })
}

/// Why a component would not open, told apart from the outside.
///
/// `NOFOLLOW` refuses a symlink whatever else is going on, but *which* error it
/// picks depends on the flags and the platform: with `O_DIRECTORY` as well,
/// Linux says `ENOTDIR` and not `ELOOP`, and `ENOTDIR` is equally what a plain
/// file in the middle of a path says. Both are refusals and neither follows
/// anything — but "a directory was replaced under you" and "you wrote
/// `notes.txt/x`" are different things to be told, so the one syscall it takes
/// to know which is worth spending on the error path.
#[cfg(unix)]
fn refusal(
    dir: &std::os::fd::OwnedFd,
    part: &std::ffi::OsStr,
    real: &Path,
    err: rustix::io::Errno,
) -> ProviderError {
    use rustix::io::Errno;
    // BSD and macOS say `EMLINK` where Linux says `ELOOP`.
    let maybe_swapped = matches!(err, Errno::LOOP | Errno::MLINK | Errno::NOTDIR);
    if maybe_swapped {
        let link = rustix::fs::statat(dir, part, rustix::fs::AtFlags::SYMLINK_NOFOLLOW).is_ok_and(
            |stat| {
                rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::Symlink
            },
        );
        if link {
            return swapped(real);
        }
    }
    failed(real, err.into())
}

/// Windows: no `*at`, so the path is carried as it always was and the race
/// stays open. Stated in one place rather than implied by its absence.
#[cfg(not(unix))]
pub fn anchor(real: &Path, _roots: &[PathBuf]) -> Result<Anchored, ProviderError> {
    let parent = real.parent().unwrap_or(real).to_path_buf();
    let name = real
        .file_name()
        .map_or_else(|| OsString::from("."), std::ffi::OsStr::to_os_string);
    Ok(Anchored {
        parent,
        name,
        path: real.to_path_buf(),
    })
}

// --- acting inside the pinned directory --------------------------------------
//
// Every one of these names the file by `self.name` **relative to the descriptor**
// this type is holding. None of them takes a path, and that is not a style
// choice: a path argument here would resolve from the root again at syscall
// time and give back the window the type exists to close.

#[cfg(unix)]
impl Anchored {
    /// Opens the final name for writing, creating it if it is not there.
    ///
    /// `NOFOLLOW`, so a symlink that has appeared at the name since the check is
    /// refused rather than written through. That is not the same guard as the
    /// walk's: this one is about the *file*, and it is what stops a link dropped
    /// at the last component from redirecting the bytes.
    pub fn create(&self, append: bool) -> Result<std::fs::File, ProviderError> {
        use rustix::fs::{Mode, OFlags, openat};
        let mut flags = OFlags::WRONLY | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW;
        flags |= if append {
            OFlags::APPEND
        } else {
            OFlags::TRUNC
        };
        let fd = openat(
            &self.parent,
            self.name.as_os_str(),
            flags,
            Mode::from(0o666),
        )
        .map_err(|e| self.io(e))?;
        Ok(std::fs::File::from(fd))
    }

    /// Opens the final name for reading, refusing a symlink that has appeared
    /// at it.
    pub fn open(&self) -> Result<std::fs::File, ProviderError> {
        use rustix::fs::{Mode, OFlags, openat};
        let fd = openat(
            &self.parent,
            self.name.as_os_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| self.io(e))?;
        Ok(std::fs::File::from(fd))
    }

    /// Opens it for writing **without** creating or truncating — what
    /// `truncate()` and `chmod()` need a descriptor for.
    fn open_write(&self) -> Result<std::fs::File, ProviderError> {
        use rustix::fs::{Mode, OFlags, openat};
        let fd = openat(
            &self.parent,
            self.name.as_os_str(),
            OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| self.io(e))?;
        Ok(std::fs::File::from(fd))
    }

    pub fn mkdir(&self) -> Result<(), ProviderError> {
        rustix::fs::mkdirat(
            &self.parent,
            self.name.as_os_str(),
            rustix::fs::Mode::from(0o777),
        )
        .map_err(|e| self.io(e))
    }

    /// Removes the name. `directory` picks between the two syscalls Unix has
    /// for it, which is a distinction the caller has already had to make.
    pub fn unlink(&self, directory: bool) -> Result<(), ProviderError> {
        let flags = if directory {
            rustix::fs::AtFlags::REMOVEDIR
        } else {
            rustix::fs::AtFlags::empty()
        };
        rustix::fs::unlinkat(&self.parent, self.name.as_os_str(), flags).map_err(|e| self.io(e))
    }

    /// Creates a symbolic link at the name, holding `target` verbatim.
    pub fn symlink_to(&self, target: &Path) -> Result<(), ProviderError> {
        rustix::fs::symlinkat(target, &self.parent, self.name.as_os_str()).map_err(|e| self.io(e))
    }

    /// Reads the link stored at the name.
    pub fn read_link(&self) -> Result<PathBuf, ProviderError> {
        use std::os::unix::ffi::OsStringExt;
        let target = rustix::fs::readlinkat(&self.parent, self.name.as_os_str(), Vec::new())
            .map_err(|e| self.io(e))?;
        Ok(PathBuf::from(OsString::from_vec(target.into_bytes())))
    }

    /// Renames into `destination` — **both ends pinned**, so neither parent can
    /// be swapped out from under the move.
    pub fn rename_to(&self, destination: &Anchored) -> Result<(), ProviderError> {
        rustix::fs::renameat(
            &self.parent,
            self.name.as_os_str(),
            &destination.parent,
            destination.name.as_os_str(),
        )
        .map_err(|e| self.io(e))
    }

    /// Sets the length of the file at the name.
    pub fn truncate(&self, len: u64) -> Result<(), ProviderError> {
        let file = self.open_write()?;
        rustix::fs::ftruncate(&file, len).map_err(|e| self.io(e))
    }

    /// Sets the permission bits of the file at the name.
    ///
    /// Through a descriptor rather than `fchmodat`, because Linux's
    /// `AT_SYMLINK_NOFOLLOW` is unimplemented there — so the portable way to
    /// say "this file, not whatever a link at this name points at" is to open
    /// it `NOFOLLOW` and set the mode on what came back.
    pub fn chmod(&self, mode: u32) -> Result<(), ProviderError> {
        use rustix::fs::{Mode, OFlags, openat};
        let fd = openat(
            &self.parent,
            self.name.as_os_str(),
            OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| self.io(e))?;
        // `mode` is a `u32`, as `PermissionsExt::mode` gives it — but `Mode` is
        // built from `RawMode`, which is `mode_t`: `u32` on Linux and `u16` on
        // macOS and the BSDs. `Mode::from` therefore only compiles on the
        // platforms where those agree. Casting to `RawMode` names the target's
        // width, and the permission bits this carries fit in twelve either way.
        let bits = mode as rustix::fs::RawMode;
        rustix::fs::fchmod(&fd, Mode::from_raw_mode(bits)).map_err(|e| self.io(e))
    }

    /// Whether the name is a directory, without following a link at it.
    pub fn is_dir(&self) -> Result<bool, ProviderError> {
        let stat = rustix::fs::statat(
            &self.parent,
            self.name.as_os_str(),
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .map_err(|e| self.io(e))?;
        Ok(rustix::fs::FileType::from_raw_mode(stat.st_mode) == rustix::fs::FileType::Directory)
    }

    /// Removes the name and everything under it, **descending by descriptor**.
    ///
    /// `remove_dir_all` walks by path, which is a fresh resolution at every
    /// level — so a directory swapped for a link mid-delete redirects the
    /// deletion, and a deletion is the one operation you cannot take back. Here
    /// each subdirectory is opened from its parent's descriptor with
    /// `NOFOLLOW`, and each entry is unlinked relative to the descriptor it was
    /// listed from. Nothing outside the tree that was opened can be reached,
    /// whatever happens to the names while it runs.
    pub fn remove_tree(&self) -> Result<(), ProviderError> {
        use rustix::fs::{Mode, OFlags, openat};
        let dir = openat(
            &self.parent,
            self.name.as_os_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| self.io(e))?;
        self.empty(&dir, 0)?;
        self.unlink(true)
    }

    /// One level of [`remove_tree`](Self::remove_tree): empties `dir`, which is
    /// already open, and leaves the directory itself for the caller to unlink.
    ///
    /// `depth` is a guard rather than a limit anyone should reach. The walk
    /// recurses, and a tree deep enough to exhaust the stack would otherwise
    /// abort the process — which is a worse answer than refusing.
    fn empty(&self, dir: &std::os::fd::OwnedFd, depth: u32) -> Result<(), ProviderError> {
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, openat};

        if depth > 256 {
            return Err(failed(
                &self.path,
                std::io::Error::other("directory tree is too deeply nested to remove"),
            ));
        }
        let listing = rustix::fs::Dir::read_from(dir).map_err(|e| self.io(e))?;
        for entry in listing {
            let entry = entry.map_err(|e| self.io(e))?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            // `d_type` is not filled in on every filesystem, and a wrong guess
            // here would mean unlinking a directory as a file (or the reverse),
            // so `Unknown` asks rather than assumes.
            let kind = match entry.file_type() {
                FileType::Unknown => {
                    let stat = rustix::fs::statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                        .map_err(|e| self.io(e))?;
                    FileType::from_raw_mode(stat.st_mode)
                }
                known => known,
            };
            if kind == FileType::Directory {
                let child = openat(
                    dir,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )
                .map_err(|e| self.io(e))?;
                self.empty(&child, depth + 1)?;
                drop(child);
                rustix::fs::unlinkat(dir, name, AtFlags::REMOVEDIR).map_err(|e| self.io(e))?;
            } else {
                // A symlink is unlinked, never followed: `unlinkat` removes the
                // name, and the name is the link.
                rustix::fs::unlinkat(dir, name, AtFlags::empty()).map_err(|e| self.io(e))?;
            }
        }
        Ok(())
    }

    fn io(&self, err: rustix::io::Errno) -> ProviderError {
        match err {
            // A link where a file was expected, at the last component. The walk
            // catches the same thing one level up; this catches it here.
            rustix::io::Errno::LOOP | rustix::io::Errno::MLINK => swapped(&self.path),
            other => failed(&self.path, other.into()),
        }
    }
}

/// Windows: the same surface, by path.
///
/// D83 says Windows "keeps the old behaviour", and this is it — resolve, then
/// act on the name. Every method here reopens `parent/name` from the root, so
/// the window the Unix arm closes stays open: a directory swapped between the
/// check and the call redirects the operation, exactly as it did before the
/// jail learned to hold descriptors. That is not an oversight to fix here.
/// Windows has no `*at` family, and the equivalent — opening each component
/// with `FILE_FLAG_OPEN_REPARSE_POINT` and reopening children by handle — is a
/// different piece of work against a different API. It is written down in one
/// place so the weaker guarantee is a decision on the record rather than
/// something inferred from a missing function.
#[cfg(not(unix))]
impl Anchored {
    /// The name to act on. `parent` and `name` rejoin to the path `anchor`
    /// split, and nothing here resolves further than that.
    fn target(&self) -> PathBuf {
        self.parent.join(&self.name)
    }

    /// Opens the final name for writing, creating it if it is not there.
    pub fn create(&self, append: bool) -> Result<std::fs::File, ProviderError> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true);
        if append {
            options.append(true);
        } else {
            options.truncate(true);
        }
        options.open(self.target()).map_err(|e| self.io(e))
    }

    /// Opens the final name for reading.
    pub fn open(&self) -> Result<std::fs::File, ProviderError> {
        std::fs::File::open(self.target()).map_err(|e| self.io(e))
    }

    /// Opens it for writing **without** creating or truncating — what
    /// `truncate()` needs a handle for.
    fn open_write(&self) -> Result<std::fs::File, ProviderError> {
        std::fs::OpenOptions::new()
            .write(true)
            .open(self.target())
            .map_err(|e| self.io(e))
    }

    pub fn mkdir(&self) -> Result<(), ProviderError> {
        std::fs::create_dir(self.target()).map_err(|e| self.io(e))
    }

    /// Removes the name. `directory` picks between the two calls, which is a
    /// distinction the caller has already had to make.
    pub fn unlink(&self, directory: bool) -> Result<(), ProviderError> {
        let target = self.target();
        if directory {
            std::fs::remove_dir(target)
        } else {
            std::fs::remove_file(target)
        }
        .map_err(|e| self.io(e))
    }

    /// Reads the link stored at the name.
    pub fn read_link(&self) -> Result<PathBuf, ProviderError> {
        std::fs::read_link(self.target()).map_err(|e| self.io(e))
    }

    /// Renames into `destination`.
    pub fn rename_to(&self, destination: &Anchored) -> Result<(), ProviderError> {
        std::fs::rename(self.target(), destination.target()).map_err(|e| self.io(e))
    }

    /// Sets the length of the file at the name.
    pub fn truncate(&self, len: u64) -> Result<(), ProviderError> {
        let file = self.open_write()?;
        file.set_len(len).map_err(|e| self.io(e))
    }

    /// Whether the name is a directory, without following a link at it.
    pub fn is_dir(&self) -> Result<bool, ProviderError> {
        let meta = std::fs::symlink_metadata(self.target()).map_err(|e| self.io(e))?;
        Ok(meta.is_dir())
    }

    /// Removes the name and everything under it.
    ///
    /// By path, where the Unix arm descends by descriptor: `remove_dir_all`
    /// resolves afresh at every level. It does refuse to recurse through a
    /// symlink — it unlinks the link — so the tree it walks stays the tree it
    /// opened, but a directory *replaced* mid-walk is followed.
    pub fn remove_tree(&self) -> Result<(), ProviderError> {
        std::fs::remove_dir_all(self.target()).map_err(|e| self.io(e))
    }

    fn io(&self, err: std::io::Error) -> ProviderError {
        failed(&self.path, err)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// A jail root, an "outside" directory to be redirected into, and the
    /// canonical path of a file inside the jail — the state every one of these
    /// starts from.
    fn scene(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("esrun-anchor-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("project");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let root = std::fs::canonicalize(&root).unwrap();
        let outside = std::fs::canonicalize(&outside).unwrap();
        (base, root, outside)
    }

    /// Replaces `data` with a symlink to somewhere outside the jail, keeping the
    /// real directory alongside — the swap, performed at the exact moment an
    /// attacker would want it.
    fn swap(root: &Path, outside: &Path) {
        std::fs::rename(root.join("data"), root.join("data.real")).unwrap();
        std::os::unix::fs::symlink(outside, root.join("data")).unwrap();
    }

    /// **The hole, demonstrated.** Resolving to a path and then writing to that
    /// path is two lookups of the same name, and the name can change in
    /// between. This is what every jailed write used to do.
    #[test]
    fn a_path_checked_and_then_used_writes_outside_the_jail() {
        let (base, root, outside) = scene("hole");
        let real = root.join("data/notes.txt");

        // The check: `data` is a real directory inside the jail. Whatever a
        // caller concludes here, it concludes about *this* directory.
        assert!(
            std::fs::canonicalize(root.join("data"))
                .unwrap()
                .starts_with(&root)
        );

        swap(&root, &outside);

        // The use: the kernel resolves the name a second time, and `data` is
        // now somewhere else.
        std::fs::write(&real, b"payload").unwrap();
        assert!(
            outside.join("notes.txt").exists(),
            "the swap has to actually work, or the test below proves nothing"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The same swap, at the same moment, against a pinned parent: the bytes
    /// land in the directory that was checked, because a descriptor names an
    /// inode and there is no second lookup for the swap to catch.
    #[test]
    fn a_pinned_parent_writes_where_the_check_agreed() {
        let (base, root, outside) = scene("pinned");
        let real = root.join("data/notes.txt");
        let roots = vec![root.clone()];

        let anchored = anchor(&real, &roots).expect("the path is inside the jail");

        swap(&root, &outside);

        use std::io::Write;
        let mut file = anchored
            .create(false)
            .expect("write into the pinned directory");
        file.write_all(b"payload").unwrap();
        drop(file);

        assert!(
            !outside.join("notes.txt").exists(),
            "the write followed the swapped name out of the jail"
        );
        assert_eq!(
            std::fs::read(root.join("data.real/notes.txt")).unwrap(),
            b"payload",
            "the write must land in the directory the check agreed to"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A swap that happens *before* the walk is caught by the walk: every
    /// component below the root is opened `NOFOLLOW`, and a canonical path has
    /// no symlinks in it — so one that does has been tampered with.
    #[test]
    fn a_component_that_became_a_link_is_refused_rather_than_followed() {
        let (base, root, outside) = scene("walk");
        let real = root.join("data/notes.txt");
        let roots = vec![root.clone()];

        swap(&root, &outside);

        let refused = anchor(&real, &roots).expect_err("a swapped component must be refused");
        assert_eq!(refused.code(), Some(ErrorCode::JailEscape), "{refused}");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// And a link dropped at the **final** name is refused too. The walk cannot
    /// see that one — it never opens the last component — so the open does.
    #[test]
    fn a_link_at_the_last_component_is_refused() {
        let (base, root, outside) = scene("final");
        let real = root.join("data/notes.txt");
        let roots = vec![root.clone()];

        let anchored = anchor(&real, &roots).expect("inside the jail");
        std::os::unix::fs::symlink(outside.join("notes.txt"), root.join("data/notes.txt")).unwrap();

        let refused = anchored
            .create(false)
            .expect_err("a link at the name must be refused");
        assert_eq!(refused.code(), Some(ErrorCode::JailEscape), "{refused}");
        assert!(!outside.join("notes.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The ordinary case still works, which is worth a line of its own: a
    /// security fix that broke the feature would be a worse outcome.
    #[test]
    fn the_ordinary_path_still_reads_and_writes() {
        let (base, root, _outside) = scene("ordinary");
        let roots = vec![root.clone()];
        std::fs::write(root.join("data/there.txt"), b"hello").unwrap();

        let anchored = anchor(&root.join("data/there.txt"), &roots).unwrap();
        use std::io::Read;
        let mut text = String::new();
        anchored.open().unwrap().read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello");
        assert!(!anchored.is_dir().unwrap());

        let dir = anchor(&root.join("data/made"), &roots).unwrap();
        dir.mkdir().unwrap();
        assert!(root.join("data/made").is_dir());
        assert!(dir.is_dir().unwrap());
        dir.unlink(true).unwrap();
        assert!(!root.join("data/made").exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}
