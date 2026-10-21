//! End-to-end tests for `runtime:archive` (DECISIONS D174).
//!
//! The real `esrun` binary, the baked module, the ops and the tar/zip crates
//! underneath. What is checked here is the module's contract: deterministic
//! bytes in and out with no capability, the format/compression split, the lazy
//! entry surface, extraction gated on `FileWrite` and confined to the sandbox,
//! `file()` sources gated on `FileRead`, the limits, and the stable error codes.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

fn temp(name: &str) -> PathBuf {
    common::tmpdir().join(name)
}

fn esrun() -> Command {
    let mut command = Command::new(common::esrun_bin());
    command.current_dir(common::tmpdir());
    command
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn run(name: &str, source: &str, flags: &[&str]) -> String {
    let app = temp(name);
    std::fs::write(&app, format!("{EQ}\n{source}")).expect("write script");
    let out = esrun().args(flags).arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    stdout(&out)
}

const EQ: &str = r#"
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual), e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${what}: expected ${e}, got ${a}`);
};
const bytes = (value) => Array.from(value);
"#;

/// Building, listing and reading bytes needs no capability. The same entries
/// produce the same archive, and every format reads back.
#[test]
fn bytes_in_and_out_need_no_capability() {
    let source = r#"
import { Archive } from "runtime:archive";

const make = () => new Archive({
  "hello.txt": "Hello",
  "nested/deep.bin": new Uint8Array([1, 2, 3]),
  "empty/": null,
});

const first = await make().bytes();
const second = await make().bytes();
eq(bytes(first), bytes(second), "deterministic tar");

const tar = new Archive(first);
eq((await tar.blob()).type, "application/x-tar", "tar type");
const entries = await tar.entries();
eq(entries.map((entry) => entry.path), ["empty", "hello.txt", "nested/deep.bin"], "sorted paths");
eq(entries.map((entry) => entry.directory), [true, false, false], "directory flags");
eq(await entries[1].text(), "Hello", "lazy text");
eq(bytes(await entries[2].bytes()), [1, 2, 3], "lazy bytes");

const files = await tar.files();
eq([...files.keys()], ["hello.txt", "nested/deep.bin"], "files skips directories");
eq(files.get("hello.txt") instanceof File, true, "files makes Files");
eq(await files.get("hello.txt").text(), "Hello", "file text");
eq([...(await tar.files("nested/**")).keys()], ["nested/deep.bin"], "glob filter");
eq([...(await tar.files(/\.txt$/)).keys()], ["hello.txt"], "regexp filter");

const zip = new Archive({ "a.txt": "A", "dir/": null }, { format: "zip" });
eq((await zip.blob()).type, "application/zip", "zip type");
const zipped = new Archive(await zip.bytes());
eq((await zipped.entries()).map((entry) => entry.path), ["a.txt", "dir"], "zip paths");
eq(await (await zipped.entries())[0].text(), "A", "zip text");

const gz = new Archive({ "a.txt": "A" }, { compress: "gzip" });
eq((await gz.blob()).type, "application/gzip", "tar.gz type");
const unzipped = new Archive(await gz.bytes());
eq(await (await unzipped.entries())[0].text(), "A", "tar.gz text");

console.log("ok");
"#;
    assert_eq!(
        run("archive_bytes.mjs", source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// Entry metadata can be consumed one at a time with `for await`, and leaving
/// the loop early does not close the archive for later calls.
#[test]
fn entries_are_async_iterable() {
    let source = r#"
import { Archive } from "runtime:archive";

const archive = new Archive({
  "a.txt": "A",
  "dir/b.txt": "B",
  "empty/": null,
}, { format: "zip" });

const seen = [];
for await (const entry of archive) {
  seen.push([entry.path, entry.size, entry.directory]);
}
eq(seen, [["a.txt", 1, false], ["dir/b.txt", 1, false], ["empty", 0, true]], "streamed metadata");

// A partial pass ends its own listing; the archive stays open.
for await (const entry of archive) {
  eq(entry.path, "a.txt", "first path");
  break;
}
eq((await archive.entries()).map((entry) => entry.path), ["a.txt", "dir/b.txt", "empty"], "snapshot after break");
console.log("ok");
"#;
    assert_eq!(
        run("archive_async_iter.mjs", source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// A file source keeps the fd it opened, so validation and extraction read the
/// same file even if the path is renamed in between.
#[test]
fn file_source_stays_stable_across_validation_and_extraction() {
    let out = temp("archive_stable_out");
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_file(temp("archive_stable_source.zip"));
    let _ = std::fs::remove_file(temp("archive_stable_moved.zip"));

    let source = r#"
import { Archive } from "runtime:archive";
import { file, rename } from "runtime:fs";

await Archive.write(file("archive_stable_source.zip"), {
  "a.txt": "A",
}, { format: "zip" });

const opened = new Archive(file("archive_stable_source.zip"));
await opened.entries();
await rename("archive_stable_source.zip", "archive_stable_moved.zip");
await opened.extract("archive_stable_out");
eq(await file("archive_stable_out/a.txt").text(), "A", "same file after rename");
console.log("ok");
"#;
    assert_eq!(
        run(
            "archive_stable.mjs",
            source,
            &["--allow-write=.", "--allow-read=."]
        )
        .trim(),
        "ok"
    );

    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_file(temp("archive_stable_moved.zip"));
}

/// Extraction is the one writing surface: denied without `FileWrite`, and with
/// it the host writes every validated entry under the destination.
#[test]
fn extraction_is_gated_and_writes_under_the_destination() {
    let denied = r#"
import { Archive } from "runtime:archive";
const archive = new Archive({ "a.txt": "A" });
try {
  await archive.extract("out");
  console.log("wrote");
} catch (error) {
  console.log(error.code);
}
"#;
    assert_eq!(
        run("archive_extract_denied.mjs", denied, &["--deny-all"]).trim(),
        "ERR_CAPABILITY_DENIED"
    );

    let allowed = r#"
import { Archive } from "runtime:archive";
import { file, stat } from "runtime:fs";

const archive = new Archive({
  "a.txt": "A",
  "dir/b.txt": "B",
  "empty/": null,
});
const result = await archive.extract("out");
eq(result, { files: 2, directories: 1 }, "counts");
eq(await file("out/a.txt").text(), "A", "top-level file");
eq(await file("out/dir/b.txt").text(), "B", "nested file");
eq((await stat("out/empty")).isDir, true, "directory entry");
eq((await stat("out/dir")).isDir, true, "implicit parent");
console.log("ok");
"#;
    assert_eq!(
        run(
            "archive_extract.mjs",
            allowed,
            &["--allow-write=.", "--allow-read=."]
        )
        .trim(),
        "ok"
    );
}

/// A `file()` source is FileRead, a `file()` destination is FileWrite, and an
/// archive written to disk reads back with the same entries.
#[test]
fn file_sources_and_destinations_use_the_fs_capabilities() {
    let source = r#"
import { Archive } from "runtime:archive";
import { file } from "runtime:fs";

const written = await Archive.write(file("bundle.zip"), {
  "one.txt": "1",
  "two.txt": "2",
}, { format: "zip" });
eq(written > 0, true, "bytes written");

const opened = new Archive(file("bundle.zip"));
eq((await opened.entries()).map((entry) => entry.path), ["one.txt", "two.txt"], "entries from file");
eq(await (await opened.files()).get("two.txt").text(), "2", "file contents");

const copied = await opened.bytes();
eq(bytes(copied), bytes(await file("bundle.zip").bytes()), "opened bytes are the source bytes");
console.log("ok");
"#;
    assert_eq!(
        run(
            "archive_files.mjs",
            source,
            &["--allow-read=.", "--allow-write=."]
        )
        .trim(),
        "ok"
    );

    let denied = r#"
import { Archive } from "runtime:archive";
import { file } from "runtime:fs";
try {
  await new Archive(file("bundle.zip")).entries();
  console.log("read");
} catch (error) {
  console.log(error.code);
}
"#;
    assert_eq!(
        run("archive_files_denied.mjs", denied, &["--deny-all"]).trim(),
        "ERR_CAPABILITY_DENIED"
    );
}

/// A bad archive is refused with a stable code, and a bad option or path is a
/// TypeError before anything is written.
#[test]
fn errors_are_typed_and_coded() {
    let source = r#"
import { Archive } from "runtime:archive";

const code = async (work) => {
  try { await work(); return "no error"; } catch (error) { return error.code ?? error.constructor.name; }
};

eq(await code(() => new Archive({ "../escape.txt": "x" }).bytes()), "TypeError", "unsafe path");
eq(await code(() => new Archive({}, { format: "rar" }).bytes()), "TypeError", "unknown format");
eq(await code(() => new Archive({}, { format: "zip", compress: "gzip" }).bytes()), "TypeError", "zip gzip");
eq(await code(() => new Archive({}, { format: "tar", compress: "deflate" }).bytes()), "TypeError", "tar deflate");
eq(await code(() => new Archive(new Uint8Array([0x50, 0x4b, 0x03, 0x04])).entries()), "ERR_ARCHIVE_DECODE_FAILED", "truncated zip");
eq(await code(() => new Archive(new Uint8Array(4)).entries()), "ERR_ARCHIVE_DECODE_FAILED", "not a tar");
eq(await code(() => new Archive({ "a": "1", "b": "2" }, { maxEntries: 1 }).bytes()), "ERR_ARCHIVE_TOO_LARGE", "entry limit");
eq(await code(() => new Archive({ "a": "12345" }, { maxBytes: 2 }).bytes()), "ERR_ARCHIVE_TOO_LARGE", "size limit");
console.log("ok");
"#;
    assert_eq!(
        run("archive_errors.mjs", source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// `compress: "none"` writes stored zip members, and `stream()` yields the
/// entry in bounded chunks rather than one copy of the whole entry.
#[test]
fn stored_zip_and_streaming_chunks() {
    let source = r#"
import { Archive } from "runtime:archive";

const stored = await new Archive({ "a.txt": "A" }, { format: "zip" }).bytes();
eq([stored[8], stored[9]], [0, 0], "zip none is stored");
const deflated = await new Archive({ "a.txt": "A" }, { format: "zip", compress: "deflate" }).bytes();
eq([deflated[8], deflated[9]], [8, 0], "zip deflate is deflated");

const archive = new Archive({
  "big.bin": new Uint8Array(600000).fill(3),
  "b.txt": "B",
}, { format: "zip" });
const opened = new Archive(await archive.bytes());
const big = (await opened.entries()).find((entry) => entry.path === "big.bin");
let chunks = 0;
let total = 0;
for await (const chunk of big.stream()) {
  chunks += 1;
  total += chunk.byteLength;
}
eq(chunks > 1, true, "stream chunks");
eq(total, 600000, "stream total");
console.log("ok");
"#;
    assert_eq!(
        run("archive_stream.mjs", source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// Extraction refuses an existing symlink in a parent directory rather than
/// following it to a different location.
#[cfg(unix)]
#[test]
fn extraction_refuses_symlink_parents() {
    let out = temp("archive_symlink_out");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(out.join("nested")).unwrap();
    std::fs::create_dir_all(out.join("outside")).unwrap();
    std::os::unix::fs::symlink("../outside", out.join("nested/link")).unwrap();

    let source = r#"
import { Archive } from "runtime:archive";
try {
  await new Archive({ "nested/link/pwn.txt": "x" }).extract("archive_symlink_out");
  console.log("wrote");
} catch (error) {
  console.log(error.code);
}
"#;
    assert_eq!(
        run(
            "archive_symlink.mjs",
            source,
            &["--allow-write=.", "--allow-read=."]
        )
        .trim(),
        "ERR_ARCHIVE_UNSAFE_PATH"
    );
    assert!(
        !out.join("outside/pwn.txt").exists(),
        "wrote through the link"
    );
}

/// Extraction refuses an existing symlink at a file's own destination, not
/// only in its parent directories.
#[cfg(unix)]
#[test]
fn extraction_refuses_symlink_destinations() {
    let out = temp("archive_symlink_file_out");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(temp("archive_symlink_file_outside")).unwrap();
    std::os::unix::fs::symlink(
        "../archive_symlink_file_outside/report.txt",
        out.join("report.txt"),
    )
    .unwrap();

    let source = r#"
import { Archive } from "runtime:archive";
try {
  await new Archive({ "report.txt": "new" }).extract("archive_symlink_file_out");
  console.log("wrote");
} catch (error) {
  console.log(error.code);
}
"#;
    assert_eq!(
        run(
            "archive_symlink_file.mjs",
            source,
            &["--allow-write=.", "--allow-read=."]
        )
        .trim(),
        "ERR_ARCHIVE_UNSAFE_PATH"
    );
    assert!(
        !temp("archive_symlink_file_outside/report.txt").exists(),
        "wrote through the link"
    );
}

/// A zip whose central directory is valid but whose later entry data is
/// corrupt is refused before staged files are copied to the destination.
#[test]
fn extraction_stages_before_writing_destination() {
    let out = temp("archive_corrupt_out");
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("existing.txt"), "keep me").unwrap();

    let source = r#"
import { Archive } from "runtime:archive";

const good = await new Archive({
  "first.txt": "first",
  "second.txt": "second",
}, { format: "zip", compress: "none" }).bytes();

let seen = 0;
let start = -1;
for (let i = 0; i + 4 <= good.length; i++) {
  if (good[i] === 0x50 && good[i + 1] === 0x4b && good[i + 2] === 0x03 && good[i + 3] === 0x04) {
    seen += 1;
    if (seen === 2) { start = i; break; }
  }
}
if (start < 0) throw new Error("second local header not found");
const nameLength = good[start + 26] | (good[start + 27] << 8);
const extraLength = good[start + 28] | (good[start + 29] << 8);
const dataStart = start + 30 + nameLength + extraLength;
const broken = good.slice();
broken[dataStart] ^= 0xff;

try {
  await new Archive(broken).extract("archive_corrupt_out");
  console.log("wrote");
} catch (error) {
  console.log(error.code);
}
"#;
    assert_eq!(
        run(
            "archive_corrupt.mjs",
            source,
            &["--allow-write=.", "--allow-read=."]
        )
        .trim(),
        "ERR_ARCHIVE_DECODE_FAILED"
    );
    assert_eq!(
        std::fs::read_to_string(out.join("existing.txt")).unwrap(),
        "keep me",
        "changed existing output before validating archive contents"
    );
    assert!(
        !out.join("first.txt").exists(),
        "copied a staged file before validating the complete archive"
    );
    assert!(
        std::fs::read_dir(&out).unwrap().count() == 1,
        "left staging files in the destination"
    );
}

/// Entry metadata is read without decompressing entry data, and a missing name
/// is the module's NotFound rather than a crash.
#[test]
fn entry_reads_are_lazy_and_missing_names_are_reported() {
    let source = r#"
import { Archive, ArchiveEntry } from "runtime:archive";

const archive = new Archive({
  "a.txt": "A",
  "large.bin": new Uint8Array(4096).fill(7),
}, { format: "zip" });
const opened = new Archive(await archive.bytes());
const entries = await opened.entries();
eq(entries.map((entry) => entry.size), [1, 4096], "declared sizes");
eq(bytes(await entries[1].bytes()).slice(0, 3), [7, 7, 7], "entry bytes");

const missing = new ArchiveEntry(opened, {
  path: "missing",
  directory: false,
  size: 0,
  lastModified: null,
});
try {
  await missing.bytes();
  console.log("read");
} catch (error) {
  console.log(error.code);
}
"#;
    assert_eq!(
        run("archive_lazy.mjs", source, &["--deny-all"]).trim(),
        "ERR_NOT_FOUND"
    );
}

/// A corrupted gzip trailer is a decode error on every path that reads the
/// whole archive, including extraction, which writes nothing.
#[test]
fn gzip_trailers_are_verified_end_to_end() {
    let source = r#"
import { Archive } from "runtime:archive";
import { exists } from "runtime:fs";

const good = await new Archive({ "a.txt": "hello" }, { compress: "gzip" }).bytes();
const bad = good.slice();
bad[bad.length - 8] ^= 0xff; // the CRC-32
const outcome = async (work) => {
  try { await work(); return "ok"; } catch (error) { return error.code; }
};
console.log(await outcome(() => new Archive(bad).entries()));
console.log(await outcome(() => new Archive(bad).files()));
console.log(await outcome(() => new Archive(bad).extract("archive_gz_out")));
console.log(await exists("archive_gz_out"));
console.log(await outcome(() => new Archive(good).extract("archive_gz_good")));
"#;
    let dir = temp("archive_gz_out");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(temp("archive_gz_good"));
    let out = run(
        "archive_gz_trailer.mjs",
        source,
        &["--deny-all", "--allow-read", "--allow-write"],
    );
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        [
            "ERR_ARCHIVE_DECODE_FAILED",
            "ERR_ARCHIVE_DECODE_FAILED",
            "ERR_ARCHIVE_DECODE_FAILED",
            "false",
            "ok"
        ]
    );
}

/// An Archive holds the bytes it was given at construction; writing to the
/// caller's buffer afterwards changes nothing, for a source or for an entry.
#[test]
fn input_bytes_are_copied_at_construction() {
    let source = r#"
import { Archive } from "runtime:archive";

const tar = await new Archive({ "a.txt": "hello" }).bytes();
const opened = new Archive(tar);
tar.fill(0);
eq((await opened.entries()).map((entry) => entry.path), ["a.txt"], "source");

const data = new TextEncoder().encode("hello");
const built = new Archive({ "a.txt": data });
data.fill(65);
eq(await (await built.files()).get("a.txt").text(), "hello", "entry");
console.log("ok");
"#;
    assert_eq!(
        run("archive_copy.mjs", source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// A limit passed to extract() overrides that limit only; the Archive's other
/// limits still apply. A directory entry streams as empty, as bytes() is.
#[test]
fn extract_limits_merge_and_directories_stream_empty() {
    let source = r#"
import { Archive } from "runtime:archive";

const tar = await new Archive({ "a.txt": "x".repeat(100) }).bytes();
try {
  await new Archive(tar, { maxBytes: 50 }).extract("archive_merge_out", { maxEntries: 10 });
  console.log("extracted");
} catch (error) {
  console.log(error.code);
}

const zip = new Archive(await new Archive({ "d/": null, "d/x": "1" }, { format: "zip" }).bytes());
const directory = (await zip.entries()).find((entry) => entry.directory);
const read = await directory.stream().getReader().read();
eq(read.done, true, "directory stream");
console.log("ok");
"#;
    let _ = std::fs::remove_dir_all(temp("archive_merge_out"));
    assert_eq!(
        run(
            "archive_merge.mjs",
            source,
            &["--deny-all", "--allow-write"]
        )
        .lines()
        .collect::<Vec<_>>(),
        ["ERR_ARCHIVE_TOO_LARGE", "ok"]
    );
}

/// The destination directory is pinned without following a link at its own
/// name (D180), and a file already there is replaced by the extracted one.
#[cfg(unix)]
#[test]
fn extraction_pins_the_destination_and_replaces_existing_files() {
    let linked = temp("archive_pinned_link");
    let real = temp("archive_pinned_real");
    let out = temp("archive_pinned_out");
    for dir in [&linked, &real, &out] {
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_file(dir);
    }
    std::fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, &linked).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("a.txt"), "old").unwrap();

    let source = r#"
import { Archive } from "runtime:archive";
const archive = new Archive({ "a.txt": "new", "d/b.txt": "b" });
try {
  await archive.extract("archive_pinned_link");
  console.log("wrote");
} catch (error) {
  console.log(error.code);
}
console.log(JSON.stringify(await archive.extract("archive_pinned_out")));
"#;
    let out_text = run(
        "archive_pinned.mjs",
        source,
        &["--allow-write=.", "--allow-read=."],
    );
    assert_eq!(
        out_text.lines().collect::<Vec<_>>(),
        ["ERR_ARCHIVE_UNSAFE_PATH", r#"{"files":2,"directories":0}"#]
    );
    assert!(std::fs::read_dir(&real).unwrap().next().is_none());
    assert_eq!(std::fs::read_to_string(out.join("a.txt")).unwrap(), "new");
    assert_eq!(std::fs::read_to_string(out.join("d/b.txt")).unwrap(), "b");
    let leftovers: Vec<_> = std::fs::read_dir(&out)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".esrun-archive-")
        })
        .collect();
    assert!(leftovers.is_empty(), "the stage was left behind");
}

/// A string or a list filter is a glob, read as Bun reads it: `!` excludes,
/// and a list of only exclusions takes everything else. A RegExp and a
/// function are applied to the path.
#[test]
fn files_filters_are_globs_regexps_or_functions() {
    let source = r#"
import { Archive } from "runtime:archive";

const archive = new Archive(await new Archive({
  "src/a.ts": "a",
  "src/a.test.ts": "t",
  "src/deep/b.ts": "b",
  "docs/readme.md": "r",
  "src/": null,
}).bytes());
const keys = async (filter) => [...(await archive.files(filter)).keys()];

eq(await keys("src/**"), ["src/a.test.ts", "src/a.ts", "src/deep/b.ts"], "glob");
eq(await keys("*.ts"), [], "* does not cross /");
eq(await keys(["**/*.ts", "!**/*.test.ts"]), ["src/a.ts", "src/deep/b.ts"], "list");
eq(await keys(["!src/**"]), ["docs/readme.md"], "only exclusions");
eq(await keys(/\.md$/), ["docs/readme.md"], "regexp");
eq(await keys((path) => path.includes("deep")), ["src/deep/b.ts"], "function");
eq(await (await archive.files("docs/*")).get("docs/readme.md").text(), "r", "contents");
try {
  await archive.files([1]);
} catch (error) {
  console.log(error.constructor.name);
}
try {
  await archive.files("a[");
} catch (error) {
  console.log(error.constructor.name);
}
"#;
    assert_eq!(
        run("archive_filters.mjs", source, &["--deny-all"])
            .lines()
            .collect::<Vec<_>>(),
        ["TypeError", "TypeError"]
    );
}

/// `extract({ filter })` writes only what the filter takes, and still refuses
/// an archive whose untaken entries are unsafe.
#[test]
fn extract_writes_only_what_the_filter_takes() {
    let out = temp("archive_selective_out");
    let fn_out = temp("archive_selective_fn_out");
    let refused = temp("archive_selective_refused");
    for dir in [&out, &fn_out, &refused] {
        let _ = std::fs::remove_dir_all(dir);
    }
    let source = r#"
import { Archive } from "runtime:archive";

const archive = new Archive({
  "src/a.ts": "a",
  "src/a.test.ts": "t",
  "docs/readme.md": "r",
});
console.log(JSON.stringify(
  await archive.extract("archive_selective_out", { filter: ["src/**", "!**/*.test.ts"] }),
));
console.log(JSON.stringify(
  await archive.extract("archive_selective_fn_out", { filter: (path) => path.startsWith("docs") }),
));
"#;
    let output = run(
        "archive_selective.mjs",
        source,
        &["--allow-write=.", "--allow-read=."],
    );
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        [
            r#"{"files":1,"directories":0}"#,
            r#"{"files":1,"directories":0}"#
        ]
    );
    assert_eq!(std::fs::read_to_string(out.join("src/a.ts")).unwrap(), "a");
    assert!(!out.join("src/a.test.ts").exists() && !out.join("docs").exists());
    assert_eq!(
        std::fs::read_to_string(fn_out.join("docs/readme.md")).unwrap(),
        "r"
    );
    assert!(!fn_out.join("src").exists());
}

/// `level` picks the compressor's trade-off, 1 to 9, and only with a
/// compressor: out of range is a RangeError, without one a TypeError.
#[test]
fn compression_level_is_chosen_and_checked() {
    let source = r#"
import { Archive } from "runtime:archive";

const entries = { "a.txt": "abcabcabc".repeat(5000) };
for (const [format, compress] of [["tar", "gzip"], ["zip", "deflate"]]) {
  const fast = await new Archive(entries, { format, compress, level: 1 }).bytes();
  const small = await new Archive(entries, { format, compress, level: 9 }).bytes();
  const again = await new Archive(entries, { format, compress, level: 9 }).bytes();
  eq(bytes(small), bytes(again), `${format} is deterministic`);
  if (bytes(fast).join() === bytes(small).join()) throw new Error(`${format}: level changed nothing`);
  eq(await (await new Archive(small).files()).get("a.txt").text(), entries["a.txt"], "round trip");
}
for (const options of [{ compress: "gzip", level: 0 }, { compress: "gzip", level: 10 },
                       { compress: "gzip", level: 1.5 }, { level: 6 }]) {
  try {
    new Archive(entries, options);
    console.log("accepted");
  } catch (error) {
    console.log(error.constructor.name);
  }
}
"#;
    assert_eq!(
        run("archive_level.mjs", source, &["--deny-all"])
            .lines()
            .collect::<Vec<_>>(),
        ["RangeError", "RangeError", "TypeError", "TypeError"]
    );
}
