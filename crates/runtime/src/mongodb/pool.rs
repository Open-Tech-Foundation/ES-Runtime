//! The pool of connections to one server (DECISIONS D186).
//!
//! In Rust rather than in `runtime:db`'s JavaScript `Pool`, because here it
//! costs nothing: checking a connection out, the exchange and checking it back
//! in are all inside the one op a command already is.
//!
//! **Each command carries a ticket**, chosen by the driver, so an
//! `AbortSignal` can reach it wherever it is: still waiting for a connection
//! (it leaves the queue), connecting, or mid-exchange (its socket is closed,
//! the read fails, and the connection is discarded rather than reused). A
//! cancel that arrives before the command does is remembered, and the command
//! does not start.
//!
//! **Nothing here runs on its own.** There is no monitor and no reaper: an idle
//! pool holds sockets but no pending work, so it never keeps a program alive.

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

pub(crate) struct Pool {
    net: Arc<dyn NetProvider>,
    entropy: Arc<dyn Entropy>,
    options: Options,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    idle: Vec<Connection>,
    /// Open connections, idle or checked out.
    total: usize,
    waiters: VecDeque<(u64, oneshot::Sender<()>)>,
    running: HashMap<u64, Running>,
    cancelled: HashSet<u64>,
    closed: bool,
}

impl Pool {
    /// A pool holding `first`, the connection that proved the server reachable
    /// and the credentials good.
    pub(crate) fn new(
        net: Arc<dyn NetProvider>,
        entropy: Arc<dyn Entropy>,
        options: Options,
        first: Connection,
    ) -> Pool {
        Pool {
            net,
            entropy,
            options,
            state: Mutex::new(State {
                idle: vec![first],
                total: 1,
                ..State::default()
            }),
        }
    }

    /// Runs one command on a pooled connection and returns the reply document.
    pub(crate) async fn command(
        &self,
        ticket: u64,
        body: &[u8],
        sequence: Option<&Sequence>,
    ) -> Result<Vec<u8>, Failure> {
        let mut connection = self.checkout(ticket).await?;
        let result = connection.command(body, sequence).await;
        self.checkin(ticket, connection).await;
        result
    }

    async fn checkout(&self, ticket: u64) -> Result<Connection, Failure> {
        loop {
            let opened: Socket = Arc::new(Mutex::new(None));
            let waiting = {
                let mut state = self.state.lock().unwrap();
                if state.closed {
                    return Err(Failure::Closed);
                }
                if state.cancelled.remove(&ticket) {
                    return Err(Failure::Lost("the operation was cancelled".into()));
                }
                if let Some(connection) = state.idle.pop() {
                    *opened.lock().unwrap() = Some(connection.socket());
                    state.running.insert(
                        ticket,
                        Running {
                            socket: opened,
                            cancelled: false,
                        },
                    );
                    return Ok(connection);
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
                    None
                } else {
                    let (tx, rx) = oneshot::channel();
                    state.waiters.push_back((ticket, tx));
                    Some(rx)
                }
            };
            match waiting {
                // Woken by a check-in, a cancel or a close: look again.
                Some(rx) => {
                    let _ = rx.await;
                }
                None => {
                    return match Connection::open(
                        self.net.clone(),
                        self.entropy.as_ref(),
                        &self.options,
                        &opened,
                    )
                    .await
                    {
                        Ok((connection, _)) => {
                            // Cancelled before the socket existed to close: the
                            // new connection is good, so it joins the pool, and
                            // the command does not run.
                            // Cancelled while connecting: the command does not
                            // run, and the connection goes, since a cancel may
                            // have closed its socket.
                            {
                                let mut state = self.state.lock().unwrap();
                                if !state.running.get(&ticket).is_some_and(|r| r.cancelled) {
                                    return Ok(connection);
                                }
                                state.running.remove(&ticket);
                                state.total -= 1;
                                wake_one(&mut state);
                            }
                            let mut connection = connection;
                            connection.close().await;
                            Err(Failure::Lost("the operation was cancelled".into()))
                        }
                        Err(failure) => {
                            let mut state = self.state.lock().unwrap();
                            state.total -= 1;
                            state.running.remove(&ticket);
                            wake_one(&mut state);
                            Err(failure)
                        }
                    };
                }
            }
        }
    }

    /// Returns a connection after a command. A lost one, or any once the pool
    /// is closed, is closed instead, and its place goes to the next waiter.
    async fn checkin(&self, ticket: u64, mut connection: Connection) {
        let discard = {
            let mut state = self.state.lock().unwrap();
            let cancelled = state.running.remove(&ticket).is_some_and(|r| r.cancelled);
            let keep = connection.healthy() && !state.closed && !cancelled;
            if !keep {
                state.total -= 1;
            }
            wake_one(&mut state);
            if keep {
                state.idle.push(connection);
                return;
            }
            true
        };
        if discard {
            connection.close().await;
        }
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

    /// Closes the pool: idle connections now, checked-out ones as they come
    /// back. Waiting commands fail.
    pub(crate) async fn close(&self) {
        let idle = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            state.waiters.clear();
            state.total -= state.idle.len();
            std::mem::take(&mut state.idle)
        };
        for mut connection in idle {
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
