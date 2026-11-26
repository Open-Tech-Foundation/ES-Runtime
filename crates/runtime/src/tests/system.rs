//! `runtime:system` child processes (D37).

use super::*;

/// Runs a module against a capability set of the test's choosing (unlike
/// [`run_module`], which grants everything), and reads the answer back off
/// `globalThis.result`.
fn run_system_module(rt: &mut Runtime, caps: CapabilitySet, source: &str) -> Value {
    rt.set_capabilities(caps);
    block_on(async {
        rt.load_module_source(ENTRY, source, MapLoader::new(&[]))
            .await
            .expect("load module graph");
        for _ in 0..500 {
            rt.tick(0);
            rt.process_dynamic_imports()
                .await
                .expect("process dynamic imports");
            if !rt.has_pending_work() {
                break;
            }
        }
    });
    rt.eval("globalThis.result").unwrap()
}

#[test]
fn spawning_requires_the_run_capability() {
    let _g = v8_guard();
    let mut rt = command_runtime(TestCommands::with_stdout(&[]), &[]);
    rt.expose_ops_for_testing();
    // Everything except the capability under test, so a denial can only be
    // Run — not some other gate the op happens to trip first.
    let mut caps = CapabilitySet::all();
    caps.revoke(Capability::Run);
    rt.set_capabilities(caps);
    let out = eval_async(
        &mut rt,
        "try { await __ops.system_spawn({ program: 'echo' }); return 'no throw'; } \
         catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
}

#[test]
fn every_other_capability_together_does_not_grant_run() {
    // Run is never implied: a child process escapes every confinement this
    // runtime applies, so it must be asked for by name (D37).
    let _g = v8_guard();
    let mut rt = command_runtime(TestCommands::with_stdout(&[]), &[]);
    rt.expose_ops_for_testing();
    let mut caps = CapabilitySet::none();
    for cap in [
        Capability::Env,
        Capability::FileSystem,
        Capability::FileRead,
        Capability::FileWrite,
        Capability::Net,
        Capability::NetListen,
        Capability::Signals,
    ] {
        caps.grant(cap);
    }
    rt.set_capabilities(caps);
    let out = eval_async(
        &mut rt,
        "try { await __ops.system_spawn({ program: 'echo' }); return 'no throw'; } \
         catch (e) { return e.code; }",
    );
    assert_eq!(out, Value::String("ERR_CAPABILITY_DENIED".into()));
}

#[test]
fn a_child_gets_no_environment_unless_one_is_passed() {
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&["out"]);
    let mut rt = command_runtime(commands.clone(), &[("HOST_SECRET", "leaked")]);
    let result = run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         const out = await new Command('echo', { args: ['hi'], env: { ONLY: 'this' } }).output(); \
         globalThis.result = new TextDecoder().decode(out.stdout);",
    );
    assert_eq!(result, Value::String("out".into()));
    let spawn = commands.last_spawn();
    assert_eq!(spawn.program, "echo");
    assert_eq!(spawn.args, vec!["hi".to_string()]);
    assert_eq!(spawn.env, vec![("ONLY".to_string(), "this".to_string())]);
}

#[test]
fn inheriting_the_environment_needs_the_env_capability_too() {
    // The point of the split: Run starts a program, Env is what lets that
    // program be handed the host's environment.
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&[]);
    let mut rt = command_runtime(commands, &[("HOST_SECRET", "leaked")]);
    let result = run_system_module(
        &mut rt,
        CapabilitySet::none().with(Capability::Run),
        "import { Command } from 'runtime:system'; \
         try { new Command('echo', { inheritEnv: true }); globalThis.result = 'no throw'; } \
         catch (e) { globalThis.result = e.code; }",
    );
    assert_eq!(result, Value::String("ERR_CAPABILITY_DENIED".into()));
}

#[test]
fn inherited_environment_reaches_the_child_when_both_are_granted() {
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&[]);
    let mut rt = command_runtime(commands.clone(), &[("HOST_VAR", "value")]);
    run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         await new Command('echo', { inheritEnv: true, env: { EXTRA: 'x' } }).output(); \
         globalThis.result = 'done';",
    );
    let env = commands.last_spawn().env;
    assert!(env.contains(&("HOST_VAR".to_string(), "value".to_string())));
    assert!(env.contains(&("EXTRA".to_string(), "x".to_string())));
}

#[test]
fn a_secret_env_value_reaches_the_child_unmasked() {
    // A masked value stringifies to "[redacted]"; handing that to a child
    // would be a silent, undebuggable failure (D30 masks accidents, and
    // this is not one).
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&[]);
    let mut rt = command_runtime(commands.clone(), &[("API_TOKEN", "s3cret")]);
    run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         import { env } from 'runtime:process'; \
         await new Command('echo', { env: { API_TOKEN: env.API_TOKEN } }).output(); \
         globalThis.result = String(env.API_TOKEN);",
    );
    assert_eq!(
        rt.eval("globalThis.result").unwrap(),
        Value::String("[redacted]".into()),
        "the Secret still masks everywhere else"
    );
    assert_eq!(
        commands.last_spawn().env,
        vec![("API_TOKEN".to_string(), "s3cret".to_string())]
    );
}

#[test]
fn output_collects_both_streams_and_the_status() {
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&["one ", "two"]);
    *commands.stderr.lock().unwrap() = vec![b"warned".to_vec()];
    *commands.status.lock().unwrap() = es_runtime_providers::ChildStatus {
        success: false,
        code: Some(3),
        signal: None,
    };
    let mut rt = command_runtime(commands.clone(), &[]);
    let result = run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         const o = await new Command('x').output(); \
         const d = new TextDecoder(); \
         globalThis.result = [o.success, o.code, d.decode(o.stdout), d.decode(o.stderr)].join('|');",
    );
    assert_eq!(result, Value::String("false|3|one two|warned".into()));
    // Reaped and drained ⇒ the child's pipes are released to the host.
    assert_eq!(*commands.closed.lock().unwrap(), vec![1]);
}

#[test]
fn output_past_max_buffer_kills_the_child_and_says_so() {
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&["0123456789", "0123456789"]);
    let mut rt = command_runtime(commands.clone(), &[]);
    let result = run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         try { await new Command('x', { maxBuffer: 12 }).output(); globalThis.result = 'no throw'; } \
         catch (e) { globalThis.result = e.code; }",
    );
    assert_eq!(result, Value::String("ERR_MAX_BUFFER".into()));
    assert_eq!(
        *commands.killed.lock().unwrap(),
        vec!["SIGTERM".to_string()]
    );
}

#[test]
fn a_written_stdin_reaches_the_child_and_closes() {
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&[]);
    let mut rt = command_runtime(commands.clone(), &[]);
    run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         const child = await new Command('cat', { stdin: 'a body' }).spawn(); \
         await child.status; \
         globalThis.result = 'done';",
    );
    assert_eq!(
        String::from_utf8(commands.written.lock().unwrap().clone()).unwrap(),
        "a body"
    );
}

#[test]
fn an_unknown_kill_signal_is_a_type_error_not_a_silent_terminate() {
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&[]);
    let mut rt = command_runtime(commands.clone(), &[]);
    let result = run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         const child = await new Command('x').spawn(); \
         try { await child.kill('SIGNOPE'); globalThis.result = 'no throw'; } \
         catch (e) { globalThis.result = e.constructor.name; }",
    );
    assert_eq!(result, Value::String("TypeError".into()));
    assert!(commands.killed.lock().unwrap().is_empty());
}

#[test]
fn arguments_are_never_a_shell_command_line() {
    // There is no shell in this module: whatever is in `args` reaches the
    // child as one argument, metacharacters and all.
    let _g = v8_guard();
    let commands = TestCommands::with_stdout(&[]);
    let mut rt = command_runtime(commands.clone(), &[]);
    run_system_module(
        &mut rt,
        CapabilitySet::all(),
        "import { Command } from 'runtime:system'; \
         await new Command('echo', { args: ['a; rm -rf /', '$(id)'] }).output(); \
         globalThis.result = 'done';",
    );
    assert_eq!(
        commands.last_spawn().args,
        vec!["a; rm -rf /".to_string(), "$(id)".to_string()]
    );
}
