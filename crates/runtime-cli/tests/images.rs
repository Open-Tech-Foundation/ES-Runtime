//! End-to-end tests for `runtime:images` (DECISIONS D159).
//!
//! These run the real `esrun` binary: the baked module, the ops, the task
//! spawner and otf-pixels underneath. Pixels' own correctness is its own
//! test suite's business; what is checked here is the module's contract — the
//! chain, the formats, the capability split between bytes and `file()`, the
//! error codes, and that the work runs inside a worker too.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

fn temp(name: &str) -> PathBuf {
    common::tmpdir().join(name)
}

fn fixture(name: &str) -> Vec<u8> {
    let path = common::manifest_dir()
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
const meta = await image.metadata();
eq([meta.width, meta.height, meta.format, meta.pixelFormat, meta.channels, meta.hasAlpha, meta.animation, meta.pages], [32, 32, "png", "rgba8", 4, true, null, 1], "metadata");
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
for (const format of ["jpeg", "png", "webp", "avif", "gif", "tiff", "jxl"]) {{
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
// With overlays too: the base and every overlay are taken at the call.
const base = {}, overlay = base.slice();
const composed = new Image(base).composite([{{ input: overlay }}]).metadata();
base.fill(0);
overlay.fill(0);
eq((await composed).width, 32, "overlays read before the change");
console.log("ok");
"#,
        bytes_literal("alpha.png"),
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

/// Each failure has its class and code: Bun's three codes, a RangeError for a crop Pixels finds outside the image, and a
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
await rejects(new Image(png, {{ maxPixels: 100 }}).metadata(), (e) => eq(e.code, "ERR_IMAGE_TOO_MANY_PIXELS", "limit"), "limit");
await rejects(new Image(png).crop({{ left: 30, top: 0, width: 10, height: 10 }}).bytes(), (e) => eq(e.name, "RangeError", "crop"), "crop");
throws(() => new Image("photo.jpg"), (e) => eq(e.name, "TypeError", "path string"), "path string");
throws(() => new Image(new SharedArrayBuffer(8)), (e) => eq(e.name, "TypeError", "shared"), "shared");
throws(() => new Image(png).rotate(45), (e) => eq(e.name, "RangeError", "rotate"), "rotate");
throws(() => new Image(png).png({{ progressive: true }}), (e) => eq(e.name, "TypeError", "png progressive"), "png progressive");
throws(() => new Image(png).avif({{ lossless: true }}), (e) => eq(e.name, "TypeError", "avif lossless"), "avif lossless");
throws(() => new Image(png).webp({{ effort: 7 }}), (e) => eq(e.name, "RangeError", "webp effort"), "webp effort");
throws(() => new Image(png).png({{ colours: 1 }}), (e) => eq(e.name, "RangeError", "colours"), "colours");
throws(() => new Image(png).png({{ colours: 8, colors: 16 }}), (e) => eq(e.name, "TypeError", "colours and colors"), "colours and colors");
throws(() => new Image(png).jpeg({{ chromaSubsampling: "4:2:2" }}), (e) => eq(e.name, "TypeError", "chroma"), "chroma");
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

/// The 0.3.0 adjustments change pixels, the positions pick different
/// crops, `withoutReduction` refuses to shrink, and TIFF pages open by
/// number. Identity operations encode byte-identical output.
#[test]
fn adjustments_positions_and_pages() {
    let png = bytes_literal("alpha.png");
    let jpg = bytes_literal("oriented.jpg");
    let tiff = bytes_literal("pages.tiff");
    let source = format!(
        r##"
import {{ Image }} from "runtime:images";
const png = {png}, jpg = {jpg}, tiff = {tiff};
const bytes = (img) => img.png().bytes();
const same = (a, b) => JSON.stringify([...a]) === JSON.stringify([...b]);
const photo = new Image(jpg);

// Positions pick different crops of an asymmetric photo.
const nw = await photo.resize(10, 10, {{ fit: "cover", position: "northwest" }}).png().bytes();
const se = await photo.resize(10, 10, {{ fit: "cover", position: "southeast" }}).png().bytes();
eq(same(nw, se), false, "position picks the crop");
eq((await photo.resize(40, 40, {{ withoutReduction: true }}).metadata()).width, 48, "withoutReduction");
eq((await photo.resize(40, 40).metadata()).width, 40, "reduces without it");
throws(() => photo.resize(10, 10, {{ position: "stretch" }}), (e) => eq(e.name, "TypeError", "position"), "position");
throws(() => photo.resize(10, 10, {{ position: "attention" }}), (e) => eq(e.name, "TypeError", "attention"), "attention");

// Identity operations are byte-identical; the rest change the bytes.
const base = new Image(png);
const id = await bytes(base);
eq(same(await bytes(base.linear(1, 0)), id), true, "linear identity");
eq(same(await bytes(base.recomb([[1, 0, 0], [0, 1, 0], [0, 0, 1]])), id), true, "recomb identity");
eq(same(await bytes(base.recomb([[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0], [0, 0, 0, 1]])), id), true, "recomb rgba identity");
eq(same(await bytes(base.ensureAlpha()), id), true, "ensureAlpha on rgba");
for (const changed of [
  base.negate(), base.negate({{ alpha: false }}), base.linear(1.2, 10),
  base.linear([1, 1, 1], [0, 10, 10]), base.threshold(), base.threshold(0, {{ greyscale: false }}),
  base.tint("#00ff00"), base.tint({{ r: 0, g: 0, b: 255 }}), base.greyscale(),
  base.recomb([[0, 1, 0], [0, 0, 1], [1, 0, 0]]),
  base.modulate({{ lightness: 10 }}),
]) {{
  const out = await bytes(changed);
  eq(same(out, id), false, "changed");
  eq(out.length > 0, true, "encoded");
}}
// Gamma fixes 0 and 1, and this fixture holds nothing else: smoke only.
eq((await base.gamma(2.2).metadata()).width, 32, "gamma runs");
eq((await base.greyscale().metadata()).pixelFormat, "rgba8", "greyscale keeps channels");
eq((await base.removeAlpha().metadata()).pixelFormat, "rgb8", "removeAlpha");
eq((await new Image(jpg).ensureAlpha().metadata()).hasAlpha, true, "ensureAlpha adds it");
eq((await new Image(jpg).removeAlpha().metadata()).pixelFormat, "rgb8", "removeAlpha keeps rgb");

// Misuse throws at the call; a channel a grey image lacks fails at the terminal.
throws(() => base.threshold(300), (e) => eq(e.name, "RangeError", "level"), "level");
throws(() => base.threshold(1.5), (e) => eq(e.name, "RangeError", "level"), "level");
throws(() => base.gamma(0), (e) => eq(e.name, "RangeError", "gamma"), "gamma");
throws(() => base.gamma(-2), (e) => eq(e.name, "RangeError", "gamma"), "gamma");
throws(() => base.linear("x"), (e) => eq(e.name, "TypeError", "linear"), "linear");
throws(() => base.linear([1, 2]), (e) => eq(e.name, "TypeError", "linear"), "linear");
throws(() => base.recomb([[1]]), (e) => eq(e.name, "TypeError", "recomb"), "recomb");
throws(() => base.recomb([[1, 0, 0], [0, NaN, 0], [0, 0, 1]]), (e) => eq(e.name, "TypeError", "recomb"), "recomb");
throws(() => base.ensureAlpha(2), (e) => eq(e.name, "RangeError", "ensureAlpha"), "ensureAlpha");
throws(() => base.extractChannel("cyan"), (e) => eq(e.name, "RangeError", "channel"), "channel");
throws(() => base.modulate({{ lightness: NaN }}), (e) => eq(e.name, "TypeError", "lightness"), "lightness");
await rejects(base.grayscale().extractChannel("green").bytes(), (e) => eq(e.name, "RangeError", "green"), "green");

// Pages: stills hold one; a TIFF opens by number, past the end fails.
eq((await base.metadata()).pages, 1, "png has one page");
eq((await new Image(png, {{ page: 0 }}).metadata()).width, 32, "page 0");
await rejects(new Image(png, {{ page: 1 }}).bytes(), (e) => eq(e.name, "RangeError", "page"), "page");
throws(() => new Image(png, {{ page: -1 }}), (e) => eq(e.name, "RangeError", "page"), "page");
eq((await new Image(tiff).metadata()).pages, 2, "tiff pages");
eq((await new Image(tiff, {{ page: 1 }}).metadata()).height, 6, "second page");
await rejects(new Image(tiff, {{ page: 2 }}).bytes(), (e) => eq(e.name, "RangeError", "page"), "page");
console.log("ok");
"##
    );
    assert_eq!(
        run("images_adjust.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// The encoder options of Pixels 0.4 and 0.5 reach the file: a palette PNG,
/// progressive and 4:4:4 JPEG, and the GIF and WebP knobs, each read back
/// from the output's own header.
#[test]
fn encoder_options_reach_the_file() {
    let png = bytes_literal("alpha.png");
    let jpg = bytes_literal("oriented.jpg");
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const png = {png}, jpg = {jpg};
const meta = async (image) => new Image(await image.bytes()).metadata();
eq((await meta(new Image(png).png())).isPalette, false, "truecolour by default");
eq((await meta(new Image(png).png({{ palette: true }}))).isPalette, true, "palette");
eq((await meta(new Image(png).png({{ colors: 4, dither: false }}))).isPalette, true, "colors implies a palette");
const level0 = await new Image(png).png({{ compressionLevel: 0 }}).bytes();
const level9 = await new Image(png).png({{ compressionLevel: 9 }}).bytes();
eq(level0.length > level9.length, true, "compressionLevel");
const photo = new Image(jpg);
eq((await meta(photo.jpeg())).isProgressive, false, "baseline");
eq((await meta(photo.jpeg({{ progressive: true }}))).isProgressive, true, "progressive");
eq((await meta(photo.jpeg({{ quality: 70, chromaSubsampling: "4:4:4" }}))).chromaSubsampling, "4:4:4", "jpeg 4:4:4");
eq((await meta(photo.jpeg({{ quality: 70 }}))).chromaSubsampling, "4:2:0", "jpeg 4:2:0");
eq((await meta(photo.avif({{ chromaSubsampling: "4:4:4", effort: 2 }}))).chromaSubsampling, "4:4:4", "avif 4:4:4");
eq((await meta(photo.gif({{ colours: 16, dither: 0.5 }}))).format, "gif", "gif options");
eq((await meta(photo.webp({{ effort: 0, alphaQuality: 50 }}))).format, "webp", "webp effort");
eq((await meta(new Image(png).webp({{ nearLossless: true, quality: 60 }}))).format, "webp", "nearLossless");
eq((await meta(photo.jxl({{ lossless: true }}))).format, "jxl", "jxl lossless");
console.log("ok");
"#
    );
    assert_eq!(
        run("images_encoders.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// EXIF and XMP are stripped unless kept; `withXmp` and `withMetadata({
/// density })` write their own; `metadata()` reports what the file says.
#[test]
fn metadata_is_stripped_unless_kept() {
    let png = bytes_literal("alpha.png");
    let jpg = bytes_literal("oriented.jpg");
    let source = format!(
        r#"
import {{ Image }} from "runtime:images";
const png = {png}, jpg = {jpg};
const meta = async (image) => new Image(await image.bytes()).metadata();
const source = await new Image(jpg).metadata();
eq(source.exif instanceof Uint8Array, true, "source exif");
eq(source.orientation, 6, "source orientation");
eq(source.hasProfile, false, "no profile");
eq("density" in (await new Image(png).metadata()), false, "absent fields are left out");
eq("exif" in (await meta(new Image(jpg).jpeg())), false, "exif stripped");
const kept = await meta(new Image(jpg).keepExif().jpeg());
eq(kept.exif instanceof Uint8Array, true, "keepExif");
eq(kept.orientation, 1, "orientation reset once upright");
eq("exif" in (await meta(new Image(jpg).keepMetadata().png())), true, "keepMetadata into png");
const xmp = '<x:xmpmeta xmlns:x="adobe:ns:meta/"></x:xmpmeta>';
const written = await meta(new Image(png).withXmp(xmp).png());
eq(new TextDecoder().decode(written.xmp), xmp, "withXmp");
const dense = await meta(new Image(png).withMetadata({{ density: 300 }}).png());
eq(Math.round(dense.density), 300, "density");
throws(() => new Image(png).withMetadata({{ orientation: 3 }}), (e) => eq(e.name, "TypeError", "orientation"), "orientation");
throws(() => new Image(png).withXmp(1), (e) => eq(e.name, "TypeError", "withXmp"), "withXmp");
console.log("ok");
"#
    );
    assert_eq!(
        run("images_metadata.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// `extend`, `trim`, `composite` and `stats`: the canvas grows and shrinks
/// back, overlays draw from any input an Image takes, and a `file()` overlay
/// is read under `FileRead` like any other file.
#[test]
fn extend_trim_composite_and_stats() {
    place("alpha.png", "images_overlay.png");
    let png = bytes_literal("alpha.png");
    let jpg = bytes_literal("oriented.jpg");
    let source = format!(
        r##"
import {{ Image }} from "runtime:images";
import {{ file }} from "runtime:fs";
const png = {png}, jpg = {jpg};
const size = async (image) => {{ const m = await image.metadata(); return [m.width, m.height]; }};
const photo = new Image(jpg);
eq(await size(photo.extend(4)), [56, 72], "extend all sides");
eq(await size(photo.extend({{ left: 3, bottom: 2, extendWith: "mirror" }})), [51, 66], "extend some");
eq((await photo.extend({{ top: 1, background: "#ff000080" }}).metadata()).hasAlpha, true, "translucent margin adds alpha");

const framed = photo.extend({{ top: 4, left: 6, right: 2, bottom: 1, background: "#00ff00" }}).trim({{ threshold: 40 }});
const trimmed = await framed.metadata();
eq([trimmed.width, trimmed.height, trimmed.trimOffsetLeft, trimmed.trimOffsetTop], [48, 64, -6, -4], "trim");
eq("trimOffsetLeft" in (await photo.metadata()), false, "no trim, no offset");
throws(() => photo.trim({{ margin: 2 }}), (e) => eq(e.name, "TypeError", "margin"), "margin");
throws(() => photo.trim({{ threshold: -1 }}), (e) => eq(e.name, "RangeError", "threshold"), "threshold");

const plain = await photo.png().bytes();
const marked = await photo.composite([
  {{ input: png, gravity: "southeast" }},
  {{ input: new Image(png).resize(8), left: 2, top: 2, blend: "multiply" }},
  {{ input: new Blob([png]), tile: true, blend: "soft-light" }},
  {{ input: file("images_overlay.png"), gravity: "north" }},
]).png().bytes();
const same = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
eq(same(marked, plain), false, "overlays drawn");
eq(await size(new Image(marked)), [48, 64], "size kept");
eq((await photo.composite([{{ input: png }}]).metadata()).hasAlpha, false, "RGBA over RGB stays RGB");
throws(() => photo.composite([{{ input: png, left: 1 }}]), (e) => eq(e.name, "TypeError", "left alone"), "left alone");
throws(() => photo.composite([{{ input: png, blend: "burn" }}]), (e) => eq(e.name, "TypeError", "blend"), "blend");
throws(() => photo.composite([{{ input: "logo.png" }}]), (e) => eq(e.name, "TypeError", "path"), "path");
throws(() => photo.composite({{ input: png }}), (e) => eq(e.name, "TypeError", "array"), "array");

const stats = await photo.stats();
eq(stats.channels.length, 3, "stats channels");
eq(Object.keys(stats.channels[0]), ["min", "max", "sum", "squaresSum", "mean", "stdev", "minX", "minY", "maxX", "maxY"], "channel stats");
eq([stats.isOpaque, typeof stats.entropy, typeof stats.sharpness, Object.keys(stats.dominant)], [true, "number", "number", ["r", "g", "b"]], "stats");
eq((await new Image(png).stats()).channels.length, 4, "alpha has stats");
console.log("ok");
"##
    );
    assert_eq!(
        run("images_canvas.mjs", &source, &["--allow-read"]).trim(),
        "ok"
    );

    let denied = exec(
        "images_overlay_denied.mjs",
        &format!(
            r#"
import {{ Image }} from "runtime:images";
import {{ file }} from "runtime:fs";
await new Image({png}).composite([{{ input: file("images_overlay.png") }}]).bytes();
"#
        ),
        &["--deny-all"],
    );
    assert!(!denied.status.success());
    assert!(stderr(&denied).contains("FileRead"), "{}", stderr(&denied));
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
