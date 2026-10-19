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
eq([...(await tar.files("nested/")).keys()], ["nested/deep.bin"], "string filter");
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
/// corrupt is refused before the first output file is created.
#[test]
fn extraction_verifies_zip_contents_before_writing() {
    let out = temp("archive_corrupt_out");
    let _ = std::fs::remove_dir_all(&out);

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
    assert!(!out.exists(), "created output before verifying contents");
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
