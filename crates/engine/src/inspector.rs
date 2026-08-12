//! The V8 inspector — and the switch that keeps it out of the binary that runs
//! in production (DECISIONS.md D59).
//!
//! An inspector port is a **total bypass of the capability model**: attach to
//! one and you own the isolate, whatever `--deny-all` said at launch. That is
//! why `esrun` has never had one, and why the V8 half of this module is
//! compiled only when the build was explicitly asked for it:
//!
//! ```sh
//! ES_RUNTIME_INSPECTOR=1 cargo build --release -p es-runtime-dev-cli
//! ```
//!
//! The switch is an environment variable read by this crate's `build.rs` rather
//! than a Cargo feature, because Cargo unifies features across everything built
//! in one invocation — a feature declared by `dev-cli` would be on in the
//! `esrun` built beside it. See `build.rs` for the whole argument, including the
//! guard in `runtime-cli` that makes the separation something a build can fail
//! on rather than something to remember.
//!
//! **The API below exists either way.** With the switch off,
//! [`Engine::attach_inspector`](crate::Engine::attach_inspector) returns
//! [`Error::Unsupported`](crate::Error::Unsupported), so `runtime` and
//! `cli-common` compile identically in both builds and a binary without an
//! inspector *says so* instead of accepting `--inspect` and ignoring it.
//!
//! ## What crosses the boundary
//!
//! Only [`InspectorTransport`], in plain Rust. The Chrome DevTools Protocol is
//! JSON text in both directions, so the engine never needs to know that the
//! client is a WebSocket — the server lives in `esdev`, where its dependencies
//! stay, exactly as `oxc` and `rolldown` do.

/// The two-way channel an inspector session speaks the Chrome DevTools Protocol
/// over.
///
/// Implemented by the embedder: `esdev` backs it with a WebSocket server on its
/// own thread, and this crate gains no transport dependency for it.
///
/// Every method takes `&self`, deliberately. The transport is shared between
/// the tick-time pump and the message loop that runs while execution is paused,
/// and V8 can re-enter the latter (a breakpoint hit inside
/// `Debugger.evaluateOnCallFrame`), so a `&mut` design would need a `RefCell`
/// that a nested pause would panic on.
pub trait InspectorTransport {
    /// A message from the client, or `None` if none is waiting. Never blocks.
    fn try_recv(&self) -> Option<String>;

    /// Blocks until the next message, or returns `None` once the client is
    /// gone.
    ///
    /// Called only while execution is paused at a breakpoint. That is the one
    /// moment when blocking the isolate's thread is not merely acceptable but
    /// required: the program *is* stopped, and the only thing that can start it
    /// again arrives here.
    fn recv_blocking(&self) -> Option<String>;

    /// Hands a CDP response or notification to the client.
    ///
    /// A message sent while nobody is attached is dropped, which is what a
    /// notification with no listener is.
    fn send(&self, message: &str);

    /// Whether a **new** client has attached since this was last asked.
    ///
    /// A fresh client needs a fresh session: V8 replays `Debugger.scriptParsed`
    /// for the scripts it already knows only on a session's *first*
    /// `Debugger.enable`, so a DevTools that reattaches to the old session opens
    /// on an empty Sources pane.
    fn take_new_connection(&self) -> bool;

    /// Hands over the driver's waker so a message arriving while the event loop
    /// is parked wakes it, instead of being noticed on whatever tick happens
    /// next.
    ///
    /// Without this, setting a breakpoint on an idle server does nothing
    /// visible until the next request arrives. A transport with no thread of
    /// its own may ignore it.
    fn set_waker(&self, waker: std::task::Waker) {
        let _ = waker;
    }
}

/// How an inspector session should start.
#[derive(Debug, Clone)]
pub struct InspectorOptions {
    /// Hold the program before its first statement until a client attaches and
    /// releases it with `Runtime.runIfWaitingForDebugger` — `--inspect-brk`.
    ///
    /// Without it a short program can be over before a debugger finishes
    /// connecting; with it, the first line of the entry module is where you
    /// land.
    pub wait_for_debugger: bool,
    /// The name DevTools shows for the execution context.
    pub context_name: String,
}

impl Default for InspectorOptions {
    fn default() -> Self {
        InspectorOptions {
            wait_for_debugger: false,
            context_name: "ES-Runtime".to_string(),
        }
    }
}

#[cfg(inspector)]
pub(crate) use imp::Inspector;

#[cfg(inspector)]
mod imp {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use v8::inspector::{
        ChannelImpl, StringBuffer, StringView, V8Inspector, V8InspectorClient,
        V8InspectorClientImpl, V8InspectorClientTrustLevel, V8InspectorSession,
    };

    use super::{InspectorOptions, InspectorTransport};

    /// The one context group there is. This runtime gives each agent its own
    /// isolate (a worker is a thread with its own engine), so a group never has
    /// a second context to tell apart.
    const CONTEXT_GROUP_ID: i32 = 1;

    /// A live inspector: V8's `V8Inspector`, the session connected to the
    /// client, and the flags the client callbacks and the engine share.
    ///
    /// Field order matters and is enforced by [`Drop`] below rather than left to
    /// it — see there.
    pub(crate) struct Inspector {
        /// The current session. Replaced when a new client attaches; `None`
        /// only between construction and the first `connect`.
        ///
        /// A `RefCell` holding a session that is *only ever* borrowed shared:
        /// dispatching a message takes `&self`, so a nested dispatch (a
        /// breakpoint inside `Debugger.evaluateOnCallFrame`) borrows again and
        /// must not conflict. The single `borrow_mut` is the reconnect below,
        /// which happens at tick time and never inside a dispatch.
        session: Rc<RefCell<Option<V8InspectorSession>>>,
        /// Shared with the client callback that clears it; see [`Client`].
        waiting: Rc<Cell<bool>>,
        transport: Rc<dyn InspectorTransport>,
        /// V8's inspector. Every session is created from it and calls back into
        /// it when dropped.
        inspector: V8Inspector,
    }

    impl Drop for Inspector {
        fn drop(&mut self) {
            // A session's C++ destructor disconnects it from the `V8Inspector`
            // that created it, so it has to go first. Rust would otherwise drop
            // the `V8Inspector` field before the `Rc` inside the client that
            // holds the session, and the disconnect would run against freed
            // memory. Field order alone cannot express that, because the client
            // — and through it a second handle to this same session — is owned
            // by the `V8Inspector`.
            self.session.borrow_mut().take();
        }
    }

    impl Inspector {
        /// Creates the inspector for `isolate` and announces `context` to it.
        ///
        /// No session yet: one is connected when a client actually attaches, and
        /// again for each client after it. Connecting eagerly would look tidier
        /// and be wrong — the first client would then be handed the session made
        /// before it arrived, and the reconnect below would throw that session
        /// away *after* it had already answered the client's `Debugger.enable`.
        pub(crate) fn new(
            isolate: &mut v8::Isolate,
            context: &v8::Global<v8::Context>,
            transport: Rc<dyn InspectorTransport>,
            options: &InspectorOptions,
        ) -> Self {
            let session = Rc::new(RefCell::new(None));
            let paused = Rc::new(Cell::new(false));
            let waiting = Rc::new(Cell::new(options.wait_for_debugger));

            let client = V8InspectorClient::new(Box::new(Client {
                transport: transport.clone(),
                session: session.clone(),
                paused: paused.clone(),
                waiting: waiting.clone(),
                isolate: std::ptr::from_mut(isolate),
                context: context.clone(),
            }));
            let inspector = V8Inspector::create(isolate, client);

            {
                v8::scope!(let scope, isolate);
                let context = v8::Local::new(scope, context);
                let name = utf16(&options.context_name);
                // `isDefault` is what makes DevTools treat this as the context
                // to evaluate in; without it the console pane has no target.
                let aux = utf16(r#"{"isDefault":true}"#);
                inspector.context_created(
                    context,
                    CONTEXT_GROUP_ID,
                    StringView::from(&name[..]),
                    StringView::from(&aux[..]),
                );
            }

            Inspector {
                session,
                waiting,
                transport,
                inspector,
            }
        }

        /// Connects a fresh session, dropping any previous one.
        fn connect_session(&mut self) {
            // Dropped before the new one is created: a session's destructor
            // reaches into the inspector, and doing that *after* connecting the
            // replacement would disconnect them in an order V8 does not expect.
            let mut slot = self.session.borrow_mut();
            slot.take();
            let channel = v8::inspector::Channel::new(Box::new(Sink {
                transport: self.transport.clone(),
            }));
            *slot = Some(self.inspector.connect(
                CONTEXT_GROUP_ID,
                channel,
                StringView::empty(),
                // The client is a debugger the developer started on their own
                // machine, over a loopback port. There is no lesser trust level
                // that would still let it inspect anything.
                V8InspectorClientTrustLevel::FullyTrusted,
            ));
        }

        /// Delivers whatever the client has sent since the last tick.
        ///
        /// Non-blocking by construction: this runs on the event loop's thread,
        /// between the program's own work.
        pub(crate) fn poll(&mut self) {
            self.adopt_new_client();
            while let Some(message) = self.transport.try_recv() {
                // Re-checked before *every* dispatch, not once at the top: a
                // client can attach and send its first message between two
                // iterations, and dispatching that message to the session the
                // last client left behind answers nobody.
                self.adopt_new_client();
                dispatch(&self.session, &message);
            }
        }

        /// Gives a client that has just attached a session of its own.
        fn adopt_new_client(&mut self) {
            if self.transport.take_new_connection() {
                self.connect_session();
            }
        }

        /// Blocks until a client has attached and released the program with
        /// `Runtime.runIfWaitingForDebugger`, then arranges to stop on the first
        /// statement that runs.
        ///
        /// Returns early if the client disconnects without ever releasing us —
        /// a debugger that gave up is not a reason for the program never to
        /// start.
        pub(crate) fn wait_for_debugger(&mut self) {
            while self.waiting.get() {
                let Some(message) = self.transport.recv_blocking() else {
                    self.waiting.set(false);
                    return;
                };
                // After the receive, not before it. The first thing that happens
                // here is a block on an empty queue, and the client that ends it
                // announces itself and sends its first message in one breath —
                // so a check before the receive is always one message too early,
                // and the `Debugger.enable` it is answering would be dispatched
                // to no session at all.
                self.adopt_new_client();
                dispatch(&self.session, &message);
            }
            // Scheduled after the client is done setting up, so its
            // `Debugger.enable` has already been dispatched and the pause it
            // asks for can be reported.
            if let Some(session) = self.session.borrow().as_ref() {
                let reason = utf16("Break on start");
                session.schedule_pause_on_next_statement(
                    StringView::from(&reason[..]),
                    StringView::empty(),
                );
            }
        }

        /// The transport, for the embedder's waker hand-off.
        pub(crate) fn transport(&self) -> &Rc<dyn InspectorTransport> {
            &self.transport
        }
    }

    /// Hands one CDP message to the session.
    ///
    /// The message is JSON, which may carry any text a developer typed into a
    /// breakpoint condition, so it is converted to UTF-16 rather than handed
    /// over as bytes — a `StringView` built from `&[u8]` is Latin-1 to V8, and
    /// would mangle every character above U+00FF.
    fn dispatch(session: &RefCell<Option<V8InspectorSession>>, message: &str) {
        let message = utf16(message);
        if let Some(session) = session.borrow().as_ref() {
            session.dispatch_protocol_message(StringView::from(&message[..]));
        }
    }

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    /// V8's callbacks into the embedder.
    struct Client {
        transport: Rc<dyn InspectorTransport>,
        session: Rc<RefCell<Option<V8InspectorSession>>>,
        /// Set for as long as V8 is stopped at a breakpoint. Written by the two
        /// message-loop callbacks and read by the engine, which must not tick
        /// while it is set.
        paused: Rc<Cell<bool>>,
        /// Set while the program is held before its first statement
        /// (`--inspect-brk`), cleared by `Runtime.runIfWaitingForDebugger`.
        waiting: Rc<Cell<bool>>,
        /// The isolate this client belongs to, and the context to hand back as
        /// its default — see [`Client::ensure_default_context_in_group`], which
        /// is the only thing either is for and the reason a raw pointer appears
        /// here at all.
        isolate: *mut v8::Isolate,
        context: v8::Global<v8::Context>,
    }

    impl V8InspectorClientImpl for Client {
        /// V8 has stopped at a breakpoint and is asking the embedder to keep
        /// serving the debugger until it is told to resume.
        ///
        /// Blocking here is the whole point: the program is stopped, so the
        /// thread has nothing else to do, and everything that could resume it
        /// arrives on the transport. Nothing else runs meanwhile — no timers,
        /// no I/O completions — which is what a paused program means.
        fn run_message_loop_on_pause(&self, _context_group_id: i32) {
            self.paused.set(true);
            while self.paused.get() {
                let Some(message) = self.transport.recv_blocking() else {
                    // The debugger is gone. Returning resumes execution, which
                    // is the only answer that does not leave the program
                    // stopped for ever with nobody left to start it.
                    break;
                };
                dispatch(&self.session, &message);
            }
            self.paused.set(false);
        }

        fn quit_message_loop_on_pause(&self) {
            self.paused.set(false);
        }

        fn run_if_waiting_for_debugger(&self, _context_group_id: i32) {
            self.waiting.set(false);
        }

        /// The context to evaluate in when the client did not name one.
        ///
        /// DevTools' Sources pane names it (from `Runtime.executionContextCreated`),
        /// but a plain `Runtime.evaluate` — what most debug consoles send, and
        /// every hand-written CDP client — does not, and without this V8 answers
        /// "Cannot find default execution context" to all of them.
        fn ensure_default_context_in_group(
            &self,
            _context_group_id: i32,
        ) -> Option<v8::Local<'_, v8::Context>> {
            // SAFETY: V8 calls this only from inside an inspector operation on
            // the isolate's own thread, and the isolate outlives this client —
            // `V8Engine` declares the inspector before the isolate so it is torn
            // down first. The scope adopts V8's own live handle scope rather than
            // opening one (`needs_scope` is false for an isolate), which is what
            // makes the handle valid to the caller after this returns.
            let isolate = unsafe { &mut *self.isolate };
            v8::callback_scope!(unsafe let scope, isolate);
            Some(v8::Local::new(scope, &self.context))
        }
    }

    /// The other direction: what V8's session wants to say to the client.
    struct Sink {
        transport: Rc<dyn InspectorTransport>,
    }

    impl Sink {
        fn emit(&self, message: &v8::UniquePtr<StringBuffer>) {
            if let Some(buffer) = message.as_ref() {
                self.transport.send(&buffer.string().to_string());
            }
        }
    }

    impl ChannelImpl for Sink {
        fn send_response(&self, _call_id: i32, message: v8::UniquePtr<StringBuffer>) {
            self.emit(&message);
        }

        fn send_notification(&self, message: v8::UniquePtr<StringBuffer>) {
            self.emit(&message);
        }

        fn flush_protocol_notifications(&self) {
            // Nothing is batched on the way out: `send` hands each message
            // straight to the transport's queue, which the server thread drains.
        }
    }
}
