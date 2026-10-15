//! The one implementation of every path, `file:` URL and module-id conversion
//! (D25, D165). Nothing else in the workspace calls `canonicalize` or `Url`'s
//! file conversions — `clippy.toml` refuses them — so the same file always
//! has the same spelling: module ids, `import.meta.url`, the sandbox jail and
//! every lookup table agree on every platform.
//!
//! The platform-sensitive parts it pins down:
//! - **Windows verbatim prefix.** `std::fs::canonicalize` returns `\\?\C:\…`,
//!   which corrupts `file:` URL conversion and matches no plain root.
//!   [`canonicalize`] uses `dunce` to return a normal path (no-op on Unix).
//! - **Separators.** Specifier/target strings are always `/`-separated;
//!   [`join_relative`] splits and joins one segment at a time so no path
//!   component ever carries an embedded separator (robust on `\` platforms).
//!   [`to_slash`] and [`slash_id`] write the `/` form JavaScript is shown;
//!   [`native_id`] reads it back.
//! - **One round-trip.** [`file_url`]/[`file_path`] are the single path↔URL
//!   implementation, so the same file always yields the same id
//!   (module-instance identity + jail comparisons depend on this).

#![allow(clippy::disallowed_methods)] // the one place they are allowed (D165)

use std::borrow::Cow;
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

/// The plain spelling of a path another library made: Windows' verbatim
/// `\\?\C:\…` read as `C:\…`, which every API there accepts. Unchanged
/// anywhere else. For a path this crate makes, [`canonicalize`] already
/// returns it.
pub fn simplified(path: impl AsRef<Path>) -> PathBuf {
    dunce::simplified(path.as_ref()).to_path_buf()
}

/// [`canonicalize`] for async code.
pub async fn canonicalize_async(path: impl AsRef<Path>) -> std::io::Result<PathBuf> {
    let path = path.as_ref().to_path_buf();
    tokio::task::spawn_blocking(move || canonicalize(path))
        .await
        .map_err(std::io::Error::other)?
}

/// The `file:` URL of an absolute path, or `None` for a relative one. A
/// verbatim (`\\?\…`) path is read as the plain path it names, which `Url`
/// alone refuses.
pub fn file_url(path: impl AsRef<Path>) -> Option<Url> {
    Url::from_file_path(dunce::simplified(path.as_ref())).ok()
}

/// [`file_url`] for a directory: the URL ends in `/`, so a relative
/// specifier joined onto it lands inside the directory, not beside it.
pub fn directory_url(path: impl AsRef<Path>) -> Option<Url> {
    Url::from_directory_path(dunce::simplified(path.as_ref())).ok()
}

/// The path a `file:` URL names, percent-decoded, with the drive or UNC host
/// Windows needs. `None` for any other scheme.
pub fn file_path(url: &Url) -> Option<PathBuf> {
    url.to_file_path().ok()
}

/// The file a module specifier names: a `file:` URL's path, its query and
/// hash dropped. `None` for any other scheme and for a bare or relative
/// specifier, which name no file until something resolves them.
pub fn specifier_path(specifier: &str) -> Option<PathBuf> {
    file_path(&Url::parse(specifier).ok()?)
}

/// Converts an absolute path to a `file:` URL string.
pub fn to_file_url(path: &Path) -> Result<String, ProviderError> {
    file_url(path)
        .map(String::from)
        .ok_or_else(|| ProviderError::Other(format!("path is not absolute: {}", path.display())))
}

/// Parses a `file:` URL string back to a path.
pub fn from_file_url(url: &str) -> Result<PathBuf, ProviderError> {
    let parsed = Url::parse(url)
        .map_err(|e| ProviderError::Other(format!("invalid module id {url:?}: {e}")))?;
    file_path(&parsed)
        .ok_or_else(|| ProviderError::Other(format!("module id is not a file path: {url}")))
}

/// The forward-slash form of `path`, as a module id is written for JavaScript
/// (D165): `C:/proj/a.js` on Windows, the path itself elsewhere. Only Windows
/// has a second separator; on POSIX a `\` is part of a file name and is kept.
pub fn to_slash(path: impl AsRef<Path>) -> String {
    slash_on(&path.as_ref().to_string_lossy(), cfg!(windows))
}

/// A module id in the form JavaScript is shown (D165): an absolute path id
/// becomes [`to_slash`]'s form; a virtual (`\0…`) or relative id is left
/// alone, because its backslashes are not separators.
pub fn slash_id(id: &str) -> Cow<'_, str> {
    slash_id_on(id, cfg!(windows))
}

/// An id JavaScript handed back, in the native form the bundler keys modules
/// by: [`slash_id`] undone. Either form is accepted, so a plugin may return
/// what it was given or a path it built itself.
pub fn native_id(id: &str) -> Cow<'_, str> {
    native_on(id, cfg!(windows))
}

fn slash_on(text: &str, windows: bool) -> String {
    if windows {
        text.replace('\\', "/")
    } else {
        text.to_owned()
    }
}

fn slash_id_on(id: &str, windows: bool) -> Cow<'_, str> {
    if !windows || !is_windows_absolute(id) {
        return Cow::Borrowed(id);
    }
    swap_in_path(id, '\\', "/")
}

fn native_on(id: &str, windows: bool) -> Cow<'_, str> {
    if !windows || !is_windows_absolute(id) {
        return Cow::Borrowed(id);
    }
    swap_in_path(id, '/', "\\")
}

/// `from` replaced in the path part of an id only: a query or hash is not
/// part of the path, and may carry separators of its own (`?url=a/b`).
fn swap_in_path<'a>(id: &'a str, from: char, to: &str) -> Cow<'a, str> {
    let end = id.find(['?', '#']).unwrap_or(id.len());
    let (path, rest) = id.split_at(end);
    if !path.contains(from) {
        return Cow::Borrowed(id);
    }
    Cow::Owned(format!("{}{rest}", path.replace(from, to)))
}

/// `C:\…`, `C:/…`, `\\host\share` or `//host/share`: a path Windows reads as
/// absolute, in either separator.
fn is_windows_absolute(id: &str) -> bool {
    let bytes = id.as_bytes();
    let sep = |b: u8| b == b'\\' || b == b'/';
    match bytes {
        [drive, b':', s, ..] if drive.is_ascii_alphabetic() && sep(*s) => true,
        [a, b, c, ..] if sep(*a) && sep(*b) && !sep(*c) => true,
        _ => false,
    }
}

/// Joins a `/`-separated relative path onto `base`, one segment at a time, so no
/// component carries an embedded separator. Leading/empty `.` segments are
/// skipped; `..` pops. (`base` is a real path; `relative` is `/`-separated: a
/// `package.json` `exports`/`main` target, or a plugin's `dependsOn` entry.)
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

/// Whether `dir` is a `node_modules` directory or lies inside one.
///
/// Such a directory is an *installed dependency*, never a project root: its
/// `package.json` describes a package somebody else wrote, and the packages it
/// imports are hoisted to the tree that installed it. Treating it as a root is
/// what made a hoisted dependency unreachable (D79).
fn is_installed_dependency(dir: &Path) -> bool {
    dir.components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new("node_modules"))
}

/// The sandbox root for `dir`: the nearest ancestor (including `dir`) containing
/// `node_modules` or `package.json` and not itself installed under a
/// `node_modules`, else `dir` — canonicalized so it can be compared against
/// canonicalized resolved paths (D25).
///
/// Skipping the installed ones is what makes `esrun node_modules/.bin/…`-shaped
/// entries work: an installed CLI's own `package.json` is not a project root, so
/// detection continues past it to the tree that installed the package — where
/// its hoisted dependencies actually live (D79). The walk is still bounded: the
/// parent of the outermost `node_modules` always contains one, so it matches.
pub fn detect_root(dir: &Path) -> PathBuf {
    let start = canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    for ancestor in start.ancestors() {
        if is_installed_dependency(ancestor) {
            continue;
        }
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
    fn a_windows_path_takes_forward_slashes_and_a_posix_one_keeps_its_name() {
        assert_eq!(slash_on(r"C:\proj\src\a.jsx", true), "C:/proj/src/a.jsx");
        assert_eq!(slash_on(r"\\host\share\a.js", true), "//host/share/a.js");
        // On POSIX a backslash is a character of the file name.
        assert_eq!(slash_on(r"/proj/a\b.js", false), r"/proj/a\b.js");
    }

    #[test]
    fn only_a_path_id_is_shown_with_forward_slashes() {
        assert_eq!(slash_id_on(r"C:\proj\a.jsx?raw", true), "C:/proj/a.jsx?raw");
        assert_eq!(
            slash_id_on(r"C:\proj\a.svg?x=a\b", true),
            r"C:/proj/a.svg?x=a\b"
        );
        assert_eq!(slash_id_on("C:/proj/a.jsx", true), "C:/proj/a.jsx");
        // A virtual id's backslashes are its own, not separators.
        assert_eq!(slash_id_on("\0plugin\\entry", true), "\0plugin\\entry");
        assert_eq!(slash_id_on(r"C:\proj\a.jsx", false), r"C:\proj\a.jsx");
    }

    #[test]
    fn a_slash_id_comes_back_native_with_its_query_untouched() {
        assert_eq!(native_on("C:/proj/a.jsx", true), r"C:\proj\a.jsx");
        assert_eq!(native_on(r"C:\proj\a.jsx", true), r"C:\proj\a.jsx");
        assert_eq!(native_on("//host/share/a.js", true), r"\\host\share\a.js");
        assert_eq!(
            native_on("C:/proj/icon.svg?url=a/b#x/y", true),
            r"C:\proj\icon.svg?url=a/b#x/y"
        );
        // Not a path: a virtual id, a bare or relative specifier.
        assert_eq!(native_on("\0virtual/entry", true), "\0virtual/entry");
        assert_eq!(native_on("react/jsx-runtime", true), "react/jsx-runtime");
        assert_eq!(native_on("./a/b.js", true), "./a/b.js");
        // Nothing to do on POSIX.
        assert_eq!(native_on("/proj/a.jsx", false), "/proj/a.jsx");
    }

    #[test]
    fn only_an_absolute_path_reads_as_one() {
        assert!(is_windows_absolute(r"C:\a"));
        assert!(is_windows_absolute("c:/a"));
        assert!(is_windows_absolute(r"\\host\share"));
        assert!(is_windows_absolute("//host/share"));
        // Drive-relative, rooted, verbatim-less relative and virtual ids are not.
        assert!(!is_windows_absolute("C:a"));
        assert!(!is_windows_absolute(r"\a"));
        assert!(!is_windows_absolute("///a"));
        assert!(!is_windows_absolute("\0x"));
        assert!(!is_windows_absolute("a/b"));
    }

    #[test]
    fn a_specifier_names_its_file_decoded_and_without_its_query() {
        let file = std::env::temp_dir().join("a b.jsx");
        let specifier = format!("{}?raw#top", file_url(&file).unwrap());
        assert!(specifier.contains("a%20b.jsx"), "{specifier}");
        assert_eq!(specifier_path(&specifier), Some(file));
        assert_eq!(specifier_path("https://example.com/a.js"), None);
        assert_eq!(specifier_path("react"), None);
        assert_eq!(specifier_path("./a.js"), None);
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

    /// An installed package's own `package.json` is not a project root (D79):
    /// running a file inside `node_modules` must anchor at the tree that
    /// installed it, or every hoisted dependency is out of reach.
    #[test]
    fn detect_root_walks_past_an_installed_package() {
        let base = std::env::temp_dir().join(format!("esrt-root-nm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let proj = base.join("proj");
        let cli = proj.join("node_modules/@acme/cli");
        std::fs::create_dir_all(cli.join("src")).unwrap();
        std::fs::create_dir_all(proj.join("node_modules/leftpad")).unwrap();
        std::fs::write(proj.join("package.json"), "{}").unwrap();
        // The dependency has a manifest of its own — the marker that used to
        // stop the walk one directory too deep.
        std::fs::write(cli.join("package.json"), "{}").unwrap();

        let root = canonicalize(&proj).unwrap();
        assert_eq!(detect_root(&cli.join("src")), root);
        assert_eq!(detect_root(&cli), root);
        // Nested (unhoisted) installs anchor at the same place.
        let nested = cli.join("node_modules/inner");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("package.json"), "{}").unwrap();
        assert_eq!(detect_root(&nested), root);

        std::fs::remove_dir_all(&base).ok();
    }
}
