//! A filesystem path allowlist — the policy behind `esrun --allow-read=<paths>`
//! and `--allow-write=<paths>` (DECISIONS D38).
//!
//! **The check runs after canonicalization, never before.** That is the whole
//! difficulty of scoping a filesystem: a path is a *name* for a file, and a
//! symlink lets one name stand for a file somewhere else entirely. Checking the
//! name the guest wrote would mean `--allow-read=./data` admits
//! `./data/link-to-etc/passwd` — the same class of hole the root jail (D25)
//! exists to close, reopened one level in. So the entries are canonicalized
//! once, at construction, and matched against the real path the jail already
//! resolved.
//!
//! An entry covers its **subtree**: `--allow-read=/srv/app` admits
//! `/srv/app/config.json`. Matching is by path component, so `/srv/app` never
//! admits `/srv/app-secrets`.

use std::path::{Path, PathBuf};

use es_runtime_common::ErrorCode;
use es_runtime_providers::ProviderError;

use crate::path;

/// Which half of the filesystem grant an operation needs. The jail resolves
/// every path through one choke point, so the choke point has to be told which
/// list to consult: `read` and `write` are separate capabilities and separate
/// lists, and an operation that does both (`rename`, `copy`) checks both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
}

impl Access {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

/// Paths a filesystem capability may be used on. An empty list permits nothing.
#[derive(Clone, Debug, Default)]
pub struct PathAllowlist {
    /// Canonicalized entries. A path that does not exist yet keeps its literal
    /// tail attached to the canonicalized part that does — the same shape the
    /// jail produces for a file about to be created, so the two compare.
    entries: Vec<PathBuf>,
}

impl PathAllowlist {
    /// Parses `entries` — paths, absolute or relative to `base` (the directory
    /// the user typed them in, i.e. the CLI's working directory).
    ///
    /// A relative entry is resolved against `base`, and each is canonicalized so
    /// that the list holds real paths: an entry written through a symlink names
    /// the same subtree it points at, which is the only reading under which the
    /// list can be checked after resolution.
    pub fn parse<I, S>(entries: I, base: &Path) -> Result<PathAllowlist, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut parsed = Vec::new();
        for entry in entries {
            let entry = entry.as_ref();
            if entry.is_empty() {
                return Err("an empty path is not a location".to_string());
            }
            let raw = Path::new(entry);
            let abs = if raw.is_absolute() {
                raw.to_path_buf()
            } else {
                base.join(raw)
            };
            parsed.push(canonicalize_existing_prefix(&abs));
        }
        Ok(PathAllowlist { entries: parsed })
    }

    /// Whether `real` — an already-resolved path from the jail — is inside one
    /// of the entries. An entry covers itself and everything under it.
    pub(crate) fn permits(&self, real: &Path) -> bool {
        self.entries.iter().any(|entry| real.starts_with(entry))
    }

    /// [`permits`](Self::permits), as a provider error naming the path.
    ///
    /// [`ErrorCode::PermissionDenied`] — a **scoped** denial ("you have `read`,
    /// but not that path"), distinct both from the `ERR_CAPABILITY_DENIED` of a
    /// missing capability and from the `ERR_JAIL_ESCAPE` of a path outside the
    /// project root, which is a different refusal with a different fix.
    pub(crate) fn check(&self, real: &Path, access: Access) -> Result<(), ProviderError> {
        if self.permits(real) {
            return Ok(());
        }
        Err(ProviderError::Coded {
            code: ErrorCode::PermissionDenied,
            message: format!(
                "{} is not an allowed path ({})",
                real.display(),
                access.as_str()
            ),
        })
    }
}

/// Canonicalizes as much of `abs` as exists and reattaches the rest literally.
///
/// An allowlist entry naming a file that has not been created yet is ordinary
/// (`--allow-write=./out/report.json` before the first run), and it must still
/// match the path the jail hands over for that file — which is built the same
/// way, for the same reason.
fn canonicalize_existing_prefix(abs: &Path) -> PathBuf {
    let mut existing = abs.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = path::canonicalize(&existing) {
            let mut out = real;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return out;
        }
        match existing.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                match existing.parent() {
                    Some(parent) => existing = parent.to_path_buf(),
                    // Nothing of the path exists (and there is no parent left):
                    // keep it literal. It cannot match a resolved path, which
                    // is the right outcome — the entry names nothing.
                    None => return abs.to_path_buf(),
                }
            }
            None => return abs.to_path_buf(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("esrt-pathallow-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        path::canonicalize(&dir).unwrap()
    }

    #[test]
    fn an_entry_covers_its_subtree() {
        let root = temp_dir("subtree");
        std::fs::create_dir_all(root.join("data/nested")).unwrap();
        let allow = PathAllowlist::parse(["data"], &root).unwrap();
        assert!(allow.permits(&root.join("data")));
        assert!(allow.permits(&root.join("data/nested/file.txt")));
        assert!(!allow.permits(&root.join("secrets.txt")));
    }

    #[test]
    fn matching_is_by_component_not_by_prefix_string() {
        let root = temp_dir("components");
        std::fs::create_dir_all(root.join("app")).unwrap();
        std::fs::create_dir_all(root.join("app-secrets")).unwrap();
        let allow = PathAllowlist::parse(["app"], &root).unwrap();
        assert!(!allow.permits(&root.join("app-secrets/key.pem")));
    }

    #[test]
    fn an_entry_is_canonicalized_so_a_symlinked_name_covers_the_real_subtree() {
        // The reason the check runs after resolution: the jail hands over real
        // paths, so the list has to hold real paths too.
        let root = temp_dir("symlink-entry");
        std::fs::create_dir_all(root.join("real")).unwrap();
        let link = root.join("link");
        let _ = std::fs::remove_file(&link);
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("real"), &link).unwrap();
        #[cfg(unix)]
        {
            let allow = PathAllowlist::parse(["link"], &root).unwrap();
            assert!(allow.permits(&root.join("real/file.txt")));
        }
    }

    #[test]
    fn a_path_that_does_not_exist_yet_still_matches() {
        // `--allow-write=./out/report.json` before the first run.
        let root = temp_dir("not-yet");
        let allow = PathAllowlist::parse(["out/report.json"], &root).unwrap();
        assert!(allow.permits(&root.join("out/report.json")));
        assert!(!allow.permits(&root.join("out/other.json")));
    }

    #[test]
    fn an_empty_list_permits_nothing() {
        assert!(!PathAllowlist::default().permits(Path::new("/anything")));
    }

    #[test]
    fn check_names_the_path_and_the_access() {
        let root = temp_dir("message");
        let allow = PathAllowlist::parse(["data"], &root).unwrap();
        let err = allow
            .check(&root.join("secrets.txt"), Access::Read)
            .unwrap_err();
        let message = err.to_string();
        assert!(message.contains("secrets.txt"), "{message}");
        assert!(message.contains("read"), "{message}");
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
    }
}
