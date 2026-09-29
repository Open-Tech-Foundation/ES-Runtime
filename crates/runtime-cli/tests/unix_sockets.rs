//! End-to-end tests for Unix domain stream sockets in `runtime:net` (DECISIONS
//! D140): the flag grammar, the rule that a path is always named, and a real
//! round trip through the `esrun` binary.

use std::path::PathBuf;
use std::process::{Command, Output};

fn esrun() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command
}

fn run(flags: &[&str], code: &str) -> Output {
    esrun()
        .args(flags)
        .arg(format!("-e={code}"))
        .output()
        .expect("spawn esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A fresh directory for one test's socket, short enough for `sun_path`.
#[cfg(unix)]
fn socket_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("esrun-uds-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create socket dir");
    dir
}

/// Listens on `path`, connects to it, echoes one message, and prints what each
/// side reported — then closes, so the file should be gone afterwards.
#[cfg(unix)]
fn echo(path: &str) -> String {
    format!(
        "import {{ connect, listen }} from 'runtime:net';\
         const server = listen({{ path: '{path}' }});\
         const addr = await server.addr;\
         (async () => {{\
           for await (const s of server) {{\
             const info = await s.opened;\
             const r = s.readable.getReader();\
             const {{ value }} = await r.read();\
             const w = s.writable.getWriter();\
             await w.write(value); await w.close();\
             console.log('server', info.localAddress === addr.path, JSON.stringify(info.remoteAddress));\
             return;\
           }}\
         }})();\
         const c = connect({{ path: '{path}' }});\
         const info = await c.opened;\
         const w = c.writable.getWriter();\
         await w.write(new TextEncoder().encode('ping'));\
         const {{ value }} = await c.readable.getReader().read();\
         console.log('client', new TextDecoder().decode(value), info.remoteAddress === addr.path, info.remotePort);\
         await c.close(); await server.close();\
         console.log('addr', JSON.stringify(Object.keys(addr)));"
    )
}

#[cfg(unix)]
#[test]
fn a_named_path_round_trips_and_its_file_is_removed_on_close() {
    let dir = socket_dir("echo");
    let path = dir.join("app.sock").display().to_string();
    let out = run(
        &[
            &format!("--allow-net=unix:{path}"),
            &format!("--allow-listen=unix:{path}"),
        ],
        &echo(&path),
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("client ping true 0"), "{text}");
    assert!(text.contains("server true \"\""), "{text}");
    assert!(text.contains("addr [\"path\"]"), "{text}");
    assert!(
        !std::path::Path::new(&path).exists(),
        "close removes the socket file"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn a_bare_grant_does_not_cover_a_socket_path() {
    // `--allow-net` for the network is not the Docker socket.
    let dir = socket_dir("bare");
    let path = dir.join("app.sock").display().to_string();
    let code = format!(
        "import {{ connect, listen }} from 'runtime:net';\
         try {{ await listen({{ path: '{path}' }}).addr; console.log('listen ok'); }}\
         catch (e) {{ console.log('listen', e.code, e.message.includes('--allow-listen=unix:')); }}\
         try {{ await connect({{ path: '{path}' }}).opened; console.log('connect ok'); }}\
         catch (e) {{ console.log('connect', e.code, e.message.includes('--allow-net=unix:')); }}"
    );
    let out = run(&["--allow-net", "--allow-listen"], &code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("listen ERR_PERMISSION_DENIED true"), "{text}");
    assert!(
        text.contains("connect ERR_PERMISSION_DENIED true"),
        "{text}"
    );
    assert!(
        !std::path::Path::new(&path).exists(),
        "a refused bind creates no file"
    );

    // Without the capability at all, the op itself is refused.
    let out = run(&[], &code);
    assert!(
        stdout(&out).contains("listen ERR_CAPABILITY_DENIED"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn allow_all_covers_every_socket_path() {
    let dir = socket_dir("all");
    let path = dir.join("app.sock").display().to_string();
    let out = run(&["--allow-all"], &echo(&path));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("client ping true 0"),
        "{}",
        stdout(&out)
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_program_must_give_an_absolute_plaintext_path_alone() {
    let code = "import { connect, listen } from 'runtime:net';\
        const tries = [\
          ['relative', () => listen({ path: 'app.sock' }).addr],\
          ['with port', () => listen({ path: '/tmp/x.sock', port: 80 }).addr],\
          ['with tls', () => connect({ path: '/tmp/x.sock' }, { secureTransport: 'on' }).opened],\
          ['empty', () => connect({ path: '' }).opened],\
        ];\
        for (const [what, fn] of tries) {\
          try { await fn(); console.log(what, 'ok'); }\
          catch (e) { console.log(what, e.name, e.code ?? '-', e.message.includes('SocketError')); }\
        }";
    let out = run(&["--allow-all"], code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    // Absolute-ness is the host's check, with its own code; the rest are the
    // module's, before anything reaches the host.
    #[cfg(unix)]
    assert!(
        text.contains("relative TypeError ERR_INVALID_PATH true"),
        "{text}"
    );
    assert!(text.contains("with port TypeError - true"), "{text}");
    assert!(text.contains("with tls TypeError - true"), "{text}");
    assert!(text.contains("empty TypeError - true"), "{text}");
}

#[test]
fn a_relative_unix_entry_is_an_argument_error() {
    let out = run(&["--allow-net=unix:app.sock"], "1");
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("--allow-net"), "{err}");
    assert!(err.contains("absolute"), "{err}");
}

#[cfg(windows)]
#[test]
fn a_socket_path_is_refused_on_windows() {
    let code = "import { connect } from 'runtime:net';\
        try { await connect({ path: 'C:/tmp/app.sock' }).opened; console.log('ok'); }\
        catch (e) { console.log(e.message.includes('not supported on this platform')); }";
    let out = run(&["--allow-all"], code);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "true");
}
