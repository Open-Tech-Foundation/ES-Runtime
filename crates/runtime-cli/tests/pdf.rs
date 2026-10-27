//! End-to-end tests for `runtime:pdf` (DECISIONS D190).
//!
//! These run the real `esrun` binary: the baked module, the ops, the task
//! spawner and otf-pdf underneath. otf-pdf's own correctness is its own
//! suite's business; what is checked here is the module's contract — the
//! reads, the edit chain, encryption, the capability split between bytes and
//! `file()`, the error codes, cancellation, and that a worker has it too.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

fn temp(name: &str) -> PathBuf {
    common::tmpdir().join(name)
}

fn fixture(name: &str) -> Vec<u8> {
    let path = common::manifest_dir().join("tests/fixtures/pdf").join(name);
    std::fs::read(path).expect("read fixture")
}

/// Copies bytes into the directory the scripts run from, under a name unique
/// to the test, so the sandbox (the working directory, D79) holds them.
fn place(bytes: &[u8], as_name: &str) {
    std::fs::write(temp(as_name), bytes).expect("place fixture");
}

/// Bytes as a JavaScript `Uint8Array` expression, for a script that must not
/// read a file.
fn literal(bytes: &[u8]) -> String {
    let list: Vec<String> = bytes.iter().map(u8::to_string).collect();
    format!("new Uint8Array([{}])", list.join(","))
}

/// Three pages reading "Page 1" to "Page 3", two bookmarks (one an explicit
/// destination, one a named one) and a JavaScript open action.
fn rich_pdf() -> Vec<u8> {
    let page_text = |n: usize| format!("BT /F1 24 Tf 72 720 Td (Page {n}) Tj ET");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R /Outlines 10 0 R /OpenAction 13 0 R /Names << /Dests 14 0 R >> >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> >>".to_string(),
    ];
    for n in 0..3 {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>",
            n + 6
        ));
    }
    for n in 1..=3 {
        let body = page_text(n);
        objects.push(format!(
            "<< /Length {} >>\nstream\n{body}\nendstream",
            body.len()
        ));
    }
    objects.extend([
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        "<< /Type /Outlines /First 11 0 R /Last 12 0 R /Count 2 >>".to_string(),
        "<< /Title (Intro) /Parent 10 0 R /Next 12 0 R /Dest [3 0 R /Fit] >>".to_string(),
        "<< /Title (End) /Parent 10 0 R /Prev 11 0 R /Dest (third) >>".to_string(),
        r"<< /S /JavaScript /JS (app.alert\(1\)) >>".to_string(),
        "<< /Names [(third) [5 0 R /Fit]] >>".to_string(),
    ]);
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
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
const code = (expected) => (e) => eq(e.code, expected, "code");
"#;

/// Reading bytes needs no capability: the work reads and reaches nothing.
#[test]
fn reads_need_no_capability() {
    let source = format!(
        r#"
import {{ PDF }} from "runtime:pdf";
const doc = await PDF.open({});
eq([doc.pageCount, doc.version, doc.encrypted, doc.permissions], [1, "1.7", false, null], "summary");
eq(doc.diagnostics, [], "a sound file needs no repair");
eq(await doc.pages(), [{{ width: 612, height: 792, rotate: 0, mediaBox: [0, 0, 612, 792], cropBox: [0, 0, 612, 792] }}], "pages");
eq(await doc.text(), "Hello\n\f", "text");
const page = await doc.pageText(0);
eq([page.text, page.lines.length, page.lines[0].words.map((w) => w.text)], ["Hello\n", 1, ["Hello"]], "page text");
eq(page.lines[0].bbox.length, 4, "a line has a box");
eq(await doc.outlines(), [], "no outlines");
eq(await doc.attachments(), [], "no attachments");
eq(await doc.attachment("none"), undefined, "a missing attachment");
await rejects(doc.pageText(1), (e) => eq(e instanceof RangeError, true, "RangeError"), "page out of range");
eq(Object.prototype.toString.call(doc), "[object PDF]", "tag");
console.log("ok");
"#,
        literal(&fixture("hello.pdf"))
    );
    assert_eq!(run("pdf_reads.mjs", &source, &["--deny-all"]).trim(), "ok");
}

/// Every edit returns a new `PDFEdit` and never changes the document; the
/// result is read by opening it.
#[test]
fn edits_return_new_documents() {
    let source = format!(
        r#"
import {{ PDF, PDFEdit }} from "runtime:pdf";
const doc = await PDF.open({});
const texts = async (bytes) => (await (await PDF.open(bytes)).text()).split("\f").filter(Boolean).map((t) => t.trim());
eq(await texts(await doc.edit().bytes()), ["Page 1", "Page 2", "Page 3"], "unchanged");

const edit = doc.edit();
const picked = edit.select([2, 0]);
eq([edit.pageCount, picked.pageCount, picked instanceof PDFEdit], [3, 2, true], "each edit is new");
eq(await texts(await picked.bytes()), ["Page 3", "Page 1"], "select");
eq(await texts(await edit.delete([1]).bytes()), ["Page 1", "Page 3"], "delete");
eq(await texts(await edit.select([0]).append(doc, [1]).append(doc.edit().select([2])).bytes()), ["Page 1", "Page 2", "Page 3"], "append");
eq(doc.pageCount, 3, "the document is untouched");
eq(await texts(await edit.bytes()), ["Page 1", "Page 2", "Page 3"], "an earlier edit is untouched");

const turned = await PDF.open(await edit.rotate(0, 90).rotate([0, 1], 90).crop(2, [0, 0, 300, 400]).bytes());
const pages = await turned.pages();
eq(pages.map((p) => p.rotate), [180, 90, 0], "rotation adds");
eq([pages[1].width, pages[1].height], [792, 612], "a turned page reports its displayed size");
eq(pages[2].cropBox, [0, 0, 300, 400], "crop");

eq(await doc.outlines(), [
  {{ title: "Intro", page: 0, open: false, children: [] }},
  {{ title: "End", page: 2, open: false, children: [] }},
], "outlines, explicit and named");

const when = new Date(Date.UTC(2026, 9, 28, 12, 30, 0));
const titled = await PDF.open(await edit.setInfo({{ title: "Report", author: "Ops", creationDate: when }}).bytes());
eq([titled.info.title, titled.info.author, titled.info.creationDate.toISOString()], ["Report", "Ops", when.toISOString()], "info");
const untitled = await PDF.open(await titled.edit().setInfo({{ title: null }}).bytes());
eq([untitled.info.title, untitled.info.author], [undefined, "Ops"], "null removes one field");

const data = new TextEncoder().encode("a,b\n1,2\n");
const attached = await PDF.open(await edit.addAttachment("data.csv", data, {{ type: "text/csv", description: "numbers" }}).bytes());
const [entry] = await attached.attachments();
eq([entry.name, entry.type, entry.description, entry.size], ["data.csv", "text/csv", "numbers", data.length], "attachment listed");
eq(new TextDecoder().decode(await attached.attachment("data.csv")), "a,b\n1,2\n", "attachment bytes");
eq(await (await PDF.open(await attached.edit().removeAttachment("data.csv").bytes())).attachments(), [], "removed");

const raw = new TextDecoder("latin1").decode(await edit.bytes({{ compress: false, objectStreams: false }}));
eq(raw.includes("/JavaScript"), true, "the script is there");
const clean = await edit.sanitize().bytes({{ compress: false, objectStreams: false }});
eq(new TextDecoder("latin1").decode(clean).includes("/JavaScript"), false, "sanitize removes it");
eq(await texts(clean), ["Page 1", "Page 2", "Page 3"], "and keeps the pages");

const blob = await edit.blob();
eq([blob.type, blob.size > 0], ["application/pdf", true], "blob");
eq(new TextDecoder().decode((await edit.bytes({{ version: "2.0" }})).subarray(0, 8)), "%PDF-2.0", "version");

throws(() => edit.select([3]), (e) => eq(e instanceof RangeError, true, "RangeError"), "index past the end");
throws(() => edit.rotate(0, 45), (e) => eq(e instanceof RangeError, true, "RangeError"), "rotation");
throws(() => edit.setInfo({{ colour: "red" }}), (e) => eq(e instanceof TypeError, true, "TypeError"), "unknown info");
throws(() => edit.sanitize({{ macros: true }}), (e) => eq(e instanceof TypeError, true, "TypeError"), "unknown sanitize option");
throws(() => edit.append({{}}), (e) => eq(e instanceof TypeError, true, "TypeError"), "append a non-PDF");
throws(() => new PDF(), (e) => eq(e instanceof TypeError, true, "TypeError"), "no constructor");
console.log("ok");
"#,
        literal(&rich_pdf())
    );
    assert_eq!(run("pdf_edits.mjs", &source, &["--deny-all"]).trim(), "ok");
}

/// A password opens an encrypted file; a full save must say whether to keep
/// it encrypted; AES-256 output opens with its passwords and keeps its
/// permissions.
#[test]
fn encryption_is_never_dropped_silently() {
    let source = format!(
        r#"
import {{ PDF }} from "runtime:pdf";
const input = {};
await rejects(PDF.open(input), code("ERR_PDF_INVALID_PASSWORD"), "no password");
await rejects(PDF.open(input, {{ password: "wrong" }}), code("ERR_PDF_INVALID_PASSWORD"), "wrong password");
const doc = await PDF.open(input, {{ password: "user" }});
eq([doc.encrypted, typeof doc.permissions.print], [true, "boolean"], "encrypted");
eq((await doc.text()).includes("Secret r6-aes-256"), true, "decrypted text");
eq(doc.info.title, "Title r6-aes-256", "decrypted info");

await rejects(doc.edit().bytes(), (e) => eq([e instanceof TypeError, /encrypt: false/.test(e.message)], [true, true], "names both"), "must choose");
const plain = await PDF.open(await doc.edit().bytes({{ encrypt: false }}));
eq(plain.encrypted, false, "decrypted on request");

const locked = await doc.edit().bytes({{ encrypt: {{ userPassword: "u", ownerPassword: "o", permissions: {{ copy: false }} }} }});
await rejects(PDF.open(locked), code("ERR_PDF_INVALID_PASSWORD"), "needs the new password");
const reopened = await PDF.open(locked, {{ password: "u" }});
eq([reopened.encrypted, reopened.permissions.copy, reopened.permissions.print], [true, false, true], "permissions kept");
eq((await reopened.text()).includes("Secret r6-aes-256"), true, "same text");
const owner = await PDF.open(locked, {{ password: "o" }});
eq(owner.pageCount, 1, "the owner password opens it too");
await rejects(doc.edit().bytes({{ encrypt: {{ userPassword: "u" }} }}), (e) => eq(e instanceof TypeError, true, "TypeError"), "an owner password is required");
console.log("ok");
"#,
        literal(&fixture("r6-aes-256.pdf"))
    );
    assert_eq!(
        run("pdf_encrypt.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// An incremental save appends to the original bytes; where otf-pdf refuses
/// one (a repaired file, legacy encryption) the error says so by code.
#[test]
fn incremental_saves_append_or_are_refused() {
    let source = format!(
        r#"
import {{ PDF }} from "runtime:pdf";
const original = {};
const doc = await PDF.open(original);
const out = await doc.edit().setInfo({{ title: "Appended" }}).bytes({{ incremental: true }});
eq(out.length > original.length, true, "longer");
eq(out.subarray(0, original.length).every((b, i) => b === original[i]), true, "the original is a prefix");
eq((await PDF.open(out)).info.title, "Appended", "the edit is read");

const damaged = await PDF.open({});
eq(damaged.diagnostics.length > 0, true, "the repair is reported");
eq(typeof damaged.diagnostics[0].kind, "string", "diagnostic kind");
await rejects(damaged.edit().bytes({{ incremental: true }}), code("ERR_PDF_INCREMENTAL_REFUSED"), "repaired file");
eq((await damaged.edit().bytes()).length > 0, true, "a full save still works");

const legacy = await PDF.open({}, {{ password: "user" }});
await rejects(legacy.edit().setInfo({{ title: "x" }}).bytes({{ incremental: true }}), code("ERR_PDF_INCREMENTAL_REFUSED"), "RC4 is never written");
console.log("ok");
"#,
        literal(&fixture("hello.pdf")),
        literal(&fixture("damaged.pdf")),
        literal(&fixture("r4-rc4-128.pdf"))
    );
    assert_eq!(
        run("pdf_incremental.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// Failures carry `ERR_PDF_*` codes; arguments are checked at the call.
#[test]
fn failures_have_stable_codes() {
    let source = format!(
        r#"
import {{ PDF }} from "runtime:pdf";
await rejects(PDF.open({}), code("ERR_PDF_MALFORMED"), "not a PDF");
await rejects(PDF.open(new Uint8Array(0)), code("ERR_PDF_MALFORMED"), "empty");
await rejects(PDF.open({}, {{ strict: true }}), code("ERR_PDF_MALFORMED"), "strict refuses a repair");
// otf-pdf drops a page content stream it cannot decode, so the bomb costs
// nothing and reads as an empty page.
const bomb = await PDF.open({}, {{ maxDecodedSize: 1_000_000 }});
eq(await bomb.text(), "\f", "a bomb in page content is not decoded");
const big = await (await PDF.open({})).edit().addAttachment("zeros", new Uint8Array(2_000_000)).bytes();
const capped = await PDF.open(big, {{ maxDecodedSize: 1_000_000 }});
await rejects(capped.attachment("zeros"), (e) => eq([e.code, e.limit], ["ERR_PDF_LIMIT", "decodedSize"], "limit"), "over maxDecodedSize");
eq((await (await PDF.open(big)).attachment("zeros")).length, 2_000_000, "under the default");
await rejects(PDF.open("in.pdf"), (e) => eq([e instanceof TypeError, /file\(\)/.test(e.message)], [true, true], "names file()"), "path string");
await rejects(PDF.open(new Uint8Array(1), {{ maxDecodedSize: 0 }}), (e) => eq(e instanceof RangeError, true, "RangeError"), "bad limit");
await rejects(PDF.open(new Uint8Array(1), {{ signal: {{}} }}), (e) => eq(e instanceof TypeError, true, "TypeError"), "bad signal");
console.log("ok");
"#,
        literal(&fixture("not-a-pdf.pdf")),
        literal(&fixture("damaged.pdf")),
        literal(&fixture("flate-bomb.pdf")),
        literal(&fixture("hello.pdf"))
    );
    assert_eq!(run("pdf_errors.mjs", &source, &["--deny-all"]).trim(), "ok");
}

/// An aborted signal stops the document's work and rejects with its reason;
/// a closed document refuses reads and its unsaved edits.
#[test]
fn signals_and_close() {
    let source = format!(
        r#"
import {{ PDF }} from "runtime:pdf";
const input = {};
const reason = new Error("stop");
await rejects(PDF.open(input, {{ signal: AbortSignal.abort(reason) }}), (e) => eq(e, reason, "the reason"), "aborted before open");

const controller = new AbortController();
const doc = await PDF.open(input, {{ signal: controller.signal }});
eq(await doc.text(), "Hello\n\f", "reads before the abort");
controller.abort();
await rejects(doc.text(), (e) => eq(e.name, "AbortError", "AbortError"), "reads after the abort");

const other = await PDF.open(input);
await rejects(other.edit().bytes({{ signal: AbortSignal.abort() }}), (e) => eq(e.name, "AbortError", "AbortError"), "aborted save");

const edit = other.edit().select([0]);
other.close();
other.close();
await rejects(other.text(), (e) => eq(e instanceof TypeError, true, "TypeError"), "closed read");
await rejects(edit.bytes(), (e) => eq(e instanceof TypeError, true, "TypeError"), "closed edit");

const disposed = await PDF.open(input);
disposed[Symbol.dispose]();
await rejects(disposed.pages(), (e) => eq(e instanceof TypeError, true, "TypeError"), "disposed");
console.log("ok");
"#,
        literal(&fixture("hello.pdf"))
    );
    assert_eq!(
        run("pdf_signals.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}

/// A `file()` is read and written through the filesystem provider: FileRead
/// to read one, FileWrite to write one.
#[test]
fn files_are_read_and_written_through_runtime_fs() {
    place(&rich_pdf(), "pdf_files_in.pdf");
    let source = r#"
import { PDF } from "runtime:pdf";
import { file } from "runtime:fs";
const doc = await PDF.open(file("pdf_files_in.pdf"));
const written = await doc.edit().select([1]).write(file("pdf_files_out.pdf"));
eq(written > 0, true, "bytes written");
eq((await (await PDF.open(file("pdf_files_out.pdf"))).text()).trim(), "Page 2", "read back");
console.log("ok");
"#;
    assert_eq!(
        run("pdf_files.mjs", source, &["--allow-read", "--allow-write"]).trim(),
        "ok"
    );

    let denied = exec(
        "pdf_files_denied.mjs",
        r#"
import { PDF } from "runtime:pdf";
import { file } from "runtime:fs";
await PDF.open(file("pdf_files_in.pdf"));
"#,
        &["--deny-all"],
    );
    assert!(!denied.status.success());
    assert!(stderr(&denied).contains("FileRead"), "{}", stderr(&denied));

    let no_write = exec(
        "pdf_files_nowrite.mjs",
        r#"
import { PDF } from "runtime:pdf";
import { file } from "runtime:fs";
const doc = await PDF.open(file("pdf_files_in.pdf"));
await doc.edit().write(file("pdf_files_nowrite.pdf"));
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

/// Text from a CID font with a ToUnicode map comes out as written.
#[test]
fn extracts_japanese_text() {
    place(&fixture("japanese.pdf"), "pdf_japanese.pdf");
    let source = r#"
import { PDF } from "runtime:pdf";
import { file } from "runtime:fs";
const text = await (await PDF.open(file("pdf_japanese.pdf"))).text();
eq(text.replace(/\s/g, "").includes("今日はとても良い天気です。"), true, "Japanese");
console.log("ok");
"#;
    assert_eq!(
        run("pdf_japanese.mjs", source, &["--allow-read"]).trim(),
        "ok"
    );
}

/// A worker has the module and the spawner too.
#[test]
fn a_worker_reads_pdfs() {
    std::fs::write(
        temp("pdf_worker.mjs"),
        format!(
            r#"
import {{ PDF }} from "runtime:pdf";
const doc = await PDF.open({});
postMessage((await doc.text()).trim());
"#,
            literal(&fixture("hello.pdf"))
        ),
    )
    .unwrap();
    let out = run(
        "pdf_worker_main.mjs",
        r#"
const w = new Worker(new URL("./pdf_worker.mjs", import.meta.url), { type: "module" });
w.onmessage = (e) => { console.log(e.data); w.terminate(); };
"#,
        &["--allow-all"],
    );
    assert_eq!(out.trim(), "Hello");
}

/// Named destinations, XMP, per-character text, media boxes, bookmark
/// editing and the sanitize report.
#[test]
fn the_rest_of_the_toolkit() {
    let source = format!(
        r#"
import {{ PDF }} from "runtime:pdf";
const doc = await PDF.open({});
eq(await doc.destinations(), [{{ name: "third", page: 2 }}], "named destinations");
eq(await doc.xmp(), undefined, "no XMP");
const titled = await PDF.open(await doc.edit().setInfo({{ title: "Quarterly" }}).bytes());
eq((await titled.xmp()).includes("Quarterly"), true, "XMP follows setInfo");

const {{ lines }} = await doc.pageText(0, {{ chars: true }});
const chars = lines[0].chars;
eq(chars.map((c) => c.text).join(""), "Page 1", "chars");
eq([chars[0].size, chars[0].font, chars[0].invisible, chars[0].origin.length], [24, "Helvetica", false, 2], "char fields");
eq("chars" in (await doc.pageText(0)).lines[0], false, "chars only when asked");

const resized = await PDF.open(await doc.edit().setMediaBox([0, 1], [0, 0, 595, 842]).bytes());
eq((await resized.pages()).map((p) => p.mediaBox), [[0, 0, 595, 842], [0, 0, 595, 842], [0, 0, 612, 792]], "media box");

const marked = await PDF.open(await doc.edit().select([2, 1, 0]).setOutlines([
  {{ title: "Start", page: 0, open: true, children: [{{ title: "Middle", page: 1 }}] }},
  {{ title: "No page" }},
]).bytes());
eq(await marked.outlines(), [
  {{ title: "Start", page: 0, open: true, children: [{{ title: "Middle", page: 1, open: false, children: [] }}] }},
  {{ title: "No page", open: false, children: [] }},
], "set outlines");
eq(await (await PDF.open(await doc.edit().setOutlines([]).bytes())).outlines(), [], "removed");

eq(await doc.sanitizeReport(), {{ javascript: 1, externalActions: 0, links: 0, attachments: 0, xfa: 0, multimedia: 0, metadata: 0 }}, "report");
eq((await doc.sanitizeReport({{ javascript: false }})).javascript, 0, "report honours options");
eq((await doc.text()).includes("Page 1"), true, "the report changes nothing");

throws(() => doc.edit().setOutlines([{{ title: "x", page: 3 }}]), (e) => eq(e instanceof RangeError, true, "RangeError"), "bookmark page");
throws(() => doc.edit().setOutlines([{{ page: 0 }}]), (e) => eq(e instanceof TypeError, true, "TypeError"), "bookmark title");
throws(() => doc.edit().setMediaBox(0, [0, 0]), (e) => eq(e instanceof TypeError, true, "TypeError"), "media box shape");
console.log("ok");
"#,
        literal(&rich_pdf())
    );
    assert_eq!(
        run("pdf_toolkit.mjs", &source, &["--deny-all"]).trim(),
        "ok"
    );
}
