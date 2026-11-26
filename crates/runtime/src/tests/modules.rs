//! ES modules: loading, semantics, dynamic `import()` and the built-ins.

use super::*;

#[test]
fn structured_clone_deep_copies_with_cycles() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const o = { a: [1, 2], m: new Map([['k', 3]]) }; o.self = o; \
         const c = structuredClone(o); \
         return c !== o && c.a[0] === 1 && c.a !== o.a && c.self === c && c.m.get('k') === 3; })()",
    );
}

/// Every `runtime:` built-in must **import** with nothing granted — the gate
/// is the op, never the import (D26, restated by D38's `--deny-all`). This
/// caught `runtime:process` calling `Env`-gated ops at module-evaluation
/// time, which made it unimportable under `--deny-env`.
#[test]
fn every_builtin_module_imports_with_no_capabilities() {
    let _g = v8_guard();
    for name in runtime_modules::NAMES {
        let engine = V8Engine::new(Limits::default()).expect("engine");
        // Providers are the *embedder's* wiring; capabilities are the
        // *guest's* authority. Only the latter is under test, so the process
        // provider is present (`runtime:process` reads the platform strings
        // through it at evaluation) and everything is denied.
        let providers = HostProviders::new(
            Arc::new(FixedClock {
                monotonic: 0,
                wall: 0,
            }),
            Arc::new(TestConsole::default()),
            Arc::new(MockNet::stub()),
            Arc::new(TestEntropy::new()),
        )
        .with_process(Arc::new(StubProcess));
        let mut rt = Runtime::new(Box::new(engine), providers).expect("runtime");
        rt.set_capabilities(CapabilitySet::none());
        let source = format!("import '{name}'; globalThis.ok = true;");
        let state = block_on(async {
            rt.load_module_source(ENTRY, &source, MapLoader::new(&[]))
                .await
                .unwrap_or_else(|e| panic!("{name} failed to import with no capabilities: {e}"));
            for _ in 0..500 {
                rt.tick(0);
                if !rt.has_pending_work() {
                    break;
                }
            }
            rt.module_eval_state()
        });
        assert!(
            matches!(state, ModuleEvalState::Completed),
            "{name} failed to evaluate with no capabilities: {state:?}"
        );
    }
}

/// The registry and the name list must not drift apart, or a module could be
/// added and silently skipped by the guard above.
#[test]
fn builtin_names_match_the_source_registry() {
    for name in runtime_modules::NAMES {
        assert!(
            runtime_modules::source(name).is_some(),
            "{name} is listed but has no baked source"
        );
    }
}

/// A host module is importable exactly like a baked one — same scheme, same
/// dedup, and (as with every `runtime:` module) no capability needed to
/// import it.
#[test]
fn host_registered_module_is_importable() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.register_module("runtime:demo", "export const answer = 42;")
        .expect("register");
    let state = run_module(
        &mut rt,
        "import { answer } from 'runtime:demo'; globalThis.result = answer;",
        MapLoader::new(&[]),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
}

/// The namespace is not open season: a host module cannot take a baked
/// module's name, because the program that imports `runtime:fs` must get
/// `runtime:fs`.
#[test]
fn host_module_cannot_shadow_a_builtin() {
    let _g = v8_guard();
    let mut rt = runtime();
    let err = rt
        .register_module("runtime:fs", "export const nope = 1;")
        .expect_err("shadowing a built-in must be refused");
    assert!(err.to_string().contains("cannot be replaced"), "{err}");
}

/// And it is a `runtime:` seam, not a general module-injection one — an
/// embedder that wants a bare specifier has a loader for that.
#[test]
fn host_module_must_be_in_the_runtime_namespace() {
    let _g = v8_guard();
    let mut rt = runtime();
    let err = rt
        .register_module("demo", "export const nope = 1;")
        .expect_err("a bare specifier must be refused");
    assert!(err.to_string().contains("runtime: namespace"), "{err}");
}

/// An unregistered `runtime:` specifier still fails at load rather than
/// resolving through the loader — `runtime:build` under `esrun` is this
/// case, and it has to be a clear failure rather than a mystery.
#[test]
fn unknown_runtime_module_fails_to_load() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let err = block_on(rt.load_module_source(ENTRY, "import 'runtime:nope';", MapLoader::new(&[])))
        .expect_err("an unknown built-in must not load");
    assert!(err.to_string().contains("unknown built-in module"), "{err}");
}

#[test]
fn module_graph_resolves_imports_across_files() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        (
            "./a.mjs",
            "import { base } from './b.mjs'; export const val = base + 1;",
        ),
        ("./b.mjs", "export const base = 41;"),
    ]);
    let state = run_module(
        &mut rt,
        "import { val } from './a.mjs'; globalThis.result = val;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
}

#[test]
fn json_module_with_attribute_parses() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./data.json", "{\"answer\": 42}")]);
    let state = run_module(
        &mut rt,
        "import data from './data.json' with { type: 'json' }; globalThis.result = data.answer;",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(42.0));
}

#[test]
fn json_module_keys_on_attribute_not_extension() {
    // No `.json` extension, but the attribute says JSON — so it parses.
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./data.conf", "{\"answer\": 7}")]);
    let state = run_module(
        &mut rt,
        "import d from './data.conf' with { type: 'json' }; globalThis.result = d.answer;",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(7.0));
}

#[test]
fn dynamic_json_import_with_attribute_parses() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./data.json", "{\"answer\": 9}")]);
    let state = run_module(
        &mut rt,
        "globalThis.result = 0; const m = await import('./data.json', { with: { type: 'json' } }); globalThis.result = m.default.answer;",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.result").unwrap(), Value::Number(9.0));
}

#[test]
fn json_file_without_attribute_is_not_transpiled() {
    // A `.json` extension alone no longer triggers JSON transpilation; without
    // the attribute the raw JSON is compiled as JS and fails (per spec).
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let loader = MapLoader::new(&[("./data.json", "{\"answer\": 1}")]);
    let result = block_on(rt.load_module_source(
        ENTRY,
        "import data from './data.json'; globalThis.x = data;",
        loader,
    ));
    assert!(
        result.is_err(),
        "a .json imported without `with {{ type: \"json\" }}` must not be transpiled"
    );
}

#[test]
fn dynamic_import_rejection_runs_catch_when_loop_would_be_idle() {
    // Regression: a dynamic import() that fails to load rejects its promise,
    // but the rejection reaction is queued as a microtask. Rejecting inline
    // during the post-tick dynamic-import drain left that microtask with no
    // checkpoint to run it once the loop went idle, so `.catch` never fired
    // (silent, exit 0). The rejection must be deferred into the tick so the
    // reaction runs — with nothing else keeping the loop alive.
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[]); // any import fails to load
    let state = run_module(
        &mut rt,
        "globalThis.caught = false; \
         import('./missing.mjs').catch(() => { globalThis.caught = true; });",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.caught").unwrap(), Value::Bool(true));
}

#[test]
fn caught_dynamic_import_eval_failure_is_not_reported_unhandled() {
    // Regression: dynamically importing a module that throws at top level and
    // catching it still reported the module's *evaluation* promise as an
    // unhandled rejection (and the CLI exited nonzero). The engine observes
    // that promise by polling and forwards its result to the import()
    // promise, so it must be marked handled — the guest's `.catch` is what
    // handles the failure.
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let loader = MapLoader::new(&[("./boom.mjs", "throw new Error('boom');")]);
    let mut unhandled: Vec<String> = Vec::new();
    block_on(async {
        rt.load_module_source(
            ENTRY,
            "globalThis.caught = false; \
             import('./boom.mjs').catch(() => { globalThis.caught = true; });",
            loader,
        )
        .await
        .expect("load module graph");
        for _ in 0..500 {
            let status = rt.tick(0);
            unhandled.extend(status.unhandled_rejections.iter().map(ToString::to_string));
            rt.process_dynamic_imports()
                .await
                .expect("process dynamic imports");
            if !rt.has_pending_work() {
                break;
            }
        }
    });
    assert_eq!(rt.eval("globalThis.caught").unwrap(), Value::Bool(true));
    assert!(
        unhandled.is_empty(),
        "caught dynamic import wrongly reported unhandled: {unhandled:?}"
    );
}

#[test]
fn reimport_of_errored_cycle_member_rejects_without_crashing() {
    // Regression: dynamically importing a member of an async (top-level
    // await) cycle whose evaluation already threw re-instantiated/re-evaluated
    // an `Errored` module, tripping a V8 CHECK and aborting the process
    // (SIGABRT) — guest-triggerable. It must instead reject with the cycle's
    // recorded evaluation error. Cycle: a→b→c→a, all async; b throws.
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        ("./a.mjs", "import './b.mjs'; import './x.mjs';"),
        (
            "./b.mjs",
            "import './c.mjs'; await Promise.resolve(0); throw new Error('boom B');",
        ),
        ("./c.mjs", "import './d.mjs'; await Promise.resolve(0);"),
        ("./d.mjs", "import './b.mjs'; await Promise.resolve(0);"),
        ("./x.mjs", "import './d.mjs'; await Promise.resolve(0);"),
    ]);
    let state = run_module(
        &mut rt,
        "globalThis.first = null; globalThis.second = null; \
         try { await import('./a.mjs'); } catch (e) { globalThis.first = e.message; } \
         try { await import('./c.mjs'); } catch (e) { globalThis.second = e.message; }",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    // Both imports reject with the cycle's recorded error, no crash.
    assert_eq!(
        rt.eval("globalThis.first").unwrap(),
        Value::String("boom B".into())
    );
    assert_eq!(
        rt.eval("globalThis.second").unwrap(),
        Value::String("boom B".into())
    );
}

#[test]
fn dynamic_import_of_syntax_error_rejects_with_syntax_error() {
    // Regression: a dynamic import() of a module that fails to compile
    // rejected with a generic `Error`, so a `.catch` checking
    // `error.name === 'SyntaxError'` (a common pattern, and what test262's
    // dynamic-import/catch tests assert) saw `'Error'`. The rejection must
    // carry the SyntaxError class.
    let _g = v8_guard();
    let mut rt = runtime();
    // `var x; function x(){}` is legal as a script but a duplicate-declaration
    // SyntaxError as a module.
    let loader = MapLoader::new(&[("./bad.mjs", "var x; function x(){}")]);
    let state = run_module(
        &mut rt,
        "globalThis.name = ''; \
         import('./bad.mjs').catch((e) => { globalThis.name = e.name; });",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.name").unwrap(),
        Value::String("SyntaxError".into())
    );
}

#[test]
fn diamond_evaluates_shared_dependency_once() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        ("./a.mjs", "import './c.mjs';"),
        ("./b.mjs", "import './c.mjs';"),
        (
            "./c.mjs",
            "globalThis.cCount = (globalThis.cCount || 0) + 1;",
        ),
    ]);
    let state = run_module(
        &mut rt,
        "import './a.mjs'; import './b.mjs';",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    // c is reachable via both a and b but compiled + evaluated exactly once.
    assert_eq!(rt.eval("globalThis.cCount").unwrap(), Value::Number(1.0));
}

#[test]
fn import_cycle_completes() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        ("./a.mjs", "import './b.mjs'; globalThis.aRan = true;"),
        ("./b.mjs", "import './a.mjs'; globalThis.bRan = true;"),
    ]);
    let state = run_module(&mut rt, "import './a.mjs';", loader.clone());
    assert_eq!(state, ModuleEvalState::Completed);
    assert_true(&mut rt, "globalThis.aRan && globalThis.bRan");
}

#[test]
fn top_level_await_settles_across_ticks() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let loader = MapLoader::new(&[]);
    block_on(rt.load_module_source(
        ENTRY,
        "await new Promise((r) => setTimeout(r, 0)); globalThis.tla = 7;",
        loader.clone(),
    ))
    .expect("load");
    // The graph is async (TLA), so it is not done before any tick runs.
    assert_eq!(rt.module_eval_state(), ModuleEvalState::Pending);
    pump_until(&mut rt, "the top-level await to complete", |rt| {
        !rt.has_pending_work()
    });
    assert_eq!(rt.module_eval_state(), ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.tla").unwrap(), Value::Number(7.0));
}

#[test]
fn import_meta_url_is_the_module_specifier() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[]);
    let state = run_module(
        &mut rt,
        "globalThis.metaUrl = import.meta.url;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.metaUrl").unwrap(),
        Value::String(ENTRY.into())
    );
}

#[test]
fn module_top_level_throw_is_failed() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[]);
    match run_module(&mut rt, "throw new Error('nope');", loader.clone()) {
        ModuleEvalState::Failed(error) => {
            assert!(error.to_string().contains("nope"), "{error}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn missing_module_is_a_load_error() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let loader = MapLoader::new(&[]); // ./gone.mjs is absent
    let err =
        block_on(rt.load_module_source(ENTRY, "import './gone.mjs';", loader.clone())).unwrap_err();
    assert!(matches!(err, Error::Loader(_)), "got {err:?}");
}

/// The loader's refusal reaches the guest with its code, not flattened to
/// text: an `import()` refused by the loader rejects with the same `code`
/// the loader gave (D141).
#[test]
fn a_loader_refusal_keeps_its_code() {
    use es_runtime_common::{ErrorCode, IntoException};
    let err = Error::Loader(es_runtime_providers::ProviderError::Coded {
        code: ErrorCode::PermissionDenied,
        message: "module \"./x.js\" is denied by the import policy".into(),
    });
    assert_eq!(err.exception_code(), Some(ErrorCode::PermissionDenied));
    assert!(
        err.to_string().starts_with("module loading failed: "),
        "{err}"
    );
}

#[test]
fn imports_denied_without_filesystem_capability() {
    let _g = v8_guard();
    let mut rt = runtime(); // deny-by-default: no FileSystem capability
    let loader = MapLoader::new(&[("./a.mjs", "export const v = 1;")]);
    let err =
        block_on(rt.load_module_source(ENTRY, "import './a.mjs';", loader.clone())).unwrap_err();
    assert!(matches!(err, Error::ImportDenied(_)), "got {err:?}");
    // Names the import that failed and where the grant is made — and stays
    // the exception a permission failure has always been.
    assert!(
        err.to_string().contains(r#"cannot import "./a.mjs""#),
        "{err}"
    );
    assert!(err.to_string().contains("--allow-imports"), "{err}");
    use es_runtime_common::IntoException;
    assert_eq!(
        err.exception_class(),
        es_runtime_common::ExceptionClass::NOT_ALLOWED
    );
    assert_eq!(
        err.exception_code(),
        es_runtime_common::Error::CapabilityDenied(Capability::FileSystem).exception_code()
    );
}

#[test]
fn self_contained_module_runs_without_capability() {
    let _g = v8_guard();
    let mut rt = runtime(); // no capabilities granted
    let loader = MapLoader::new(&[]);
    // No imports → the loader is never consulted → no capability needed.
    block_on(rt.load_module_source(ENTRY, "globalThis.ok = 5;", loader.clone())).expect("load");
    pump_until(&mut rt, "the module to evaluate", |rt| {
        !rt.has_pending_work()
    });
    assert_eq!(rt.module_eval_state(), ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.ok").unwrap(), Value::Number(5.0));
}

// ----- ES module semantics ----------------------------------------------

#[test]
fn default_export_and_import() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./greet.mjs", "export default (name) => 'hi ' + name;")]);
    let state = run_module(
        &mut rt,
        "import greet from './greet.mjs'; globalThis.greeting = greet('x');",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.greeting").unwrap(),
        Value::String("hi x".into())
    );
}

#[test]
fn namespace_import_exposes_all_exports() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./m.mjs", "export const a = 1; export const b = 2;")]);
    let state = run_module(
        &mut rt,
        "import * as ns from './m.mjs'; \
         globalThis.keys = Object.keys(ns).sort().join(','); globalThis.sum = ns.a + ns.b;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.keys").unwrap(),
        Value::String("a,b".into())
    );
    assert_eq!(rt.eval("globalThis.sum").unwrap(), Value::Number(3.0));
}

#[test]
fn module_instance_is_shared_across_importers() {
    // A module imported by two others is evaluated once and its namespace is
    // the same object on both sides (module identity).
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        ("./shared.mjs", "export const x = 1;"),
        (
            "./a.mjs",
            "import * as s from './shared.mjs'; export const sa = s;",
        ),
        (
            "./b.mjs",
            "import * as s from './shared.mjs'; export const sb = s;",
        ),
    ]);
    let state = run_module(
        &mut rt,
        "import { sa } from './a.mjs'; import { sb } from './b.mjs'; \
         globalThis.same = sa === sb;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_true(&mut rt, "globalThis.same");
}

#[test]
fn re_export_forwards_a_binding() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        ("./b.mjs", "export const val = 7;"),
        ("./a.mjs", "export { val } from './b.mjs';"),
    ]);
    let state = run_module(
        &mut rt,
        "import { val } from './a.mjs'; globalThis.reexport = val;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.reexport").unwrap(), Value::Number(7.0));
}

#[test]
fn imported_binding_is_live() {
    // A `let` export mutated by the module is observed through the importer's
    // binding (ESM live bindings, not a value copy).
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[(
        "./c.mjs",
        "export let count = 0; export function bump() { count += 1; }",
    )]);
    let state = run_module(
        &mut rt,
        "import { count, bump } from './c.mjs'; bump(); bump(); globalThis.live = count;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.live").unwrap(), Value::Number(2.0));
}

#[test]
fn dependencies_evaluate_before_dependents() {
    // Post-order: a depends on b, main on a → b, then a, then main.
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        (
            "./a.mjs",
            "import './b.mjs'; globalThis.order = (globalThis.order||'') + 'a';",
        ),
        (
            "./b.mjs",
            "globalThis.order = (globalThis.order||'') + 'b';",
        ),
    ]);
    let state = run_module(
        &mut rt,
        "import './a.mjs'; globalThis.order = (globalThis.order||'') + 'main';",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.order").unwrap(),
        Value::String("bamain".into())
    );
}

#[test]
fn cyclic_imports_resolve_via_function_hoisting() {
    // The canonical working ESM cycle: a calls into b which calls back into a
    // through hoisted function declarations.
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        (
            "./a.mjs",
            "import { getB } from './b.mjs'; \
             export function getA() { return 'A'; } \
             globalThis.cycleResult = getB();",
        ),
        (
            "./b.mjs",
            "import { getA } from './a.mjs'; \
             export function getB() { return 'B+' + getA(); }",
        ),
    ]);
    let state = run_module(&mut rt, "import './a.mjs';", loader.clone());
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(
        rt.eval("globalThis.cycleResult").unwrap(),
        Value::String("B+A".into())
    );
}

#[test]
fn duplicate_import_of_one_module_evaluates_it_once() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[(
        "./m.mjs",
        "globalThis.mCount = (globalThis.mCount || 0) + 1; export const v = 21;",
    )]);
    let state = run_module(
        &mut rt,
        "import { v } from './m.mjs'; import { v as v2 } from './m.mjs'; \
         globalThis.dup = v + v2;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.mCount").unwrap(), Value::Number(1.0));
    assert_eq!(rt.eval("globalThis.dup").unwrap(), Value::Number(42.0));
}

#[test]
fn three_level_graph_resolves() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[
        (
            "./a.mjs",
            "import { b } from './b.mjs'; export const a = b + 1;",
        ),
        (
            "./b.mjs",
            "import { c } from './c.mjs'; export const b = c + 1;",
        ),
        ("./c.mjs", "export const c = 1;"),
    ]);
    let state = run_module(
        &mut rt,
        "import { a } from './a.mjs'; globalThis.deep = a;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.deep").unwrap(), Value::Number(3.0));
}

#[test]
fn dependency_top_level_await_blocks_dependent() {
    // main must observe the dependency's TLA having completed before main's
    // own body runs.
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[(
        "./a.mjs",
        "await new Promise((r) => setTimeout(r, 0)); globalThis.depReady = true;",
    )]);
    let state = run_module(
        &mut rt,
        "import './a.mjs'; globalThis.mainSawDep = globalThis.depReady === true;",
        loader.clone(),
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_true(&mut rt, "globalThis.mainSawDep");
}

#[test]
fn throw_in_a_dependency_fails_the_graph() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./a.mjs", "throw new Error('dep boom');")]);
    match run_module(
        &mut rt,
        "import './a.mjs'; globalThis.reached = true;",
        loader.clone(),
    ) {
        ModuleEvalState::Failed(error) => {
            assert!(error.to_string().contains("dep boom"), "{error}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    // The dependent's body must not have run.
    assert_eq!(rt.eval("globalThis.reached").unwrap(), Value::Undefined);
}

#[test]
fn syntax_error_in_a_dependency_is_a_load_error() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.set_capabilities(CapabilitySet::all());
    let loader = MapLoader::new(&[("./a.mjs", "export const = ;")]);
    // The error surfaces while compiling the dependency during the load walk.
    let err =
        block_on(rt.load_module_source(ENTRY, "import './a.mjs';", loader.clone())).unwrap_err();
    assert!(matches!(err, Error::Engine(_)), "got {err:?}");
}

// ----- dynamic import() -------------------------------------------------

#[test]
fn dynamic_import_resolves_to_namespace() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./dep.mjs", "export const value = 55;")]);
    let state = run_module(
        &mut rt,
        "const m = await import('./dep.mjs'); globalThis.v = m.value;",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.v").unwrap(), Value::Number(55.0));
}

#[test]
fn dynamic_import_then_chain_without_tla() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./dep.mjs", "export const value = 9;")]);
    // The entry is synchronous; the import() resolves over later ticks.
    let state = run_module(
        &mut rt,
        "globalThis.v = 0; import('./dep.mjs').then((m) => { globalThis.v = m.value; });",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.v").unwrap(), Value::Number(9.0));
}

#[test]
fn dynamic_import_shares_instance_with_static_import() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[(
        "./shared.mjs",
        "globalThis.n = (globalThis.n || 0) + 1; export const x = 1;",
    )]);
    // Imported statically and dynamically: evaluated once, same namespace.
    let state = run_module(
        &mut rt,
        "import './shared.mjs'; const m = await import('./shared.mjs'); \
         globalThis.same = globalThis.n === 1 && m.x === 1;",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_true(&mut rt, "globalThis.same");
}

#[test]
fn dynamic_import_of_missing_module_rejects() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[]); // ./gone.mjs absent
    let state = run_module(
        &mut rt,
        "globalThis.err = ''; try { await import('./gone.mjs'); } \
         catch (e) { globalThis.err = String(e.message || e); }",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    match rt.eval("globalThis.err").unwrap() {
        Value::String(s) => assert!(s.contains("not found"), "{s}"),
        other => panic!("expected string, got {other:?}"),
    }
}

#[test]
fn dynamic_import_of_top_level_await_module() {
    let _g = v8_guard();
    let mut rt = runtime();
    let loader = MapLoader::new(&[("./tla.mjs", "export const v = await Promise.resolve(7);")]);
    let state = run_module(
        &mut rt,
        "const m = await import('./tla.mjs'); globalThis.tla = m.v;",
        loader,
    );
    assert_eq!(state, ModuleEvalState::Completed);
    assert_eq!(rt.eval("globalThis.tla").unwrap(), Value::Number(7.0));
}
