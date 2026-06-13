//! Cross-OS path ↔ `file:` URL resolution, shared by the module loaders (and
//! reused by the future `runtime:fs` provider). Centralizing it keeps module
//! ids, `import.meta.url`, and the sandbox jail byte-identical across platforms.
//!
//! The platform-sensitive parts it pins down:
//! - **Windows verbatim prefix.** `std::fs::canonicalize` returns `\\?\C:\…`,
//!   which corrupts `file:` URL conversion. [`canonicalize`] uses `dunce` to
//!   return a normal path (no-op on Unix), so URLs/keys stay clean (D25).
//! - **Separators.** Specifier/target strings are always `/`-separated;
//!   [`join_relative`] splits and joins one segment at a time so no path
//!   component ever carries an embedded separator (robust on `\` platforms).
//! - **One round-trip.** [`to_file_url`]/[`from_file_url`] are the single
//!   path↔URL implementation, so the same file always yields the same id
//!   (module-instance identity + jail comparisons depend on this).

use std::path::{Path, PathBuf};

use es_runtime_providers::ProviderError;
use url::Url;

/// Canonicalizes to a real, absolute, platform-normal path: resolves symlinks
/// and `.`/`..`, and on Windows strips the `\\?\` verbatim prefix (via `dunce`)
/// that would otherwise break `file:` URL conversion. No-op normalization on
/// Unix. The path must exist.
pub fn canonicalize(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// [`canonicalize`] mapping the IO error to a [`ProviderError`] naming the path.
pub fn canonicalize_checked(path: impl AsRef<Path>) -> Result<PathBuf, ProviderError> {
    let path = path.as_ref();
    canonicalize(path)
        .map_err(|e| ProviderError::Other(format!("cannot resolve path {}: {e}", path.display())))
}

/// Converts an absolute path to a `file:` URL string.
pub fn to_file_url(path: &Path) -> Result<String, ProviderError> {
    Url::from_file_path(path)
        .map(String::from)
        .map_err(|()| ProviderError::Other(format!("path is not absolute: {}", path.display())))
}

/// Parses a `file:` URL string back to a path.
pub fn from_file_url(url: &str) -> Result<PathBuf, ProviderError> {
    Url::parse(url)
        .map_err(|e| ProviderError::Other(format!("invalid module id {url:?}: {e}")))?
        .to_file_path()
        .map_err(|()| ProviderError::Other(format!("module id is not a file path: {url}")))
}

/// Joins a `/`-separated relative path onto `base`, one segment at a time, so no
/// component carries an embedded separator. Leading/empty `.` segments are
/// skipped; `..` pops. (`base` is a real path; `relative` comes from
/// `package.json` `exports`/`main`, always `/`-separated.)
pub fn join_relative(base: &Path, relative: &str) -> PathBuf {
    let mut out = base.to_path_buf();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            seg => out.push(seg),
        }
    }
    out
}

/// Whether `real` is `root` or under it. Both must come from [`canonicalize`]
/// so the comparison is on one normal form (case/separators/prefix consistent).
pub fn within_root(real: &Path, root: &Path) -> bool {
    real.starts_with(root)
}

/// The sandbox root for `dir`: the nearest ancestor (including `dir`) containing
/// `node_modules` or `package.json`, else `dir` — canonicalized so it can be
/// compared against canonicalized resolved paths (D25).
pub fn detect_root(dir: &Path) -> PathBuf {
    let start = canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for ancestor in start.ancestors() {
        if ancestor.join("node_modules").is_dir() || ancestor.join("package.json").is_file() {
            return ancestor.to_path_buf();
        }
    }
    start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_relative_is_segment_wise() {
        let base = Path::new("/pkg");
        assert_eq!(
            join_relative(base, "./fns/double.mjs"),
            Path::new("/pkg/fns/double.mjs")
        );
        assert_eq!(join_relative(base, "a/b/c"), Path::new("/pkg/a/b/c"));
        // No component carries an embedded separator: each split segment is one.
        assert_eq!(
            join_relative(base, "@scope/pkg").components().count(),
            Path::new("/pkg").components().count() + 2
        );
        // `..` pops, `.`/empty skip.
        assert_eq!(join_relative(base, "x/../y"), Path::new("/pkg/y"));
        assert_eq!(join_relative(base, "./a//b"), Path::new("/pkg/a/b"));
    }

    #[test]
    fn within_root_checks_containment() {
        assert!(within_root(Path::new("/proj/a/b.mjs"), Path::new("/proj")));
        assert!(within_root(Path::new("/proj"), Path::new("/proj")));
        assert!(!within_root(Path::new("/other/x.mjs"), Path::new("/proj")));
        // Prefix-of-component, not prefix-of-string: /projX is not under /proj.
        assert!(!within_root(Path::new("/projX/x.mjs"), Path::new("/proj")));
    }

    #[test]
    fn file_url_round_trips() {
        let dir = std::env::temp_dir().join(format!("esrt-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("m.mjs");
        std::fs::write(&file, "export const v = 1;").unwrap();

        let real = canonicalize(&file).unwrap();
        let url = to_file_url(&real).unwrap();
        assert!(url.starts_with("file://"), "{url}");
        // No Windows verbatim prefix leaks into the URL.
        assert!(!url.contains("?"), "verbatim prefix leaked: {url}");
        assert_eq!(from_file_url(&url).unwrap(), real);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detect_root_finds_marker_ancestor() {
        let base = std::env::temp_dir().join(format!("esrt-root-{}", std::process::id()));
        let proj = base.join("proj");
        let deep = proj.join("src/sub");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(proj.join("node_modules")).unwrap();

        assert_eq!(detect_root(&deep), canonicalize(&proj).unwrap());
        std::fs::remove_dir_all(&base).ok();
    }
}
