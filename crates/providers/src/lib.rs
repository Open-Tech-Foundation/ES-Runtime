//! I/O provider traits — the integration seam (ARCHITECTURE.md §6, DECISIONS.md
//! D5).
//!
//! The runtime owns no I/O and carries no ambient authority: time, entropy,
//! timers, and offloaded work all arrive through the traits defined here. This
//! crate holds **only the trait definitions** — concrete implementations live in
//! `default-providers` (tokio-backed, for standalone use) or, later, in Layer B.
//!
//! Because clock and entropy are providers, a run is **fully reproducible** under
//! a deterministic provider set (DECISIONS.md D5): the same inputs and the same
//! providers yield the same outputs.
//!
//! Phase 3 defines [`Clock`], [`Entropy`], [`Timers`], and [`TaskSpawner`].
//! `NetTransport` and `FileSystem` arrive with their consuming APIs (fetch, FS).

// Providers are pure trait definitions; no `unsafe` (ARCHITECTURE.md §7).
#![forbid(unsafe_code)]

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use es_runtime_common::{ErrorCode, ExceptionClass, IntoException, UncaughtError};

/// A heap-allocated, `Send` future returned by async provider methods.
///
/// Providers must be usable from a driver that may move work across threads, so
/// the future is `Send`. `'static` because provider futures outlive the call.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// An error raised by a provider.
///
/// Provider calls return typed errors (ARCHITECTURE.md §6); this is the shared
/// shape. It maps to a JS exception via [`IntoException`] so the runtime can
/// surface it uniformly (DECISIONS.md D12).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// The entropy source failed to produce randomness.
    #[error("entropy source failed: {0}")]
    Entropy(String),

    /// The operation was cancelled before completing.
    #[error("provider operation cancelled")]
    Cancelled,

    /// Any other provider failure.
    #[error("provider error: {0}")]
    Other(String),

    /// A failure carrying a stable guest-facing [`ErrorCode`] (SPEC §6 Phase
    /// 13): the message stays human prose, the code is what guest JS branches
    /// on (`e.code === "ERR_NOT_FOUND"`). Providers should prefer this (or
    /// [`ProviderError::from_io`]) over [`Other`](ProviderError::Other) when a
    /// stable classification exists.
    #[error("{message}")]
    Coded {
        /// The stable classification surfaced to JS as the exception's `code`.
        code: ErrorCode,
        /// The human-readable message (free to change between releases).
        message: String,
    },
}

impl ProviderError {
    /// A [`Coded`](ProviderError::Coded) error classified from an
    /// [`std::io::Error`]'s kind, with `context` (typically the path or
    /// address) prefixed to the message.
    pub fn from_io(context: impl std::fmt::Display, e: &std::io::Error) -> ProviderError {
        ProviderError::Coded {
            code: ErrorCode::from_io_kind(e.kind()),
            message: format!("{context}: {e}"),
        }
    }

    /// The stable guest-facing code, if this error carries one.
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            ProviderError::Entropy(_) => Some(ErrorCode::Entropy),
            ProviderError::Cancelled => Some(ErrorCode::Cancelled),
            ProviderError::Other(_) => None,
            ProviderError::Coded { code, .. } => Some(*code),
        }
    }
}

impl IntoException for ProviderError {
    fn exception_class(&self) -> ExceptionClass {
        match self {
            // A failed CSPRNG is an environment/operation failure, not a type
            // error; surface as a generic Error (Web crypto would use
            // OperationError, a DOMException — added with the prelude).
            ProviderError::Entropy(_) => ExceptionClass::Error,
            ProviderError::Cancelled => ExceptionClass::NOT_ALLOWED,
            ProviderError::Other(_) | ProviderError::Coded { .. } => ExceptionClass::Error,
        }
    }

    fn exception_code(&self) -> Option<ErrorCode> {
        self.code()
    }
}

/// A source of time (ARCHITECTURE.md §6).
///
/// Backs `performance.now`, timers, and wall-clock reads. Splitting monotonic
/// from wall time keeps timer math immune to wall-clock jumps. A deterministic
/// `Clock` makes timer-driven runs reproducible.
pub trait Clock: Send + Sync {
    /// Milliseconds from an arbitrary fixed epoch, never decreasing. Used for
    /// timer deadlines and elapsed-time measurement.
    fn monotonic_ms(&self) -> u64;

    /// Microseconds from the same epoch as [`monotonic_ms`](Self::monotonic_ms),
    /// never decreasing. Backs `performance.now()`'s sub-millisecond precision.
    /// The default derives from `monotonic_ms` (whole-ms resolution), so
    /// deterministic/test clocks stay correct without overriding.
    fn monotonic_micros(&self) -> u64 {
        self.monotonic_ms() * 1_000
    }

    /// Milliseconds since the Unix epoch (UTC) — the basis for `Date.now`.
    fn wall_ms(&self) -> u64;
}

/// A cryptographically secure source of randomness (ARCHITECTURE.md §6).
///
/// Backs `crypto.getRandomValues` and `crypto.randomUUID`. A deterministic
/// (seeded, non-secure) implementation is permitted **only** for reproducible
/// tests, never for production.
pub trait Entropy: Send + Sync {
    /// Fills `dest` entirely with random bytes, or returns
    /// [`ProviderError::Entropy`] if the source failed.
    fn fill(&self, dest: &mut [u8]) -> Result<(), ProviderError>;
}

/// A scheduler of delayed wakeups (ARCHITECTURE.md §6).
///
/// Backs the `setTimeout`/`setInterval` family and lets the driver park until
/// the next timer is due instead of busy-polling.
pub trait Timers: Send + Sync {
    /// A future that completes no sooner than `delay_ms` from now.
    fn sleep(&self, delay_ms: u64) -> BoxFuture<()>;
}

/// An offloader of blocking work (ARCHITECTURE.md §6).
///
/// Lets an op run a blocking closure off the driving thread at the provider's
/// discretion. Results flow through state the closure captures (e.g. a channel);
/// the returned future completes when the work has run.
pub trait TaskSpawner: Send + Sync {
    /// Runs `work` off the calling thread; the future resolves once it finishes.
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send + 'static>) -> BoxFuture<()>;
}

/// The severity of a `console` message, mirroring the method that produced it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConsoleLevel {
    /// `console.debug`.
    Debug,
    /// `console.info`.
    Info,
    /// `console.log`.
    Log,
    /// `console.warn`.
    Warn,
    /// `console.error`.
    Error,
}

/// A stream of body byte-chunks, as produced/consumed by [`NetTransport`].
///
/// Modeled as a `futures` [`Stream`](futures_core::Stream) so the response body
/// can be delivered incrementally and fed to a JS `ReadableStream` (streaming
/// downloads). Each item is a chunk or a [`ProviderError`]; the stream ends at
/// `None`.
pub type ByteStream =
    Pin<Box<dyn futures_core::Stream<Item = Result<Vec<u8>, ProviderError>> + Send>>;

/// An outbound HTTP request handed to a [`NetTransport`].
pub struct HttpRequest {
    /// The HTTP method (`GET`, `POST`, …).
    pub method: String,
    /// The absolute request URL.
    pub url: String,
    /// Header name/value pairs, in order.
    pub headers: Vec<(String, String)>,
    /// The request body — absent, fully buffered, or streamed incrementally.
    pub body: RequestBody,
    /// What to do with a redirect response.
    pub redirect: RedirectMode,
}

/// What a [`NetTransport`] does when the server answers with a redirect.
///
/// These are the two behaviours a *transport* can implement; Fetch's third mode,
/// `"error"`, is a rule about the resulting response rather than about the wire,
/// so the runtime asks for [`Manual`](RedirectMode::Manual) and rejects the
/// `fetch` promise itself. That keeps redirect *policy* in one place instead of
/// obliging every embedder transport to reimplement it identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RedirectMode {
    /// Follow redirects transparently, up to the Fetch specification's cap of
    /// 20; exceeding it is a [`ProviderError`]. The default.
    #[default]
    Follow,
    /// Return the redirect response itself, unfollowed.
    Manual,
}

/// The body of an outbound [`HttpRequest`].
///
/// A buffered body ([`Bytes`](RequestBody::Bytes)) is the common case and lets
/// the transport set `Content-Length`. A [`Stream`](RequestBody::Stream) is sent
/// with chunked transfer-encoding without ever materializing the whole payload —
/// the runtime feeds it incrementally from a guest `ReadableStream`, so a large
/// upload streams with bounded memory.
pub enum RequestBody {
    /// No request body.
    Empty,
    /// A fully-buffered body.
    Bytes(Vec<u8>),
    /// A body streamed as byte-chunks (chunked transfer-encoding). Ends at the
    /// stream's `None`; an item `Err` aborts the in-flight request.
    Stream(ByteStream),
}

impl RequestBody {
    /// Whether there is no body (`Empty`).
    pub fn is_empty(&self) -> bool {
        matches!(self, RequestBody::Empty)
    }
}

/// The response a [`NetTransport`] returns: metadata available immediately, body
/// streamed.
pub struct HttpResponse {
    /// The HTTP status code.
    pub status: u16,
    /// The status reason phrase (e.g. `"OK"`).
    pub status_text: String,
    /// The final URL after any redirects.
    pub url: String,
    /// Whether at least one redirect was followed to arrive here. Always `false`
    /// under [`RedirectMode::Manual`], which follows nothing.
    pub redirected: bool,
    /// Response header name/value pairs, in order.
    pub headers: Vec<(String, String)>,
    /// The response body, streamed as byte-chunks.
    pub body: ByteStream,
    /// Header fields that arrived **after** the body, or `None` if the
    /// transport does not surface them.
    ///
    /// Trailers are on the wire only once the body is over, so this resolves
    /// when [`body`](Self::body) has been read to its end — or when it is
    /// dropped, which yields none rather than waiting for a body nobody is
    /// reading. A response with no trailers resolves to an empty list.
    pub trailers: Option<BoxFuture<Vec<(String, String)>>>,
}

/// Outbound HTTP for `fetch` (ARCHITECTURE.md §6, SPEC §2.9).
///
/// The runtime routes all networking through this trait; it never opens a socket
/// itself (no ambient authority). A `fetch` op is **capability-checked**
/// (`Capability::Net`) before this is ever called.
pub trait NetTransport: Send + Sync {
    /// Sends `request` and resolves to the response once its headers arrive; the
    /// body then streams via [`HttpResponse::body`].
    fn fetch(&self, request: HttpRequest) -> BoxFuture<Result<HttpResponse, ProviderError>>;
}

/// Resolves and loads ES module sources (ARCHITECTURE.md §6, SPEC §2.1).
///
/// `runtime` walks the import graph through this: for each module it
/// [`resolve`](Self::resolve)s a specifier to a canonical id, then
/// [`load`](Self::load)s that id's source. Because V8 resolves the graph
/// synchronously, the whole graph is loaded *before* instantiation — so loading
/// is async here but resolution is pure.
///
/// Loading is **capability-checked** by `runtime` before this is ever called: a
/// file-backed loader requires `Capability::FileSystem`. An embedder that grants
/// no module capability supplies no loader, and any `import` then fails cleanly.
pub trait ModuleLoader: Send + Sync {
    /// Resolves `specifier` relative to `referrer` into a canonical module id
    /// (the string later passed to [`load`](Self::load) and exposed as
    /// `import.meta.url`).
    ///
    /// Async because resolution may touch the host — e.g. a `node_modules`
    /// walk that stats files and reads `package.json`. A pure path/URL loader
    /// just returns a ready future. `referrer` is the canonical id of the
    /// importing module, or `""` for an entry point (resolve against the
    /// loader's base, e.g. the working dir).
    fn resolve(&self, specifier: &str, referrer: &str) -> BoxFuture<Result<String, ProviderError>>;

    /// Resolves **synchronously**, or reports that this loader cannot.
    ///
    /// This exists for `import.meta.resolve`, which the language defines as
    /// returning a string rather than a promise, so there is nowhere to await.
    /// A loader whose resolution is blocking-capable (a filesystem walk) answers
    /// `Some`; one whose modules come from the network or a database — where
    /// synchronous resolution is impossible, not merely slow — keeps the default
    /// `None`, and the guest gets a `TypeError` naming the specifier instead of
    /// a URL that was never resolved.
    ///
    /// An implementation **must** agree with [`resolve`](Self::resolve): the same
    /// specifier and referrer must yield the same id, through the same
    /// confinement checks. Returning an id here that `resolve` would refuse hands
    /// the guest a URL that imports differently, which is worse than refusing.
    fn resolve_sync(
        &self,
        specifier: &str,
        referrer: &str,
    ) -> Option<Result<String, ProviderError>> {
        let _ = (specifier, referrer);
        None
    }

    /// Loads the module at a canonical id (as returned by
    /// [`resolve`](Self::resolve)) as either source text or WebAssembly bytes.
    fn load(&self, specifier: &str) -> BoxFuture<Result<ModuleSource, ProviderError>>;
}

/// What a [`ModuleLoader`] returned for a module id.
///
/// Text and WebAssembly are separate variants because a `.wasm` file is binary
/// and has no UTF-8 reading: the runtime compiles it and joins it to the graph
/// through the WebAssembly ES-module integration, rather than parsing it as
/// source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleSource {
    /// UTF-8 source text — JavaScript, or JSON when the import carries
    /// `with { type: "json" }`.
    Text(String),
    /// A WebAssembly binary (`.wasm`).
    Wasm(Vec<u8>),
}

impl ModuleSource {
    /// The source text, or `None` for a WebAssembly module.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            ModuleSource::Text(s) => Some(s),
            ModuleSource::Wasm(_) => None,
        }
    }
}

impl From<String> for ModuleSource {
    fn from(text: String) -> Self {
        ModuleSource::Text(text)
    }
}

/// An OS signal the runtime can observe — or send to a child process.
///
/// A closed set rather than a raw number: the runtime never delivers a signal
/// whose default action it cannot sensibly suppress, and a fixed set is what
/// lets the same names mean the same thing on every platform the CLI ships for.
///
/// Two of the variants are **send-only**: `SIGKILL` cannot be caught at all and
/// `SIGQUIT` is not something this runtime offers to intercept, so neither
/// appears in [`Signals::available`] and [`Signals::watch`] rejects them. They
/// exist because [`CommandProvider::kill`] must be able to escalate past a
/// `SIGTERM` a child chose to ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Signal {
    /// Interactive interrupt — Ctrl+C (`SIGINT`; Windows Ctrl+C).
    Int,
    /// Polite termination request — what an orchestrator sends first
    /// (`SIGTERM`). Not available on Windows.
    Term,
    /// Controlling terminal hung up; conventionally "reload your config"
    /// (`SIGHUP`). Not available on Windows.
    Hup,
    /// User-defined (`SIGUSR1`). Not available on Windows.
    Usr1,
    /// User-defined (`SIGUSR2`). Not available on Windows.
    Usr2,
    /// Windows Ctrl+Break (`SIGBREAK`). Windows only.
    Break,
    /// Unconditional termination (`SIGKILL`). Send-only: it cannot be caught,
    /// blocked, or watched — it is the escalation after `SIGTERM`.
    Kill,
    /// Quit from keyboard, conventionally with a core dump (`SIGQUIT`).
    /// Send-only here.
    Quit,
}

impl Signal {
    /// The conventional name, as guest code spells it (`"SIGTERM"`).
    pub const fn name(self) -> &'static str {
        match self {
            Signal::Int => "SIGINT",
            Signal::Term => "SIGTERM",
            Signal::Hup => "SIGHUP",
            Signal::Usr1 => "SIGUSR1",
            Signal::Usr2 => "SIGUSR2",
            Signal::Break => "SIGBREAK",
            Signal::Kill => "SIGKILL",
            Signal::Quit => "SIGQUIT",
        }
    }

    /// Parses a conventional name, or `None` if it is not one this runtime
    /// knows. Whether a known signal is *available* is the provider's answer,
    /// not this one's — that varies by platform.
    pub fn from_name(name: &str) -> Option<Signal> {
        match name {
            "SIGINT" => Some(Signal::Int),
            "SIGTERM" => Some(Signal::Term),
            "SIGHUP" => Some(Signal::Hup),
            "SIGUSR1" => Some(Signal::Usr1),
            "SIGUSR2" => Some(Signal::Usr2),
            "SIGBREAK" => Some(Signal::Break),
            "SIGKILL" => Some(Signal::Kill),
            "SIGQUIT" => Some(Signal::Quit),
            _ => None,
        }
    }

    /// The exit status a process conventionally reports when this signal kills
    /// it: 128 + the signal number. Used for the CLI's exit code, so `^C` still
    /// looks like `^C` to a shell or an orchestrator.
    pub const fn exit_code(self) -> i32 {
        match self {
            Signal::Int => 130,   // 128 + 2
            Signal::Term => 143,  // 128 + 15
            Signal::Hup => 129,   // 128 + 1
            Signal::Usr1 => 138,  // 128 + 10
            Signal::Usr2 => 140,  // 128 + 12
            Signal::Break => 149, // 128 + 21 (SIGBREAK)
            Signal::Kill => 137,  // 128 + 9
            Signal::Quit => 131,  // 128 + 3
        }
    }
}

/// Delivery of OS signals to the guest, backing `runtime:process` `onSignal`.
/// Capability-checked (`Capability::Signals`).
///
/// Pull-based, like [`HttpServerProvider::next_requests`]: the runtime owns no
/// loop and no thread, so it *asks* for the next signal and the provider's
/// future resolves when one arrives. Installing a watch is what suppresses the
/// signal's default action — which is the whole point, and the reason this needs
/// a capability of its own.
pub trait Signals: Send + Sync {
    /// Which signals this host can deliver. Guest code asking for one outside
    /// this set gets a clear error naming what is available, rather than a
    /// handler that silently never fires.
    fn available(&self) -> Vec<Signal>;

    /// Starts watching `signal`, suppressing its default action. Idempotent.
    /// Errors if the signal is not in [`available`](Self::available) or the host
    /// refuses the registration.
    fn watch(&self, signal: Signal) -> Result<(), ProviderError>;

    /// Stops watching `signal`, restoring the default action where the platform
    /// allows it. Idempotent; unwatching one never watched is not an error.
    fn unwatch(&self, signal: Signal);

    /// Resolves with the next delivery of any currently watched signal, or
    /// `None` once nothing is watched — so a caller that unwatches everything
    /// is released rather than parked forever.
    fn next(&self) -> BoxFuture<Option<Signal>>;
}

/// Host process information — environment, arguments, working directory,
/// platform — and the exit hook, backing the `runtime:process` module
/// (DECISIONS D24). Capability-checked (`Capability::Env`) before any op
/// consults it; an embedder supplies a controlled view rather than the runtime
/// reaching for the real process (no ambient authority, D5).
pub trait Process: Send + Sync {
    /// Environment as `(name, value)` pairs — a snapshot taken at first read.
    fn env(&self) -> Vec<(String, String)>;

    /// Program arguments (the user args, excluding the runtime binary and the
    /// entry script / `-e` code), in order.
    fn args(&self) -> Vec<String>;

    /// The current working directory as a path string.
    fn cwd(&self) -> Result<String, ProviderError>;

    /// The host platform — Rust's `std::env::consts::OS` values (`"linux"`,
    /// `"macos"`, `"windows"`, …).
    fn platform(&self) -> String;

    /// The host CPU architecture — Rust's `std::env::consts::ARCH` values
    /// (`"x86_64"`, `"aarch64"`, `"arm"`, …).
    fn arch(&self) -> String;

    /// How many workers can usefully run at once — `navigator.hardwareConcurrency`.
    ///
    /// Defaults to 1, which is honest for a host that does not know or does not
    /// wish to say. Ungated for the same reason as [`platform`](Self::platform):
    /// it describes the machine the guest is already running on, and a program
    /// that cannot ask simply guesses its pool size worse.
    fn hardware_concurrency(&self) -> u32 {
        1
    }

    /// The environment this agent was **handed**, if it was handed one.
    ///
    /// `None` — the default, and what the agent driving the process always
    /// reports — means "read the host environment", which is
    /// [`Capability::Env`](es_runtime_common::Capability::Env)'s business.
    /// `Some` is a worker whose parent passed `new Worker(url, { env })`: the
    /// parent narrowed what it already held and handed the result over, so it
    /// is *data*, not authority, and reading it needs no capability. An empty
    /// vector is a real answer — a worker given no environment at all.
    fn provided_env(&self) -> Option<Vec<(String, String)>> {
        None
    }

    /// Records a guest `process.exit(code)` request. The runtime also halts
    /// execution (via its interrupt handle); the embedder reads
    /// [`requested_exit_code`](Self::requested_exit_code) after the run to learn
    /// the code and that exit (not an error) caused the stop.
    fn exit(&self, code: i32);

    /// The exit code requested via [`exit`](Self::exit), if any.
    fn requested_exit_code(&self) -> Option<i32>;
}

/// How one of a child process's standard streams is connected
/// ([`CommandSpec`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stdio {
    /// A pipe the guest reads from / writes to.
    Piped,
    /// The parent's own stream — the child shares this process's console.
    Inherit,
    /// The null device: reads see EOF, writes are discarded.
    #[default]
    Null,
}

/// Which of a child's output streams a [`CommandProvider::read`] targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStream {
    /// The child's standard output.
    Stdout,
    /// The child's standard error.
    Stderr,
}

/// What to launch, for [`CommandProvider::spawn`].
///
/// The environment is **always complete and explicit**: the provider does not
/// merge the host's environment into it. A guest that wants inheritance reads
/// the environment through the `Env`-gated `runtime:process` ops and passes the
/// pairs here, so a `Run` grant alone never leaks the host environment into a
/// child (DECISIONS D37). `program` resolution is still the provider's job —
/// it may consult the host `PATH`, which is host authority, not the guest's.
#[derive(Default)]
pub struct CommandSpec {
    /// The executable: a path (absolute, or relative to `cwd`), or a bare name
    /// to look up on the host `PATH`. Never a shell command line — there is no
    /// shell, so nothing here is word-split, glob-expanded, or interpolated.
    pub program: String,
    /// Arguments, passed to the child verbatim (no quoting or escaping needed).
    pub args: Vec<String>,
    /// Working directory. `None` ⇒ inherit the parent's.
    pub cwd: Option<String>,
    /// The child's complete environment as `(name, value)` pairs.
    pub env: Vec<(String, String)>,
    /// How to connect the child's standard input.
    pub stdin: Stdio,
    /// How to connect the child's standard output.
    pub stdout: Stdio,
    /// How to connect the child's standard error.
    pub stderr: Stdio,
}

/// How a child process ended, from [`CommandProvider::wait`].
#[derive(Debug, Clone, Default)]
pub struct ChildStatus {
    /// Whether the child exited with status 0.
    pub success: bool,
    /// The exit status, or `None` when a signal ended the process.
    pub code: Option<i32>,
    /// The name of the signal that ended the process (`"SIGKILL"`), if one did.
    /// A free string rather than a [`Signal`]: a child can die from any signal
    /// the OS has, far beyond the set this runtime watches or sends.
    pub signal: Option<String>,
}

/// Child processes, backing the `runtime:system` module (DECISIONS D37).
///
/// [`spawn`](Self::spawn) is capability-checked on `Capability::Run` before it
/// is ever called; the rest operate on the id it returns, which is already
/// proof of an authorized spawn (D7), exactly like [`NetProvider`]'s read/write.
///
/// The implementation owns every child and its pipes, keyed by that opaque id.
/// Two properties an implementation is expected to hold:
///
/// - **No shell.** `program` and `args` reach the OS as an argv, never a command
///   line a shell re-parses. This is what makes guest-supplied arguments
///   inert rather than an injection vector.
/// - **No orphans.** Children still running when the provider is dropped are
///   killed. A server that restarts must not accumulate abandoned processes.
///
/// An embedder that installs no `CommandProvider` has no `runtime:system`
/// access at all, whatever the capability set says.
pub trait CommandProvider: Send + Sync {
    /// Launches `spec`; resolves to (child id, OS process id).
    ///
    /// Failure to *find or start* the program is an error here. A program that
    /// starts and then fails is not: that is an exit status from
    /// [`wait`](Self::wait).
    fn spawn(&self, spec: CommandSpec) -> BoxFuture<Result<(u64, u32), ProviderError>>;

    /// Reads the next chunk from child `id`'s `stream`; `None` signals EOF.
    /// Errors if that stream was not [`Stdio::Piped`].
    fn read(
        &self,
        id: u64,
        stream: ChildStream,
    ) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>>;

    /// Writes `data` to child `id`'s standard input.
    fn write(&self, id: u64, data: Vec<u8>) -> BoxFuture<Result<(), ProviderError>>;

    /// Closes child `id`'s standard input (sends EOF). Idempotent.
    fn close_stdin(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;

    /// Resolves once child `id` has exited, with how it ended. Callable more
    /// than once: after the child is reaped, later calls return the same
    /// recorded status rather than an error.
    fn wait(&self, id: u64) -> BoxFuture<Result<ChildStatus, ProviderError>>;

    /// Sends `signal` to child `id`. Sending to an already-exited child is not
    /// an error (it is a race the caller cannot avoid), it is a no-op.
    ///
    /// Only the direct child is signalled — descendants it spawned are not, so
    /// a child that forks can leave the grandchildren running. Platforms
    /// without POSIX signals may terminate the child whatever the signal.
    fn kill(&self, id: u64, signal: Signal) -> BoxFuture<Result<(), ProviderError>>;

    /// Releases child `id` and everything held for it (idempotent). A child
    /// still running is killed first. The runtime calls this once the guest can
    /// no longer observe the child — its status is settled and its piped
    /// streams are finished — so a long-lived server does not accumulate the
    /// pipes and buffers of every process it has ever spawned.
    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;
}

/// Metadata about a filesystem entry, from [`FileSystem::stat`].
pub struct FileStat {
    /// Size in bytes.
    pub size: u64,
    /// Whether the entry is a regular file.
    pub is_file: bool,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Whether the entry is a symbolic link.
    pub is_symlink: bool,
    /// Modification time in milliseconds since the Unix epoch, if the host
    /// exposes it.
    pub mtime_ms: Option<f64>,
}

/// One entry in a directory listing, from [`FileSystem::read_dir`].
pub struct DirEntry {
    /// The entry's file name (no directory components).
    pub name: String,
    /// Whether the entry is a regular file.
    pub is_file: bool,
    /// Whether the entry is a directory.
    pub is_dir: bool,
    /// Whether the entry is a symbolic link.
    pub is_symlink: bool,
}

/// Options for [`FileSystem::glob_scan`].
pub struct GlobScanOptions {
    /// Match dotfiles and dot-directories (default: skipped).
    pub dot: bool,
    /// Return absolute paths instead of paths relative to the scan base.
    pub absolute: bool,
    /// Yield only files, skipping directories.
    pub only_files: bool,
    /// Traverse into symlinked directories. The implementation still rejects any
    /// followed entry whose real path leaves the root jail.
    pub follow_symlinks: bool,
}

/// Filesystem access backing `runtime:fs` (DECISIONS D25, SPEC §11).
///
/// The implementation confines every path to a **root jail** (canonicalize then
/// containment-check); a path that escapes is rejected. Reads are
/// capability-checked on `Capability::FileRead` and mutations on
/// `Capability::FileWrite` by `runtime` before any method here runs. Methods are
/// async because file I/O is blocking work the driver offloads; an embedder that
/// installs no `FileSystem` provider has no `runtime:fs` access at all.
pub trait FileSystem: Send + Sync {
    /// Reads the whole file at `path` as bytes.
    fn read(&self, path: String) -> BoxFuture<Result<Vec<u8>, ProviderError>>;

    /// Writes `data` to `path`, resolving to the number of bytes written. With
    /// `append`, bytes are added at the end (creating the file if needed);
    /// otherwise the file is created or truncated.
    ///
    /// It must resolve only once the bytes are **visible to a subsequent read**.
    /// An implementation that buffers has to flush before it resolves: a guest's
    /// `await write(p, data)` followed by `read(p)` is ordinary code, and
    /// resolving early makes it return a truncated file.
    fn write(
        &self,
        path: String,
        data: Vec<u8>,
        append: bool,
    ) -> BoxFuture<Result<u64, ProviderError>>;

    /// Metadata for `path` (follows symlinks).
    fn stat(&self, path: String) -> BoxFuture<Result<FileStat, ProviderError>>;

    /// Whether `path` exists (a missing path is `false`, not an error).
    fn exists(&self, path: String) -> BoxFuture<Result<bool, ProviderError>>;

    /// Lists the entries of the directory at `path` (no `.`/`..`).
    fn read_dir(&self, path: String) -> BoxFuture<Result<Vec<DirEntry>, ProviderError>>;

    /// Creates the directory at `path`; with `recursive`, creates missing
    /// parents and succeeds if it already exists.
    fn mkdir(&self, path: String, recursive: bool) -> BoxFuture<Result<(), ProviderError>>;

    /// Removes the file or (with `recursive`) directory tree at `path`.
    fn remove(&self, path: String, recursive: bool) -> BoxFuture<Result<(), ProviderError>>;

    /// Renames/moves `from` to `to` (both jailed).
    fn rename(&self, from: String, to: String) -> BoxFuture<Result<(), ProviderError>>;

    /// Copies the file `from` to `to` (both jailed), overwriting `to`. Resolves
    /// to the number of bytes copied.
    fn copy(&self, from: String, to: String) -> BoxFuture<Result<u64, ProviderError>>;

    /// Resolves `path` to its canonical location — symlinks followed, `.`/`..`
    /// removed. Errors if the target does not exist, or if it lands outside the
    /// jail: this is precisely the operation a caller uses to ask "where does
    /// this really point?", so it must not answer with somewhere unreachable.
    fn real_path(&self, path: String) -> BoxFuture<Result<String, ProviderError>>;

    /// Reads the target of the symbolic link at `path`, verbatim — the stored
    /// value, which may be relative and may not exist. Use
    /// [`real_path`](Self::real_path) to resolve it.
    fn read_link(&self, path: String) -> BoxFuture<Result<String, ProviderError>>;

    /// Truncates or extends the file at `path` to exactly `len` bytes. Extending
    /// zero-fills.
    fn truncate(&self, path: String, len: u64) -> BoxFuture<Result<(), ProviderError>>;

    /// Sets the permission bits of `path` to `mode` (a Unix mode such as
    /// `0o600`).
    ///
    /// Unix applies it as given. Windows has no such bits, so an implementation
    /// there can honour only the owner-write bit as the read-only flag — a
    /// partial mapping that must be documented rather than silently pretended.
    fn chmod(&self, path: String, mode: u32) -> BoxFuture<Result<(), ProviderError>>;

    /// Creates a new directory with an unpredictable name under `dir` (jailed;
    /// the base directory when empty), named `<prefix>XXXXXX`, and resolves to
    /// its path.
    ///
    /// The name comes from the host's temp-file machinery rather than being
    /// composed by the caller: a guessable temp name in a shared directory is a
    /// symlink-attack invitation, and getting that right is not something each
    /// caller should re-derive.
    fn make_temp_dir(
        &self,
        dir: String,
        prefix: String,
    ) -> BoxFuture<Result<String, ProviderError>>;

    /// Creates a new empty file with an unpredictable name under `dir`, on the
    /// same terms as [`make_temp_dir`](Self::make_temp_dir), and resolves to its
    /// path.
    fn make_temp_file(
        &self,
        dir: String,
        prefix: String,
    ) -> BoxFuture<Result<String, ProviderError>>;

    /// Tests whether `path` matches the glob `pattern` (pure; no I/O). Supports
    /// `*`, `**`, `?`, character classes, and `{a,b}` alternation.
    fn glob_match(&self, pattern: &str, path: &str) -> Result<bool, ProviderError>;

    /// Walks `base` (jailed) and returns the paths matching the glob `pattern`,
    /// relative to `base` unless `opts.absolute`.
    fn glob_scan(
        &self,
        base: String,
        pattern: String,
        opts: GlobScanOptions,
    ) -> BoxFuture<Result<Vec<String>, ProviderError>>;
}

/// An open handle held by a [`SyncFileSystem`]. Opaque to the guest: it indexes
/// a table the provider owns, so a forged number can only miss.
pub type SyncFd = u32;

/// Where a [`SyncFileSystem::seek`] offset is measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncWhence {
    /// From the start of the file.
    Start,
    /// From the current position.
    Current,
    /// From the end of the file.
    End,
}

/// How a [`SyncFileSystem::open`] should open its target.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SyncOpenOptions {
    /// Open for reading.
    pub read: bool,
    /// Open for writing.
    pub write: bool,
    /// Create the file if it does not exist.
    pub create: bool,
    /// Fail if the file already exists (implies `create`).
    pub create_new: bool,
    /// Truncate an existing file to zero length.
    pub truncate: bool,
    /// Append rather than overwrite.
    pub append: bool,
    /// Open a *directory* handle, usable only as an anchor for path resolution.
    /// WASI needs these for its `path_*` calls, which are all relative to a
    /// directory fd.
    pub directory: bool,
}

/// Blocking filesystem access, for callers that cannot await.
///
/// WASI is why this exists. Its syscalls are **synchronous** — a guest calls
/// `fd_read` and expects bytes back with no opportunity to yield — so the async
/// [`FileSystem`] cannot serve them however the ops are arranged. Rather than
/// bend WASI, this is a separate, deliberately small seam.
///
/// Every method blocks the calling thread, which is the runtime's own thread. An
/// embedder that cannot afford that simply installs no implementation, and WASI
/// then reports `ENOTCAPABLE` for every file call — the same as today.
///
/// Implementations are expected to confine paths to a root jail (DECISIONS D25),
/// exactly as [`FileSystem`] does; `runtime` gates reads on
/// [`Capability::FileRead`](es_runtime_common::Capability::FileRead) and
/// mutations on [`FileWrite`](es_runtime_common::Capability::FileWrite) before
/// any method here runs.
pub trait SyncFileSystem: Send + Sync {
    /// Opens `path`, returning a handle for the fd-based methods below.
    fn open(&self, path: &str, options: SyncOpenOptions) -> Result<SyncFd, ProviderError>;

    /// Reads into `buf`, returning the number of bytes read (0 at end of file).
    fn read(&self, fd: SyncFd, buf: &mut [u8]) -> Result<usize, ProviderError>;

    /// Writes `data`, returning the number of bytes written.
    fn write(&self, fd: SyncFd, data: &[u8]) -> Result<usize, ProviderError>;

    /// Moves the file cursor, returning the new absolute position.
    fn seek(&self, fd: SyncFd, offset: i64, whence: SyncWhence) -> Result<u64, ProviderError>;

    /// Releases `fd`. A handle that is not open is an error, not a panic.
    fn close(&self, fd: SyncFd) -> Result<(), ProviderError>;

    /// Metadata for an open handle.
    fn fstat(&self, fd: SyncFd) -> Result<FileStat, ProviderError>;

    /// Metadata for `path` (follows symlinks).
    fn stat(&self, path: &str) -> Result<FileStat, ProviderError>;

    /// Lists the entries of the directory at `path` (no `.`/`..`).
    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, ProviderError>;

    /// Creates the directory at `path` (parents must exist).
    fn mkdir(&self, path: &str) -> Result<(), ProviderError>;

    /// Removes the file at `path`.
    fn remove_file(&self, path: &str) -> Result<(), ProviderError>;

    /// Removes the (empty) directory at `path`.
    fn remove_dir(&self, path: &str) -> Result<(), ProviderError>;

    /// Renames/moves `from` to `to` (both jailed).
    fn rename(&self, from: &str, to: &str) -> Result<(), ProviderError>;
}

/// Metadata about a socket, from [`NetProvider::connect`]/`accept`/`listen`.
#[derive(Default)]
pub struct SocketInfo {
    /// Remote peer address (empty for a listener).
    pub remote_address: String,
    /// Remote peer port (0 for a listener).
    pub remote_port: u16,
    /// Local address.
    pub local_address: String,
    /// Local (or bound) port.
    pub local_port: u16,
    /// Negotiated ALPN protocol for a TLS connection (`None` for plaintext, a
    /// failed negotiation, or a listener). Surfaces as WinterTC `SocketInfo.alpn`.
    pub alpn: Option<String>,
}

/// Options for [`NetProvider::connect`], mirroring the TLS-relevant members of
/// the WinterTC `SocketOptions` (DECISIONS D28).
#[derive(Default)]
pub struct ConnectOptions {
    /// Negotiate TLS (`secureTransport: "on"`). When false, plain TCP.
    pub secure: bool,
    /// TLS Server Name Indication. `None` ⇒ use the connect host.
    pub sni: Option<String>,
    /// ALPN protocols to offer, in preference order (empty ⇒ none).
    pub alpn: Vec<String>,
}

/// Options for [`NetProvider::listen`] (DECISIONS D28). With a non-empty
/// `cert`+`key` the listener **terminates TLS**: every accepted connection
/// completes a server-side handshake before it surfaces, and the negotiated
/// protocol comes back as [`SocketInfo::alpn`]. The certificate and key are
/// supplied inline (PEM) by the guest, so server-side TLS needs no new
/// capability beyond the `Capability::NetListen` the bind already requires — the
/// guest loads the material itself (e.g. via `runtime:fs`, capability-checked)
/// rather than the provider reaching for ambient files (no ambient authority, D5).
#[derive(Default)]
pub struct ListenOptions {
    /// PEM-encoded certificate chain (leaf first). Empty ⇒ plaintext TCP.
    pub cert: Vec<u8>,
    /// PEM-encoded private key (PKCS#8, PKCS#1, or SEC1). Empty ⇒ plaintext TCP.
    pub key: Vec<u8>,
    /// ALPN protocols to advertise, in preference order (empty ⇒ none).
    pub alpn: Vec<String>,
    /// Allow several processes to bind this same address, letting the kernel
    /// balance new connections across them (`SO_REUSEPORT`).
    ///
    /// How a server is run across cores without a front proxy, and how one is
    /// replaced without dropping connections: the replacement binds alongside
    /// the old process before it exits. Unix-only — Windows has no equivalent
    /// (its `SO_REUSEADDR` lets an *unrelated* process take a bound port, which
    /// is a different and unsafe thing), so an implementation should refuse
    /// rather than bind exclusively and leave the caller to discover it.
    pub reuse_port: bool,
}

/// Raw TCP sockets backing `runtime:net` (SPEC §12, the WinterTC `connect()`
/// shape). The implementation owns every connection and listener, keyed by an
/// opaque id it hands back; the runtime drives reads, writes, accepts, and
/// closes by id. `connect` is capability-checked on `Capability::Net` and
/// `listen` on `Capability::NetListen` before these are ever called; an embedder
/// that installs no `NetProvider` has no `runtime:net` access at all.
pub trait NetProvider: Send + Sync {
    /// Opens an outbound TCP connection; resolves to (socket id, info). When
    /// `opts.secure`, negotiates TLS using `opts.sni` (or `host`) as the server
    /// name and offering `opts.alpn`; the negotiated protocol is returned in
    /// [`SocketInfo::alpn`].
    fn connect(
        &self,
        host: String,
        port: u16,
        opts: ConnectOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>>;

    /// Reads the next chunk from socket `id`; `None` signals end of stream.
    fn read(&self, id: u64) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>>;

    /// Writes `data` to socket `id`.
    fn write(&self, id: u64, data: Vec<u8>) -> BoxFuture<Result<(), ProviderError>>;

    /// Half-closes the write side of socket `id` (sends FIN); reads still work.
    fn shutdown(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;

    /// Closes socket `id` (idempotent).
    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;

    /// Binds a listening socket; resolves to (listener id, bound-address info).
    /// When `opts.cert`/`opts.key` are present the listener terminates TLS: each
    /// accepted connection completes a server-side handshake (advertising
    /// `opts.alpn`) before [`accept`](Self::accept) yields it, with the
    /// negotiated protocol in [`SocketInfo::alpn`].
    fn listen(
        &self,
        host: String,
        port: u16,
        opts: ListenOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>>;

    /// Accepts the next inbound connection on listener `id`; resolves to a new
    /// (socket id, info), or `None` once the listener is closed.
    fn accept(&self, id: u64) -> BoxFuture<Result<Option<(u64, SocketInfo)>, ProviderError>>;

    /// Closes listener `id` (idempotent).
    fn close_listener(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;

    /// Upgrades plaintext socket `id` to TLS in place (the WinterTC
    /// `startTls()`), using `server_name` for SNI + certificate verification and
    /// offering `alpn`. Resolves to a **new** (socket id, info) for the encrypted
    /// stream; the old id is consumed. Only valid for a socket opened with
    /// `secureTransport: "starttls"`. The default errors — a provider can support
    /// it only if it keeps the raw stream reclaimable until the upgrade.
    fn start_tls(
        &self,
        id: u64,
        server_name: String,
        alpn: Vec<String>,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>> {
        let _ = (id, server_name, alpn);
        Box::pin(async { Err(ProviderError::Other("startTls is not supported".into())) })
    }
}

/// Metadata about an opened WebSocket, from [`WebSocketProvider::connect`].
#[derive(Default)]
pub struct WebSocketInfo {
    /// The negotiated subprotocol (empty if none was selected).
    pub protocol: String,
    /// The negotiated `extensions` header (empty until permessage-deflate; D29).
    pub extensions: String,
}

/// How a [`WebSocketProvider`] should bind a server, from
/// [`serve`](WebSocketProvider::serve).
///
/// Deliberately the same shape as [`HttpServeOptions`], down to the field names:
/// a WebSocket server is an HTTP server that stops after one request, its
/// opening handshake *is* an HTTP request head, and the two live in the same
/// process under the same descriptor budget. A second vocabulary for the same
/// two questions would be a thing to learn twice for no gain.
pub struct WsServeOptions {
    /// Address to bind (`"0.0.0.0"`, `"127.0.0.1"`, …).
    pub host: String,
    /// Port to bind; `0` picks an ephemeral one, read back from the returned
    /// [`SocketInfo`].
    pub port: u16,
    /// When to give up on a connection that is not making progress.
    pub timeouts: WsTimeouts,
    /// The most connections to hold at once, or `None` for no limit.
    ///
    /// `None` is the default for the same reason as
    /// [`HttpServeOptions::max_connections`]: the right number follows from a
    /// deployment's file-descriptor budget, which this crate cannot read.
    ///
    /// It matters more here than it does there, though, and for the opposite
    /// reason: HTTP connections churn, while a WebSocket connection is
    /// long-lived *by design*. A count that a busy HTTP server keeps flat is one
    /// a WebSocket server accumulates, so this is the option that decides
    /// whether it has an upper bound at all.
    ///
    /// An implementation that honours this should hold connections *back*
    /// rather than accept and discard them — a limit that still costs a
    /// descriptor and a task per refused connection does not bound anything
    /// under the flood it exists for.
    pub max_connections: Option<usize>,
    /// The most connections **one peer address** may hold at once, or `None`
    /// for no limit.
    ///
    /// The same policy as [`HttpServeOptions::max_connections_per_ip`], and it
    /// matters more here for the same reason [`max_connections`](Self::max_connections)
    /// does: a WebSocket connection is long-lived by design, so one peer's share
    /// of the budget is not something churn takes back.
    ///
    /// A connection over this should be **refused**, not held.
    pub max_connections_per_ip: Option<usize>,
}

/// When a [`WebSocketProvider`] server should give up on a connection.
///
/// [`Default`] is the recommended posture rather than "off", so a provider gets
/// the protection without asking and a guest opts *out* deliberately — the same
/// choice as [`HttpTimeouts`], for the same reason (D43): a timeout nobody
/// configures protects nobody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WsTimeouts {
    /// From accept until the opening handshake completes.
    ///
    /// RFC 6455's handshake is an HTTP request head and a `101` answer, so this
    /// is the WebSocket spelling of [`HttpTimeouts::header_read`] — the same
    /// slowloris bound, on the same bytes. It does **not** bound an established
    /// connection: a WebSocket that has said nothing for a week is idle, not
    /// stalled, and closing it is the application's decision.
    ///
    /// Default: 10s.
    pub handshake: Option<Duration>,
}

impl Default for WsTimeouts {
    fn default() -> Self {
        Self {
            handshake: Some(Duration::from_secs(10)),
        }
    }
}

/// A message to send on a WebSocket, from [`WebSocketProvider::send`].
pub enum WsMessage {
    /// A UTF-8 text frame.
    Text(String),
    /// A binary frame.
    Binary(Vec<u8>),
}

/// One inbound WebSocket event surfaced by [`WebSocketProvider::recv`]. Control
/// frames (ping/pong) are answered inside the provider and never appear here;
/// only application messages and the peer's closing handshake reach the guest
/// (DECISIONS D29).
pub enum WsIncoming {
    /// A received text frame.
    Text(String),
    /// A received binary frame.
    Binary(Vec<u8>),
    /// The peer's closing handshake (a clean close) with its code and reason.
    Close {
        /// The close code (`1000`, `1001`, … or an application `3000`–`4999`).
        code: u16,
        /// The close reason (may be empty).
        reason: String,
    },
}

/// The WebSocket client backing the `WebSocket` global (DECISIONS D29). Like
/// [`NetProvider`], the implementation owns each connection keyed by an opaque
/// id it hands back; the runtime drives sends, receives, and closes by id.
/// `connect` is capability-checked on `Capability::Net` before this is ever
/// called (the same gate as `fetch` / `runtime:net` `connect`); an embedder that
/// installs no `WebSocketProvider` has no `WebSocket` access at all. The provider
/// answers ping/pong control frames itself — only `message`/`close` surface.
pub trait WebSocketProvider: Send + Sync {
    /// Opens a WebSocket to `url` (`ws:`/`wss:`), offering `protocols`; resolves
    /// to (socket id, negotiated info) once the opening handshake completes.
    fn connect(
        &self,
        url: String,
        protocols: Vec<String>,
    ) -> BoxFuture<Result<(u64, WebSocketInfo), ProviderError>>;

    /// Sends one message on socket `id`.
    fn send(&self, id: u64, message: WsMessage) -> BoxFuture<Result<(), ProviderError>>;

    /// Sends one message to every socket in `ids` (a fan-out / broadcast). This
    /// is the batched form of [`send`](Self::send): one op crossing instead of
    /// one per connection, so a server publishing to many sockets pays the JS↔
    /// host boundary and payload marshaling once. Implementations should enqueue
    /// to all connections without letting a slow one block the rest. Unknown ids
    /// are skipped.
    fn broadcast(&self, ids: Vec<u64>, message: WsMessage) -> BoxFuture<Result<(), ProviderError>>;

    /// Awaits the next inbound event on socket `id`; `None` signals an abnormal
    /// close (the connection dropped without a closing handshake).
    fn recv(&self, id: u64) -> BoxFuture<Result<Option<WsIncoming>, ProviderError>>;

    /// Begins the closing handshake on socket `id`. `code`/`reason` follow
    /// `WebSocket.close()`; `code` is `None` for a bare close frame. Idempotent.
    fn close(
        &self,
        id: u64,
        code: Option<u16>,
        reason: String,
    ) -> BoxFuture<Result<(), ProviderError>>;

    /// Binds a listening WebSocket server (`ws:` only — a `wss:` server is a
    /// follow-up) and starts accepting; resolves to (server id, bound-address
    /// info). Port 0 picks an ephemeral port. Capability-checked on
    /// `Capability::NetListen` (like `runtime:net` `listen`) before this is ever
    /// called; backs the `runtime:websocket` `serve()` (DECISIONS D29).
    fn serve(&self, options: WsServeOptions)
    -> BoxFuture<Result<(u64, SocketInfo), ProviderError>>;

    /// Accepts the next inbound connection on server `id`, once its opening
    /// handshake completes; resolves to a new (connection id, info), or `None`
    /// when the server is closed. The connection id is driven by the same
    /// [`send`](Self::send) / [`recv`](Self::recv) / [`close`](Self::close) as a
    /// client `connect` id (one shared id space, like [`NetProvider`] sockets).
    fn accept(&self, id: u64) -> BoxFuture<Result<Option<(u64, WebSocketInfo)>, ProviderError>>;

    /// Closes server `id` (idempotent); stops accepting new connections. Already
    /// accepted connections keep working until individually closed.
    fn close_server(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;
}

/// Everything a [`WorkerHost`] needs to start one worker agent.
#[derive(Clone, Debug, Default)]
pub struct WorkerSpec {
    /// Absolute URL of the worker's entry module, already resolved against the
    /// spawning module by the parent — the parent held the authority to resolve
    /// it, so the worker never needs `FileSystem` merely to start.
    pub specifier: String,
    /// The entry module's source, read by the parent — which held the authority
    /// to read it. Passing it in means a spawn that cannot find or read the
    /// file fails where the guest can see it, rather than inside a thread that
    /// has already been created.
    pub source: String,
    /// The worker's `name`, as passed to `new Worker(url, { name })`.
    pub name: String,
    /// The environment the worker is handed, or `None` to read the host's.
    ///
    /// A parent can only pass values it could already read, so this narrows
    /// rather than grants: it is how a worker is given `DATABASE_URL` without
    /// being given the environment.
    pub env: Option<Vec<(String, String)>>,
    /// What the worker agent is allowed to do.
    ///
    /// **Never inherited.** The runtime builds this from the options the guest
    /// passed, intersected with the spawning agent's own set, so a spawn cannot
    /// widen what its parent holds. The default is
    /// [`CapabilitySet::none`](es_runtime_common::CapabilitySet::none).
    pub capabilities: es_runtime_common::CapabilitySet,
    /// The set the worker's **static import graph** is loaded under, before
    /// [`capabilities`](Self::capabilities) takes effect — the spawning agent's
    /// own set, narrowed to what module loading needs.
    ///
    /// Without this a worker granted nothing could not read its own imports, so
    /// deny-by-default would mean single-file workers only. The parent already
    /// held that authority and is the one that named the module, so lending it
    /// for the load grants nothing new.
    ///
    /// It is safe to lend precisely because instantiation runs no guest code:
    /// the host loads and links the graph under this set, narrows to
    /// [`capabilities`](Self::capabilities), and only then evaluates. Nothing
    /// the worker's author wrote executes under the wider set.
    pub load_capabilities: es_runtime_common::CapabilitySet,
    /// Resource ceilings for the worker's isolate.
    ///
    /// [`can_block`](es_runtime_common::Limits::can_block) is `true` here: a
    /// worker owns its thread, so `Atomics.wait` blocking it is exactly what
    /// the call is for, where on the agent driving the loop it is a hang.
    pub limits: es_runtime_common::Limits,
}

/// One event from a worker agent, surfaced by [`WorkerHost::recv`].
pub enum WorkerIncoming {
    /// A `postMessage` payload, in the engine's structured-clone format.
    ///
    /// Opaque here: the bytes are produced and consumed by the two isolates,
    /// and this seam only moves them. They are **not** a wire format — see the
    /// engine's `serialize` module.
    Message(Vec<u8>),
    /// The worker failed: an uncaught exception, or a module that would not
    /// load. Surfaces on the parent's `Worker` as an `error` event.
    Error {
        /// What failed, kept in pieces — class, message, stack, location — so
        /// the parent's `error` event can offer the same to a supervisor.
        error: UncaughtError,
    },
    /// The worker ended — `close()`, or its entry module reaching quiescence.
    Closed,
}

/// Starts and drives worker agents: each its own thread, each its own isolate
/// (backs the `Worker` global).
///
/// The runtime cannot do this itself and does not try to: it owns no thread and
/// no loop (ARCHITECTURE §1/§5), so "start an agent" is an injected capability
/// like every other reach outside the isolate. `spawn` is capability-checked on
/// [`Capability::Worker`](es_runtime_common::Capability::Worker) before this is
/// ever called; an embedder that installs no `WorkerHost` has no `Worker` at
/// all. The remaining methods take an id that a checked `spawn` produced, so
/// they need no capability of their own — the same reasoning as
/// [`NetProvider`] reads and [`WebSocketProvider::send`].
///
/// This is the seam a scheduler-backed host replaces: an implementation is free
/// to run agents as green tasks on one thread, or route them to other
/// processes, so long as the observable contract holds.
pub trait WorkerHost: Send + Sync {
    /// Starts a worker agent; resolves to its id once the agent exists and its
    /// entry module has begun evaluating.
    ///
    /// Deliberately does **not** wait for the module to finish: a worker whose
    /// top level never settles (a server, a `for await` over a queue) is
    /// ordinary, and `new Worker()` is not allowed to block on it.
    fn spawn(&self, spec: WorkerSpec) -> BoxFuture<Result<u64, ProviderError>>;

    /// Delivers one structured-clone payload to worker `id`. Ordered with
    /// respect to other `post` calls on the same id.
    ///
    /// Synchronous, like [`PortHub::post`] and for the same reason: a queue
    /// push has nothing to wait for, and `postMessage` is synchronous in the
    /// specification. Making it a future was not merely redundant — every send
    /// held an async-op slot, so ~1150 posts in one turn exhausted the agent's
    /// `max_pending_ops` and made *every* async op fail, `terminate()`
    /// included. Only [`recv`](Self::recv) waits, because waiting is what it is
    /// for.
    ///
    /// A message to a worker that has already ended is dropped rather than
    /// refused: the specification gives `postMessage` no delivery guarantee,
    /// and racing a `close()` is ordinary rather than a fault in the sender.
    fn post(&self, id: u64, message: Vec<u8>) -> Result<(), ProviderError>;

    /// How many messages have been handed to [`post`](Self::post) for `id` and
    /// not yet taken by that worker.
    ///
    /// Advisory, and the only backpressure signal there is: nothing here ever
    /// refuses a message — HTML does not permit `postMessage` to fail for queue
    /// depth, and Node, Deno and Bun all queue without limit — so a producer
    /// that outruns its worker grows memory unless it chooses to pace itself.
    /// This is what it paces against, the way a socket's `bufferedAmount` works.
    fn queued(&self, id: u64) -> usize;

    /// Awaits the next event from worker `id`. Resolves to `None` once the
    /// worker is gone and its queue is drained, which ends the parent's pump.
    fn recv(&self, id: u64) -> BoxFuture<Result<Option<WorkerIncoming>, ProviderError>>;

    /// Stops worker `id` immediately, wherever it is — `Worker.terminate()`.
    /// Idempotent, and safe to call on an already-finished worker.
    ///
    /// "Immediately" includes a worker running a synchronous loop or parked in
    /// `Atomics.wait`: an implementation is expected to interrupt the agent,
    /// not merely ask it to stop.
    fn terminate(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;
}

/// The queues behind `MessagePort`, so that a port can be **transferred** to
/// another agent.
///
/// A port is host-owned rather than a pair of JS objects because transferring
/// one has to survive leaving the isolate that made it: what travels in a
/// `postMessage` is the port's id, and whichever agent holds that id is the one
/// its peer's messages reach. Two entangled ports are two ids; moving one
/// between agents moves nothing but a number.
///
/// Ungated. A port conveys no authority — an agent can only be handed one by
/// something that already had it.
pub trait PortHub: Send + Sync {
    /// Creates an entangled pair, returning both ids (`MessageChannel`).
    ///
    /// Synchronous, and so are `post`, `detach_reader` and `close` below:
    /// `new MessageChannel()` and `port.postMessage(x)` are synchronous in the
    /// specification, and a queue push has nothing to wait for. Only
    /// [`recv`](Self::recv) waits, because waiting is what it is for.
    fn create(&self) -> Result<(u64, u64), ProviderError>;

    /// Queues `message` for the **peer** of `id`. Dropped if the peer is gone,
    /// which is what a closed or never-transferred port looks like.
    fn post(&self, id: u64, message: Vec<u8>) -> Result<(), ProviderError>;

    /// Awaits the next message for `id`; `None` once the port is closed, or as
    /// soon as [`detach_reader`](Self::detach_reader) is called.
    fn recv(&self, id: u64) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>>;

    /// Stops this agent reading `id`, **without consuming a queued message** —
    /// the port has been transferred, and the agent receiving it must find
    /// everything that was already in flight.
    ///
    /// This is why the pump cannot simply be abandoned: an outstanding `recv`
    /// holding a message would swallow it on the way out.
    fn detach_reader(&self, id: u64);

    /// Closes `id` and disentangles its peer. Idempotent.
    fn close(&self, id: u64);
}

/// The broker behind `BroadcastChannel`: delivery to every other channel of the
/// same name, across every agent.
///
/// One delivered broadcast: the subscription it is for, and its bytes.
pub type Broadcast = (u64, Vec<u8>);

/// Delivers `BroadcastChannel` messages across the agent cluster.
///
/// A provider rather than a map in the prelude because the spec's scope is the
/// **agent cluster**, not one agent. With a single agent that distinction did
/// not exist; once workers do, a `BroadcastChannel` that reached only its own
/// isolate would be quietly wrong rather than merely limited. Where the cluster
/// lives is the host's business — one process here, potentially more than one
/// under a scheduler-backed host — which is exactly why it is a seam.
///
/// Ungated. It conveys no authority: a channel reaches only agents this runtime
/// already started, and the payload is one the sender could already construct.
pub trait BroadcastHub: Send + Sync {
    /// Opens a subscription to `name`, returning its id. Two subscriptions to
    /// the same name are peers even within one agent.
    ///
    /// Synchronous, because `new BroadcastChannel(name)` is: a channel is
    /// joined the moment it is constructed, with no window in which it can miss
    /// a message posted by the line after it. Registering a subscriber has
    /// nothing to await anyway — the same reasoning as [`PortHub::create`].
    fn subscribe(&self, name: String) -> Result<u64, ProviderError>;

    /// Delivers `message` to every open subscription to the same name **except**
    /// `id` — a channel never receives its own posts — in the order those
    /// subscriptions were opened, which is the delivery order the spec requires
    /// ("port creation order").
    fn publish(&self, id: u64, message: Vec<u8>) -> BoxFuture<Result<(), ProviderError>>;

    /// Awaits the next message for **the calling agent**, across every
    /// subscription it holds. `None` once that agent has no open subscription
    /// left.
    ///
    /// One stream per agent rather than per channel, because that is the order
    /// the spec delivers in: its channels share an event loop, so every
    /// destination of one post is delivered before any destination of the next.
    /// A receive per channel would hand back whichever future happened to be
    /// polled first instead. Which agent is asking is identified the way this
    /// host identifies agents everywhere — by the calling thread.
    fn recv_next(&self) -> BoxFuture<Result<Option<Broadcast>, ProviderError>>;

    /// Closes subscription `id`. Idempotent.
    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;
}

/// The other end of [`WorkerHost`], installed **only** in a worker agent's own
/// runtime — the `DedicatedWorkerGlobalScope` half.
///
/// Its presence is what tells the prelude it is running inside a worker: on the
/// agent that drives the process there is no `WorkerScope`, so no global
/// `postMessage`, no `onmessage`, no `close()`. That is the spec's own way of
/// distinguishing the two, and it is why there is no `isMainThread` flag —
/// which is a Node-ism, and absent from HTML, Deno and Bun alike.
pub trait WorkerScope: Send + Sync {
    /// This worker's `name`, from `new Worker(url, { name })`.
    fn name(&self) -> String;

    /// The absolute URL of the worker's entry module — what `location` reports
    /// inside it. The same string as [`WorkerSpec::specifier`], handed back to
    /// the agent that is running it.
    fn url(&self) -> String;

    /// Sends one structured-clone payload to the parent agent. Synchronous,
    /// for the reason [`WorkerHost::post`] gives.
    fn post(&self, message: Vec<u8>) -> Result<(), ProviderError>;

    /// How many messages this worker has sent to its parent and the parent has
    /// not yet taken — see [`WorkerHost::queued`], which is the same number
    /// read from the other side of a different queue.
    fn queued(&self) -> usize;

    /// Awaits the next payload from the parent. `None` once the parent is gone.
    fn recv(&self) -> BoxFuture<Result<Option<Vec<u8>>, ProviderError>>;

    /// `self.close()`: stop this worker after the current task. The agent
    /// finishes what it is doing and its loop then ends — unlike
    /// [`WorkerHost::terminate`], which interrupts from outside.
    fn close(&self);

    /// A failure this worker's own listeners did not claim: report it to the
    /// parent as [`WorkerIncoming::Error`], then end the agent as `close()`
    /// would.
    ///
    /// Both halves are the policy. Reporting has to happen *now* rather than
    /// when the worker finishes, because a worker holding a receive pump open
    /// never finishes on its own — a supervisor asking "did this job fail?"
    /// needs the answer while the job is still the current one. Ending follows
    /// because the failure escaped every handler the worker's author wrote, so
    /// the agent's state is whatever the exception left behind; a pool
    /// restarting on failure wants one clean transition, not an agent that
    /// stays in the rotation with unknown state.
    ///
    /// A worker that *takes* responsibility never reaches here: an `error` or
    /// `unhandledrejection` listener calling `preventDefault()` claims the
    /// failure, and a claimed failure is neither reported nor fatal.
    fn report_error(&self, error: UncaughtError);
}

/// A body crossing the [`HttpServerProvider`] seam, in either direction.
///
/// Mirrors the outbound [`RequestBody`]: [`Bytes`](HttpServerBody::Bytes) is the
/// buffered fast path, [`Stream`](HttpServerBody::Stream) delivers chunks
/// incrementally with bounded memory — an inbound request body the guest reads
/// as it arrives, or an outbound response body sent with chunked
/// transfer-encoding as the guest produces it. The stream ends at `None`; an
/// item `Err` aborts the request/response it belongs to.
pub enum HttpServerBody {
    /// No body.
    Empty,
    /// A fully-buffered body.
    Bytes(Vec<u8>),
    /// A body streamed as byte-chunks.
    Stream(ByteStream),
}

impl HttpServerBody {
    /// Whether there is definitely no body (`Empty`, or `Bytes` with no bytes).
    pub fn is_empty(&self) -> bool {
        match self {
            HttpServerBody::Empty => true,
            HttpServerBody::Bytes(b) => b.is_empty(),
            HttpServerBody::Stream(_) => false,
        }
    }
}

/// An inbound HTTP request delivered to an [`HttpServerProvider`] consumer.
///
/// The body arrives as an [`HttpServerBody`] — a provider should hand a
/// [`Stream`](HttpServerBody::Stream) so the guest reads it incrementally
/// (bounded memory for large uploads); a buffered
/// [`Bytes`](HttpServerBody::Bytes) is also accepted. `url` is reconstructed as
/// an absolute URL (scheme + `Host` header, or the bound address when no `Host`
/// is sent) so the guest can build a web `Request` from it.
pub struct HttpServerRequest {
    /// The HTTP method (`GET`, `POST`, …).
    pub method: String,
    /// The absolute request URL.
    pub url: String,
    /// Request header name/value pairs, in order.
    pub headers: Vec<(String, String)>,
    /// The request body.
    pub body: HttpServerBody,
    /// The peer this request arrived from — the address of the other end of the
    /// socket, and nothing else. Empty when the provider has no peer to report
    /// (a mock, a transport with no address), which is what a guest reads as
    /// "unknown" rather than as an address it could compare.
    ///
    /// This is the *socket* peer, so behind a reverse proxy it is the proxy.
    /// Resolving a forwarded header to the original client is a deployment's
    /// decision — it requires knowing which hop to trust, and a header anyone
    /// can send is not an identity until something says which sender to believe.
    ///
    /// On HTTP/2 every stream on a connection reports the same peer, because
    /// they *are* one connection.
    pub remote_address: String,
    /// The peer's port, or `0` when unknown. Mirrors
    /// [`SocketInfo`](SocketInfo::remote_port)'s convention.
    pub remote_port: u16,
}

/// The response a guest hands back for one [`HttpServerRequest`].
pub struct HttpServerResponse {
    /// The HTTP status code.
    pub status: u16,
    /// Response header name/value pairs, in order.
    pub headers: Vec<(String, String)>,
    /// The response body — a [`Stream`](HttpServerBody::Stream) is sent with
    /// chunked transfer-encoding as chunks arrive, never fully materialized.
    pub body: HttpServerBody,
    /// Header fields to send **after** the body, or `None` for none.
    ///
    /// A future rather than a value because that is the only shape that is
    /// useful: trailers exist to carry something that is not known until the
    /// body has been produced — the status of a gRPC call, a checksum, a row
    /// count. It resolves once the guest has them, which for a streamed body is
    /// after the last chunk has already gone out.
    ///
    /// On HTTP/2 these become a trailing `HEADERS` frame. On HTTP/1.1 they
    /// become a trailer section after the terminating chunk, and **only the
    /// fields named in the response's `Trailer` header are sent** — that is the
    /// wire format's rule, not this crate's, and a field not named there is
    /// dropped by the encoder.
    pub trailers: Option<BoxFuture<Vec<(String, String)>>>,
}

/// Where and how an [`HttpServerProvider`] should listen.
///
/// A struct rather than positional arguments so binding options can grow without
/// breaking every implementation each time.
pub struct HttpServeOptions {
    /// Address to bind (`"0.0.0.0"`, `"127.0.0.1"`, …).
    pub host: String,
    /// Port to bind; `0` picks an ephemeral one, read back from the returned
    /// [`SocketInfo`].
    pub port: u16,
    /// TLS to terminate on accept, or `None` for plain HTTP.
    pub tls: Option<HttpServerTls>,
    /// When to give up on a connection that is not making progress.
    pub timeouts: HttpTimeouts,
    /// The most connections to serve at once, or `None` for no limit.
    ///
    /// `None` is the default because the right number is a deployment's to
    /// know: it follows from the file-descriptor budget and the memory a
    /// connection costs, neither of which this crate can read. A cap set too
    /// low throttles legitimate traffic silently, which is worse than the
    /// unbounded case it was meant to improve.
    ///
    /// An implementation that honours this should hold connections *back*
    /// rather than accept and discard them — a limit that still costs a
    /// descriptor and a task per refused connection does not bound anything
    /// under the flood it exists for.
    pub max_connections: Option<usize>,
    /// The most connections **one peer address** may hold at once, or `None`
    /// for no limit.
    ///
    /// [`max_connections`](Self::max_connections) bounds what the deployment
    /// spends and nothing else: one peer taking every slot reaches it exactly as
    /// a thousand peers taking one each do, and the server is then full for
    /// everybody. This is the half that says *whose* connections they are.
    ///
    /// Unlike the whole-server cap, an implementation should **refuse** a
    /// connection over this rather than hold it. Holding is right there because
    /// the excess is legitimate traffic waiting for a slot; here the excess is
    /// by definition one client past its share, and a held connection is
    /// already accepted — it costs a descriptor, and the peer decides when it
    /// ends, which is the hold this exists to prevent.
    ///
    /// `None` by default, and deliberately: the count is per address, so
    /// everything behind one NAT or one load balancer shares a budget. Only the
    /// deployment knows what sits in front of it.
    pub max_connections_per_ip: Option<usize>,
    /// Allow several processes to bind this same address, letting the kernel
    /// balance new connections across them (`SO_REUSEPORT`).
    ///
    /// How a server is run across cores without a front proxy, and how one is
    /// replaced without dropping connections: the replacement binds alongside
    /// the old process before it exits. Unix-only — Windows has no equivalent
    /// (its `SO_REUSEADDR` lets an *unrelated* process take a bound port, which
    /// is a different and unsafe thing), so an implementation should refuse
    /// rather than bind exclusively and leave the caller to discover it.
    pub reuse_port: bool,
}

/// When an [`HttpServerProvider`] should give up on a connection.
///
/// Every field is `None` to disable that timeout entirely — which is what a
/// server has if nothing sets these, and why they exist: a connection that
/// stalls at any stage before its request head is complete otherwise holds a
/// task and a file descriptor for as long as the peer cares to keep the socket
/// open, at no cost to the peer. These are the cheapest defences there are, and
/// they bound only connections that are *not* making progress: a request that is
/// in flight, a body still arriving, and a response still streaming are all
/// untouched however long they take.
///
/// [`Default`] is the recommended posture rather than "off", so a provider gets
/// the protection without asking and a guest opts *out* deliberately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HttpTimeouts {
    /// From accept until the connection is ready to carry requests: the TLS
    /// handshake, and the wait for the first byte the HTTP version is read from.
    /// A TLS connection passes both stages, so it may take up to twice this
    /// before it counts as established.
    ///
    /// Default: 10s.
    pub handshake: Option<Duration>,
    /// How long a request head may take to arrive in full — the classic
    /// slowloris bound.
    ///
    /// On HTTP/1.1 this is **also the idle keep-alive limit**, because waiting
    /// for the next request on a kept-alive connection is waiting for a request
    /// head: an idle connection is closed after this long, and a client that
    /// wants another request opens a new one. HTTP/2 keeps its connections open
    /// and relies on [`h2_keep_alive`](Self::h2_keep_alive) instead.
    ///
    /// Default: 30s.
    pub header_read: Option<Duration>,
    /// How often an idle HTTP/2 connection is probed with a PING, and how long
    /// the ACK may take before the connection is dropped.
    ///
    /// HTTP/2 connections are meant to be long-lived, so there is no idle
    /// timeout to fall back on: without probing, a peer that vanishes without a
    /// FIN — a NAT that forgot the mapping, a killed VM, an unplugged cable —
    /// keeps its connection *and* its share of the concurrent-stream budget
    /// until the OS TCP keepalive notices, which is two hours by default on
    /// Linux.
    ///
    /// Default: 20s, so a dead peer is reclaimed within 40s.
    pub h2_keep_alive: Option<Duration>,
    /// How long a request **body** may take, before the allowance that
    /// [`body_min_rate`](Self::body_min_rate) earns it.
    ///
    /// [`header_read`](Self::header_read) bounds the head and nothing after it,
    /// so a peer that sends a complete head and then dribbles its body a byte at
    /// a time holds a connection, a task, a descriptor and the handler awaiting
    /// it for as long as it likes. That is the same attack as slowloris, moved
    /// one phase later, and the head timeout cannot answer it: the head arrived.
    ///
    /// A flat cap cannot answer it either — a large upload over a slow link is
    /// indistinguishable from a dribbler by elapsed time alone, and any number
    /// big enough for the upload is big enough for the attack. So the deadline
    /// is **earned**: a body starts with this long and gains more by arriving
    /// (see [`body_min_rate`](Self::body_min_rate)). An upload extends its own
    /// deadline by uploading; a dribbler does not send enough to extend it.
    ///
    /// Default: 30s.
    pub body_read: Option<Duration>,
    /// Bytes per second that extend [`body_read`](Self::body_read) — the floor
    /// a body must beat, not a rate it must sustain.
    ///
    /// The deadline is `start + body_read + received / body_min_rate`, so at the
    /// default a 100 MiB upload has over a day to arrive and a peer sending one
    /// byte a minute is closed at ~30s. `0` disables the allowance, making
    /// `body_read` a flat cap on the whole body.
    ///
    /// Default: 1024 (1 KiB/s).
    pub body_min_rate: u64,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            handshake: Some(Duration::from_secs(10)),
            header_read: Some(Duration::from_secs(30)),
            h2_keep_alive: Some(Duration::from_secs(20)),
            body_read: Some(Duration::from_secs(30)),
            body_min_rate: 1024,
        }
    }
}

/// Server-side TLS material for [`HttpServeOptions`].
///
/// The certificate and key travel **inline** rather than as paths, exactly as
/// `runtime:net` `listen` takes them: reading a file is the filesystem's
/// privilege, so a guest that wants to serve HTTPS from a cert on disk reads it
/// with `runtime:fs` under its own gate. Serving needs no grant beyond
/// `NetListen`.
pub struct HttpServerTls {
    /// PEM certificate chain, leaf first.
    pub cert: Vec<u8>,
    /// PEM private key.
    pub key: Vec<u8>,
    /// ALPN protocols to advertise. A server speaking both versions offers
    /// `["h2", "http/1.1"]` — ALPN order is the server's preference — which is
    /// what `runtime:http` `serve` sends when the guest names no `alpn`.
    pub alpn: Vec<String>,
}

/// An HTTP server backing `runtime:http` (the `serve((req) => res)` shape).
///
/// The HTTP version is the implementation's business, not this trait's: nothing
/// in the handoff below names one, so a provider is free to serve HTTP/1.1,
/// HTTP/2, or both on one listener (the default provider does the last, choosing
/// per connection). Responses are matched to requests by id rather than by
/// arrival order, which is what lets an HTTP/2 provider answer multiplexed
/// streams out of order without the runtime knowing.
///
/// The implementation owns the listener and every accepted connection, parsing
/// requests and writing responses; the runtime only supplies the response for
/// each request. The flow is a handoff: [`serve`](Self::serve) binds and starts
/// accepting, [`next_requests`](Self::next_requests) drains a batch of parsed
/// requests (each with an opaque id), and [`respond`](Self::respond) completes
/// an id. This lets a multi-threaded HTTP backend feed the single-threaded JS
/// isolate, amortizing the crossing over a batch. `serve` is capability-checked
/// on `Capability::NetListen`
/// (like `runtime:net` `listen`) before this is ever called; an embedder that
/// installs no `HttpServerProvider` has no `runtime:http` access at all.
pub trait HttpServerProvider: Send + Sync {
    /// Binds an HTTP server per `options` and starts accepting; resolves to
    /// (server id, bound-address info). `port` 0 picks an ephemeral port.
    fn serve(
        &self,
        options: HttpServeOptions,
    ) -> BoxFuture<Result<(u64, SocketInfo), ProviderError>>;

    /// Waits for inbound requests on server `id`, then drains any others already
    /// queued (up to `max`) so the single-threaded consumer can amortize the
    /// per-request crossing over a whole batch. Resolves to one-or-more
    /// `(request id, request)` pairs, or an **empty** vec once the server is
    /// closed. `max` bounds the batch (caller picks the cap); at least one
    /// request is awaited before returning.
    fn next_requests(
        &self,
        id: u64,
        max: usize,
    ) -> BoxFuture<Result<Vec<(u64, HttpServerRequest)>, ProviderError>>;

    /// Completes request `request_id` by sending `response` to its client
    /// (idempotent; a stale/unknown id is ignored).
    fn respond(
        &self,
        request_id: u64,
        response: HttpServerResponse,
    ) -> BoxFuture<Result<(), ProviderError>>;

    /// Watches request `request_id` for its client going away *before* a
    /// response was handed over — what backs the handler's `request.signal`, so
    /// work nobody will read can be abandoned.
    ///
    /// Resolves `true` if the peer disconnected first, and `false` once the
    /// response was delivered, the id is unknown, or the transport cannot tell.
    /// It **must** settle either way: a future that never resolves would hold a
    /// driven loop open for the life of the process.
    ///
    /// A disconnect *during* a streamed response body is a different event, and
    /// is already reported by [`HttpServerBody::Stream`] consumption ending.
    ///
    /// The default answers `false` immediately — correct for a transport with no
    /// way to observe the peer, and it costs such a transport nothing but a
    /// signal that never fires.
    fn request_disconnected(&self, request_id: u64) -> BoxFuture<bool> {
        let _ = request_id;
        Box::pin(std::future::ready(false))
    }

    /// Closes server `id`: stops accepting and tears the listener down
    /// (idempotent).
    fn close(&self, id: u64) -> BoxFuture<Result<(), ProviderError>>;
}

/// A sink for guest `console.*` output (SPEC.md §2.2).
///
/// console output is the **guest program's** output, not the runtime's
/// telemetry, so — like every other side effect — it arrives through an
/// injectable sink rather than reaching for an ambient global (no ambient
/// authority, DECISIONS.md D5). Because executed JS may be hostile
/// (ARCHITECTURE.md §7), an implementation may bound, rate-limit, drop, or
/// route output per-tenant; that is the embedder's choice, not the runtime's.
///
/// It is the lightest provider — an output sink needing no capability beyond
/// "may emit" — and is distinct from the heavier I/O providers above.
pub trait Console: Send + Sync {
    /// Records one already-formatted console message at `level`.
    fn write(&self, level: ConsoleLevel, message: &str);
}

#[cfg(test)]
mod tests {
    use super::*;

    // A trivial in-test implementation proves the traits are object-safe and
    // usable through `dyn`, which is how the runtime/driver consume them.
    struct FixedClock;
    impl Clock for FixedClock {
        fn monotonic_ms(&self) -> u64 {
            42
        }
        fn wall_ms(&self) -> u64 {
            1_000
        }
    }

    #[test]
    fn clock_is_object_safe() {
        let clock: &dyn Clock = &FixedClock;
        assert_eq!(clock.monotonic_ms(), 42);
        assert_eq!(clock.wall_ms(), 1_000);
    }

    #[test]
    fn provider_error_maps_to_exception() {
        let err = ProviderError::Entropy("no /dev/urandom".into());
        assert_eq!(err.exception_class(), ExceptionClass::Error);
        assert_eq!(
            ProviderError::Cancelled.exception_class(),
            ExceptionClass::NOT_ALLOWED
        );
    }
}
