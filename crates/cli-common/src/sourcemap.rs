//! Stack frames, put back where they were written.
//!
//! A deployed program is a bundle: `esdev build` inlines a program's modules
//! into one file, and everything a stack trace says afterwards is about that
//! file. `dist/server.js:1:4821` is a true statement and a useless one — the
//! line it names is not a line anybody wrote, and finding the code behind it
//! means reading generated output and counting.
//!
//! This is the other half of `--sourcemap`. The build writes a `.map`; this
//! reads it when a stack trace is printed, and rewrites each frame it can:
//!
//! ```text
//! before:  at boom (file:///srv/app/dist/server.js:3:19)
//! after:   at boom (file:///srv/app/src/util.ts:2:14)
//! ```
//!
//! # Why in the printer rather than in the engine
//!
//! Because it is a **presentation** concern and doing it deeper would make it a
//! semantic one. `error.stack` inside the program stays exactly what V8 built:
//! a guest that parses its own stack, or ships it to an error reporter, keeps
//! getting the truth about the file that ran. What changes is only what is
//! written to the operator's terminal, which is the one place the question
//! "where is this in my source?" is being asked.
//!
//! It is also the only place both binaries share. `esrun` and `esdev` print an
//! uncaught exception through [`crate::diagnostics::print_error`], so putting
//! it here means the deployed runtime gets it — which is where an unreadable
//! stack costs the most.
//!
//! # What it will not do
//!
//! **Nothing is read that a frame did not name.** A map is looked for beside
//! the file a frame points at, or through the `sourceMappingURL` that file
//! carries, and nothing else is opened. No source *content* is printed — a
//! frame keeps its shape and gains a better path — so a map that ships with a
//! deployment discloses no more through this than it already does by existing.
//!
//! A frame whose file has no map, or whose position is not in one, is left
//! exactly as it was. Half a remapped stack is worse than none: a reader has to
//! be able to trust that what they are looking at is one coordinate system.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;

/// Every `file://…:line:column` in `text`, rewritten to the source it came
/// from where a map says so.
pub fn remap(text: &str) -> String {
    if !text.contains("file://") {
        return text.to_string();
    }
    text.lines().map(remap_line).collect::<Vec<_>>().join("\n")
}

/// One line of a stack trace.
///
/// Only the first location on a line is considered, which is the only shape V8
/// produces: `at name (url:line:column)`, or `at url:line:column`.
fn remap_line(line: &str) -> String {
    let Some(start) = line.find("file://") else {
        return line.to_string();
    };
    // The URL ends where the frame does: a closing paren, or the end of the
    // line. Whitespace cannot appear in one — V8 percent-encodes it.
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c == ')' || c.is_whitespace())
        .unwrap_or(rest.len());
    let url = &rest[..end];
    let Some((path, source_line, column)) = split_position(url) else {
        return line.to_string();
    };
    let Some(map) = map_for(&path) else {
        return line.to_string();
    };
    let Some((source, mapped_line, mapped_column)) = map.lookup(source_line, column) else {
        return line.to_string();
    };
    format!(
        "{}file://{}:{}:{}{}",
        &line[..start],
        source,
        mapped_line,
        mapped_column,
        &line[start + end..]
    )
}

/// `file:///a/b.js:12:34` → the path, the 1-based line, and the 1-based column.
fn split_position(url: &str) -> Option<(PathBuf, u32, u32)> {
    let without_scheme = url.strip_prefix("file://")?;
    let (rest, column) = without_scheme.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    let line = line.parse::<u32>().ok()?;
    let column = column.parse::<u32>().ok()?;
    // Percent-decoding is deliberately not done: the paths this runtime loads
    // are the ones it was given, and a name with an escape in it is rare enough
    // that guessing wrongly about it is worse than leaving the frame alone.
    (!path.is_empty()).then(|| (PathBuf::from(path), line, column))
}

/// The map for one output file, read once per process.
///
/// The cache holds the misses too. A stack is a run of frames in the same file,
/// and a bundle with no map beside it should be looked for once rather than
/// once per frame.
fn map_for(file: &Path) -> Option<Arc<SourceMap>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<SourceMap>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut held = cache.lock().ok()?;
    if let Some(known) = held.get(file) {
        return known.clone();
    }
    let found = read_map(file)
        .and_then(|text| SourceMap::parse(&text))
        .map(Arc::new);
    held.insert(file.to_path_buf(), found.clone());
    found
}

/// The map's JSON, from beside the file or from inside it.
fn read_map(file: &Path) -> Option<String> {
    let beside = file.with_extension(format!(
        "{}.map",
        file.extension().and_then(|e| e.to_str()).unwrap_or("js")
    ));
    if let Ok(text) = std::fs::read_to_string(&beside) {
        return Some(text);
    }
    // An inline map, which is what a dev build writes. The whole file is read
    // because the marker is at the end and the payload runs back from it — and
    // this only happens on the way to printing an error that has already
    // stopped the program.
    let source = std::fs::read_to_string(file).ok()?;
    let marker = source.rfind("//# sourceMappingURL=")?;
    let url = source[marker + "//# sourceMappingURL=".len()..]
        .lines()
        .next()?
        .trim();
    if let Some(rest) = url.strip_prefix("data:") {
        // The media type is read rather than matched whole: what a bundler
        // writes is `application/json;charset=utf-8;base64`, and matching the
        // shorter spelling this was first written against is how an inline map
        // goes silently unread.
        let (media, payload) = rest.split_once(',')?;
        if !media.split(';').any(|part| part == "base64") {
            // A percent-encoded map is legal and nothing emits one; decoding it
            // half-correctly would be worse than leaving the frame alone.
            return None;
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?;
        return String::from_utf8(bytes).ok();
    }
    // A named map beside the file, when it is not called what this looked for
    // first.
    let named = file.parent()?.join(url);
    std::fs::read_to_string(named).ok()
}

/// One source map: the files it names, and its segments by generated line.
struct SourceMap {
    sources: Vec<String>,
    /// Per generated line, the segments on it, sorted by generated column:
    /// `(generated column, source index, source line, source column)`, all
    /// 0-based.
    lines: Vec<Vec<(u32, usize, u32, u32)>>,
}

impl SourceMap {
    fn parse(text: &str) -> Option<SourceMap> {
        let json: serde_json::Value = serde_json::from_str(text).ok()?;
        let sources = json
            .get("sources")?
            .as_array()?
            .iter()
            .map(|source| source.as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>();
        // `sourceRoot` is prepended by the spec, and a map that has one is
        // otherwise read as naming files that are not there.
        let root = json
            .get("sourceRoot")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let sources = sources
            .into_iter()
            .map(|source| match root {
                "" => source,
                root if root.ends_with('/') => format!("{root}{source}"),
                root => format!("{root}/{source}"),
            })
            .collect();
        let mappings = json.get("mappings")?.as_str()?;
        Some(SourceMap {
            sources,
            lines: decode(mappings),
        })
    }

    /// The source position for a 1-based generated line and column.
    ///
    /// The segment taken is the **last one at or before** the column, which is
    /// what makes a position inside an expression map to the start of the thing
    /// it belongs to rather than to nothing at all.
    fn lookup(&self, line: u32, column: u32) -> Option<(&str, u32, u32)> {
        let segments = self.lines.get(line.checked_sub(1)? as usize)?;
        let column = column.saturating_sub(1);
        let index = match segments.binary_search_by_key(&column, |(at, _, _, _)| *at) {
            Ok(exact) => exact,
            Err(0) => return None,
            Err(after) => after - 1,
        };
        let (_, source, source_line, source_column) = segments[index];
        Some((
            self.sources.get(source)?.as_str(),
            source_line + 1,
            source_column + 1,
        ))
    }
}

/// The `mappings` field: `;` between generated lines, `,` between segments,
/// each segment a run of base64 VLQ numbers, every field but the generated
/// column carried across the whole map.
fn decode(mappings: &str) -> Vec<Vec<(u32, usize, u32, u32)>> {
    let mut lines = Vec::new();
    let (mut source, mut source_line, mut source_column) = (0i64, 0i64, 0i64);
    for line in mappings.split(';') {
        let mut segments: Vec<(u32, usize, u32, u32)> = Vec::new();
        let mut column = 0i64;
        for segment in line.split(',').filter(|s| !s.is_empty()) {
            let mut fields = segment.chars().peekable();
            let Some(delta) = vlq(&mut fields) else {
                continue;
            };
            column += delta;
            // One field is a generated position with no source behind it — the
            // bundler's own preamble, a runtime helper — and there is nothing to
            // map it to.
            let (Some(source_delta), Some(line_delta), Some(column_delta)) =
                (vlq(&mut fields), vlq(&mut fields), vlq(&mut fields))
            else {
                continue;
            };
            source += source_delta;
            source_line += line_delta;
            source_column += column_delta;
            if let (Ok(column), Ok(source), Ok(source_line), Ok(source_column)) = (
                u32::try_from(column),
                usize::try_from(source),
                u32::try_from(source_line),
                u32::try_from(source_column),
            ) {
                segments.push((column, source, source_line, source_column));
            }
        }
        segments.sort_by_key(|(at, _, _, _)| *at);
        lines.push(segments);
    }
    lines
}

/// One base64 VLQ number: six bits per character, the lowest bit of the value
/// being its sign and the top bit of each character saying whether another
/// follows.
fn vlq(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<i64> {
    let mut result = 0i64;
    let mut shift = 0u32;
    loop {
        let digit = base64_digit(*chars.peek()?)?;
        chars.next();
        result += i64::from(digit & 0b1_1111) << shift;
        shift += 5;
        if digit & 0b10_0000 == 0 {
            break;
        }
        // A number this long is a corrupt map rather than a large offset.
        if shift > 60 {
            return None;
        }
    }
    let negative = result & 1 == 1;
    result >>= 1;
    Some(if negative { -result } else { result })
}

fn base64_digit(c: char) -> Option<u8> {
    let value = match c {
        'A'..='Z' => c as u8 - b'A',
        'a'..='z' => c as u8 - b'a' + 26,
        '0'..='9' => c as u8 - b'0' + 52,
        '+' => 62,
        '/' => 63,
        _ => return None,
    };
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The map for `const x = 1;` on the second generated line coming from the
    /// fifth line of `src/a.ts`, hand-encoded.
    fn map() -> SourceMap {
        SourceMap::parse(r#"{"version":3,"sources":["/p/src/a.ts"],"mappings":";AAIA,CAAC"}"#)
            .expect("a map")
    }

    #[test]
    fn a_position_is_the_segment_at_or_before_it() {
        let map = map();
        // Generated line 2, column 1 → the first segment: source line 5.
        assert_eq!(map.lookup(2, 1), Some(("/p/src/a.ts", 5, 1)));
        // Column 2 is the second segment: one column along in the source, on
        // the same line — the deltas carry, and only the generated column
        // restarts per line.
        assert_eq!(map.lookup(2, 2), Some(("/p/src/a.ts", 5, 2)));
        // Past the last segment, the last segment still owns the position.
        assert_eq!(map.lookup(2, 9), Some(("/p/src/a.ts", 5, 2)));
        // A line with no segments at all.
        assert_eq!(map.lookup(1, 1), None);
        // Past the end of the map.
        assert_eq!(map.lookup(99, 1), None);
    }

    #[test]
    fn a_vlq_number_carries_its_sign_in_the_lowest_bit() {
        let read = |text: &str| vlq(&mut text.chars().peekable());
        assert_eq!(read("A"), Some(0));
        assert_eq!(read("C"), Some(1));
        assert_eq!(read("D"), Some(-1));
        assert_eq!(read("gB"), Some(16));
        assert_eq!(read("!"), None);
    }

    /// The spelling a bundler actually writes, which is not the shortest one.
    #[test]
    fn an_inline_map_is_read_whatever_its_media_type_says() {
        let dir = std::env::temp_dir().join("esrun_sourcemap_inline");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let map = r#"{"version":3,"sources":["/p/src/a.ts"],"mappings":";AAIA"}"#;
        let encoded = base64::engine::general_purpose::STANDARD.encode(map);
        let file = dir.join("app.js");
        std::fs::write(
            &file,
            format!(
                "const x = 1;\n//# sourceMappingURL=data:application/json;charset=utf-8;base64,{encoded}\n"
            ),
        )
        .expect("write");

        let read = read_map(&file).expect("a map");
        assert!(read.contains("/p/src/a.ts"), "{read}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_url_splits_into_a_path_and_a_position() {
        assert_eq!(
            split_position("file:///a/b.js:12:34"),
            Some((PathBuf::from("/a/b.js"), 12, 34))
        );
        // Not a position, so not a frame.
        assert_eq!(split_position("file:///a/b.js"), None);
        assert_eq!(split_position("https://x/a.js:1:1"), None);
    }

    /// The half a reader sees: everything around the location is untouched, and
    /// a line with nothing to remap comes back byte for byte.
    #[test]
    fn a_frame_keeps_its_shape() {
        let unmapped = "    at boom (file:///nowhere/app.js:3:19)";
        assert_eq!(remap_line(unmapped), unmapped);
        assert_eq!(remap_line("Error: too big"), "Error: too big");
        assert_eq!(
            remap_line("    at boom (runtime:test:5:1)"),
            "    at boom (runtime:test:5:1)"
        );
    }
}
