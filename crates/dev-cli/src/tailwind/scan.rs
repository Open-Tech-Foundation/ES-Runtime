//! Which class names a project uses: the files Tailwind would read, and the
//! candidates in them.
//!
//! Tailwind's own scanner (`@tailwindcss/oxide`) is a native addon, which this
//! runtime does not load, so this is that half written here. It has two jobs,
//! and each follows what Tailwind documents rather than inventing a policy:
//!
//! * **Which files.** Automatic detection walks the project and skips what
//!   `.gitignore` ignores, `node_modules`, stylesheets, lock files and binary
//!   files. `@source "…"` adds a file, a directory or a glob — scanned even
//!   where `.gitignore` would have skipped it, which is what it is for.
//!   `@source not "…"` takes paths away from both.
//! * **Which strings.** Every run of text that *could* be a class name. The
//!   compiler decides which ones are: a candidate that is not a utility
//!   generates nothing, so over-collecting costs a lookup and under-collecting
//!   costs a missing style. The split is therefore generous — the whole
//!   whitespace-separated token, and the pieces of it between quotes and
//!   punctuation — and it never splits inside `[…]`, where an arbitrary value
//!   may hold a quote, a comma or a `>`.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// One `@source` the compiler reported: where it was written, and what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub base: PathBuf,
    pub pattern: String,
    pub negated: bool,
}

/// What a scan found.
#[derive(Debug, Default)]
pub struct Scan {
    pub candidates: BTreeSet<String>,
    /// How many files were read, for a test to count.
    pub files: usize,
}

/// A file larger than this is not source anybody writes class names into — a
/// bundle, a data dump — and reading it would cost more than it could find.
const MAX_FILE: u64 = 4 * 1024 * 1024;

/// The longest string worth offering the compiler. Arbitrary values make
/// candidates long, but not this long.
const MAX_CANDIDATE: usize = 512;

/// Scans `auto` (the project, unless `source(none)` said otherwise) and every
/// explicit source.
pub fn scan(auto: Option<&Path>, sources: &[Source]) -> Result<Scan, String> {
    let excluded = exclusions(sources)?;
    let mut files = BTreeSet::new();

    if let Some(root) = auto {
        let walk = ignore::WalkBuilder::new(root)
            // `.gitignore` is the project's statement of what is not source,
            // and it holds whether or not the directory is a git checkout yet.
            .require_git(false)
            .filter_entry(|entry| entry.file_name() != "node_modules")
            .build();
        for entry in walk.flatten() {
            if entry.file_type().is_some_and(|kind| kind.is_file())
                && !skipped_by_name(entry.path())
            {
                files.insert(entry.into_path());
            }
        }
    }

    for source in sources.iter().filter(|source| !source.negated) {
        let full = absolute(&source.base, &source.pattern);
        if full.is_file() {
            files.insert(full);
        } else if full.is_dir() {
            walk_all(&full, None, &mut files)?;
        } else if let Some(prefix) = glob_prefix(&full) {
            let matcher = glob(&slashed(&full))?;
            walk_all(&prefix, Some(&matcher), &mut files)?;
        }
        // A path naming nothing is not an error: `@source` is a statement of
        // where class names *may* be, and a directory that does not exist yet
        // holds none of them.
    }

    let mut scan = Scan::default();
    for file in files {
        if excluded
            .as_ref()
            .is_some_and(|set| set.is_match(slashed(&file)))
        {
            continue;
        }
        if let Some(text) = read(&file) {
            scan.files += 1;
            extract(&text, &mut scan.candidates);
        }
    }
    Ok(scan)
}

/// Every file under `dir`, with none of the standard filters: an explicit
/// source is scanned whatever `.gitignore` says.
fn walk_all(
    dir: &Path,
    matcher: Option<&globset::GlobMatcher>,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in ignore::WalkBuilder::new(dir)
        .standard_filters(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if matcher.is_some_and(|matcher| !matcher.is_match(slashed(&path))) {
            continue;
        }
        if !binary_by_name(&path) {
            files.insert(path);
        }
    }
    Ok(())
}

/// What `@source not` takes away, as one set.
fn exclusions(sources: &[Source]) -> Result<Option<globset::GlobSet>, String> {
    let mut builder = globset::GlobSetBuilder::new();
    let mut any = false;
    for source in sources.iter().filter(|source| source.negated) {
        let full = absolute(&source.base, &source.pattern);
        let pattern = if full.is_dir() {
            format!("{}/**", slashed(&full).trim_end_matches('/'))
        } else {
            slashed(&full)
        };
        builder.add(glob_of(&pattern)?);
        any = true;
    }
    if !any {
        return Ok(None);
    }
    builder
        .build()
        .map(Some)
        .map_err(|e| format!("@source not: {e}"))
}

fn glob(pattern: &str) -> Result<globset::GlobMatcher, String> {
    Ok(glob_of(pattern)?.compile_matcher())
}

fn glob_of(pattern: &str) -> Result<globset::Glob, String> {
    globset::GlobBuilder::new(pattern)
        // `*` stays within a directory and `**` crosses them, as Tailwind's
        // globs do.
        .literal_separator(true)
        .build()
        .map_err(|e| format!("@source \"{pattern}\" is not a glob: {e}"))
}

/// The directory a glob can match under: its path up to the first segment
/// with a wildcard in it. `None` when there is no wildcard at all.
fn glob_prefix(path: &Path) -> Option<PathBuf> {
    let mut prefix = PathBuf::new();
    let mut wild = false;
    for component in path.components() {
        let text = component.as_os_str().to_string_lossy();
        if text.contains(['*', '?', '[', '{']) {
            wild = true;
            break;
        }
        prefix.push(component);
    }
    wild.then_some(prefix)
}

/// `pattern` against `base`, with `.` and `..` folded away by name — the glob
/// matcher compares text, and `src/../app` is not text it will ever see.
fn absolute(base: &Path, pattern: &str) -> PathBuf {
    let joined = base.join(pattern);
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A path with `/` separators, which is what a glob is written in.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Files automatic detection never reads: stylesheets (a class a stylesheet
/// names is one it *defines*), lock files, and binaries.
fn skipped_by_name(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if matches!(
        name.as_str(),
        "package-lock.json"
            | "npm-shrinkwrap.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "deno.lock"
            | "cargo.lock"
    ) {
        return true;
    }
    matches!(
        extension(path).as_str(),
        "css" | "scss" | "sass" | "less" | "styl"
    ) || binary_by_name(path)
}

/// Files that are not text, by what they are called. A file this misses is
/// caught by [`read`], which looks.
fn binary_by_name(path: &Path) -> bool {
    matches!(
        extension(path).as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "avif"
            | "ico"
            | "bmp"
            | "tif"
            | "tiff"
            | "woff"
            | "woff2"
            | "ttf"
            | "otf"
            | "eot"
            | "mp3"
            | "mp4"
            | "m4a"
            | "ogg"
            | "wav"
            | "webm"
            | "mov"
            | "avi"
            | "pdf"
            | "zip"
            | "gz"
            | "tgz"
            | "br"
            | "zst"
            | "tar"
            | "7z"
            | "wasm"
            | "exe"
            | "dll"
            | "so"
            | "dylib"
            | "node"
            | "db"
            | "sqlite"
            | "map"
    )
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// A file's text, or `None` for one too large to be source or that is not
/// text at all — a NUL in its first few kilobytes, which is how a binary with
/// an unfamiliar extension shows itself.
fn read(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > MAX_FILE {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Every string in `text` that could be a class name.
///
/// Splits, all unioned into `out`. The coarse ones break on whitespace and
/// quotes, which leaves `bg-(--brand)` and `supports-(display:grid):grid`
/// whole — once never inside `[…]`, which keeps `content-['hi']` whole, and
/// once through it, because a `[` is as often a JavaScript array as an
/// arbitrary value: `["px-2", "py-1"]`. The fine one breaks each coarse token
/// again on the punctuation around a class name in source — `className={cn(`
/// — which recovers the name inside a call.
pub fn extract(text: &str, out: &mut BTreeSet<String>) {
    let quote = |byte: u8| byte.is_ascii_whitespace() || matches!(byte, b'"' | b'\'' | b'`');
    let punctuation = |byte: u8| {
        matches!(
            byte,
            b'<' | b'>' | b'{' | b'}' | b'(' | b')' | b',' | b';' | b'=' | b'\\' | b'$'
        )
    };
    for brackets in [true, false] {
        for token in split(text, brackets, quote) {
            offer(token, out);
            for piece in split(token, true, punctuation) {
                if piece.len() != token.len() {
                    offer(piece, out);
                }
            }
        }
    }
}

/// `text` split wherever `boundary` says — except inside square brackets,
/// when `brackets` says to respect them.
///
/// Whitespace always ends a bracket: an arbitrary value cannot contain one
/// (Tailwind writes a space as `_`), so a `[` left open by prose must not
/// swallow the rest of the file.
fn split(text: &str, brackets: bool, boundary: impl Fn(u8) -> bool) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'[' if brackets => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            _ if byte.is_ascii_whitespace() => depth = 0,
            _ => {}
        }
        let open = depth > 0 && !byte.is_ascii_whitespace();
        if !open && boundary(byte) {
            if start < i {
                pieces.push(&text[start..i]);
            }
            start = i + 1;
        }
    }
    if start < bytes.len() {
        pieces.push(&text[start..]);
    }
    pieces
}

/// Adds `token` if it could be a class name, and the same with the sentence
/// punctuation that trails a word in prose taken off.
fn offer(token: &str, out: &mut BTreeSet<String>) {
    for candidate in [token, token.trim_end_matches(['.', ',', ';', ':'])] {
        if !candidate.is_empty()
            && candidate.len() <= MAX_CANDIDATE
            && candidate.bytes().any(|byte| byte.is_ascii_alphabetic())
        {
            out.insert(candidate.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(text: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        extract(text, &mut out);
        out
    }

    #[track_caller]
    fn finds(text: &str, expected: &[&str]) {
        let found = candidates(text);
        for name in expected {
            assert!(found.contains(*name), "{name:?} not found in {found:?}");
        }
    }

    #[test]
    fn a_class_attribute_yields_its_names() {
        finds(
            r#"<div class="flex items-center p-4">"#,
            &["flex", "items-center", "p-4"],
        );
    }

    #[test]
    fn a_name_inside_a_call_is_recovered() {
        finds(
            r#"<a className={cn("px-2", active && "font-bold")}>"#,
            &["px-2", "font-bold"],
        );
        finds(
            "const c = `mt-2 ${big ? 'text-lg' : 'text-sm'}`;",
            &["mt-2", "text-lg", "text-sm"],
        );
    }

    /// Everything Tailwind's syntax puts inside a class name survives:
    /// variants, fractions, important, negatives, opacity modifiers.
    #[test]
    fn variants_and_modifiers_stay_whole() {
        finds(
            r#"class="md:hover:bg-red-500/50 w-1/2 !font-bold -mt-4 dark:text-white/[.8] @md:grid""#,
            &[
                "md:hover:bg-red-500/50",
                "w-1/2",
                "!font-bold",
                "-mt-4",
                "dark:text-white/[.8]",
                "@md:grid",
            ],
        );
    }

    /// An arbitrary value may hold a quote, a comma, parentheses or a `>`.
    /// None of them splits it.
    #[test]
    fn an_arbitrary_value_is_never_split() {
        finds(
            r#"<p className="content-['hi'] grid-cols-[repeat(2,1fr)] [&>*]:p-4 bg-[url('/a.png')]">"#,
            &[
                "content-['hi']",
                "grid-cols-[repeat(2,1fr)]",
                "[&>*]:p-4",
                "bg-[url('/a.png')]",
            ],
        );
        finds(r#"cn("bg-[url('/a.png')]")"#, &["bg-[url('/a.png')]"]);
    }

    /// v4's variable shorthand is parenthesised, which the fine split would
    /// break; the coarse split keeps it.
    #[test]
    fn a_variable_shorthand_stays_whole() {
        finds(
            r#"<b class="bg-(--brand) supports-(display:grid):grid">"#,
            &["bg-(--brand)", "supports-(display:grid):grid"],
        );
    }

    /// A `[` opening a JavaScript array is not an arbitrary value, and the
    /// names quoted inside it are still found.
    #[test]
    fn names_in_an_array_are_found() {
        finds(
            r#"el.className = ["text-brand", "hover:underline"].join(" ");"#,
            &["text-brand", "hover:underline"],
        );
        finds(r#"const c = ['p-2','m-1'];"#, &["p-2", "m-1"]);
    }

    #[test]
    fn prose_punctuation_does_not_hide_a_name() {
        finds("Use flex, then grid.", &["flex", "grid"]);
    }

    /// A `[` that prose never closes must not turn the rest of the file into
    /// one candidate.
    #[test]
    fn an_unclosed_bracket_ends_at_whitespace() {
        finds("see note [1 and then \"p-4\"", &["p-4"]);
    }

    #[test]
    fn noise_is_not_offered() {
        let found = candidates("1 2 --- 3.5 {} ()");
        assert!(found.is_empty(), "{found:?}");
        let long = "a".repeat(MAX_CANDIDATE + 1);
        assert!(candidates(&long).is_empty());
    }

    /// Non-ASCII text is split on ASCII boundaries only, so a multibyte
    /// character never lands on a cut.
    #[test]
    fn unicode_text_is_split_safely() {
        finds(
            r#"<p class="p-4">héllo — ünïcode ✓ "m-2"</p>"#,
            &["p-4", "m-2", "héllo"],
        );
    }

    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-tailwind-scan-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, contents) in files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
            std::fs::write(file, contents).expect("write");
        }
        dir
    }

    /// Automatic detection reads the project and skips what Tailwind skips:
    /// ignored files, `node_modules`, stylesheets, lock files, binaries.
    #[test]
    fn automatic_detection_skips_what_is_not_source() {
        let dir = tree(
            "auto",
            &[
                (".gitignore", "dist/\n"),
                ("src/app.jsx", "<div className=\"from-source\" />"),
                ("index.html", "<b class=\"from-html\"></b>"),
                ("dist/app.js", "\"from-dist\""),
                ("node_modules/pkg/index.js", "\"from-node-modules\""),
                ("src/app.css", ".from-css {}"),
                ("pnpm-lock.yaml", "from-lockfile: 1"),
                ("logo.png", "from-png"),
                ("data.bin", "from\0binary"),
            ],
        );
        let found = scan(Some(&dir), &[]).expect("scan").candidates;
        assert!(found.contains("from-source"), "{found:?}");
        assert!(found.contains("from-html"), "{found:?}");
        for skipped in [
            "from-dist",
            "from-node-modules",
            ".from-css",
            "from-lockfile",
            "from-png",
            "from",
        ] {
            assert!(!found.contains(skipped), "{skipped} was read: {found:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An explicit source is read even where automatic detection would not
    /// look — which is what it exists for.
    #[test]
    fn an_explicit_source_reaches_what_detection_skips() {
        let dir = tree(
            "explicit",
            &[
                ("node_modules/ui/button.js", "\"from-library\""),
                ("node_modules/ui/other.txt", "\"not-a-js-file\""),
                ("vendor/widget.html", "<i class=\"from-vendor\"></i>"),
            ],
        );
        let sources = [
            Source {
                base: dir.clone(),
                pattern: "node_modules/ui/**/*.js".to_string(),
                negated: false,
            },
            Source {
                base: dir.join("src"),
                pattern: "../vendor".to_string(),
                negated: false,
            },
        ];
        let found = scan(None, &sources).expect("scan").candidates;
        assert!(found.contains("from-library"), "{found:?}");
        assert!(found.contains("from-vendor"), "{found:?}");
        assert!(!found.contains("not-a-js-file"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `@source not` removes paths from both kinds of source.
    #[test]
    fn a_negated_source_takes_paths_away() {
        let dir = tree(
            "negated",
            &[
                ("src/keep.js", "\"kept\""),
                ("src/legacy/old.js", "\"dropped\""),
            ],
        );
        let sources = [Source {
            base: dir.clone(),
            pattern: "src/legacy".to_string(),
            negated: true,
        }];
        let found = scan(Some(&dir), &sources).expect("scan").candidates;
        assert!(found.contains("kept"), "{found:?}");
        assert!(!found.contains("dropped"), "{found:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `source(none)` is no automatic root: only what `@source` names.
    #[test]
    fn no_root_reads_only_explicit_sources() {
        let dir = tree("none", &[("src/a.js", "\"anywhere\"")]);
        let scan = scan(None, &[]).expect("scan");
        assert_eq!(scan.files, 0);
        assert!(scan.candidates.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pattern_is_folded_before_it_is_matched() {
        assert_eq!(
            absolute(Path::new("/p/src"), "../lib/./x"),
            PathBuf::from("/p/lib/x")
        );
        assert_eq!(
            glob_prefix(Path::new("/p/lib/**/*.js")),
            Some(PathBuf::from("/p/lib"))
        );
        assert_eq!(glob_prefix(Path::new("/p/lib/a.js")), None);
    }
}
