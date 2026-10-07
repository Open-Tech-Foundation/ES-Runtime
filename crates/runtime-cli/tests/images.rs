//! End-to-end tests for `runtime:images` (DECISIONS D159).
//!
//! These run the real `esrun` binary: the baked module, the ops, the task
//! spawner and otf-pixels underneath. Pixels' own correctness is its own
//! test suite's business; what is checked here is the module's contract — the
//! chain, the formats, the capability split between bytes and `file()`, the
//! error codes, and that the work runs inside a worker too.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/images")
        .join(name);
    std::fs::read(path).expect("read fixture")
}

/// Copies a fixture into the directory the scripts run from, under a name
/// unique to the test, so the sandbox (the working directory, D79) holds it.
fn place(fixture_name: &str, as_name: &str) {
    std::fs::write(temp(as_name), fixture(fixture_name)).expect("place fixture");
}

/// A fixture's bytes as a JavaScript `Uint8Array` expression, for a script
/// that must not read a file.
fn bytes_literal(name: &str) -> String {
    let list: Vec<String> = fixture(name).iter().map(u8::to_string).collect();
    format!("new Uint8Array([{}])", list.join(","))
}

fn esrun() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn exec(name: &str, source: &str, flags: &[&str]) -> Output {
    let app = temp(name);
    std::fs::write(&app, format!("{EQ}\n{source}")).expect("write script");
    esrun().args(flags).arg(&app).output().unwrap()
}

/// Runs `source` with `flags` and returns its stdout, failing with the child's
/// stderr unless it exited cleanly. The JS asserts for itself.
fn run(name: &str, source: &str, flags: &[&str]) -> String {
    let out = exec(name, source, flags);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    stdout(&out)
}

const EQ: &str = r#"
const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual), e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${what}: expected ${e}, got ${a}`);
};
const rejects = async (promise, check, what) => {
  try { await promise; } catch (e) { check(e); return; }
  throw new Error(`${what}: did not reject`);
};
const throws = (fn, check, what) => {
  try { fn(); } catch (e) { check(e); return; }
  throw new Error(`${what}: did not throw`);
};
"#;

/// Bytes in, bytes out needs no capability: the work reads and reaches
/// nothing, as `runtime:hashing`'s does not.
#[test]
fn bytes_in_and_out_need_no_capability() {
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const image = new Image({});
eq(await image.metadata(), {{ width: 32, height: 32, format: "png", pixelFormat: "rgba8", hasAlpha: true, animation: null }}, "metadata");
const webp = await image.resize(16).webp().bytes();
eq((await new Image(webp).metadata()).width, 16, "resized");
console.log("ok");
"#,
        bytes_literal("alpha.png")
    );
    assert_eq!(
        run("images_denied.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// Every format encodes and decodes back to the size it was given, and the
/// chain reports that size before anything is decoded.
#[test]
fn every_format_round_trips() {
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const image = new Image({}).resize(20, 10);
eq((await image.metadata()).width, 20, "metadata of a chain");
for (const format of ["jpeg", "png", "webp", "avif", "gif", "tiff"]) {{
  const blob = await image[format]().blob();
  eq(blob.type, `image/${{format}}`, `${{format}} type`);
  const meta = await new Image(blob).metadata();
  eq([meta.format, meta.width, meta.height], [format, 20, 10], format);
}}
// No format chained keeps the source's.
eq((await new Image(await image.bytes()).metadata()).format, "png", "source format");
console.log("ok");
"#,
        bytes_literal("alpha.png")
    );
    assert_eq!(
        run("images_formats.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// Each method returns a new image, so one source fans out to several sizes,
/// and every option `resize` documents is accepted.
#[test]
fn transforms_return_new_images() {
    let source = format!(
        r##"
import {{ Image }} from "runtime:images";
const base = new Image({});
const small = base.resize(8);
const wide = base.resize(40, 10, {{ fit: "contain", background: "#ff000080", filter: "mitchell" }});
eq((await base.metadata()).width, 32, "base untouched");
eq((await small.metadata()).width, 8, "small");
eq([(await wide.metadata()).width, (await wide.metadata()).height], [40, 10], "contain");
eq((await base.resize(64, 64, {{ withoutEnlargement: true, fit: "inside" }}).metadata()).width, 32, "withoutEnlargement");
eq((await base.crop({{ left: 4, top: 4, width: 10, height: 6 }}).metadata()).height, 6, "crop");
eq((await base.grayscale().metadata()).pixelFormat, "graya8", "grayscale keeps alpha");
eq((await base.flatten({{ background: {{ r: 0, g: 0, b: 255 }} }}).metadata()).hasAlpha, false, "flatten");
eq((await base.extractChannel("alpha").metadata()).pixelFormat, "gray8", "extractChannel");
await base.rotate(-90).flip().flop().modulate({{ brightness: 1.2, saturation: 0, hue: 30 }}).blur(1.5).sharpen().png().bytes();
console.log("ok");
"##,
        bytes_literal("alpha.png")
    );
    assert_eq!(
        run("images_chain.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// The bytes are taken when the terminal is called, so changing the buffer
/// afterwards does not change what is encoded.
#[test]
fn input_is_copied_when_the_terminal_is_called() {
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const input = {};
const pending = new Image(input).metadata();
input.fill(0);
eq((await pending).width, 32, "read before the change");
console.log("ok");
"#,
        bytes_literal("alpha.png")
    );
    assert_eq!(
        run("images_copy.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// A `file()` is read and written through the filesystem provider: FileRead
/// to read one, FileWrite to write one, and the extension never picks the
/// format.
#[test]
fn files_are_read_and_written_through_runtime_fs() {
    place("oriented.jpg", "images_files_in.jpg");
    let source = r#"
import { Image } from "runtime:images";
import { file } from "runtime:fs";
const image = new Image(file("images_files_in.jpg"));
const written = await image.resize(12).webp().write(file("images_files_out.png"));
eq(written > 0, true, "bytes written");
eq((await new Image(file("images_files_out.png")).metadata()).format, "webp", "chained format, not the extension");
console.log("ok");
"#;
    assert_eq!(
        run(
            "images_files.mjs",
            source,
            &["--allow-read", "--allow-write"]
        )
        .trim(),
        "ok"
    );

    let denied = exec(
        "images_files_denied.mjs",
        r#"
import { Image } from "runtime:images";
import { file } from "runtime:fs";
await new Image(file("images_files_in.jpg")).metadata();
"#,
        &["--deny-all"],
    );
    assert!(!denied.status.success());
    assert!(stderr(&denied).contains("FileRead"), "{}", stderr(&denied));

    let no_write = exec(
        "images_files_nowrite.mjs",
        r#"
import { Image } from "runtime:images";
import { file } from "runtime:fs";
await new Image(file("images_files_in.jpg")).png().write(file("images_files_nowrite.png"));
"#,
        &["--allow-read"],
    );
    assert!(!no_write.status.success());
    assert!(
        stderr(&no_write).contains("FileWrite"),
        "{}",
        stderr(&no_write)
    );
}

/// EXIF orientation is applied by default, so the upright size is reported;
/// `autoOrient: false` gives the pixels as stored.
#[test]
fn orientation_is_applied_unless_turned_off() {
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const bytes = {};
const upright = await new Image(bytes).metadata();
eq([upright.width, upright.height], [48, 64], "upright");
const stored = await new Image(bytes, {{ autoOrient: false }}).metadata();
eq([stored.width, stored.height], [64, 48], "as stored");
console.log("ok");
"#,
        bytes_literal("oriented.jpg")
    );
    assert_eq!(
        run("images_orient.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// An animation is reported and processed as its first frame; asking for every
/// frame is refused until Pixels can deliver them, so the option keeps its
/// meaning.
#[test]
fn animation_is_reported_and_animated_is_refused() {
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const bytes = {};
eq((await new Image(bytes).metadata()).animation, {{ frames: 2, loop: 0, durations: [100, 200] }}, "animation");
eq((await new Image(await new Image(bytes).png().bytes()).metadata()).width, 8, "first frame");
await rejects(new Image(bytes, {{ animated: true }}).bytes(), (e) => eq(e.code, "ERR_IMAGE_FORMAT_UNSUPPORTED", "code"), "animated");
console.log("ok");
"#,
        bytes_literal("animated.gif")
    );
    assert_eq!(
        run("images_anim.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// Each failure has its class and code: Bun's two codes, ours for the pixel
/// limit, a RangeError for a crop Pixels finds outside the image, and a
/// TypeError at the call for what the chain can see is wrong.
#[test]
fn failures_have_stable_codes() {
    let png = bytes_literal("alpha.png");
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const png = {png};
await rejects(new Image(new Uint8Array(16)).bytes(), (e) => eq(e.code, "ERR_IMAGE_FORMAT_UNSUPPORTED", "unknown"), "unknown");
await rejects(new Image(png.subarray(0, 60)).bytes(), (e) => eq(e.code, "ERR_IMAGE_DECODE_FAILED", "truncated"), "truncated");
await rejects(new Image(png, {{ maxPixels: 100 }}).metadata(), (e) => eq(e.code, "ERR_IMAGE_TOO_LARGE", "limit"), "limit");
await rejects(new Image(png).crop({{ left: 30, top: 0, width: 10, height: 10 }}).bytes(), (e) => eq(e.name, "RangeError", "crop"), "crop");
throws(() => new Image("photo.jpg"), (e) => eq(e.name, "TypeError", "path string"), "path string");
throws(() => new Image(new SharedArrayBuffer(8)), (e) => eq(e.name, "TypeError", "shared"), "shared");
throws(() => new Image(png).rotate(45), (e) => eq(e.name, "RangeError", "rotate"), "rotate");
throws(() => new Image(png).png({{ palette: true }}), (e) => eq(e.name, "TypeError", "palette"), "palette");
throws(() => new Image(png).jpeg({{ quality: 0 }}), (e) => eq(e.name, "RangeError", "quality"), "quality");
throws(() => new Image(png).resize(), (e) => eq(e.name, "TypeError", "resize"), "resize");
await rejects(new Image(png).write("out.png"), (e) => eq(e.name, "TypeError", "write path"), "write path");
console.log("ok");
"#
    );
    assert_eq!(
        run("images_errors.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// A worker has the module and the spawner too.
#[test]
fn a_worker_encodes_images() {
    std::fs::write(
        temp("images_worker.mjs"),
        format!(
            r#"
import {{ Image }} from "runtime:images";
const bytes = await new Image({}).resize(4).gif().bytes();
postMessage((await new Image(bytes).metadata()).format);
"#,
            bytes_literal("alpha.png")
        ),
    )
    .unwrap();
    let out = run(
        "images_worker_main.mjs",
        r#"
const w = new Worker(new URL("./images_worker.mjs", import.meta.url), { type: "module" });
w.onmessage = (e) => { console.log(e.data); w.terminate(); };
"#,
        &["--allow-all"],
    );
    assert_eq!(out.trim(), "gif");
}
