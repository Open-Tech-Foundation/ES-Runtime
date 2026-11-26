//! The op boundary: dispatch, capability gates, and what guest code cannot reach.

use super::*;

#[test]
fn sync_op_is_callable_from_js() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    rt.register_op(OpDecl::sync("add", |args| {
        let a = args.first().and_then(Value::as_number).unwrap_or(0.0);
        let b = args.get(1).and_then(Value::as_number).unwrap_or(0.0);
        Ok(Value::Number(a + b))
    }))
    .unwrap();
    assert_eq!(rt.eval("__ops.add(2, 3)").unwrap(), Value::Number(5.0));
}

#[test]
fn capability_gated_op_denies_then_allows() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    rt.register_op(
        OpDecl::sync("netcall", |_args| Ok(Value::Bool(true))).requires(Capability::Net),
    )
    .unwrap();

    // Deny-by-default: the op throws before its handler runs.
    assert!(rt.eval("__ops.netcall()").is_err());

    rt.set_capabilities(CapabilitySet::none().with(Capability::Net));
    assert_eq!(rt.eval("__ops.netcall()").unwrap(), Value::Bool(true));
}

#[test]
fn async_op_resolves_across_a_tick() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    // Pending on the first poll, ready on the second — so this op takes the
    // registration path rather than the dispatcher's eager one, which is
    // what `async_ops_settled` counts.
    rt.register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
        let mut polled = false;
        Box::pin(async move {
            std::future::poll_fn(move |cx| {
                if polled {
                    std::task::Poll::Ready(())
                } else {
                    polled = true;
                    cx.waker().wake_by_ref();
                    std::task::Poll::Pending
                }
            })
            .await;
            Ok(Value::Number(42.0))
        })
    }))
    .unwrap();

    // The op returns a pending promise; its `.then` has not run yet.
    rt.eval("globalThis.result = 0; __ops.answer().then((v) => { globalThis.result = v; });")
        .unwrap();
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(0.0));

    // One tick settles the op and runs the microtask that observes it.
    let status = rt.tick(0);
    assert_eq!(status.async_ops_settled, 1);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
    assert!(!rt.has_pending_work());
}

/// An async op whose future is ready on its first poll settles its promise
/// during the call, without ever being registered as pending work — but its
/// *reactions* still wait for the microtask checkpoint, so `await` behaves
/// exactly as it does for an op that took a trip through the loop.
///
/// This is the difference between "resolved" and "delivered", and getting it
/// wrong would run guest code inside a host op call. The ops this affects are
/// the ones that are async only in shape (`fs.exists` answers from a single
/// syscall), which were paying a full driver round trip each.
#[test]
fn an_immediately_ready_async_op_settles_without_a_loop_turn() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    rt.register_op(OpDecl::r#async("answer", |_args| -> AsyncOp {
        Box::pin(async { Ok(Value::Number(42.0)) })
    }))
    .unwrap();

    // Still not delivered synchronously: the reaction is a microtask.
    rt.eval("globalThis.result = 0; __ops.answer().then((v) => { globalThis.result = v; });")
        .unwrap();
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(0.0));

    // Nothing was ever registered, so the tick settles no pending op — and
    // the value arrives all the same.
    let status = rt.tick(0);
    assert_eq!(status.async_ops_settled, 0);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
    assert!(!rt.has_pending_work());
}

/// A rejected eager op rejects its promise, rather than resolving it or
/// throwing synchronously out of the op call.
#[test]
fn an_immediately_failing_async_op_rejects_its_promise() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    rt.register_op(OpDecl::r#async("boom", |_args| -> AsyncOp {
        Box::pin(async { Err(OpError::type_error("no")) })
    }))
    .unwrap();

    rt.eval(
        "globalThis.caught = ''; __ops.boom().catch((e) => { globalThis.caught = e.message; });",
    )
    .unwrap();
    rt.tick(0);
    assert_eq!(
        rt.eval("globalThis.caught").unwrap(),
        Value::String("no".into())
    );
}

#[test]
fn error_codes_capability_denied_then_provider_unavailable() {
    let _g = v8_guard();
    // Stable guest-facing codes (SPEC §6 Phase 13). The capability gate
    // runs before the handler, so with nothing granted the denial code
    // surfaces; with FileRead granted, the missing-provider code does.
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    let out = eval_async(
        &mut rt,
        "try { await __ops.fs_read('/x'); return 'no-throw'; } \
         catch (e) { return e.code + ':' + e.name; }",
    );
    assert_eq!(
        out,
        Value::String("ERR_CAPABILITY_DENIED:NotAllowedError".into())
    );

    let mut rt = runtime();
    rt.expose_ops_for_testing();
    rt.set_capabilities(CapabilitySet::none().with(Capability::FileRead));
    let out = eval_async(
        &mut rt,
        "try { await __ops.fs_read('/x'); return 'no-throw'; } \
         catch (e) { return e.code + ':' + e.name; }",
    );
    assert_eq!(out, Value::String("ERR_PROVIDER_UNAVAILABLE:Error".into()));
}

#[test]
fn capability_gate_survives_js_tampering() {
    // The security boundary is in Rust (OpState owns the op table + the
    // capability set), so guest JS cannot tamper its way past a gate
    // (SPEC §4 intrinsic integrity).
    let _g = v8_guard();
    let mut rt = runtime();
    rt.register_op(
        OpDecl::sync("guarded", |_args| Ok(Value::Bool(true))).requires(Capability::Net),
    )
    .unwrap();
    // Even code holding the op table — which guest code does not (D253),
    // so the test puts it back — is stopped by the gate, not by JS.
    rt.expose_ops_for_testing();
    let out = eval_async(
        &mut rt,
        "Object.prototype.granted = true; \
         globalThis.fetch = () => 'pwned'; \
         let denied = false; \
         try { __ops.guarded(); } catch (e) { denied = e instanceof Error; } \
         return `denied=${denied}`;",
    );
    assert_eq!(out, Value::String("denied=true".into()));
}

/// Guest code cannot call an op directly (D253), but it reaches every one
/// through the API that wraps it — an `Isolate`'s guest too, which holds no
/// capability and so is stopped only by each op's own check. An op that
/// requires nothing is therefore reachable by the least-trusted code this
/// runtime runs, and each one has to be a deliberate choice: a pure
/// function of its arguments, or an act on a handle the agent owns (D50).
/// This holds the set to `ungated_ops.txt`, so a new one fails here until
/// someone has looked at it and added it (D247).
#[test]
fn every_ungated_op_is_one_that_was_reviewed() {
    let _g = v8_guard();
    let rt = runtime();
    let mut ungated: Vec<String> = rt
        .engine
        .op_requirements()
        .into_iter()
        .filter(|(_, required)| required.is_empty())
        .map(|(name, _)| name)
        .collect();
    ungated.sort();
    ungated.dedup();
    let reviewed: Vec<&str> = include_str!("../ungated_ops.txt")
        .lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter(|line| !line.is_empty())
        .collect();
    let added: Vec<&String> = ungated
        .iter()
        .filter(|op| !reviewed.contains(&op.as_str()))
        .collect();
    let removed: Vec<&&str> = reviewed
        .iter()
        .filter(|op| !ungated.iter().any(|u| u == **op))
        .collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "ungated ops changed.\n\nNew, with no capability check — review each, then add it to \
         crates/runtime/src/ungated_ops.txt:\n{}\n\nNo longer ungated — remove from the list:\n{}",
        added
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        removed.iter().map(|s| **s).collect::<Vec<_>>().join("\n"),
    );
}

#[test]
fn guest_code_cannot_reach_or_replace_the_op_table() {
    // D253. Before it, `globalThis.__ops` was the table itself, its entries
    // writable: replacing `random_bytes` made `crypto.randomUUID()` all
    // zeros, and wrapping `hash_hmac` captured `runtime:hashing`'s keys.
    let _g = v8_guard();
    let mut rt = runtime();
    let source = "globalThis.visible = typeof globalThis.__ops;
         globalThis.__ops = { random_bytes: (n) => new Uint8Array(n) };
         const bytes = crypto.getRandomValues(new Uint8Array(32));
         globalThis.random = bytes.some((b) => b !== 0);
         try { await import('runtime:internal/ops'); globalThis.refused = 'imported'; }
         catch (e) { globalThis.refused = e.message; }";
    let state = run_module(&mut rt, source, MapLoader::new(&[]));
    assert!(matches!(state, ModuleEvalState::Completed), "{state:?}");
    let out = rt.eval("`${visible} ${random} ${refused}`").unwrap();
    let Value::String(out) = out else {
        panic!("{out:?}")
    };
    assert!(
        out.starts_with("undefined true ") && out.contains("unknown built-in module"),
        "{out}"
    );
}

#[test]
fn a_static_import_of_the_op_table_is_refused_too() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let err = block_on(rt.load_module_source(
        ENTRY,
        "import ops from 'runtime:internal/ops'; globalThis.got = ops;",
        MapLoader::new(&[]),
    ))
    .expect_err("a guest module must not link to the op table");
    assert!(err.to_string().contains("unknown built-in module"), "{err}");
}

/// An id is not authority (D50): an op that uses a handle checks this
/// agent minted it before anything else, so another agent's id — here, ids
/// this agent never minted — is refused as foreign rather than acted on.
/// End to end this was a worker naming its parent's resources through the
/// op table; guest code can no longer reach the table (D253), so the check
/// is held here, where a test can.
#[test]
fn every_handle_op_refuses_an_id_this_agent_did_not_mint() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    rt.expose_ops_for_testing();
    let out = eval_async(
        &mut rt,
        "const calls = { \
           http_respond: (id) => __ops.http_respond(id, 200, [], 'hijacked', null), \
           ws_upgrade: (id) => __ops.ws_upgrade(id), \
           http_request_headers: (id) => __ops.http_request_headers(id), \
           http_request_peer: (id) => __ops.http_request_peer(id), \
         }; \
         const said = []; \
         for (const [name, call] of Object.entries(calls)) { \
           for (const id of [1, 2, 424242]) { \
             try { await call(id); said.push(`${name}(${id}) acted`); } \
             catch (e) { if (e.code !== 'ERR_FOREIGN_HANDLE') said.push(`${name}(${id}) ${e.code ?? e.message}`); } \
           } \
         } \
         return said.join('; ');",
    );
    assert_eq!(out, Value::String(String::new()));

    // A child process, with a command provider to ask — the ownership
    // check comes after the provider lookup, so without one the refusal
    // would be a different one.
    let mut rt = command_runtime(TestCommands::with_stdout(&[]), &[]);
    rt.set_capabilities(CapabilitySet::all());
    rt.expose_ops_for_testing();
    let out = eval_async(
        &mut rt,
        "const said = []; \
         for (const id of [1, 2, 424242]) { \
           try { await __ops.system_kill(id, 'SIGKILL'); said.push(`killed ${id}`); } \
           catch (e) { said.push(e.code); } \
         } \
         return said.join(' ');",
    );
    assert_eq!(
        out,
        Value::String("ERR_FOREIGN_HANDLE ERR_FOREIGN_HANDLE ERR_FOREIGN_HANDLE".into())
    );
}

#[test]
fn only_a_builtin_may_import_the_op_table() {
    let _g = v8_guard();
    let mut rt = runtime();
    // An embedder's module is a built-in, so it may; a name in the
    // internal namespace cannot be registered at all.
    rt.register_module("runtime:probe", "export const kind = typeof __ops;")
        .unwrap();
    assert!(
        rt.register_module("runtime:internal/ops", "export default 1;")
            .is_err()
    );
    assert!(rt.register_module("runtime:internal/other", "").is_err());
    let state = run_module(
        &mut rt,
        "import { kind } from 'runtime:probe'; globalThis.kind = kind;",
        MapLoader::new(&[]),
    );
    assert!(matches!(state, ModuleEvalState::Completed), "{state:?}");
    assert_eq!(rt.eval("kind").unwrap(), Value::String("object".into()));
}

#[test]
fn op_dispatch_survives_prototype_pollution() {
    // Op dispatch + marshaling run in Rust, so polluting the JS primordials
    // cannot derail a host op call.
    let _g = v8_guard();
    let mut rt = runtime();
    rt.register_op(OpDecl::sync("ping", |_args| Ok(Value::Number(7.0))))
        .unwrap();
    rt.expose_ops_for_testing();
    let out = rt
        .eval(
            "Array.prototype.push = function () { throw new Error('polluted'); }; \
             Object.prototype.evil = 1; \
             __ops.ping();",
        )
        .unwrap();
    assert_eq!(out, Value::Number(7.0));
}
