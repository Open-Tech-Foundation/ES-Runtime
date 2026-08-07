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
fn messages_arrive_in_the_order_they_were_posted() {
    // A dedicated worker's implicit port is entangled when the constructor
    // returns, so HTML has no window in which posting order can be lost. Ours
    // is the only spawn that is asynchronous — the entry is read and the agent
    // started over two awaits — so messages posted meanwhile are queued in the
    // `Worker` and flushed once there is an id to send them to.
    //
    // Flushing them one await at a time reordered the queue against anything
    // posted from a microtask: the id was already set, so a later `postMessage`
    // took the direct path and overtook messages still waiting to be drained.
    // Node, Deno and Bun all deliver 0..19 here.
    let out = run(
        "order",
        r#"
        const seen = [];
        self.onmessage = (e) => {
          if (e.data === "end") { postMessage(seen.join(",")); return; }
          seen.push(e.data);
        };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        // Posted before the spawn resolves: these queue in the `Worker`.
        for (let i = 0; i < 5; i++) w.postMessage(i);
        // Posted from microtasks that run *while* the queue is being flushed.
        queueMicrotask(() => { for (let i = 5; i < 10; i++) w.postMessage(i); });
        Promise.resolve().then(() => {}).then(() => {}).then(() => {
          for (let i = 10; i < 15; i++) w.postMessage(i);
        });
        // And once the worker is long since running.
        setTimeout(() => {
          for (let i = 15; i < 20; i++) w.postMessage(i);
          w.postMessage("end");
        }, 300);
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let expected: Vec<String> = (0..20).map(|i| i.to_string()).collect();
    assert_eq!(stdout(&out).trim(), expected.join(","));
}

#[test]
fn a_message_crosses_as_a_real_object_graph() {
    // The point of structured clone over JSON: a Map, a Set, a Date and a cycle
    // all survive a crossing between two isolates.
    //
    // The Blob is here for a different reason. It is a *host* object — V8's
    // serializer has no representation for one, so it crosses through the codec
    // registered beside its own definition — and it is the only type in this
    // message whose contents live outside the value graph.
    let out = run(
        "graph",
        r#"
        self.onmessage = async (e) => {
          const { map, set, date, cyclic, blob } = e.data;
          postMessage([
            map instanceof Map && map.get("k") === "v",
            set instanceof Set && set.has(7),
            date instanceof Date && date.getTime() === 1234,
            cyclic.self === cyclic,
            blob instanceof Blob && blob.type === "text/plain" && (await blob.text()) === "hi",
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
          blob: new Blob(["hi"], { type: "text/plain" }),
        });
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true,true,true,true,true");
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
fn a_worker_failure_carries_its_class_message_and_location() {
    // What a supervisor reads before it decides anything: which error, what it
    // said, and where. The failure crosses a thread boundary in pieces, so the
    // `error` here is necessarily a rebuilt object — but it is rebuilt as the
    // class it was thrown as, with the worker's own stack, and the location
    // fields are filled the way `filename`/`lineno`/`colno` always promised.
    //
    // The peers each give half of this: Node hands over a real reconstructed
    // `Error` but no location fields at all (its `worker.on("error")` passes an
    // `Error`, not an `ErrorEvent`); Deno fills the location fields but leaves
    // `e.error` null; Bun leaves both empty and puts the whole formatted stack
    // in `message`.
    let out = run(
        "located",
        r#"
        function inner() { throw new RangeError("out of range"); }
        self.onmessage = () => inner();
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onerror = (e) => {
          console.log(JSON.stringify({
            message: e.message,
            base: e.filename.split("/").pop(),
            lineno: e.lineno,
            colno: e.colno,
            name: e.error.name,
            isRange: e.error instanceof RangeError,
            topFrame: e.error.stack.split("\n")[1].trim().startsWith("at inner"),
          }));
          e.preventDefault();
          w.terminate();
        };
        w.postMessage("go");
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    // Column 34 is the `new RangeError` in the worker source above; line 2 is
    // the `function inner()` line, since the source starts with a newline.
    assert!(
        out.contains(
            r#"{"message":"out of range","base":"located-worker.mjs","lineno":2,"colno":34,"#
        ),
        "stdout: {out}"
    );
    assert!(
        out.contains(r#""name":"RangeError","isRange":true,"topFrame":true}"#),
        "stdout: {out}"
    );
}

#[test]
fn an_error_in_a_running_worker_reaches_the_parent_at_once_and_ends_it() {
    // The failure that matters to a supervisor: not a worker that fails to
    // start, but one that has been serving and throws on a job. It used to be
    // collected into the drive's outcome and reported only when the worker
    // ended — so a parent that terminated the worker never heard about it at
    // all, and one that waited heard about it far too late to retry anything.
    //
    // Ending the worker is the other half of the signal: an `error` on a
    // `Worker` now means "this one is gone", which is the single fact a pool
    // restarting on failure needs. Node, Deno and Bun all end it here too.
    let out = run(
        "runtime-error",
        r#"
        self.onmessage = (e) => {
          if (e.data === "bad") throw new TypeError("job failed");
          postMessage("handled " + e.data);
        };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        const seen = [];
        w.onmessage = (e) => seen.push(e.data);
        w.onerror = (e) => {
          console.log("error:", e.error.name, "|", e.message);
          console.log("instanceof:", e.error instanceof TypeError);
          e.preventDefault();
        };
        w.postMessage("first");
        setTimeout(() => w.postMessage("bad"), 100);
        setTimeout(() => w.postMessage("after the error"), 300);
        setTimeout(() => {
          console.log("handled:", JSON.stringify(seen));
          w.terminate();
        }, 600);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    assert!(
        out.contains("error: TypeError | job failed"),
        "the parent should hear about it while the worker is still the one \
         running the job; stdout: {out}"
    );
    // Rebuilt as the class it was thrown as, so a supervisor can branch on it
    // rather than match a substring.
    assert!(out.contains("instanceof: true"), "stdout: {out}");
    // Ended by the error, so the message posted afterwards was never handled —
    // which is what makes an `error` on the parent mean "this worker is gone".
    assert!(
        out.contains(r#"handled: ["handled first"]"#),
        "stdout: {out}"
    );
}

#[test]
fn an_unhandled_rejection_in_a_running_worker_reaches_the_parent_at_once() {
    // The same rule, by the other route in: a rejection nothing took
    // responsibility for is a failure the worker's author did not handle, so it
    // is reported and it ends the agent. Node, Deno and Bun agree — Node exits
    // the thread with code 1.
    let out = run(
        "runtime-rejection",
        r#"
        self.onmessage = () => { Promise.reject(new RangeError("nobody caught me")); };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => console.log("still serving:", e.data);
        w.onerror = (e) => {
          console.log("error:", e.error.name, "|", e.message);
          e.preventDefault();
        };
        w.postMessage("go");
        setTimeout(() => w.postMessage("are you there"), 300);
        setTimeout(() => { console.log("done"); w.terminate(); }, 600);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    // The reason arrives as itself, not re-worded: a rejection whose reason is
    // a `RangeError` is a `RangeError` on the parent's side too.
    assert!(
        out.contains("error: RangeError | nobody caught me"),
        "stdout: {out}"
    );
    assert!(!out.contains("still serving"), "stdout: {out}");
}

#[test]
fn a_worker_that_claims_its_own_error_is_neither_reported_nor_ended() {
    // The escape hatch, and the reason ending the worker costs nothing: a
    // worker that takes responsibility for its own failures says so in its own
    // source, with the same `preventDefault()` that keeps an error off the
    // console on any other agent. Claimed means the parent is never told and
    // the agent carries on — which is how a worker absorbs a bad job without
    // being recycled for it.
    let out = run(
        "claimed",
        r#"
        self.addEventListener("error", (e) => {
          postMessage("absorbed: " + e.message);
          e.preventDefault();
        });
        self.onmessage = (e) => {
          if (e.data === "bad") throw new TypeError("job failed");
          postMessage("handled " + e.data);
        };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => console.log(e.data);
        w.onerror = (e) => { console.log("PROPAGATED (it should not have)"); e.preventDefault(); };
        w.postMessage("bad");
        setTimeout(() => w.postMessage("the next job"), 200);
        setTimeout(() => w.terminate(), 500);
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    assert!(out.contains("absorbed: job failed"), "{out}");
    assert!(
        out.contains("handled the next job"),
        "a claimed failure must leave the worker running; stdout: {out}"
    );
    assert!(!out.contains("PROPAGATED"), "{out}");
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
fn a_worker_can_be_handed_an_environment_instead_of_the_capability() {
    // Attenuation, the same move `permissions` makes, applied to data: the
    // parent narrows what it can already read and hands over the result. No
    // `env` capability is involved, because nothing is being granted — reading
    // what someone handed you is not an authority.
    write(
        "env-worker.mjs",
        r#"
        import { env, permissions, unmask } from "runtime:process";
        postMessage(JSON.stringify({
          capability: permissions.has("env"),
          keys: Object.keys(env).sort(),
          token: `${env.API_TOKEN ?? ""}`,
          real: env.API_TOKEN ? unmask(env.API_TOKEN) : null,
        }));
        "#,
    );
    let app = write(
        "env-main.mjs",
        r#"
        const w = new Worker(new URL("./env-worker.mjs", import.meta.url), {
          permissions: [],
          env: { API_TOKEN: "sk-handed", MODE: "worker" },
        });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.onerror = (e) => { console.log(`err: ${e.message}`); e.preventDefault(); w.terminate(); };
        "#,
    );
    let out = esrun()
        .arg(&app)
        .env("API_TOKEN", "sk-from-host")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.contains(r#""capability":false"#), "{stdout}");
    assert!(
        stdout.contains(r#""keys":["API_TOKEN","MODE"]"#),
        "{stdout}"
    );
    // The host's own API_TOKEN is not what it got.
    assert!(stdout.contains(r#""real":"sk-handed""#), "{stdout}");
    // And a secret-looking name is re-masked on arrival, by the same
    // convention the parent's environment follows.
    assert!(stdout.contains(r#""token":"[redacted]""#), "{stdout}");
}

#[test]
fn a_handed_environment_wins_over_the_hosts() {
    // A worker holding `env` *and* handed one reads what it was handed: being
    // allowed to read the host environment is not a reason to ignore the
    // narrower thing its parent chose.
    write(
        "envwin-worker.mjs",
        r#"
        import { env } from "runtime:process";
        postMessage(Object.keys(env).join(","));
        "#,
    );
    let app = write(
        "envwin-main.mjs",
        r#"
        const w = new Worker(new URL("./envwin-worker.mjs", import.meta.url), {
          permissions: ["env"],
          env: { ONLY: "this" },
        });
        w.onmessage = (e) => { console.log(`saw:${e.data}`); w.terminate(); };
        "#,
    );
    let out = esrun()
        .arg(&app)
        .env("SECRET_FROM_HOST", "leak")
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "saw:ONLY");
}

#[test]
fn an_empty_handed_environment_is_an_empty_environment() {
    write(
        "envnone-worker.mjs",
        r#"
        import { env } from "runtime:process";
        postMessage(`${Object.keys(env).length}`);
        "#,
    );
    let app = write(
        "envnone-main.mjs",
        r#"
        const w = new Worker(new URL("./envnone-worker.mjs", import.meta.url), { env: {} });
        w.onmessage = (e) => { console.log(`count:${e.data}`); w.terminate(); };
        w.onerror = (e) => { console.log(`err: ${e.message}`); e.preventDefault(); w.terminate(); };
        "#,
    );
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "count:0");
}

#[test]
fn a_malformed_env_option_throws_from_the_constructor() {
    // A bad argument throws where the bad argument was written. Only a worker
    // that fails to *start* reports asynchronously through `onerror`.
    let app = write(
        "envbad-main.mjs",
        r#"
        try {
          new Worker(new URL("./envbad-main.mjs", import.meta.url), { env: "nope" });
        } catch (e) {
          console.log(`${e.constructor.name}: ${e.message}`);
        }
        "#,
    );
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let stdout = stdout(&out);
    assert!(stdout.starts_with("TypeError:"), "{stdout}");
    assert!(
        stdout.contains(r#""env" must be "inherit" or an object"#),
        "{stdout}"
    );
    // It names what it got, so the fix is obvious from the message alone.
    assert!(stdout.contains(r#""nope""#), "{stdout}");
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
fn a_worker_is_held_to_the_memory_it_was_given() {
    // Node is the only peer with a per-worker memory ceiling at all
    // (`resourceLimits.maxOldGenerationSizeMb`); Deno and Bun have none, so a
    // runaway job takes the whole process with it. Here it takes one agent, and
    // the parent is told which agent and against what limit.
    let out = run(
        "hungry",
        r#"
        self.onmessage = () => {
          const held = [];
          for (;;) held.push(new Array(100000).fill(held.length));
        };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), { memory: 64 });
        w.onerror = (e) => {
          console.log("failed:", e.error.name, "|", e.message);
          e.preventDefault();
          w.terminate();
        };
        w.postMessage("go");
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains(
            "failed: ERR_WORKER_OUT_OF_MEMORY | worker terminated: it reached its 64MB memory limit"
        ),
        "stdout: {}",
        stdout(&out)
    );
}

#[test]
fn a_worker_cannot_be_given_more_memory_than_its_parent_has() {
    // The same rule `permissions` follows: a spawn narrows, never widens. A
    // ceiling a child could raise would not be one, since anything holding
    // `workers` could step over it by starting an agent to do the work in.
    let out = run(
        "greedy",
        r#"postMessage("should never run");"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), { memory: 4096 });
        w.onerror = (e) => { console.log("refused:", e.message); e.preventDefault(); };
        "#,
        &["--max-heap=256"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains(
            "a worker cannot be given more memory than the agent starting it: \
             asked for 4096MB, this agent's own limit is 256MB"
        ),
        "stdout: {}",
        stdout(&out)
    );
}

#[test]
fn a_worker_inherits_the_ceiling_the_process_was_given() {
    // The point of `--max-heap`: one number bounds the process however many
    // agents it grows. Before this, a worker took a fixed 256 MiB regardless —
    // so a tightened runtime was escaped simply by doing the work in a worker.
    let out = run(
        "inheriting",
        r#"
        self.onmessage = () => {
          const held = [];
          for (;;) held.push(new Array(100000).fill(held.length));
        };
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onerror = (e) => { console.log("failed:", e.message); e.preventDefault(); w.terminate(); };
        w.postMessage("go");
        "#,
        &["--max-heap=64"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("it reached its 64MB memory limit"),
        "stdout: {}",
        stdout(&out)
    );
}

#[test]
fn a_bad_memory_option_throws_from_the_constructor() {
    // A malformed option is a bad argument, so it belongs to the call that made
    // it — not to `onerror`, which is for a worker that failed to *start*.
    let out = run(
        "badmem",
        r#"postMessage("should never run");"#,
        r#"
        try {
          new Worker(new URL("WORKER_URL", import.meta.url), { memory: "64MB" });
        } catch (e) {
          console.log(e.constructor.name + ":", e.message);
        }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains(
            r#"TypeError: Worker option "memory" must be a positive whole number of megabytes"#
        ),
        "stdout: {}",
        stdout(&out)
    );
}

#[test]
fn granting_only_workers_says_what_else_is_needed() {
    // Starting a worker means reading its entry module, so it takes `imports`
    // as well — Node wants `--allow-fs-read` alongside `--allow-worker` for the
    // same reason, and Deno wants `--allow-read`. Deno is the one that says
    // which flag fixes it, and a refusal naming a permission the author never
    // mentioned is otherwise a dead end.
    let out = run(
        "twogrants",
        r#"postMessage("should never run");"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onerror = (e) => {
          console.log("refused:", e.message);
          console.log("code:", e.error.name);
          e.preventDefault();
        };
        "#,
        &["--deny-all", "--allow-workers"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    assert!(
        out.contains(r#"reading its module needs the "imports" permission — add --allow-imports"#),
        "stdout: {out}"
    );
    // Context only: the refusal is still the host's, unchanged where a program
    // might branch on it.
    assert!(
        out.contains("(capability denied: FileSystem)"),
        "stdout: {out}"
    );
    assert!(out.contains("code: NotAllowedError"), "stdout: {out}");
}

#[test]
fn workers_and_imports_together_can_spawn() {
    // The other half of the same claim, and the command DECISIONS.md D49 should
    // have carried from the start.
    let out = run(
        "twogrants-ok",
        r#"postMessage("worker ran");"#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.onerror = (e) => { console.log("refused:", e.message); e.preventDefault(); };
        "#,
        &["--deny-all", "--allow-workers", "--allow-imports"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "worker ran");
}

#[test]
fn a_workers_dynamic_import_says_where_the_grant_is_made() {
    // The asymmetry is deliberate and worth knowing about: a worker's *static*
    // graph is resolved by its parent before the worker exists — literal
    // specifiers, in source the parent already read, instantiated with no guest
    // code running — while `import()` computes its specifier at runtime and is
    // therefore a read-and-execute primitive on the worker's own authority.
    //
    // So `import` works where `import()` does not, and the refusal has to say
    // where the grant is made: in the parent, at the spawn. `--allow-imports`
    // cannot help — it is the flag for the agent that is not the one refused.
    write("dynimport-dep.mjs", "export const answer = 42;");
    let out = run(
        "dynimport",
        r#"
        import { answer } from "./dynimport-dep.mjs";
        postMessage("static ok: " + answer);
        try {
          const m = await import("./dynimport-dep.mjs");
          postMessage("dynamic ok: " + m.answer);
        } catch (e) {
          postMessage("dynamic failed: " + e.message);
          postMessage(e.name + "|" + e.code);
        }
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url));
        let seen = 0;
        w.onmessage = (e) => { console.log(e.data); if (++seen === 3) w.terminate(); };
        w.onerror = (e) => { console.log("err:", e.message); e.preventDefault(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    assert!(out.contains("static ok: 42"), "stdout: {out}");
    assert!(
        out.contains(r#"cannot import "./dynimport-dep.mjs": this worker was not granted"#),
        "stdout: {out}"
    );
    // The remedy is the point: it names the spawn, not a flag.
    assert!(
        out.contains(r#"grant it at the spawn, new Worker(url, { permissions: ["imports"] })"#),
        "stdout: {out}"
    );
    // And it is still a permission failure to catch, not a plain `Error` to
    // substring-match.
    assert!(
        out.contains("NotAllowedError|ERR_CAPABILITY_DENIED"),
        "stdout: {out}"
    );
}

#[test]
fn a_worker_granted_imports_can_import_dynamically() {
    write("dynimport-ok-dep.mjs", "export const answer = 42;");
    let out = run(
        "dynimport-ok",
        r#"
        const m = await import("./dynimport-ok-dep.mjs");
        postMessage("dynamic ok: " + m.answer);
        "#,
        r#"
        const w = new Worker(new URL("WORKER_URL", import.meta.url), {
          permissions: ["imports"],
        });
        w.onmessage = (e) => { console.log(e.data); w.terminate(); };
        w.onerror = (e) => { console.log("err:", e.message); e.preventDefault(); };
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "dynamic ok: 42");
}

#[test]
fn an_unknown_permission_name_throws_rather_than_being_dropped() {
    // Dropping it fails closed, which sounds harmless until the worker takes
    // the degraded path forever and the denial surfaces three layers from the
    // typo. `permissions.has()` already refuses to answer `false` for a name it
    // does not know, for exactly this reason; the constructor now agrees.
    //
    // Deno accepts `deno: { permissions: { nett: true } }` silently.
    let out = run(
        "badperm",
        r#"postMessage("should never run");"#,
        r#"
        for (const value of [["nett"], "net", 7]) {
          try {
            new Worker(new URL("WORKER_URL", import.meta.url), { permissions: value });
          } catch (e) {
            console.log(e.constructor.name + ":", e.message);
          }
        }
        "#,
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    assert!(
        out.contains(
            r#"TypeError: unknown Worker permission "nett" — expected one of: read, write, imports, net, listen, env, run, signals, workers"#
        ),
        "stdout: {out}"
    );
    // A bare string is the likelier slip, and was ignored outright.
    assert!(
        out.contains(r#"TypeError: Worker option "permissions" must be "inherit" or an array of permission names, not "net""#),
        "stdout: {out}"
    );
    assert!(out.contains(r#"names, not 7"#), "stdout: {out}");
}

#[test]
fn permissions_inherit_gives_the_parents_set_and_no_more() {
    // `"inherit"` mirrors `env`'s spelling, but only as a *value*: omitting the
    // option still grants nothing, which is the whole of D48. And it is a
    // ceiling rather than an escape — under `--deny-net` an inheriting worker
    // is denied net too, because the host intersects whatever was asked for
    // with the spawning agent's own set.
    let out = run(
        "inherit",
        r#"
        import { permissions } from "runtime:process";
        postMessage(permissions.denied.join(","));
        "#,
        r#"
        for (const value of [undefined, "inherit", ["net"]]) {
          const options = value === undefined ? {} : { permissions: value };
          const w = new Worker(new URL("WORKER_URL", import.meta.url), options);
          await new Promise((done) => {
            w.onmessage = (e) => {
              console.log(JSON.stringify(value ?? "omitted") + " -> [" + e.data + "]");
              w.terminate();
              done();
            };
            w.onerror = (e) => { console.log("err:", e.message); e.preventDefault(); done(); };
          });
        }
        "#,
        &["--deny-net"],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = stdout(&out);
    // Omitted: everything denied, unchanged.
    assert!(
        out.contains(r#""omitted" -> [read,write,imports,net,listen,env,run,signals,workers]"#),
        "stdout: {out}"
    );
    // Inherit: exactly what the parent lacks, and nothing more.
    assert!(out.contains(r#""inherit" -> [net]"#), "stdout: {out}");
    // Asking for what the parent does not hold still yields nothing.
    assert!(
        out.contains(r#"["net"] -> [read,write,imports,net,listen,env,run,signals,workers]"#),
        "stdout: {out}"
    );
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
    assert!(
        stdout(&out).contains("grandchild reported: inner up"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("outer terminated"),
        "{}",
        stdout(&out)
    );
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
    assert!(
        stdout(&out).contains("reported: inner up"),
        "{}",
        stdout(&out)
    );
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
