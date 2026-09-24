//! The child's half of coverage: V8's precise coverage, collected through an
//! inspector session this process speaks to itself (DECISIONS D108).
//!
//! The session is attached before the entry module is compiled, holding the
//! program as `--inspect-brk` would while `Profiler.startPreciseCoverage` is
//! dispatched and then releasing it — so the first statement already counts.
//! When the file's tests have drained, `runtime:test` asks for the counts,
//! and they are written for the parent with the code each module ran as.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::task::Waker;

use es_runtime_cli_common::{Inspector, InspectorTransport};
use serde_json::{Value as Json, json};

/// What a module ran as: the text V8 compiled, and — when esdev reprinted it —
/// where each of its positions came from in the file as written.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Executed {
    /// The text V8 compiled.
    pub text: String,
    /// What was put before the file's own first line, in UTF-16 units: the
    /// generated positions on line 0 are this much further right.
    pub prelude: u32,
    /// `[generated line, generated column, original line, original column]`,
    /// all 0-based, columns in UTF-16 units. `None` when the file ran as
    /// written.
    pub mappings: Option<Vec<[u32; 4]>>,
}

fn executed() -> &'static Mutex<HashMap<String, Executed>> {
    static EXECUTED: OnceLock<Mutex<HashMap<String, Executed>>> = OnceLock::new();
    EXECUTED.get_or_init(Mutex::default)
}

static RECORDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether modules' executed text is being kept, for coverage.
pub fn recording() -> bool {
    RECORDING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Keeps what `url` ran as, when coverage is being collected.
pub fn record(url: &str, module: Executed) {
    if recording()
        && let Ok(mut all) = executed().lock()
    {
        all.insert(url.to_string(), module);
    }
}

/// The in-process client: messages queued for V8, and V8's answers by id.
#[derive(Default)]
struct Client {
    inbox: RefCell<VecDeque<String>>,
    answers: RefCell<HashMap<u64, Json>>,
    fresh: Cell<bool>,
    /// The event loop's waker, so a message queued while it is parked is seen.
    driver: RefCell<Option<Waker>>,
    /// Whoever is waiting for an answer.
    waiting: RefCell<Option<Waker>>,
    next: Cell<u64>,
    /// Each `takePreciseCoverage` result, in order.
    taken: RefCell<Vec<Json>>,
}

impl Client {
    fn queue(&self, method: &str, params: Json) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.inbox
            .borrow_mut()
            .push_back(json!({ "id": id, "method": method, "params": params }).to_string());
        if let Some(waker) = self.driver.borrow().as_ref() {
            waker.wake_by_ref();
        }
        id
    }
}

impl InspectorTransport for Client {
    fn try_recv(&self) -> Option<String> {
        self.inbox.borrow_mut().pop_front()
    }

    fn recv_blocking(&self) -> Option<String> {
        self.try_recv()
    }

    fn send(&self, message: &str) {
        let Ok(message) = serde_json::from_str::<Json>(message) else {
            return;
        };
        if let Some(id) = message.get("id").and_then(Json::as_u64) {
            self.answers.borrow_mut().insert(id, message);
            if let Some(waker) = self.waiting.borrow_mut().take() {
                waker.wake();
            }
        }
    }

    fn take_new_connection(&self) -> bool {
        self.fresh.replace(false)
    }

    fn set_waker(&self, waker: Waker) {
        *self.driver.borrow_mut() = Some(waker);
    }
}

thread_local! {
    static CLIENT: RefCell<Option<Rc<Client>>> = const { RefCell::new(None) };
}

/// The inspector a run attaches to collect coverage: precise counts started,
/// and the program released, before its first statement.
pub fn inspector() -> Inspector {
    RECORDING.store(true, std::sync::atomic::Ordering::Relaxed);
    let client = Rc::new(Client {
        fresh: Cell::new(true),
        ..Client::default()
    });
    client.queue("Profiler.enable", json!({}));
    client.queue(
        "Profiler.startPreciseCoverage",
        json!({ "callCount": true, "detailed": true }),
    );
    client.queue("Runtime.runIfWaitingForDebugger", json!({}));
    CLIENT.with_borrow_mut(|slot| *slot = Some(client.clone()));
    Inspector {
        transport: client,
        wait: true,
    }
}

/// Takes the counts so far — V8 resets them — and keeps them for [`write`].
/// Nothing when coverage is not being collected.
pub async fn take() {
    let Some(client) = CLIENT.with_borrow(Clone::clone) else {
        return;
    };
    let id = client.queue("Profiler.takePreciseCoverage", json!({}));
    let answer = std::future::poll_fn(|cx| match client.answers.borrow_mut().remove(&id) {
        Some(answer) => std::task::Poll::Ready(answer),
        None => {
            *client.waiting.borrow_mut() = Some(cx.waker().clone());
            std::task::Poll::Pending
        }
    })
    .await;
    if let Some(result) = answer.get("result") {
        client.taken.borrow_mut().push(result.clone());
    }
}

/// Writes what was collected for the parent: each take's scripts that are
/// files under `root`, and what each of those ran as.
pub fn write(out: &Path, root: &Path) {
    let Some(client) = CLIENT.with_borrow(Clone::clone) else {
        return;
    };
    let under_root = |url: &str| {
        url::Url::parse(url)
            .ok()
            .and_then(|url| url.to_file_path().ok())
            .is_some_and(|path| path.starts_with(root))
    };
    let takes: Vec<Json> = client
        .taken
        .borrow()
        .iter()
        .map(|result| {
            let scripts: Vec<&Json> = result
                .get("result")
                .and_then(Json::as_array)
                .into_iter()
                .flatten()
                .filter(|script| {
                    script
                        .get("url")
                        .and_then(Json::as_str)
                        .is_some_and(under_root)
                })
                .collect();
            json!(scripts)
        })
        .collect();
    let modules: HashMap<String, Executed> = executed()
        .lock()
        .map(|all| {
            all.iter()
                .filter(|(url, _)| under_root(url))
                .map(|(url, module)| (url.clone(), module.clone()))
                .collect()
        })
        .unwrap_or_default();
    let _ = std::fs::write(
        out,
        json!({ "takes": takes, "modules": modules }).to_string(),
    );
}
