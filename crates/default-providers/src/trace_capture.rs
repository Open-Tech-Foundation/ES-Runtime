//! Test-only sink that captures emitted `tracing` events, so a test can assert
//! that the runtime *reported* something rather than only that it survived.
//!
//! The servers log from spawned tasks, so `with_default` — which is
//! thread-local — would miss every event that matters here. That leaves the
//! process-global slot, which this module claims exactly once for the whole
//! test binary; tests then search a shared buffer for a substring rather than
//! asserting counts, because they run concurrently against it.

use std::fmt::Write as _;
use std::sync::{Arc, Mutex, OnceLock};

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;

type Buffer = Arc<Mutex<Vec<String>>>;

fn buffer() -> &'static Buffer {
    static BUFFER: OnceLock<Buffer> = OnceLock::new();
    BUFFER.get_or_init(Buffer::default)
}

/// Renders each field as `name=value`, with `message` first and unprefixed —
/// close enough to the `fmt` layer's output that an assertion reads like the
/// line an operator would see.
struct Render(String);

impl Visit for Render {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, " {value:?}");
        } else {
            let _ = write!(self.0, " {}={:?}", field.name(), value);
        }
    }
}

struct CaptureLayer;

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for CaptureLayer {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        ctx: Context<'_, S>,
    ) {
        // The span's fields are recorded once, at creation, and stashed on the
        // span itself: an event only carries a *reference* to its parent, so
        // without this the `peer` a connection span holds is unreachable by the
        // time the event that needs it arrives.
        let mut render = Render(String::new());
        attrs.record(&mut render);
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(render);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        // Only *our* instrumentation. `tracing-subscriber` bridges the `log`
        // crate, so a dependency's `log::trace!` arrives here as an event with
        // the target `"log"` — and because it is emitted on our task, it lands
        // inside our connection span and carries its `peer` field. A test
        // asserting "a healthy connection logged nothing" then fails on
        // `mio::poll` saying it deregistered an event source.
        //
        // Whether that bridge is active is not this crate's decision: it turns
        // on when anything else in the workspace pulls in `tracing-subscriber`,
        // which `esdev`'s bundler does. These tests are about what the runtime
        // reports, so a record it did not emit is noise by definition.
        if event.metadata().target() == "log" {
            return;
        }
        let mut line = format!(
            "[{}] {}",
            event.metadata().level(),
            event.metadata().target()
        );
        for span in ctx.event_scope(event).into_iter().flatten() {
            let _ = write!(line, " {}{{", span.name());
            if let Some(fields) = span.extensions().get::<Render>() {
                line.push_str(fields.0.trim_start());
            }
            line.push('}');
        }
        let mut render = Render(line);
        event.record(&mut render);
        buffer().lock().unwrap().push(render.0);
    }
}

/// Installs the capture layer, once per process. Safe to call from every test.
pub fn install() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // No filter: this binary's only subscriber is this one, and a test that
        // asserts on a `debug!` cannot have it filtered out from under it.
        let _ = tracing_subscriber::registry().with(CaptureLayer).try_init();
    });
}

/// Every line captured so far that contains **all** of `needles`.
///
/// All of them, rather than one: the buffer is shared by every test in the
/// binary and they run concurrently, so "a line saying `tls handshake failed`"
/// is not the same question as "*my* connection's line". The peer address is
/// what makes a line this test's own.
pub fn lines_containing(needles: &[&str]) -> Vec<String> {
    buffer()
        .lock()
        .unwrap()
        .iter()
        .filter(|line| needles.iter().all(|needle| line.contains(needle)))
        .cloned()
        .collect()
}

/// Waits for a line matching `needles` to appear, up to `timeout`.
///
/// Logging happens on a task this test does not join, so "has it logged yet" is
/// a race; polling makes the assertion about *whether* the event is emitted
/// rather than about scheduling order. Returning early on someone else's
/// matching line would reintroduce exactly that race, which is why the match is
/// conjunctive.
pub async fn wait_for(needles: &[&str], timeout: std::time::Duration) -> Vec<String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let found = lines_containing(needles);
        if !found.is_empty() || std::time::Instant::now() >= deadline {
            return found;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
