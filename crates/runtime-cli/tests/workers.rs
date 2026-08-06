//! End-to-end tests for the `Worker` global.
//!
//! These drive the real `esrun` binary, so each one exercises the whole path: a
//! real OS thread, a second V8 isolate, the structured-clone bytes crossing
//! between them, and the capability narrowing in between.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esrun() -> Command {
    Command::new(env!("CARGO_BIN_EXE_esrun"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Writes a worker and a main module, runs the main module, returns its stdout.
fn run(prefix: &str, worker: &str, main: &str, flags: &[&str]) -> Output {
    write(&format!("{prefix}-worker.mjs"), worker);
    let app = write(
        &format!("{prefix}-main.mjs"),
        &main.replace("WORKER_URL", &format!("./{prefix}-worker.mjs")),
    );
    esrun().args(flags).arg(&app).output().unwrap()
}

#[test]
fn a_worker_echoes_a_message_back() {
    let out = run(
        "echo",
        r#"self.onmessage = (e) => { postMessage(`${e.data} back, from ${self.name}`); };"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), { name: "echo" });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.postMessage("there and");
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "there and back, from echo");
}

#[test]
fn a_message_crosses_as_a_real_object_graph() {
    // The point of structured clone over JSON: a Map, a Set, a Date and a cycle
    // all survive a crossing between two isolates.
    let out = run(
        "graph",
        r#"
        self.onmessage = (e) => {
          const { map, set, date, cyclic } = e.data;
          postMessage([
            map instanceof Map && map.get("k") === "v",
            set instanceof Set && set.has(7),
            date instanceof Date && date.getTime() === 1234,
            cyclic.self === cyclic,
          ].join(","));
        };
        "#,
        r#"
        const cyclic = {};
        cyclic.self = cyclic;
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.postMessage({
          map: new Map([["k", "v"]]),
          set: new Set([7]),
          date: new Date(1234),
          cyclic,
        });
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true,true,true,true");
}

#[test]
fn a_worker_that_throws_reaches_the_parents_onerror() {
    // And promptly: a worker with a receive pump never reaches quiescence, so
    // this only works because the drive reports the module's failure as soon as
    // evaluation settles rather than when the worker ends.
    let out = run(
        "boom",
        r#"throw new Error("worker blew up");"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onerror = (e) => {
          console.log("caught:", e.message.split("\n")[0]);
          e.preventDefault();
          w.terminate();
        };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("worker blew up"),
        "stdout: {}",
        stdout(&out)
    );
    // `preventDefault()` claimed it, so nothing should have been reported on
    // top of the guest's own handling.
    assert!(!stderr(&out).contains("worker blew up"), "{}", stderr(&out));
}

#[test]
fn a_live_worker_keeps_the_process_alive_and_close_ends_it() {
    // The parent's module finishes immediately; the message still arrives,
    // because the worker is live work. `close()` then lets the process exit —
    // without it this would hang.
    let out = run(
        "live",
        r#"setTimeout(() => { postMessage("late"); close(); }, 200);"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => console.log("got:", e.data);
        console.log("main done");
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "main done\ngot: late\n");
}

#[test]
fn a_worker_starts_with_nothing_and_is_granted_explicitly() {
    let out = run(
        "grant",
        r#"
        import { permissions } from "runtime:process";
        postMessage(permissions.denied.includes("net") ? "net denied" : "net granted");
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), { permissions: ["net"] });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "net granted");
}

#[test]
fn a_worker_cannot_be_granted_what_its_parent_lacks() {
    // The whole point of narrowing-only: a sandboxed program must not be able
    // to escape by spawning something less sandboxed than itself.
    let out = run(
        "narrow",
        r#"
        import { permissions } from "runtime:process";
        postMessage(permissions.denied.includes("net") ? "net denied" : "net granted");
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), { permissions: ["net"] });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        "#,
        &["--deny-net"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "net denied");
}

#[test]
fn deny_workers_refuses_the_spawn() {
    let out = run(
        "denied",
        r#"postMessage("should never run");"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onerror = (e) => { console.log("refused:", e.message); e.preventDefault(); };
        "#,
        &["--deny-workers"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("workers"),
        "expected the denial to name the permission; stdout: {}",
        stdout(&out)
    );
}

#[test]
fn a_classic_worker_is_refused_with_a_reason() {
    // This runtime evaluates every input as a module, so there is no classic
    // script path for a classic worker to use. Deno refuses them too.
    let out = run(
        "classic",
        r#"postMessage("unused");"#,
        r#"
        try {
          new Worker(new URL("WORKER_URL", import.meta.url), { type: "classic" });
        } catch (e) {
          console.log(e.constructor.name);
        }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "TypeError");
}

#[test]
fn a_workers_own_imports_load_even_though_it_was_granted_nothing() {
    // Deny-by-default would mean single-file workers only if the static graph
    // were loaded under the worker's own set. It is loaded under the parent's
    // authority instead — safe because instantiation runs no guest code.
    write(
        "helper-dep.mjs",
        r#"export const answer = "imported with no grant";"#,
    );
    let out = run(
        "helper",
        r#"
        import { answer } from "./helper-dep.mjs";
        postMessage(answer);
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.onerror = (e) => { console.log("error:", e.message); e.preventDefault(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "imported with no grant");
}

#[test]
fn an_array_buffer_transfers_rather_than_copying() {
    let out = run(
        "transfer",
        r#"
        self.onmessage = (e) => {
          const u = new Uint8Array(e.data);
          postMessage(`${u.byteLength} bytes, first=${u[0]}`);
        };
        "#,
        r#"
        const buf = new ArrayBuffer(8);
        new Uint8Array(buf)[0] = 42;
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => {
          console.log(`worker: ${e.data}`);
          console.log(`sender detached: ${buf.detached}`);
          w.terminate();
        };
        w.postMessage(buf, [buf]);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "worker: 8 bytes, first=42\nsender detached: true\n"
    );
}

#[test]
fn a_shared_array_buffer_is_one_allocation_in_both_agents() {
    // Not a copy: the worker writes and the parent reads the same slot. This is
    // what `SharedArrayBuffer` is for, and what made it useless before workers
    // existed — the backing store is handed over, where an ArrayBuffer's
    // contents would be copied into the message.
    let out = run(
        "sab",
        r#"
        self.onmessage = (e) => {
          Atomics.store(new Int32Array(e.data), 0, 99);
          postMessage(`shared=${e.data instanceof SharedArrayBuffer}`);
        };
        "#,
        r#"
        const sab = new SharedArrayBuffer(16);
        const a = new Int32Array(sab);
        Atomics.store(a, 0, 1);
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => {
          console.log(e.data);
          console.log(`parent reads ${Atomics.load(a, 0)}`);
          w.terminate();
        };
        w.postMessage(sab);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "shared=true\nparent reads 99\n");
}

#[test]
fn a_broadcast_channel_reaches_every_agent_but_never_its_sender() {
    // The spec scopes a BroadcastChannel to the agent cluster. With one agent
    // that was indistinguishable from "this isolate"; with workers it is not,
    // and a channel that reached only its own agent would be wrong rather than
    // merely limited.
    let out = run(
        "broadcast",
        r#"
        const ch = new BroadcastChannel("room");
        ch.onmessage = (e) => { postMessage(`worker heard ${e.data}`); ch.close(); };
        "#,
        r#"
        const sender = new BroadcastChannel("room");
        const peer = new BroadcastChannel("room");
        let heardBySender = false;
        sender.onmessage = () => { heardBySender = true; };
        peer.onmessage = (e) => console.log(`same-agent peer heard ${e.data}`);
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => {
          console.log(e.data);
          console.log(`sender heard itself: ${heardBySender}`);
          peer.close();
          sender.close();
          w.terminate();
        };
        setTimeout(() => sender.postMessage("it"), 150);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "same-agent peer heard it\nworker heard it\nsender heard itself: false\n"
    );
}

#[test]
fn a_message_port_transferred_into_a_worker_carries_a_private_channel() {
    // The HTML spec's composition primitive: hand a worker one end of a channel
    // and talk to it directly, rather than through the Worker object.
    let out = run(
        "port",
        r#"
        self.onmessage = (e) => {
          const port = e.data.port;
          port.onmessage = (m) => port.postMessage(`echo: ${m.data}`);
          postMessage("worker holds the port");
        };
        "#,
        r#"
        const { port1, port2 } = new MessageChannel();
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        port1.onmessage = (e) => {
          console.log(e.data);
          port1.close();
          w.terminate();
        };
        w.onmessage = (e) => {
          console.log(e.data);
          port1.postMessage("over the port");
        };
        w.postMessage({ port: port2 }, [port2]);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "worker holds the port\necho: over the port\n");
}

#[test]
fn a_readable_stream_transferred_to_a_worker_streams_rather_than_copies() {
    // A stream is not serialized — its chunks cross a MessageChannel as they
    // are produced, which is why an infinite stream is transferable at all.
    let out = run(
        "stream",
        r#"
        self.onmessage = async (e) => {
          const seen = [];
          for await (const chunk of e.data.stream) seen.push(chunk);
          postMessage(`worker read ${seen.join(",")}`);
        };
        "#,
        r#"
        const stream = new ReadableStream({
          start(c) { c.enqueue("a"); c.enqueue("b"); c.enqueue("c"); c.close(); },
        });
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.postMessage({ stream }, [stream]);
        console.log(`sender locked: ${stream.locked}`);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "sender locked: true\nworker read a,b,c\n");
}

#[test]
fn a_transferred_stream_applies_backpressure_across_agents() {
    // The `pull` half of the protocol, and the reason it exists: without it an
    // unbounded producer would serialize its whole output into the port queue,
    // which is precisely what a stream is for avoiding.
    let out = run(
        "backpressure",
        r#"
        self.onmessage = async (e) => {
          const reader = e.data.stream.getReader();
          await reader.read();
          await reader.read();
          postMessage("read 2");
        };
        "#,
        r#"
        let produced = 0;
        const stream = new ReadableStream({ pull(c) { c.enqueue(++produced); } });
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = () => setTimeout(() => {
          console.log(produced < 20 ? "bounded" : `ran away (${produced})`);
          w.terminate();
        }, 300);
        w.postMessage({ stream }, [stream]);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "bounded");
}

#[test]
fn a_message_channel_alone_does_not_keep_the_process_running() {
    // An open port is not a reason to stay alive: its peer is in this agent, so
    // with the loop otherwise idle nothing could ever post to it. Before ports
    // were host-backed this exited on its own, and it still must — otherwise
    // every program that merely opened a channel would hang.
    let out = run(
        "lifetime",
        r#"postMessage("unused");"#,
        r#"
        const { port1, port2 } = new MessageChannel();
        port1.onmessage = (e) => console.log(`got ${e.data}`);
        port2.postMessage("hi");
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "got hi\n");
}

#[test]
fn atomics_wait_blocks_in_a_worker_but_throws_on_the_main_agent() {
    // The ECMAScript agent record's [[CanBlock]]: false where the loop is
    // driven (parking it stops everything), true in a worker, which owns its
    // thread. Verified from one process so the two answers are unmistakably
    // about the agent rather than the build.
    let out = run(
        "atomics",
        r#"
        const a = new Int32Array(new SharedArrayBuffer(8));
        // Not equal, so this returns at once — proof the call was permitted,
        // not that it parked.
        postMessage(`worker: ${Atomics.wait(a, 0, 1)}`);
        "#,
        r#"
        const a = new Int32Array(new SharedArrayBuffer(8));
        try {
          Atomics.wait(a, 0, 0);
          console.log("main: did not throw");
        } catch (e) {
          console.log(`main: ${e.constructor.name}`);
        }
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "main: TypeError\nworker: not-equal\n");
}

#[test]
fn a_worker_may_start_its_own_worker() {
    // The spec allows nesting. What bounds it is the capability chain, not
    // hiding the constructor: a worker can only spawn if it holds `workers`,
    // and can only pass on what it holds, so a chain narrows and never widens.
    write("nested-inner.mjs", r#"postMessage("inner ran"); close();"#);
    let out = run(
        "nested",
        r#"
        const inner = new Worker(new URL("./nested-inner.mjs", import.meta.url), {
          permissions: ["workers", "imports"],
        });
        inner.onmessage = (e) => { postMessage(`nested: ${e.data}`); inner.terminate(); close(); };
        inner.onerror = (e) => { postMessage(`nested error: ${e.message}`); e.preventDefault(); close(); };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), {
          permissions: ["workers", "imports"],
        });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.onerror = (e) => { console.log(`err: ${e.message}`); e.preventDefault(); w.terminate(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "nested: inner ran");
}

#[test]
fn terminating_a_worker_terminates_the_workers_it_started() {
    // HTML's "terminate a worker" destroys the global scope, and that takes the
    // workers started from it. Left running, a nested worker is unreachable —
    // its parent is gone — and still holds the process open, because a live
    // worker is a reason not to exit. The test is the process exiting at all:
    // the grandchild here would otherwise keep it alive forever.
    write(
        "orphan-inner.mjs",
        r#"
        // Deliberately immortal: an onmessage handler is an outstanding
        // receive, so this agent never reaches quiescence on its own.
        self.onmessage = () => {};
        postMessage("inner up");
        "#,
    );
    let out = run(
        "orphan",
        r#"
        const inner = new Worker(new URL("./orphan-inner.mjs", import.meta.url), {
          permissions: ["workers", "imports"],
        });
        inner.onmessage = (e) => postMessage(e.data);
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), {
          permissions: ["workers", "imports"],
        });
        w.onmessage = (e) => {
          console.log(`grandchild reported: ${e.data}`);
          w.terminate();
          console.log("outer terminated");
        };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("grandchild reported: inner up"), "{}", stdout(&out));
    assert!(stdout(&out).contains("outer terminated"), "{}", stdout(&out));
}

#[test]
fn a_worker_that_finishes_takes_its_own_workers_with_it() {
    // The same rule at the end a worker reaches by itself: `close()` destroys
    // this global scope too, so its children are not left behind.
    write(
        "closing-inner.mjs",
        r#"self.onmessage = () => {}; postMessage("inner up");"#,
    );
    let out = run(
        "closing",
        r#"
        const inner = new Worker(new URL("./closing-inner.mjs", import.meta.url), {
          permissions: ["workers", "imports"],
        });
        inner.onmessage = (e) => { postMessage(e.data); close(); };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), {
          permissions: ["workers", "imports"],
        });
        w.onmessage = (e) => console.log(`reported: ${e.data}`);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("reported: inner up"), "{}", stdout(&out));
}

#[test]
fn a_worker_cannot_watch_signals() {
    // A signal is delivered to the process, and watching one suppresses the
    // default action — so a worker taking SIGTERM would decide, from a thread
    // the program may not know is running, whether the process declines to die.
    let out = run(
        "signals",
        r#"
        import { onSignal } from "runtime:process";
        try {
          onSignal("SIGINT", () => {});
          postMessage("watched");
        } catch (e) {
          postMessage("refused");
        }
        close();
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), {
          permissions: ["signals", "imports"],
        });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "refused");
}

#[test]
fn process_exit_in_a_worker_ends_only_that_worker() {
    let out = run(
        "exit",
        r#"
        import { exit } from "runtime:process";
        postMessage("worker exiting");
        exit(3);
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), {
          permissions: ["env", "imports"],
        });
        w.onmessage = (e) => console.log(e.data);
        setTimeout(() => { console.log("parent still alive"); w.terminate(); }, 300);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "worker exiting\nparent still alive\n");
}

#[test]
fn a_timeout_stops_a_worker_spinning_in_a_synchronous_loop() {
    let out = run(
        "spin",
        r#"postMessage("spinning"); while (true) {}"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => console.log(e.data);
        setTimeout(() => {}, 60000);
        "#,
        &["--timeout=1000"],
    );
    // The deadline is the point: without it this test would not finish.
    assert!(
        stderr(&out).contains("timed out"),
        "expected a timeout; stderr: {}",
        stderr(&out)
    );
}
