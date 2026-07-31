//! OS-backed [`CommandProvider`] — child processes for `runtime:system`
//! (DECISIONS D37).
//!
//! Each child is owned by a **reaper task** that holds the `Child`, applies
//! kills to it, and publishes the exit status over a `watch` channel; its piped
//! streams get the same reader/writer-task-over-channels shape the sockets use
//! (`system_net`), so a read that must wait for output makes progress on the
//! reactor rather than blocking the op loop.
//!
//! Three properties this implementation is responsible for, beyond moving bytes:
//!
//! - **No shell, ever.** `program` + `args` become an argv. Nothing is
//!   word-split, glob-expanded, or re-parsed, so a guest-supplied argument
//!   cannot become a command.
//! - **Explicit resolution.** The program is resolved to a concrete file here —
//!   against the host `PATH` for a bare name, against `cwd` for a relative path
//!   — rather than left to the platform's implicit lookup, whose rules differ
//!   between Unix and Windows and between `Command::current_dir` and the
//!   parent's own directory.
//! - **No orphans.** Every child is spawned `kill_on_drop`, so dropping the
//!   provider (or tearing the runtime down) kills what is still running.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use es_runtime_common::ErrorCode;
use es_runtime_providers::{
    BoxFuture, ChildStatus, ChildStream, CommandProvider, CommandSpec, ProviderError, Signal, Stdio,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::{mpsc, watch};

type ReadRx = mpsc::Receiver<Result<Vec<u8>, String>>;
/// `None` until the child is reaped; then the outcome, or the failure to wait.
type StatusRx = watch::Receiver<Option<Result<ChildStatus, String>>>;

/// One live (or recently exited) child: the channel ends its ops work through.
struct Slot {
    /// Sends to the child's stdin. Dropping it closes stdin (EOF).
    stdin: Option<mpsc::Sender<Vec<u8>>>,
    /// Taken during a read and put back, like a socket's; left taken at EOF.
    stdout: Option<ReadRx>,
    stderr: Option<ReadRx>,
    /// Kill requests to the reaper task, which owns the `Child`.
    kill: mpsc::UnboundedSender<Signal>,
    status: StatusRx,
}

impl Slot {
    /// Whether the child has been reaped (its status is known).
    fn exited(&self) -> bool {
        self.status.borrow().is_some()
    }
}

/// The child registry, shared with every in-flight op future (which must be
/// `'static`) without handing out the `Arc<Inner>` whose `Drop` does the
/// killing.
type Registry = Arc<Mutex<HashMap<u64, Slot>>>;

struct Inner {
    children: Registry,
    next_id: AtomicU64,
    /// Programs this provider will start. `None` ⇒ any. Matched against both
    /// the name as written and the resolved file name, so an allowlist of
    /// `["git"]` admits `git`, `/usr/bin/git`, and `git.exe` alike.
    allow: Option<HashSet<String>>,
    /// Cap on children alive at once. `None` ⇒ unlimited.
    max_children: Option<usize>,
}

/// A [`CommandProvider`] over real OS processes (tokio). Cloning is cheap and
/// shares one child registry; the **last** clone dropped kills whatever is
/// still running.
#[derive(Clone)]
pub struct SystemCommands {
    inner: Arc<Inner>,
}

impl Default for SystemCommands {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemCommands {
    /// A provider that will start any program the host user can.
    pub fn new() -> Self {
        SystemCommands::build(None, None)
    }

    /// Restricts spawning to `programs` — the policy seam for an embedder that
    /// must grant `Capability::Run` without granting a shell. A program outside
    /// the list fails to spawn, whatever the capability set says.
    #[must_use]
    pub fn with_allowlist<I, S>(self, programs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let allow = programs.into_iter().map(Into::into).collect();
        SystemCommands::build(Some(allow), self.inner.max_children)
    }

    /// Caps how many children may be alive at once — a spawn past the cap
    /// fails rather than fork-bombing the host.
    #[must_use]
    pub fn with_max_children(self, max: usize) -> Self {
        SystemCommands::build(self.inner.allow.clone(), Some(max))
    }

    /// The builders each produce a fresh registry, so they are only meaningful
    /// before anything has been spawned — which is where policy belongs.
    fn build(allow: Option<HashSet<String>>, max_children: Option<usize>) -> Self {
        SystemCommands {
            inner: Arc::new(Inner {
                children: Arc::new(Mutex::new(HashMap::new())),
                next_id: AtomicU64::new(0),
                allow,
                max_children,
            }),
        }
    }

    fn id(&self) -> u64 {
        self.inner.next_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn registry(&self) -> Registry {
        self.inner.children.clone()
    }
}

/// Kills anything still running when the registry goes away. `kill_on_drop`
/// covers the tasks; this covers the case where the tasks outlive the provider
/// (a child whose slot was never closed) by asking each reaper to kill first.
impl Drop for Inner {
    fn drop(&mut self) {
        let children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        for slot in children.values() {
            if !slot.exited() {
                let _ = slot.kill.send(Signal::Kill);
            }
        }
    }
}

/// Resolves `program` to a concrete file (see the module note on why this is
/// not left to the platform).
///
/// A name containing a path separator is a path — absolute, or relative to
/// `cwd` (the *child's* directory, which is the reading a caller expects and
/// not what `std::process` guarantees). A bare name is looked up on the host
/// `PATH`, which is host authority: the guest's `env` describes the child's
/// environment, never where this runtime goes looking for executables.
fn resolve_program(program: &str, cwd: Option<&Path>) -> Result<PathBuf, ProviderError> {
    if program.is_empty() || program.contains('\0') {
        return Err(coded(ErrorCode::NotFound, "the program name is empty"));
    }
    let has_separator = program.contains('/') || (cfg!(windows) && program.contains('\\'));
    if has_separator {
        let path = Path::new(program);
        let full = if path.is_absolute() {
            path.to_path_buf()
        } else {
            match cwd {
                Some(dir) => dir.join(path),
                None => std::env::current_dir()
                    .map_err(|e| ProviderError::from_io("cannot read the working directory", &e))?
                    .join(path),
            }
        };
        return usable(&full).ok_or_else(|| not_found(program));
    }

    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        if let Some(found) = usable_with_extensions(&dir.join(program)) {
            return Ok(found);
        }
    }
    Err(not_found(program))
}

/// The candidate itself, plus (on Windows) each `PATHEXT` spelling of it —
/// `git` finding `git.exe` is the platform's rule, and implementing it here is
/// why a bare name resolves the same way on every OS.
fn usable_with_extensions(candidate: &Path) -> Option<PathBuf> {
    if let Some(found) = usable(candidate) {
        return Some(found);
    }
    if !cfg!(windows) {
        return None;
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string());
    for ext in pathext.split(';').filter(|e| !e.is_empty()) {
        let ext = ext.strip_prefix('.').unwrap_or(ext);
        let mut with_ext = candidate.as_os_str().to_os_string();
        with_ext.push(".");
        with_ext.push(ext);
        if let Some(found) = usable(Path::new(&with_ext)) {
            return Some(found);
        }
    }
    None
}

/// `Some(path)` if this is a file that can actually be executed. On Unix that
/// means the executable bit — a readable-but-not-executable file is not a
/// program, and skipping it lets the `PATH` search keep looking.
fn usable(candidate: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(candidate).ok()?;
    if !meta.is_file() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return None;
        }
    }
    Some(candidate.to_path_buf())
}

/// Windows cannot execute a `.bat`/`.cmd` directly — only `cmd.exe` can, and
/// handing a batch file plus guest-supplied arguments to the command
/// interpreter is precisely the injection this module refuses to enable
/// (CVE-2024-27980). Refusing with an explanation beats silently spawning a
/// shell.
#[cfg(windows)]
fn reject_batch_files(resolved: &Path) -> Result<(), ProviderError> {
    let ext = resolved
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if ext == "bat" || ext == "cmd" {
        return Err(ProviderError::Other(format!(
            "{} is a batch file: running one requires the command interpreter, \
             which this runtime does not spawn. Invoke the underlying executable instead.",
            resolved.display()
        )));
    }
    Ok(())
}

#[cfg(not(windows))]
fn reject_batch_files(_resolved: &Path) -> Result<(), ProviderError> {
    Ok(())
}

fn not_found(program: &str) -> ProviderError {
    coded(
        ErrorCode::NotFound,
        &format!("program not found: {program}"),
    )
}

fn coded(code: ErrorCode, message: &str) -> ProviderError {
    ProviderError::Coded {
        code,
        message: message.to_string(),
    }
}

fn gone(id: u64) -> ProviderError {
    coded(ErrorCode::NotFound, &format!("no such child process: {id}"))
}

fn to_stdio(mode: Stdio) -> std::process::Stdio {
    match mode {
        Stdio::Piped => std::process::Stdio::piped(),
        Stdio::Inherit => std::process::Stdio::inherit(),
        Stdio::Null => std::process::Stdio::null(),
    }
}

/// Drains one of the child's output pipes into a bounded channel. Bounded, so a
/// guest that stops reading eventually stops the child rather than buffering
/// its output without limit.
fn spawn_reader<R>(mut pipe: R) -> ReadRx
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>(8);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut buf).await {
                Ok(0) => break, // EOF — dropping tx signals it
                Ok(n) => {
                    if tx.send(Ok(buf[..n].to_vec())).await.is_err() {
                        break; // consumer gone
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });
    rx
}

/// Feeds the child's stdin from a channel; closing the channel closes stdin.
fn spawn_writer<W>(mut pipe: W) -> mpsc::Sender<Vec<u8>>
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(8);
    tokio::spawn(async move {
        while let Some(data) = rx.recv().await {
            if pipe.write_all(&data).await.is_err() {
                break; // the child closed its end (or died)
            }
        }
        let _ = pipe.shutdown().await; // sender dropped ⇒ EOF for the child
    });
    tx
}

/// Sends `signal` to a running child. On Unix any signal can be delivered; on
/// other platforms there are no signals to deliver, so every request becomes
/// the one thing the OS does offer — termination. That is also how Node and
/// Deno behave on Windows.
#[cfg(unix)]
fn signal_child(child: &mut tokio::process::Child, signal: Signal) {
    use rustix::process::{Pid, Signal as Sig, kill_process};
    let raw = match signal {
        Signal::Int => Sig::INT,
        Signal::Term => Sig::TERM,
        Signal::Hup => Sig::HUP,
        Signal::Usr1 => Sig::USR1,
        Signal::Usr2 => Sig::USR2,
        Signal::Quit => Sig::QUIT,
        Signal::Kill => Sig::KILL,
        // A Windows console event: nothing to send here, and killing on a
        // request the caller spelled `SIGBREAK` would be a surprise.
        Signal::Break => return,
    };
    // No id ⇒ already reaped, and the kill is a no-op by contract.
    if let Some(pid) = child.id().and_then(|pid| Pid::from_raw(pid as i32)) {
        let _ = kill_process(pid, raw);
    }
}

#[cfg(not(unix))]
fn signal_child(child: &mut tokio::process::Child, _signal: Signal) {
    let _ = child.start_kill();
}

/// The name of the signal that killed a child, for [`ChildStatus::signal`].
#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<String> {
    use rustix::process::Signal as Sig;
    use std::os::unix::process::ExitStatusExt;
    let raw = status.signal()?;
    // The well-known names, resolved through the platform's own numbering
    // (SIGUSR1 is 10 on Linux and 30 on macOS). Anything else keeps its number
    // rather than being guessed at.
    let known = [
        (Sig::HUP, "SIGHUP"),
        (Sig::INT, "SIGINT"),
        (Sig::QUIT, "SIGQUIT"),
        (Sig::ILL, "SIGILL"),
        (Sig::ABORT, "SIGABRT"),
        (Sig::FPE, "SIGFPE"),
        (Sig::KILL, "SIGKILL"),
        (Sig::SEGV, "SIGSEGV"),
        (Sig::PIPE, "SIGPIPE"),
        (Sig::ALARM, "SIGALRM"),
        (Sig::TERM, "SIGTERM"),
        (Sig::USR1, "SIGUSR1"),
        (Sig::USR2, "SIGUSR2"),
    ];
    Some(
        known
            .iter()
            .find(|(sig, _)| sig.as_raw() == raw)
            .map(|(_, name)| (*name).to_string())
            .unwrap_or_else(|| format!("SIG{raw}")),
    )
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<String> {
    None
}

fn to_status(status: std::process::ExitStatus) -> ChildStatus {
    ChildStatus {
        success: status.success(),
        code: status.code(),
        signal: exit_signal(&status),
    }
}

impl CommandProvider for SystemCommands {
    fn spawn(&self, spec: CommandSpec) -> BoxFuture<Result<(u64, u32), ProviderError>> {
        let this = self.clone();
        Box::pin(async move {
            if let Some(max) = this.inner.max_children {
                let live = this
                    .inner
                    .children
                    .lock()
                    .unwrap()
                    .values()
                    .filter(|slot| !slot.exited())
                    .count();
                if live >= max {
                    return Err(ProviderError::Other(format!(
                        "too many child processes: {live} already running (limit {max})"
                    )));
                }
            }

            let cwd = spec.cwd.as_ref().map(PathBuf::from);
            let resolved = resolve_program(&spec.program, cwd.as_deref())?;
            reject_batch_files(&resolved)?;
            if let Some(allow) = &this.inner.allow {
                let file_name = resolved
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                if !allow.contains(&spec.program) && !allow.contains(file_name) {
                    return Err(coded(
                        ErrorCode::PermissionDenied,
                        &format!("{} is not an allowed program", spec.program),
                    ));
                }
            }

            let mut command = Command::new(&resolved);
            command
                .args(&spec.args)
                // The environment is exactly what the guest passed — never the
                // host's, unless the guest read it (through the Env-gated ops)
                // and passed it along.
                .env_clear()
                .envs(spec.env.iter().map(|(k, v)| (k, v)))
                .stdin(to_stdio(spec.stdin))
                .stdout(to_stdio(spec.stdout))
                .stderr(to_stdio(spec.stderr))
                // A child outliving the runtime that started it is a leak, not
                // a feature.
                .kill_on_drop(true);
            if let Some(dir) = &cwd {
                command.current_dir(dir);
            }

            let mut child = command.spawn().map_err(|e| {
                ProviderError::from_io(format!("cannot spawn {}", resolved.display()), &e)
            })?;
            let pid = child.id().unwrap_or_default();

            let stdin = child.stdin.take().map(spawn_writer);
            let stdout = child.stdout.take().map(spawn_reader);
            let stderr = child.stderr.take().map(spawn_reader);

            let (status_tx, status_rx) = watch::channel(None);
            let (kill_tx, mut kill_rx) = mpsc::unbounded_channel::<Signal>();
            // The reaper owns the child from here: it is the only place that
            // holds a `Child`, so a kill and a wait can never contend for it.
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        exit = child.wait() => {
                            let _ = status_tx.send(Some(exit.map(to_status).map_err(|e| e.to_string())));
                            return;
                        }
                        Some(signal) = kill_rx.recv() => signal_child(&mut child, signal),
                    }
                }
            });

            let id = this.id();
            this.inner.children.lock().unwrap().insert(
                id,
                Slot {
                    stdin,
                    stdout,
                    stderr,
                    kill: kill_tx,
                    status: status_rx,
                },
            );
            Ok((id, pid))
        })
    }

    fn read(
        &self,
        id: u64,
        stream: ChildStream,
    ) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>> {
        let children = self.registry();
        Box::pin(async move {
            let taken = {
                let mut map = children.lock().unwrap();
                let slot = map.get_mut(&id).ok_or_else(|| gone(id))?;
                match stream {
                    ChildStream::Stdout => slot.stdout.take(),
                    ChildStream::Stderr => slot.stderr.take(),
                }
            };
            let mut rx = match taken {
                Some(rx) => rx,
                None => return Ok(None), // not piped, or already at EOF
            };
            match rx.recv().await {
                Some(Ok(buf)) => {
                    if let Some(slot) = children.lock().unwrap().get_mut(&id) {
                        match stream {
                            ChildStream::Stdout => slot.stdout = Some(rx),
                            ChildStream::Stderr => slot.stderr = Some(rx),
                        }
                    }
                    Ok(Some(buf))
                }
                Some(Err(e)) => Err(ProviderError::Other(e)),
                None => Ok(None), // reader task ended — leave it taken
            }
        })
    }

    fn write(&self, id: u64, data: Vec<u8>) -> BoxFuture<Result<(), ProviderError>> {
        let children = self.registry();
        Box::pin(async move {
            let tx = children
                .lock()
                .unwrap()
                .get(&id)
                .and_then(|slot| slot.stdin.clone());
            match tx {
                Some(tx) => tx
                    .send(data)
                    .await
                    .map_err(|_| ProviderError::Other("the child's stdin is closed".into())),
                None => Err(ProviderError::Other(
                    "the child's stdin is closed (or was not piped)".into(),
                )),
            }
        })
    }

    fn close_stdin(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let children = self.registry();
        Box::pin(async move {
            // Dropping the sender ends the writer task, which closes the pipe:
            // the child sees EOF on stdin.
            if let Some(slot) = children.lock().unwrap().get_mut(&id) {
                slot.stdin = None;
            }
            Ok(())
        })
    }

    fn wait(&self, id: u64) -> BoxFuture<Result<ChildStatus, ProviderError>> {
        let children = self.registry();
        Box::pin(async move {
            let mut status = children
                .lock()
                .unwrap()
                .get(&id)
                .map(|slot| slot.status.clone())
                .ok_or_else(|| gone(id))?;
            loop {
                // Cloned out of the borrow before any await — a watch borrow is
                // a live lock on the channel.
                let current = status.borrow().clone();
                if let Some(result) = current {
                    return result.map_err(|e| {
                        ProviderError::Other(format!("cannot wait for child {id}: {e}"))
                    });
                }
                status.changed().await.map_err(|_| {
                    ProviderError::Other(format!("child {id} was released while waiting"))
                })?;
            }
        })
    }

    fn kill(&self, id: u64, signal: Signal) -> BoxFuture<Result<(), ProviderError>> {
        let children = self.registry();
        Box::pin(async move {
            let map = children.lock().unwrap();
            let slot = map.get(&id).ok_or_else(|| gone(id))?;
            // Already exited: signalling is a no-op, not an error. The caller
            // cannot rule out the race, so it must not have to handle it.
            if !slot.exited() {
                let _ = slot.kill.send(signal);
            }
            Ok(())
        })
    }

    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>> {
        let children = self.registry();
        Box::pin(async move {
            // Dropping the slot drops the kill sender and both readers; the
            // reaper task then sees its channel close and, if the child is
            // still running, `kill_on_drop` finishes it when the task ends.
            if let Some(slot) = children.lock().unwrap().remove(&id)
                && !slot.exited()
            {
                let _ = slot.kill.send(Signal::Kill);
            }
            Ok(())
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn spec(program: &str, args: &[&str]) -> CommandSpec {
        CommandSpec {
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            stdout: Stdio::Piped,
            stderr: Stdio::Piped,
            ..CommandSpec::default()
        }
    }

    async fn collect(commands: &SystemCommands, id: u64, stream: ChildStream) -> String {
        let mut out = Vec::new();
        while let Some(chunk) = commands.read(id, stream).await.unwrap() {
            out.extend_from_slice(&chunk);
        }
        String::from_utf8(out).unwrap()
    }

    #[tokio::test]
    async fn runs_a_program_and_reports_its_output_and_status() {
        let commands = SystemCommands::new();
        let (id, pid) = commands.spawn(spec("echo", &["hello"])).await.unwrap();
        assert!(pid > 0);
        assert_eq!(collect(&commands, id, ChildStream::Stdout).await, "hello\n");
        let status = commands.wait(id).await.unwrap();
        assert!(status.success);
        assert_eq!(status.code, Some(0));
        assert_eq!(status.signal, None);
    }

    #[tokio::test]
    async fn a_failing_program_reports_its_exit_code() {
        let commands = SystemCommands::new();
        let (id, _) = commands.spawn(spec("false", &[])).await.unwrap();
        let status = commands.wait(id).await.unwrap();
        assert!(!status.success);
        assert_eq!(status.code, Some(1));
    }

    #[tokio::test]
    async fn the_environment_is_exactly_what_the_spec_carries() {
        // The host's own environment must not reach the child: the guest passes
        // what the child gets, or the child gets nothing (D37). PATH stands in
        // for "something the parent definitely has" — and its absence from the
        // child also shows that program resolution used the *host's* PATH.
        assert!(std::env::var_os("PATH").is_some(), "test needs a PATH");
        let commands = SystemCommands::new();
        let mut s = spec("env", &[]);
        s.env = vec![("ONLY".to_string(), "this".to_string())];
        let (id, _) = commands.spawn(s).await.unwrap();
        let out = collect(&commands, id, ChildStream::Stdout).await;
        assert_eq!(out.trim(), "ONLY=this", "the child's whole environment");
    }

    #[tokio::test]
    async fn stdin_is_piped_and_closing_it_is_eof() {
        let commands = SystemCommands::new();
        let mut s = spec("cat", &[]);
        s.stdin = Stdio::Piped;
        let (id, _) = commands.spawn(s).await.unwrap();
        commands.write(id, b"streamed".to_vec()).await.unwrap();
        commands.close_stdin(id).await.unwrap();
        assert_eq!(
            collect(&commands, id, ChildStream::Stdout).await,
            "streamed"
        );
        assert!(commands.wait(id).await.unwrap().success);
    }

    #[tokio::test]
    async fn stderr_is_a_separate_stream() {
        let commands = SystemCommands::new();
        let (id, _) = commands
            .spawn(spec("sh", &["-c", "echo out; echo err >&2"]))
            .await
            .unwrap();
        assert_eq!(collect(&commands, id, ChildStream::Stdout).await, "out\n");
        assert_eq!(collect(&commands, id, ChildStream::Stderr).await, "err\n");
    }

    #[tokio::test]
    async fn kill_terminates_and_reports_the_signal() {
        let commands = SystemCommands::new();
        let (id, _) = commands.spawn(spec("sleep", &["30"])).await.unwrap();
        commands.kill(id, Signal::Term).await.unwrap();
        let status = commands.wait(id).await.unwrap();
        assert!(!status.success);
        assert_eq!(status.signal.as_deref(), Some("SIGTERM"));
        // Killing again, after the exit, is a no-op rather than an error.
        commands.kill(id, Signal::Kill).await.unwrap();
    }

    #[tokio::test]
    async fn a_missing_program_is_not_found_before_anything_starts() {
        let commands = SystemCommands::new();
        let err = commands
            .spawn(spec("definitely-not-a-real-program", &[]))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::NotFound));
    }

    #[tokio::test]
    async fn arguments_are_never_interpreted_by_a_shell() {
        // The classic injection: the argument would run `id` under a shell.
        let commands = SystemCommands::new();
        let (id, _) = commands
            .spawn(spec("echo", &["hello; id > /tmp/pwned"]))
            .await
            .unwrap();
        assert_eq!(
            collect(&commands, id, ChildStream::Stdout).await,
            "hello; id > /tmp/pwned\n"
        );
    }

    #[tokio::test]
    async fn the_allowlist_is_the_last_word() {
        let commands = SystemCommands::new().with_allowlist(["echo"]);
        assert!(commands.spawn(spec("echo", &["fine"])).await.is_ok());
        let err = commands.spawn(spec("cat", &[])).await.unwrap_err();
        assert_eq!(err.code(), Some(ErrorCode::PermissionDenied));
        // The absolute path to an allowed program is still that program.
        assert!(commands.spawn(spec("/bin/echo", &["fine"])).await.is_ok());
    }

    #[tokio::test]
    async fn max_children_bounds_what_is_alive_at_once() {
        let commands = SystemCommands::new().with_max_children(1);
        let (first, _) = commands.spawn(spec("sleep", &["30"])).await.unwrap();
        assert!(commands.spawn(spec("sleep", &["30"])).await.is_err());
        commands.kill(first, Signal::Kill).await.unwrap();
        commands.wait(first).await.unwrap();
        // The slot no longer counts once the child has exited.
        assert!(commands.spawn(spec("echo", &["ok"])).await.is_ok());
    }

    #[tokio::test]
    async fn cwd_applies_to_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let commands = SystemCommands::new();
        let mut s = spec("pwd", &[]);
        s.cwd = Some(dir.path().to_string_lossy().into_owned());
        let (id, _) = commands.spawn(s).await.unwrap();
        let out = collect(&commands, id, ChildStream::Stdout).await;
        // Compared through the real path: macOS temp dirs are symlinked.
        let expected = std::fs::canonicalize(dir.path()).unwrap();
        assert_eq!(
            std::fs::canonicalize(out.trim()).unwrap(),
            expected,
            "child ran in {out}"
        );
    }

    #[tokio::test]
    async fn wait_is_repeatable_and_close_releases_the_child() {
        let commands = SystemCommands::new();
        let (id, _) = commands.spawn(spec("echo", &["done"])).await.unwrap();
        let first = commands.wait(id).await.unwrap();
        let second = commands.wait(id).await.unwrap();
        assert_eq!(first.code, second.code);
        commands.close(id).await.unwrap();
        commands.close(id).await.unwrap(); // idempotent
        // Released: the id no longer names anything.
        assert!(commands.wait(id).await.is_err());
    }
}
