//! Standard input (DECISIONS D143), through the real binary: `stdin` in
//! `runtime:process`, the web's `prompt()`/`confirm()`/`alert()`, and raw mode.
//!
//! The pipe tests run everywhere. The terminal tests drive esrun through a
//! pseudo-terminal, because what they are about — a prompt that waits for a
//! person, keypresses in raw mode, a terminal given back afterwards — only
//! exists on one.

use std::io::Write;
use std::process::{Command, Output, Stdio};

/// esrun running `code`, with `input` piped to its standard input.
fn piped(flags: &[&str], code: &str, input: &str) -> Output {
    piped_in(&std::env::temp_dir(), flags, code, input)
}

/// The same, from `dir`.
fn piped_in(dir: &std::path::Path, flags: &[&str], code: &str, input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_esrun"))
        .current_dir(dir)
        .args(flags)
        .arg(format!("-e={code}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn esrun");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait for esrun")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A line read past by one reader is the next reader's: `question()` takes one
/// line and `lines()` gets the rest, terminators stripped, `\r\n` included.
#[test]
fn question_and_lines_share_one_stream() {
    let out = piped(
        &[],
        "import { stdin } from 'runtime:process'; \
         const first = await stdin.question('Name? '); \
         const rest = []; for await (const line of stdin.lines()) rest.push(line); \
         console.log(JSON.stringify({ first, rest, end: await stdin.question() }));",
        "Ada\nsecond\r\nlast",
    );
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        r#"{"first":"Ada","rest":["second","last"],"end":null}"#
    );
    // The question is on standard error, where a redirected stdout leaves it.
    assert_eq!(stderr(&out), "Name? ");
}

/// `readable` is the bytes, unchanged.
#[test]
fn readable_is_the_raw_bytes() {
    let out = piped(
        &[],
        "import { stdin } from 'runtime:process'; \
         const bytes = await new Response(stdin.readable).bytes(); \
         console.log(bytes.length, new TextDecoder().decode(bytes) === 'a\\nb\\r\\n', stdin.isTTY);",
        "a\nb\r\n",
    );
    assert_eq!(stdout(&out).trim(), "5 true false", "{}", stderr(&out));
}

/// With nobody at a terminal the web's questions do not wait: a question in a
/// pipeline is a script that hangs.
#[test]
fn prompts_do_not_wait_on_a_pipe() {
    let out = piped(
        &[],
        "console.log(prompt('Name?'), confirm('Sure?'), alert('Hi'));",
        "yes\n",
    );
    assert_eq!(
        stdout(&out).trim(),
        "null false undefined",
        "{}",
        stderr(&out)
    );
    assert_eq!(stderr(&out), "", "nothing was asked");
}

/// Raw mode needs a terminal, and says so.
#[test]
fn raw_mode_needs_a_terminal() {
    let out = piped(
        &[],
        "import { stdin } from 'runtime:process'; \
         try { stdin.setRawMode(true); } catch (e) { console.log(e.message); }",
        "",
    );
    assert_eq!(
        stdout(&out).trim(),
        "setRawMode needs a terminal, and standard input is not one"
    );
}

/// A process has one standard input, and it is the main agent's.
#[test]
fn a_worker_cannot_read_stdin() {
    let dir = std::env::temp_dir().join(format!("esrun-stdin-worker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("worker.mjs"),
        "import { stdin } from 'runtime:process'; \
         try { await stdin.question(); } catch (e) { postMessage(e.message); }",
    )
    .expect("write worker");
    let out = piped_in(
        &dir,
        &["--allow-imports", "--allow-workers"],
        "const w = new Worker(new URL('./worker.mjs', import.meta.url), { type: 'module' }); \
         w.onmessage = (e) => { console.log(e.data); w.terminate(); };",
        "hello\n",
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        stdout(&out).trim(),
        "this host has no standard input",
        "{}",
        stderr(&out)
    );
}

#[cfg(unix)]
mod terminal {
    use std::os::fd::{AsFd, OwnedFd};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use rustix::termios::{LocalModes, tcgetattr};

    /// A pseudo-terminal: the side a person types into, and the side esrun
    /// runs on.
    fn terminal() -> (OwnedFd, OwnedFd) {
        use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
        let controller = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).expect("openpt");
        grantpt(&controller).expect("grantpt");
        unlockpt(&controller).expect("unlockpt");
        let name = ptsname(&controller, Vec::new()).expect("ptsname");
        let user = rustix::fs::open(
            name.as_c_str(),
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY,
            rustix::fs::Mode::empty(),
        )
        .expect("open the terminal");
        (controller, user)
    }

    /// esrun on the terminal's user side, for all three streams.
    fn spawn(code: &str, user: &OwnedFd) -> Child {
        let stream = || Stdio::from(user.try_clone().expect("dup"));
        Command::new(env!("CARGO_BIN_EXE_esrun"))
            .arg(format!("-e={code}"))
            .stdin(stream())
            .stdout(stream())
            .stderr(stream())
            .spawn()
            .expect("spawn esrun")
    }

    /// Everything the program writes, read on a thread so a test can type
    /// while it collects.
    fn collect(controller: &OwnedFd) -> mpsc::Receiver<Vec<u8>> {
        let reader = controller.try_clone().expect("dup");
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            // EIO once the program has exited and the terminal has no user.
            while let Ok(n) = rustix::io::read(&reader, &mut buf) {
                if n == 0 || send.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        receive
    }

    /// Output until it contains `needle`, or panics after a few seconds.
    fn wait_for(output: &mpsc::Receiver<Vec<u8>>, seen: &mut String, needle: &str) {
        while !seen.contains(needle) {
            match output.recv_timeout(Duration::from_secs(10)) {
                Ok(bytes) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                Err(_) => panic!("never saw {needle:?} in {seen:?}"),
            }
        }
    }

    fn type_in(controller: &OwnedFd, text: &str) {
        rustix::io::write(controller, text.as_bytes()).expect("type");
    }

    /// Whether the terminal is back in its ordinary line-editing, echoing mode.
    fn cooked(user: &OwnedFd) -> bool {
        let modes = tcgetattr(user.as_fd()).expect("tcgetattr").local_modes;
        modes.contains(LocalModes::ICANON | LocalModes::ECHO)
    }

    /// `prompt()` shows its default and takes it for an empty answer;
    /// `confirm()` takes a `y`. Both wait for the person at the terminal.
    #[test]
    fn prompt_and_confirm_wait_for_an_answer() {
        let (controller, user) = terminal();
        let output = collect(&controller);
        let mut child = spawn(
            "const name = prompt('Name?', 'anon'); const sure = confirm('Sure?'); \
             console.log('GOT', JSON.stringify(name), sure);",
            &user,
        );
        let mut seen = String::new();
        wait_for(&output, &mut seen, "Name? [anon] ");
        type_in(&controller, "\n");
        wait_for(&output, &mut seen, "Sure? [y/N] ");
        type_in(&controller, "y\n");
        wait_for(&output, &mut seen, "GOT \"anon\" true");
        assert!(child.wait().expect("wait").success());
    }

    /// In raw mode a key arrives as it is pressed, with no Enter — and the
    /// terminal is given back when the program ends, however it ends.
    #[test]
    fn raw_mode_reads_a_key_and_gives_the_terminal_back() {
        for ending in ["", "throw new Error('boom');", "exit(3);"] {
            let (controller, user) = terminal();
            let output = collect(&controller);
            let mut child = spawn(
                &format!(
                    "import {{ stdin, exit }} from 'runtime:process'; \
                     stdin.setRawMode(true); console.log('READY', stdin.isRaw); \
                     const {{ value }} = await stdin.readable.getReader().read(); \
                     console.log('KEY', value[0]); {ending}"
                ),
                &user,
            );
            let mut seen = String::new();
            wait_for(&output, &mut seen, "READY true");
            assert!(!cooked(&user), "raw mode was not entered");
            type_in(&controller, "x");
            wait_for(&output, &mut seen, "KEY 120");
            child.wait().expect("wait");
            assert!(
                cooked(&user),
                "left the terminal raw after {ending:?}: {seen}"
            );
        }
    }
}
