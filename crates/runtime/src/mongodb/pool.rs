//! The pool of connections to one server (DECISIONS D186).
//!
//! In Rust rather than in `runtime:db`'s JavaScript `Pool`, because here it
//! costs nothing: checking a connection out, the exchange and checking it back
//! in are all inside the one op a command already is.
//!
//! **Each command carries a ticket**, chosen by the driver, so an
//! `AbortSignal` can reach it wherever it is: still waiting for a connection
//! (it leaves the queue), connecting, or mid-exchange (its socket is closed,
//! and the connection is discarded rather than reused, even if the reply won
//! the race). A cancel that arrives before the command does is remembered, and
//! the command does not start.
//!
//! **A clear starts a new generation.** Idle connections close at once, and a
//! checked-out one from an earlier generation closes when it comes back, so a
//! server marked Unknown after a network error hands out no connection opened
//! before it.
//!
//! **Nothing here runs on its own.** There is no reaper: an idle pool holds
//! sockets but no pending work, so it never keeps a program alive.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use es_runtime_providers::{Entropy, NetProvider};
use futures_channel::oneshot;

use super::{Connection, Failure, Options, Sequence};

/// Where a command's socket is, once it has one.
type Socket = Arc<Mutex<Option<u64>>>;

/// A command that has a connection, or is opening one.
struct Running {
    socket: Socket,
    /// Set by a cancel. Its connection is not reused, even if the command
    /// finished first: the cancel is closing that socket.
    cancelled: bool,
}

/// A failure, with what SDAM needs to judge it.
pub(crate) struct Failed {
    pub failure: Failure,
    /// The pool generation the command ran in.
    pub generation: u64,
    /// Whether the connection had finished its handshake: a failure while
    /// opening one is judged differently from one on an established connection.
    pub completed_handshake: bool,
}

pub(crate) struct Pool {
    net: Arc<dyn NetProvider>,
    entropy: Arc<dyn Entropy>,
    options: Options,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    idle: Vec<(u64, Connection)>,
    /// Open connections, idle or checked out.
    total: usize,
    waiters: VecDeque<(u64, oneshot::Sender<()>)>,
    running: HashMap<u64, Running>,
    cancelled: HashSet<u64>,
    generation: u64,
    closed: bool,
}

impl Pool {
    /// An empty pool for the server `options` names.
    pub(crate) fn new(
        net: Arc<dyn NetProvider>,
        entropy: Arc<dyn Entropy>,
        options: Options,
    ) -> Pool {
        Pool {
            net,
            entropy,
            options,
            state: Mutex::new(State::default()),
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.state.lock().unwrap().generation
    }

    /// Runs one command on a pooled connection and returns the reply document.
    pub(crate) async fn command(
        &self,
        ticket: u64,
        body: &[u8],
        sequence: Option<&Sequence>,
    ) -> Result<Vec<u8>, Failed> {
        let (mut connection, generation) = self.checkout(ticket).await?;
        let result = connection.command(body, sequence).await;
        let cancelled = self.checkin(ticket, connection, generation).await;
        result.map_err(|failure| Failed {
            failure: if cancelled {
                Failure::Cancelled
            } else {
                failure
            },
            generation,
            completed_handshake: true,
        })
    }

    async fn checkout(&self, ticket: u64) -> Result<(Connection, u64), Failed> {
        loop {
            let opened: Socket = Arc::new(Mutex::new(None));
            let (waiting, generation) = {
                let mut state = self.state.lock().unwrap();
                let generation = state.generation;
                let fail = |failure| Failed {
                    failure,
                    generation,
                    completed_handshake: true,
                };
                if state.closed {
                    return Err(fail(Failure::Closed));
                }
                if state.cancelled.remove(&ticket) {
                    return Err(fail(Failure::Cancelled));
                }
                if let Some((generation, connection)) = state.idle.pop() {
                    *opened.lock().unwrap() = Some(connection.socket());
                    state.running.insert(
                        ticket,
                        Running {
                            socket: opened,
                            cancelled: false,
                        },
                    );
                    return Ok((connection, generation));
                }
                if state.total < self.options.max_pool {
                    state.total += 1;
                    state.running.insert(
                        ticket,
                        Running {
                            socket: opened.clone(),
                            cancelled: false,
                        },
                    );
                    (None, generation)
                } else {
                    let (tx, rx) = oneshot::channel();
                    state.waiters.push_back((ticket, tx));
                    (Some(rx), generation)
                }
            };
            let Some(rx) = waiting else {
                return self.open(ticket, &opened, generation).await;
            };
            // Woken by a check-in, a cancel, a clear or a close: look again.
            let _ = rx.await;
        }
    }

    /// Opens a connection for the command with `ticket`, which holds a place in
    /// the pool's count.
    async fn open(
        &self,
        ticket: u64,
        opened: &Socket,
        generation: u64,
    ) -> Result<(Connection, u64), Failed> {
        let opening = Connection::open(
            self.net.clone(),
            self.entropy.as_ref(),
            &self.options,
            opened,
            true,
        )
        .await;
        let leftover = {
            let mut state = self.state.lock().unwrap();
            let cancelled = state.running.get(&ticket).is_some_and(|r| r.cancelled);
            match opening {
                Ok((connection, _)) if !cancelled => return Ok((connection, generation)),
                Ok((connection, _)) => {
                    state.running.remove(&ticket);
                    state.total -= 1;
                    wake_one(&mut state);
                    connection
                }
                Err(failure) => {
                    state.running.remove(&ticket);
                    state.total -= 1;
                    wake_one(&mut state);
                    return Err(Failed {
                        failure: if cancelled {
                            Failure::Cancelled
                        } else {
                            failure
                        },
                        generation,
                        completed_handshake: false,
                    });
                }
            }
        };
        // Cancelled while connecting: the command does not run, and the
        // connection goes, since the cancel may have closed its socket.
        let mut leftover = leftover;
        leftover.close().await;
        Err(Failed {
            failure: Failure::Cancelled,
            generation,
            completed_handshake: false,
        })
    }

    /// Returns a connection after a command: to the idle list if it is sound,
    /// its generation current and its command not cancelled; closed otherwise,
    /// its place going to the next waiter. Says whether it was cancelled.
    async fn checkin(&self, ticket: u64, mut connection: Connection, generation: u64) -> bool {
        let cancelled = {
            let mut state = self.state.lock().unwrap();
            let cancelled = state.running.remove(&ticket).is_some_and(|r| r.cancelled);
            let keep = connection.healthy()
                && !state.closed
                && !cancelled
                && generation == state.generation;
            if keep {
                state.idle.push((generation, connection));
                wake_one(&mut state);
                return false;
            }
            state.total -= 1;
            wake_one(&mut state);
            cancelled
        };
        connection.close().await;
        cancelled
    }

    /// Stops the command with `ticket`. Returns the socket to close when it is
    /// connecting or mid-exchange; a waiting command leaves the queue, and one
    /// not started yet will not start.
    pub(crate) fn cancel(&self, ticket: u64) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        if let Some(running) = state.running.get_mut(&ticket) {
            running.cancelled = true;
            return *running.socket.lock().unwrap();
        }
        state.cancelled.insert(ticket);
        if let Some(at) = state.waiters.iter().position(|(t, _)| *t == ticket) {
            // Dropping the sender wakes the waiter, which then finds itself
            // cancelled.
            state.waiters.remove(at);
        }
        None
    }

    /// Starts a new generation: idle connections close now, checked-out ones
    /// when they come back.
    pub(crate) async fn clear(&self) {
        let idle = {
            let mut state = self.state.lock().unwrap();
            state.generation += 1;
            state.total -= state.idle.len();
            wake_all(&mut state);
            std::mem::take(&mut state.idle)
        };
        for (_, mut connection) in idle {
            connection.close().await;
        }
    }

    /// Closes the pool: idle connections now, checked-out ones as they come
    /// back. Waiting commands fail.
    pub(crate) async fn close(&self) {
        let idle = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            wake_all(&mut state);
            state.total -= state.idle.len();
            std::mem::take(&mut state.idle)
        };
        for (_, mut connection) in idle {
            connection.close().await;
        }
    }
}

fn wake_one(state: &mut State) {
    while let Some((_, waiter)) = state.waiters.pop_front() {
        if waiter.send(()).is_ok() {
            return;
        }
    }
}

fn wake_all(state: &mut State) {
    for (_, waiter) in state.waiters.drain(..) {
        let _ = waiter.send(());
    }
}
