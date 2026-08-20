//! `runtime:watch` — file-change events, delivered to guest JS.
//!
//! **`esdev` only.** A watcher is development machinery by definition: the
//! thing it watches is source, and there is no source on a production box.
//! `esrun` therefore does not carry this module, and a program that imports it
//! there fails at load rather than quietly finding no events.
//!
//! It exists because [`crate::watch`] — `esdev --watch` — cannot serve the case
//! it was built for. That watcher's answer to a change is to `SIGTERM` the
//! child and start another, which is right for "rerun this program" and wrong
//! for a dev server: a framework's server has to *stay up* through a save,
//! drop the three cached chunks whose dependencies changed, and keep the other
//! thirty-seven — along with its compile server, its websocket clients and its
//! warm bundles. That is a decision only the program holding those caches can
//! make, so what it needs from the runtime is the events, not a restart.
//!
//! Two things follow from that use, and both are in the API:
//!
//! * **The watch set changes while it runs.** A bundle's dependencies are known
//!   only after it is built, so a shared `lib/` outside `app/` starts being
//!   watched the moment a chunk proves it depends on it. [`add`] and [`remove`]
//!   are therefore ordinary methods, not construction-time arguments.
//! * **Events are debounced per path.** One editor save is several filesystem
//!   events — a truncate, a write, sometimes a rename over the top — and a
//!   consumer that rebuilds on each of them rebuilds three times, the last two
//!   against a file that is already finished. The quiet period is the same one
//!   `--watch` uses, for the same reason.
//!
//! Paths are resolved through the run's own [`FileSystem`] view, so the root
//! jail (D25) and `--allow-read` (D38) bound what can be watched exactly as
//! they bound what can be read. Watching a directory is a read of its contents
//! in every way that matters — you learn which files exist and when they are
//! touched — so it is gated on `FileRead` and scoped by the same list.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use es_runtime_cli_common::{
    AsyncOp, ExtensionContext, FileSystem, HostExtension, HostModule, OpDecl, OpError, Value,
};
use es_runtime_common::{Capability, ErrorCode, ExceptionClass, IntoException};
use notify::event::EventKind;
use notify::{Event, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// How long a path must be quiet before its event is delivered.
///
/// The same 120ms `--watch` waits, and for the same reason: one save is a burst
/// of events, and a consumer that acts on the first acts on a half-written
/// file. Per path rather than global — a build touching forty files should not
/// hold back the one the developer just saved.
const DEBOUNCE: Duration = Duration::from_millis(120);

/// One delivered change: what happened, and to what.
struct Change {
    kind: &'static str,
    path: String,
}

/// A live watcher, as the guest's handle number refers to it.
struct WatchHandle {
    /// The notify watcher itself. Held because dropping it stops the watch —
    /// this field being unread is the point.
    watcher: notify::RecommendedWatcher,
    /// Whether new paths are added recursively, from the options the guest
    /// opened with. Remembered so [`add`] does not need to be told twice.
    recursive: bool,
    /// Debounced changes waiting to be read. Polled in place rather than taken,
    /// so an abandoned `next()` promise cannot lose the queue.
    events: Rc<RefCell<mpsc::UnboundedReceiver<Change>>>,
    /// The paths currently watched, canonicalized. Kept so `remove` can undo
    /// exactly what `add` did.
    paths: Vec<PathBuf>,
}

/// Every watcher this agent has open, by handle.
type Watchers = Rc<RefCell<HashMap<u64, WatchHandle>>>;

/// The `runtime:watch` extension.
pub struct WatchExtension;

const MODULES: &[HostModule] = &[HostModule {
    specifier: "runtime:watch",
    source: include_str!("watch.js"),
}];

impl HostExtension for WatchExtension {
    fn modules(&self) -> &[HostModule] {
        MODULES
    }

    fn ops(&self, ctx: &ExtensionContext<'_>) -> Vec<OpDecl> {
        let watchers: Watchers = Rc::new(RefCell::new(HashMap::new()));
        let next_id = Rc::new(RefCell::new(0u64));
        let mut ops = Vec::new();

        // open(paths, recursive) -> handle
        let fs = ctx.file_system.clone();
        let map = watchers.clone();
        let ids = next_id.clone();
        ops.push(
            OpDecl::r#async("watch_open", move |args| -> AsyncOp {
                let fs = fs.clone();
                let map = map.clone();
                let ids = ids.clone();
                let paths = arg_paths(&args, 0);
                let recursive = matches!(args.get(1), Some(Value::Bool(true)));
                Box::pin(async move {
                    // Resolved before the watcher exists, so a denied path is
                    // an error from `watch()` itself rather than a watcher that
                    // silently never fires.
                    let mut resolved = Vec::with_capacity(paths.len());
                    for path in &paths {
                        resolved.push(PathBuf::from(real_path(&fs, path).await?));
                    }
                    let (tx, rx) = mpsc::unbounded_channel::<Change>();
                    let watcher = start_watcher(tx)?;
                    let mut handle = WatchHandle {
                        watcher,
                        recursive,
                        events: Rc::new(RefCell::new(rx)),
                        paths: Vec::new(),
                    };
                    for path in resolved {
                        watch_path(&mut handle, path)?;
                    }
                    let id = {
                        let mut ids = ids.borrow_mut();
                        *ids += 1;
                        *ids
                    };
                    map.borrow_mut().insert(id, handle);
                    Ok(Value::Number(id as f64))
                })
            })
            .requires(Capability::FileRead),
        );

        // next(handle) -> { kind, path } | null
        let map = watchers.clone();
        ops.push(
            OpDecl::r#async("watch_next", move |args| -> AsyncOp {
                let map = map.clone();
                let id = arg_id(&args, 0);
                Box::pin(async move {
                    let events = {
                        let watchers = map.borrow();
                        match watchers.get(&id) {
                            // A closed watcher yields the end of the stream,
                            // not an error: the iterator reading it is racing
                            // the `close()` that ended it, and that race is
                            // ordinary rather than exceptional.
                            None => return Ok(Value::Null),
                            Some(handle) => handle.events.clone(),
                        }
                    };
                    // Borrowed per poll rather than held: nothing else touches
                    // this receiver, and an abandoned promise gives it back.
                    let change = NextChange { events }.await;
                    Ok(match change {
                        Some(change) => Value::Object(vec![
                            ("kind".to_string(), Value::String(change.kind.to_string())),
                            ("path".to_string(), Value::String(change.path)),
                        ]),
                        None => Value::Null,
                    })
                })
            })
            .requires(Capability::FileRead),
        );

        // add(handle, path)
        let fs = ctx.file_system.clone();
        let map = watchers.clone();
        ops.push(
            OpDecl::r#async("watch_add", move |args| -> AsyncOp {
                let fs = fs.clone();
                let map = map.clone();
                let id = arg_id(&args, 0);
                let path = arg_str(&args, 1);
                Box::pin(async move {
                    let real = PathBuf::from(real_path(&fs, &path).await?);
                    let mut watchers = map.borrow_mut();
                    let handle = require_handle(&mut watchers, id)?;
                    // Watching one tree twice delivers every event twice on the
                    // backends that allow it — and **a parent is the same tree**
                    // when the watcher is recursive, which is the spelling that
                    // actually happens: a dev server watches `app/` and then
                    // adds the package `app/` lives in as a dependency.
                    // Comparing for equality alone caught the exact repeat and
                    // let the overlap through.
                    if handle.covered(&real) {
                        return Ok(Value::Bool(false));
                    }
                    // The other direction: the new path covers trees already
                    // watched, so they stop being watches of their own rather
                    // than becoming a second delivery of the same events.
                    for covered in handle.covering(&real) {
                        let _ = handle.watcher.unwatch(&covered);
                        handle.paths.retain(|p| *p != covered);
                    }
                    watch_path(handle, real)?;
                    Ok(Value::Bool(true))
                })
            })
            .requires(Capability::FileRead),
        );

        // remove(handle, path)
        let fs = ctx.file_system.clone();
        let map = watchers.clone();
        ops.push(
            OpDecl::r#async("watch_remove", move |args| -> AsyncOp {
                let fs = fs.clone();
                let map = map.clone();
                let id = arg_id(&args, 0);
                let path = arg_str(&args, 1);
                Box::pin(async move {
                    let real = PathBuf::from(real_path(&fs, &path).await?);
                    let mut watchers = map.borrow_mut();
                    let handle = require_handle(&mut watchers, id)?;
                    let Some(at) = handle.paths.iter().position(|p| *p == real) else {
                        return Ok(Value::Bool(false));
                    };
                    handle
                        .watcher
                        .unwatch(&real)
                        .map_err(|e| watch_error(&format!("cannot stop watching: {e}")))?;
                    handle.paths.remove(at);
                    Ok(Value::Bool(true))
                })
            })
            .requires(Capability::FileRead),
        );

        // close(handle)
        let map = watchers;
        ops.push(OpDecl::sync("watch_close", move |args| {
            let id = arg_id(&args, 0);
            // Dropping the handle drops the notify watcher, which is what
            // actually stops the OS-level watch and closes its descriptors.
            let closed = map.borrow_mut().remove(&id).is_some();
            Ok(Value::Bool(closed))
        }));

        ops
    }
}

/// The pending half of `watch_next`: yields the next debounced change, or
/// `None` when every sender is gone.
struct NextChange {
    events: Rc<RefCell<mpsc::UnboundedReceiver<Change>>>,
}

impl std::future::Future for NextChange {
    type Output = Option<Change>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.events.borrow_mut().poll_recv(cx)
    }
}

/// Starts the OS watcher and the debounce task in front of it.
///
/// Two channels, not one. The first carries raw filesystem events off notify's
/// own thread; the second carries what survives the quiet period. They cannot
/// be the same channel because the debounce has to *hold* an event for a while
/// and then decide, which a receiver the guest is awaiting cannot do.
fn start_watcher(
    out: mpsc::UnboundedSender<Change>,
) -> Result<notify::RecommendedWatcher, OpError> {
    let (raw_tx, raw_rx) = mpsc::unbounded_channel::<(&'static str, PathBuf)>();
    let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        // `inotify` reports reads: a program opening a watched file raises
        // `Access(Open)` on it, and delivering that as a change is how a
        // watcher ends up reacting to its own consumer.
        if !crate::watch::is_change(&event.kind) {
            return;
        }
        let kind = kind_name(&event.kind);
        for path in event.paths {
            let _ = raw_tx.send((kind, path));
        }
    })
    .map_err(|e| watch_error(&format!("cannot start the file watcher: {e}")))?;
    // An ordinary task, not a local one: the debounce holds nothing from the
    // isolate, so it runs wherever the driver's runtime puts it and keeps
    // coalescing while JS is busy.
    tokio::spawn(debounce(raw_rx, out));
    Ok(watcher)
}

/// Holds each path's latest event until that path has been quiet for
/// [`DEBOUNCE`], then delivers it.
///
/// Ends when the watcher is dropped (the raw sender goes with it) or when the
/// guest stops reading, whichever comes first.
async fn debounce(
    mut raw: mpsc::UnboundedReceiver<(&'static str, PathBuf)>,
    out: mpsc::UnboundedSender<Change>,
) {
    let mut pending: HashMap<PathBuf, (&'static str, tokio::time::Instant)> = HashMap::new();
    loop {
        let next_deadline = pending.values().map(|(_, at)| *at).min();
        let event = match next_deadline {
            None => raw.recv().await,
            Some(deadline) => tokio::select! {
                event = raw.recv() => event,
                () = tokio::time::sleep_until(deadline) => {
                    let now = tokio::time::Instant::now();
                    let due: Vec<PathBuf> = pending
                        .iter()
                        .filter(|(_, (_, at))| *at <= now)
                        .map(|(path, _)| path.clone())
                        .collect();
                    for path in due {
                        let Some((kind, _)) = pending.remove(&path) else { continue };
                        let change = Change {
                            kind,
                            path: path.to_string_lossy().into_owned(),
                        };
                        // The guest stopped reading — nothing left to deliver to.
                        if out.send(change).is_err() {
                            return;
                        }
                    }
                    continue;
                }
            },
        };
        match event {
            Some((kind, path)) => {
                let deadline = tokio::time::Instant::now() + DEBOUNCE;
                let kind = match pending.get(&path) {
                    Some((held, _)) => merge(held, kind),
                    None => kind,
                };
                pending.insert(path, (kind, deadline));
            }
            None => return,
        }
    }
}

impl WatchHandle {
    /// Whether this path is already being watched — itself, or inside a tree a
    /// recursive watch already covers.
    fn covered(&self, path: &Path) -> bool {
        self.paths
            .iter()
            .any(|watched| watched == path || (self.recursive && path.starts_with(watched)))
    }

    /// The paths this one would swallow: watches that sit inside it, and so
    /// stop being watches of their own once it is added.
    fn covering(&self, path: &Path) -> Vec<PathBuf> {
        if !self.recursive {
            return Vec::new();
        }
        self.paths
            .iter()
            .filter(|watched| watched.starts_with(path))
            .cloned()
            .collect()
    }
}

/// Adds one already-resolved path to a watcher's set.
fn watch_path(handle: &mut WatchHandle, path: PathBuf) -> Result<(), OpError> {
    let mode = if handle.recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    handle
        .watcher
        .watch(&path, mode)
        .map_err(|e| watch_error(&format!("cannot watch {}: {e}", path.display())))?;
    handle.paths.push(path);
    Ok(())
}

/// What a burst of events about one path adds up to.
///
/// Last-one-wins is wrong here, and the way it is wrong matters: a save of a
/// *new* file raises a create and then a write, so last-wins reports "modified"
/// for a file that did not exist a moment ago — and a dev server deciding
/// whether to add a route reads that as "nothing new". So a create is sticky,
/// and a remove — the only event that says the path is *gone* — beats
/// everything. The exception is the atomic save every editor does: remove the
/// file, put the new one in its place. The path existed before and exists now,
/// which to a consumer is a modification, whatever the two syscalls were.
fn merge(held: &'static str, incoming: &'static str) -> &'static str {
    match (held, incoming) {
        (_, "removed") => "removed",
        ("removed", "created") => "modified",
        ("created", _) => "created",
        _ => incoming,
    }
}

/// The event's name in guest terms.
///
/// Deliberately fewer names than notify has. A consumer's question is "does
/// what I cached still stand?", which has three answers; the backends disagree
/// about the rest, and a name that means something different on macOS than on
/// Linux is worse than no name.
fn kind_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Create(_) => "created",
        EventKind::Remove(_) => "removed",
        _ => "modified",
    }
}

/// Resolves a guest path the same way `runtime:fs` does — through the jail and
/// the `--allow-read` list — and returns the real path events will name.
async fn real_path(fs: &std::sync::Arc<dyn FileSystem>, path: &str) -> Result<String, OpError> {
    fs.real_path(path.to_string()).await.map_err(|e| {
        OpError::new(e.exception_class(), e.exception_message()).with_code_opt(e.code())
    })
}

fn require_handle(
    watchers: &mut HashMap<u64, WatchHandle>,
    id: u64,
) -> Result<&mut WatchHandle, OpError> {
    watchers
        .get_mut(&id)
        .ok_or_else(|| OpError::new(ExceptionClass::Error, "this watcher is closed"))
}

fn watch_error(message: &str) -> OpError {
    OpError::new(ExceptionClass::Error, message).with_code(ErrorCode::Io)
}

fn arg_str(args: &[Value], i: usize) -> String {
    args.get(i)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn arg_id(args: &[Value], i: usize) -> u64 {
    args.get(i).and_then(Value::as_number).unwrap_or(0.0) as u64
}

fn arg_paths(args: &[Value], i: usize) -> Vec<String> {
    match args.get(i) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}
