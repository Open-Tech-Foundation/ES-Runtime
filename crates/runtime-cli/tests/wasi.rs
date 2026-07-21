//! End-to-end `runtime:wasi` tests: run the real `esrun` binary against
//! JavaScript that drives a WASI preview-1 instance, including one real
//! `wasm32`-style command module that writes to stdout through `fd_write`.
//!
//! The ABI assertions live in the JS rather than in Rust: they need to read back
//! what the syscalls wrote into wasm memory, which is far more direct from the
//! guest side. A failed assertion throws, so `esrun` exits non-zero and the test
//! fails with the message on stderr.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workdir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esrun-wasi-{test}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create work dir");
    dir
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    std::fs::write(dir.join(rel), bytes).expect("write fixture");
}

fn run(dir: &Path, entry: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg(dir.join(entry))
        .output()
        .expect("failed to spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Shared JS: a minimal WASM encoder plus a `_start` module that prints one
/// string through `fd_write`. `i32.const` takes a *signed* LEB128, so a value
/// like 100 needs two bytes — one byte would read as negative.
const ENCODER_JS: &str = r#"
export const MAGIC = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
export const vec = (i) => [i.length, ...i.flat()];
export const sec = (id, p) => [id, p.length, ...p];
export const str = (s) => [s.length, ...Array.from(s, (c) => c.charCodeAt(0))];
export const body = (c) => [c.length + 2, 0x00, ...c, 0x0b];
export const sleb = (n) => {
  const out = [];
  for (;;) {
    let b = n & 0x7f;
    n >>= 7;
    if ((n === 0 && !(b & 0x40)) || (n === -1 && b & 0x40)) { out.push(b); return out; }
    out.push(b | 0x80);
  }
};
export const i32c = (n) => [0x41, ...sleb(n)];
const store = (ptr, val) => [...i32c(ptr), ...i32c(val), 0x36, 0x02, 0x00];

/// A command module that writes `msg` to `fd` via one iovec and returns.
export function printerModule(msg, fd = 1) {
  const bytes = Array.from(new TextEncoder().encode(msg));
  const AT = 100;
  return new Uint8Array([...MAGIC,
    ...sec(1, vec([
      [0x60, 0x04, 0x7f, 0x7f, 0x7f, 0x7f, 0x01, 0x7f],
      [0x60, 0x00, 0x00],
    ])),
    ...sec(2, vec([[...str("wasi_snapshot_preview1"), ...str("fd_write"), 0x00, 0x00]])),
    ...sec(3, vec([[0x01]])),
    ...sec(5, vec([[0x00, 0x01]])),
    ...sec(7, vec([[...str("memory"), 0x02, 0x00], [...str("_start"), 0x00, 0x01]])),
    ...sec(10, vec([body([
      ...store(0, AT),
      ...store(4, bytes.length),
      ...i32c(fd), ...i32c(0), ...i32c(1), ...i32c(20),
      0x10, 0x00,
      0x1a,
    ])])),
    ...sec(11, vec([[0x00, ...i32c(AT), 0x0b, bytes.length, ...bytes]])),
  ]);
}

export const assert = (cond, msg) => { if (!cond) throw new Error("FAIL: " + msg); };
"#;

#[test]
fn runs_a_wasi_command_module_that_writes_to_stdout() {
    let dir = workdir("stdout");
    write(&dir, "enc.js", ENCODER_JS.as_bytes());
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
import { printerModule } from "./enc.js";

const wasi = new WASI({ args: ["prog"] });
const { instance } = await WebAssembly.instantiate(
  printerModule("hello wasi\n"),
  wasi.getImportObject(),
);
console.log("STATUS:", wasi.start(instance));
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("hello wasi"), "{stdout}");
    // Returning from `_start` without calling proc_exit is status 0.
    assert!(stdout.contains("STATUS: 0"), "{stdout}");
}

#[test]
fn implements_the_preview1_abi_for_args_env_clocks_and_random() {
    let dir = workdir("abi");
    write(&dir, "enc.js", ENCODER_JS.as_bytes());
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
import { assert } from "./enc.js";

const memory = new WebAssembly.Memory({ initial: 1 });
const wasi = new WASI({ args: ["prog", "x"], env: { A: "1", BB: "22" } });
// Bind memory without a real module; `_start` does nothing.
wasi.start({ exports: { memory, _start() {} } });
const w = wasi.getImportObject().wasi_snapshot_preview1;

const dv = () => new DataView(memory.buffer);
const u8 = () => new Uint8Array(memory.buffer);
const cstr = (p) => { let s = ""; const m = u8(); while (m[p]) s += String.fromCharCode(m[p++]); return s; };

assert(w.args_sizes_get(0, 4) === 0, "args_sizes_get errno");
assert(dv().getUint32(0, true) === 2, "argc");
assert(dv().getUint32(4, true) === "prog\0x\0".length, "argv buffer size");
assert(w.args_get(100, 200) === 0, "args_get errno");
assert(cstr(dv().getUint32(100, true)) === "prog", "argv[0]");
assert(cstr(dv().getUint32(104, true)) === "x", "argv[1]");

assert(w.environ_sizes_get(0, 4) === 0, "environ_sizes_get errno");
assert(dv().getUint32(0, true) === 2, "environ count");
assert(w.environ_get(300, 400) === 0, "environ_get errno");
assert(cstr(dv().getUint32(300, true)) === "A=1", "environ[0]");
assert(cstr(dv().getUint32(304, true)) === "BB=22", "environ[1]");

// Realtime is nanoseconds since the epoch, so comfortably past 2020.
assert(w.clock_time_get(0, 0n, 500) === 0, "realtime errno");
assert(dv().getBigUint64(500, true) > 1600000000000000000n, "realtime plausible");
assert(w.clock_time_get(1, 0n, 508) === 0, "monotonic errno");
assert(w.clock_time_get(99, 0n, 508) === 28, "unknown clock is EINVAL");
assert(w.clock_res_get(0, 516) === 0, "clock_res_get errno");

u8().fill(0, 600, 632);
assert(w.random_get(600, 32) === 0, "random_get errno");
assert(u8().subarray(600, 632).some((b) => b !== 0), "random_get filled the buffer");

console.log("ABI OK");
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ABI OK"), "{}", stdout(&out));
}

#[test]
fn reports_stdio_and_filesystem_errnos_without_link_errors() {
    let dir = workdir("errnos");
    write(&dir, "enc.js", ENCODER_JS.as_bytes());
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
import { assert } from "./enc.js";

const memory = new WebAssembly.Memory({ initial: 1 });
const wasi = new WASI({});
wasi.start({ exports: { memory, _start() {} } });
const w = wasi.getImportObject().wasi_snapshot_preview1;
const dv = () => new DataView(memory.buffer);

// stdout is a character device; seeking a pipe is ESPIPE.
assert(w.fd_fdstat_get(1, 700) === 0, "fdstat errno");
assert(new Uint8Array(memory.buffer)[700] === 2, "stdout is a character device");
assert(w.fd_fdstat_get(5, 700) === 8, "unknown fd is EBADF");
assert(w.fd_seek(1) === 70, "seeking stdout is ESPIPE");

// No stdin source: a clean EOF, not an error.
assert(w.fd_read(0, 0, 0, 800) === 0, "stdin read errno");
assert(dv().getUint32(800, true) === 0, "stdin reads zero bytes");
// An fd that was never opened is EBADF; ENOTCAPABLE is for a path the guest is
// not permitted to reach.
assert(w.fd_write(7, 0, 0, 800) === 8, "writing an unknown fd is EBADF");
assert(w.fd_seek(7, 0, 0, 800) === 8, "seeking an unknown fd is EBADF");

// This instance was built without preopens, so no path resolves.
assert(w.path_open(0, 0, 0, 0, 0, 0n, 0n, 0, 0) === 76, "path_open is ENOTCAPABLE");
assert(w.fd_prestat_get(3, 0) === 8, "no preopen at fd 3 is EBADF");
assert(typeof w.path_unlink_file === "function", "path_unlink_file is linkable");
assert(typeof w.poll_oneoff === "function", "poll_oneoff is linkable");

console.log("ERRNOS OK");
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ERRNOS OK"), "{}", stdout(&out));
}

#[test]
fn proc_exit_becomes_a_status_and_real_errors_still_propagate() {
    let dir = workdir("exit");
    write(&dir, "enc.js", ENCODER_JS.as_bytes());
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
const memory = new WebAssembly.Memory({ initial: 1 });
const fresh = () => { const w = new WASI({}); return [w, w.getImportObject().wasi_snapshot_preview1]; };

const [w1, i1] = fresh();
console.log("EXIT:", w1.start({ exports: { memory, _start() { i1.proc_exit(3); } } }));

const [w2] = fresh();
console.log("NORMAL:", w2.start({ exports: { memory, _start() {} } }));

// A genuine fault is not swallowed by the proc_exit unwinding.
const [w3] = fresh();
try { w3.start({ exports: { memory, _start() { throw new Error("boom"); } } }); }
catch (e) { console.log("THREW:", e.message); }
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("EXIT: 3"), "{stdout}");
    assert!(stdout.contains("NORMAL: 0"), "{stdout}");
    assert!(stdout.contains("THREW: boom"), "{stdout}");
}

#[test]
fn stdout_is_line_buffered_and_flushed_at_exit() {
    let dir = workdir("buffering");
    write(&dir, "enc.js", ENCODER_JS.as_bytes());
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
const memory = new WebAssembly.Memory({ initial: 1 });
const wasi = new WASI({});
const w = wasi.getImportObject().wasi_snapshot_preview1;

const put = (s, at) => {
  const bytes = new TextEncoder().encode(s);
  new Uint8Array(memory.buffer).set(bytes, at);
  const dv = new DataView(memory.buffer);
  dv.setUint32(0, at, true);
  dv.setUint32(4, bytes.length, true);
  w.fd_write(1, 0, 1, 8);
};

// Two partial writes join into one line; the unterminated tail is flushed when
// the program finishes rather than being dropped.
wasi.start({ exports: { memory, _start() { put("par", 100); put("tial\n", 200); put("tail", 300); } } });
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("partial"), "{stdout}");
    assert!(stdout.contains("tail"), "{stdout}");
    // "par" and "tial" must not have been emitted as separate lines.
    assert!(!stdout.contains("par\n"), "{stdout}");
}

#[test]
fn rejects_a_wasi_version_it_does_not_implement() {
    let dir = workdir("version");
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
try {
  new WASI({ version: "preview2" });
  console.log("NO THROW");
} catch (e) {
  console.log("REJECTED:", e.constructor.name);
}
console.log("DEFAULT OK:", new WASI({}) instanceof WASI);
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains("REJECTED: TypeError"), "{stdout}");
    assert!(stdout.contains("DEFAULT OK: true"), "{stdout}");
}

/// Drives the whole preview-1 filesystem surface against a real preopened
/// directory: discovery, open/read/write, seek/tell, filestat, readdir, the
/// mutating `path_*` calls, and the two refusal paths.
#[test]
fn serves_the_filesystem_from_a_preopened_directory() {
    let dir = workdir("fs");
    std::fs::create_dir_all(dir.join("data")).unwrap();
    write(&dir, "data/greet.txt", b"hello from disk");
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";

const memory = new WebAssembly.Memory({ initial: 2 });
const wasi = new WASI({ preopens: { "/sandbox": "./data" } });
wasi.start({ exports: { memory, _start() {} } });
const w = wasi.getImportObject().wasi_snapshot_preview1;

const dv = () => new DataView(memory.buffer);
const u8 = () => new Uint8Array(memory.buffer);
const enc = new TextEncoder(), dec = new TextDecoder();
const put = (s, at) => { const b = enc.encode(s); u8().set(b, at); return [at, b.length]; };
const assert = (c, m) => { if (!c) throw new Error("FAIL: " + m); };

// A guest discovers its preopens by walking fds until EBADF.
assert(w.fd_prestat_get(3, 0) === 0, "prestat fd3");
assert(dv().getUint32(4, true) === "/sandbox".length, "prestat name length");
assert(w.fd_prestat_dir_name(3, 100, 64) === 0, "prestat_dir_name");
assert(dec.decode(u8().subarray(100, 108)) === "/sandbox", "preopen name");
assert(w.fd_prestat_get(4, 0) === 8, "one past the last preopen is EBADF");

// Read an existing file.
const [p, l] = put("greet.txt", 200);
assert(w.path_open(3, 0, p, l, 0, 0n, 0n, 0, 300) === 0, "path_open for read");
const fd = dv().getUint32(300, true);
dv().setUint32(400, 500, true); dv().setUint32(404, 64, true);
assert(w.fd_read(fd, 400, 1, 408) === 0, "fd_read");
const n = dv().getUint32(408, true);
assert(dec.decode(u8().subarray(500, 500 + n)) === "hello from disk", "file contents");
assert(w.fd_close(fd) === 0, "fd_close");

// Create and write.
const [p2, l2] = put("out.txt", 600);
const CREAT = 1, TRUNC = 8, RIGHT_WRITE = 1n << 6n;
assert(w.path_open(3, 0, p2, l2, CREAT | TRUNC, RIGHT_WRITE, 0n, 0, 700) === 0, "path_open for write");
const wfd = dv().getUint32(700, true);
const [wp, wl] = put("written by wasi", 800);
dv().setUint32(900, wp, true); dv().setUint32(904, wl, true);
assert(w.fd_write(wfd, 900, 1, 908) === 0, "fd_write");
assert(dv().getUint32(908, true) === wl, "bytes written");
w.fd_close(wfd);

// Seek and tell agree on the position.
assert(w.path_open(3, 0, p2, l2, 0, 0n, 0n, 0, 1000) === 0, "reopen");
const sfd = dv().getUint32(1000, true);
assert(w.fd_seek(sfd, 8, 0, 1008) === 0, "fd_seek");
assert(dv().getBigUint64(1008, true) === 8n, "seek position");
assert(w.fd_tell(sfd, 1016) === 0, "fd_tell");
assert(dv().getBigUint64(1016, true) === 8n, "tell position");
w.fd_close(sfd);

// filestat reports size and type.
assert(w.path_filestat_get(3, 0, p2, l2, 1100) === 0, "path_filestat_get");
assert(dv().getBigUint64(1124, true) === 15n, "filestat size");
assert(u8()[1116] === 4, "filetype is regular file");

// readdir walks the packed dirent records.
assert(w.fd_readdir(3, 1200, 512, 0n, 1180) === 0, "fd_readdir");
const used = dv().getUint32(1180, true);
const names = [];
for (let off = 0; off + 24 <= used; ) {
  const len = dv().getUint32(1200 + off + 16, true);
  names.push(dec.decode(u8().subarray(1200 + off + 24, 1200 + off + 24 + len)));
  off += 24 + len;
}
assert(names.sort().join(",") === "greet.txt,out.txt", "readdir names: " + names);

// Mutating path calls.
const [dp, dl] = put("sub", 1400);
assert(w.path_create_directory(3, dp, dl) === 0, "path_create_directory");
const [np, nl] = put("renamed.txt", 1500);
assert(w.path_rename(3, p2, l2, 3, np, nl) === 0, "path_rename");
assert(w.path_unlink_file(3, np, nl) === 0, "path_unlink_file");
assert(w.path_remove_directory(3, dp, dl) === 0, "path_remove_directory");

// Climbing out of the preopen is refused, and a miss is ENOENT rather than a
// generic failure.
const [tp, tl] = put("../escape.txt", 1600);
assert(w.path_open(3, 0, tp, tl, 0, 0n, 0n, 0, 1700) === 76, "traversal is ENOTCAPABLE");
const [mp, ml] = put("nope.txt", 1800);
assert(w.path_open(3, 0, mp, ml, 0, 0n, 0n, 0, 1900) === 44, "missing file is ENOENT");

console.log("FS OK");
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("FS OK"), "{}", stdout(&out));
    // The write really landed on disk, not just in the guest's view of it.
    assert!(!dir.join("data/out.txt").exists(), "renamed then unlinked");
}

/// Without preopens a guest has no filesystem at all, however the host is
/// configured — WASI's own model, and the first of the three checks a file
/// access passes here.
#[test]
fn without_preopens_there_is_no_filesystem() {
    let dir = workdir("no-preopens");
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
const memory = new WebAssembly.Memory({ initial: 1 });
const wasi = new WASI({});
wasi.start({ exports: { memory, _start() {} } });
const w = wasi.getImportObject().wasi_snapshot_preview1;

const assert = (c, m) => { if (!c) throw new Error("FAIL: " + m); };
// No preopen occupies fd 3, so discovery stops immediately.
assert(w.fd_prestat_get(3, 0) === 8, "no preopens means EBADF at fd 3");
// And nothing resolves against a directory fd that does not exist.
new Uint8Array(memory.buffer).set(new TextEncoder().encode("x.txt"), 100);
assert(w.path_open(3, 0, 100, 5, 0, 0n, 0n, 0, 200) === 76, "path_open is ENOTCAPABLE");
console.log("NO FS OK");
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("NO FS OK"), "{}", stdout(&out));
}

/// A preopen cannot be used to reach a sibling preopen's tree: each guest path
/// is anchored to the directory fd it was resolved against.
#[test]
fn preopens_are_isolated_from_each_other() {
    let dir = workdir("isolation");
    std::fs::create_dir_all(dir.join("a")).unwrap();
    std::fs::create_dir_all(dir.join("b")).unwrap();
    write(&dir, "b/secret.txt", b"top secret");
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
const memory = new WebAssembly.Memory({ initial: 1 });
const wasi = new WASI({ preopens: { "/a": "./a", "/b": "./b" } });
wasi.start({ exports: { memory, _start() {} } });
const w = wasi.getImportObject().wasi_snapshot_preview1;

const u8 = () => new Uint8Array(memory.buffer);
const put = (s, at) => { const b = new TextEncoder().encode(s); u8().set(b, at); return [at, b.length]; };
const assert = (c, m) => { if (!c) throw new Error("FAIL: " + m); };

// fd 3 is /a, fd 4 is /b.
const [esc, escLen] = put("../b/secret.txt", 100);
assert(w.path_open(3, 0, esc, escLen, 0, 0n, 0n, 0, 300) === 76, "cannot climb into a sibling preopen");
// The same file is reachable through its own preopen.
const [ok, okLen] = put("secret.txt", 400);
assert(w.path_open(4, 0, ok, okLen, 0, 0n, 0n, 0, 500) === 0, "reachable via its own preopen");
console.log("ISOLATED OK");
"#,
    );

    let out = run(&dir, "main.js");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ISOLATED OK"), "{}", stdout(&out));
}

#[test]
fn does_not_read_the_hosts_real_environment() {
    let dir = workdir("no-ambient");
    write(
        &dir,
        "main.js",
        br#"import { WASI } from "runtime:wasi";
const memory = new WebAssembly.Memory({ initial: 1 });
// Constructed with no env at all: the host's real environment must not leak in.
const wasi = new WASI({});
wasi.start({ exports: { memory, _start() {} } });
const w = wasi.getImportObject().wasi_snapshot_preview1;
w.environ_sizes_get(0, 4);
console.log("ENVCOUNT:", new DataView(memory.buffer).getUint32(0, true));
"#,
    );

    let out = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .arg(dir.join("main.js"))
        .env("ESRUN_WASI_LEAK_CANARY", "should-not-appear")
        .output()
        .expect("failed to spawn esrun");
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // No ambient authority (D5): the environment is exactly what was passed in.
    assert!(stdout(&out).contains("ENVCOUNT: 0"), "{}", stdout(&out));
}
