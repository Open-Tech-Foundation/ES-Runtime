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

use es_runtime_common::{ExceptionClass, IntoException};

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
}

impl IntoException for ProviderError {
    fn exception_class(&self) -> ExceptionClass {
        match self {
            // A failed CSPRNG is an environment/operation failure, not a type
            // error; surface as a generic Error (Web crypto would use
            // OperationError, a DOMException — added with the prelude).
            ProviderError::Entropy(_) => ExceptionClass::Error,
            ProviderError::Cancelled => ExceptionClass::NOT_ALLOWED,
            ProviderError::Other(_) => ExceptionClass::Error,
        }
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
    /// The request body, already buffered. Streaming request bodies are a
    /// follow-up (SPEC §7); response bodies stream via [`ByteStream`].
    pub body: Option<Vec<u8>>,
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
    /// Response header name/value pairs, in order.
    pub headers: Vec<(String, String)>,
    /// The response body, streamed as byte-chunks.
    pub body: ByteStream,
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

    /// Loads the UTF-8 source for a canonical id (as returned by
    /// [`resolve`](Self::resolve)).
    fn load(&self, specifier: &str) -> BoxFuture<Result<String, ProviderError>>;
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

    /// Records a guest `process.exit(code)` request. The runtime also halts
    /// execution (via its interrupt handle); the embedder reads
    /// [`requested_exit_code`](Self::requested_exit_code) after the run to learn
    /// the code and that exit (not an error) caused the stop.
    fn exit(&self, code: i32);

    /// The exit code requested via [`exit`](Self::exit), if any.
    fn requested_exit_code(&self) -> Option<i32>;
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
