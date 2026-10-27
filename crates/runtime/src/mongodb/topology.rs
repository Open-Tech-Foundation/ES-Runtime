//! A client's view of a deployment, and the I/O around it (DECISIONS D186):
//! checking servers, keeping a pool per server, selecting one for each
//! operation, and feeding operation failures back into what is known.
//!
//! The rules are `sdam`'s and `select`'s; this is their state and their I/O.
//!
//! **Checks are asked for, never scheduled here.** The driver calls
//! [`Topology::check_all`] from an unreferenced timer every heartbeat, after an
//! error that asks for it, and while an operation waits for a suitable server,
//! so there is no background task to keep a finished program alive. A check
//! uses a connection of its own that does not authenticate, as the monitoring
//! specification requires.
//!
//! **Selection does not wait here either.** When no server is suitable,
//! [`Topology::run`] says so at once and the driver rechecks and retries until
//! its `serverSelectionTimeoutMS`, so the deadline lives with the other
//! timers, in JavaScript.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use es_runtime_bson::{Document, Element, Writer, kind};
use es_runtime_engine::Value;
use es_runtime_providers::{Entropy, NetProvider};

use super::pool::{Failed, Pool};
use super::sdam::{
    AppError, AppErrorKind, Effects, ServerDescription, ServerType, TopologyDescription,
    TopologyType, topology_version,
};
use super::select::{self, Mode, Operation, ReadPreference};
use super::{Connection, Failure, Options, Sequence};

/// What an operation asks of selection.
pub(crate) struct Selector {
    pub preference: ReadPreference,
    pub operation: Operation,
    /// A server the operation must run on: a cursor's, for its `getMore`.
    pub server: Option<String>,
}

/// A command's reply, with the server it ran on and whether the driver should
/// check the deployment again now.
pub(crate) struct Ran {
    pub reply: Vec<u8>,
    pub server: String,
    pub recheck: bool,
}

/// The message-size limits the servers last reported.
#[derive(Clone, Copy)]
struct Limits {
    max_bson: i64,
    max_message: i64,
    max_write_batch: i64,
}

pub(crate) struct Topology {
    net: Arc<dyn NetProvider>,
    entropy: Arc<dyn Entropy>,
    options: Options,
    heartbeat_ms: i64,
    local_threshold_ms: f64,
    started: Instant,
    state: Mutex<State>,
}

struct State {
    description: TopologyDescription,
    pools: HashMap<String, Arc<Pool>>,
    /// Each server's monitoring connection, between checks.
    monitors: HashMap<String, Connection>,
    /// The socket of each check in flight, so closing can stop it.
    checking: HashMap<String, Arc<Mutex<Option<u64>>>>,
    /// Operations each server is running, for selection within the window.
    operations: HashMap<String, usize>,
    /// The server each running command was sent to, by ticket.
    running: HashMap<u64, String>,
    cancelled: HashSet<u64>,
    limits: Limits,
    random: u64,
    closed: bool,
}

impl Topology {
    #[expect(
        clippy::too_many_arguments,
        reason = "the connection string's settings, once"
    )]
    pub(crate) fn new(
        net: Arc<dyn NetProvider>,
        entropy: Arc<dyn Entropy>,
        options: Options,
        seeds: &[String],
        direct: bool,
        set_name: Option<String>,
        heartbeat_ms: i64,
        local_threshold_ms: f64,
    ) -> Topology {
        let mut seed = [0u8; 8];
        let _ = entropy.fill(&mut seed);
        Topology {
            net,
            entropy,
            options,
            heartbeat_ms,
            local_threshold_ms,
            started: Instant::now(),
            state: Mutex::new(State {
                description: TopologyDescription::new(seeds, direct, set_name),
                pools: HashMap::new(),
                monitors: HashMap::new(),
                checking: HashMap::new(),
                operations: HashMap::new(),
                running: HashMap::new(),
                cancelled: HashSet::new(),
                limits: Limits {
                    max_bson: 16 * 1024 * 1024,
                    max_message: 48_000_000,
                    max_write_batch: 100_000,
                },
                random: u64::from_le_bytes(seed) | 1,
                closed: false,
            }),
        }
    }

    fn now(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    /// The options for connecting to `address`.
    fn options_for(&self, address: &str) -> Options {
        let mut options = self.options.clone();
        let (host, port) = match address.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().unwrap_or(27017)),
            None => (address, 27017),
        };
        options.host = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        options.port = port;
        options
    }

    /// Checks every server, and again any it discovers, until a round finds no
    /// new one. A refusal by the provider (an address outside the allowlist)
    /// for a server the caller named is returned as an error.
    pub(crate) async fn check_all(&self) -> Result<(), Failure> {
        let mut checked: HashSet<String> = HashSet::new();
        for _ in 0..8 {
            let addresses: Vec<String> = {
                let state = self.state.lock().unwrap();
                if state.closed {
                    return Err(Failure::Closed);
                }
                state
                    .description
                    .servers
                    .keys()
                    .filter(|a| !checked.contains(*a))
                    .cloned()
                    .collect()
            };
            if addresses.is_empty() {
                break;
            }
            let results =
                futures_util::future::join_all(addresses.iter().map(|a| self.check(a))).await;
            checked.extend(addresses);
            for result in results {
                result?;
            }
        }
        Ok(())
    }

    async fn check(&self, address: &str) -> Result<(), Failure> {
        let opened = Arc::new(Mutex::new(None));
        let monitor = {
            let mut state = self.state.lock().unwrap();
            if state.closed
                || !state.description.servers.contains_key(address)
                || state.checking.contains_key(address)
            {
                return Ok(());
            }
            state.checking.insert(address.to_string(), opened.clone());
            state.monitors.remove(address)
        };
        let started = Instant::now();
        let result = match monitor {
            Some(mut connection) => match connection.hello().await {
                Ok(hello) => Ok((connection, hello)),
                Err(failure) => {
                    connection.close().await;
                    Err(failure)
                }
            },
            None => Connection::open(
                self.net.clone(),
                self.entropy.as_ref(),
                &self.options_for(address),
                &opened,
                false,
            )
            .await
            .map(|(connection, server)| (connection, server.hello)),
        };
        #[expect(clippy::cast_precision_loss, reason = "a round trip in milliseconds")]
        let rtt = started.elapsed().as_micros() as f64 / 1000.0;
        let now = self.now();

        let (effects, leftover, refused, clear) = {
            let mut state = self.state.lock().unwrap();
            state.checking.remove(address);
            match result {
                Ok((connection, hello)) => {
                    let description = match Document::new(&hello) {
                        Ok(doc) => {
                            let description =
                                ServerDescription::from_hello(address, &doc, Some(rtt), now);
                            if description.kind.data_bearing() {
                                state.limits = limits(&doc, state.limits);
                            }
                            description
                        }
                        Err(e) => ServerDescription::failed(
                            address,
                            format!("the server's hello is malformed: {e}"),
                            None,
                        ),
                    };
                    let effects = state.description.update(description);
                    let keep = !state.closed && state.description.servers.contains_key(address);
                    let leftover = if keep {
                        state.monitors.insert(address.to_string(), connection);
                        None
                    } else {
                        Some(connection)
                    };
                    (effects, leftover, None, false)
                }
                Err(failure) => {
                    let message = failure_message(&failure);
                    let effects = state
                        .description
                        .update(ServerDescription::failed(address, message, None));
                    // A failed check clears the server's pool, as an
                    // operation's network error does.
                    *state
                        .description
                        .generations
                        .entry(address.to_string())
                        .or_insert(0) += 1;
                    let refused = matches!(failure, Failure::Refused(_)).then_some(failure);
                    (effects, None, refused, true)
                }
            }
        };
        if let Some(mut connection) = leftover {
            connection.close().await;
        }
        if clear {
            let pool = self.state.lock().unwrap().pools.get(address).cloned();
            if let Some(pool) = pool {
                pool.clear().await;
            }
        }
        self.apply(effects).await;
        match refused {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }

    /// Acts on what SDAM decided: removed servers lose their pools and
    /// monitors, cleared pools start a new generation.
    async fn apply(&self, effects: Effects) {
        let (pools, monitors, clears) = {
            let mut state = self.state.lock().unwrap();
            let mut pools = Vec::new();
            let mut monitors = Vec::new();
            for address in &effects.removed {
                pools.extend(state.pools.remove(address));
                monitors.extend(state.monitors.remove(address));
            }
            let clears: Vec<Arc<Pool>> = effects
                .clear
                .iter()
                .filter_map(|a| state.pools.get(a).cloned())
                .collect();
            (pools, monitors, clears)
        };
        for pool in pools {
            pool.close().await;
        }
        for mut monitor in monitors {
            monitor.close().await;
        }
        for pool in clears {
            pool.clear().await;
        }
    }

    /// Selects a server and runs one command on it.
    pub(crate) async fn run(
        &self,
        ticket: u64,
        body: Vec<u8>,
        sequence: Option<&Sequence>,
        selector: &Selector,
    ) -> Result<Ran, Failure> {
        let (pool, address, server_kind, topology_kind) = {
            let mut state = self.state.lock().unwrap();
            if state.closed {
                return Err(Failure::Closed);
            }
            if state.cancelled.remove(&ticket) {
                return Err(Failure::Cancelled);
            }
            if let Some(error) = &state.description.compatibility_error {
                return Err(Failure::Unsupported(error.clone()));
            }
            let address = match &selector.server {
                Some(address) => {
                    if !state.description.servers.contains_key(address) {
                        return Err(Failure::Selection(format!(
                            "{address}, where this cursor is, is no longer part of the deployment"
                        )));
                    }
                    address.clone()
                }
                None => self.select(&mut state, selector)?,
            };
            let server_kind = state.description.servers[&address].kind;
            *state.operations.entry(address.clone()).or_insert(0) += 1;
            state.running.insert(ticket, address.clone());
            let pool = state
                .pools
                .entry(address.clone())
                .or_insert_with(|| {
                    Arc::new(Pool::new(
                        self.net.clone(),
                        self.entropy.clone(),
                        self.options_for(&address),
                    ))
                })
                .clone();
            (pool, address, server_kind, state.description.kind)
        };
        let body = with_read_preference(body, selector, server_kind, topology_kind)?;
        let result = pool.command(ticket, &body, sequence).await;

        let effects = {
            let mut state = self.state.lock().unwrap();
            if let Some(n) = state.operations.get_mut(&address) {
                *n = n.saturating_sub(1);
            }
            state.running.remove(&ticket);
            match app_error(&result, pool.generation()) {
                Some(error) => state.description.application_error(&address, &error),
                None => Effects::default(),
            }
        };
        let recheck = !effects.check.is_empty();
        self.apply(effects).await;
        match result {
            Ok(reply) => Ok(Ran {
                reply,
                server: address,
                recheck,
            }),
            Err(failed) => Err(failed.failure),
        }
    }

    fn select(&self, state: &mut State, selector: &Selector) -> Result<String, Failure> {
        let found = select::suitable(
            &state.description,
            &selector.preference,
            selector.operation,
            self.heartbeat_ms,
            &[],
        )
        .map_err(Failure::Unsupported)?;
        let window = select::in_latency_window(&found, self.local_threshold_ms);
        let random = &mut state.random;
        let mut draw = |n: usize| {
            *random ^= *random << 13;
            *random ^= *random >> 7;
            *random ^= *random << 17;
            usize::try_from(*random % n as u64).unwrap_or(0)
        };
        let operations = &state.operations;
        match select::pick(
            &window,
            |a| operations.get(a).copied().unwrap_or(0),
            &mut draw,
        ) {
            Some(server) => Ok(server.address.clone()),
            None => Err(Failure::Selection(why_none(&state.description, selector))),
        }
    }

    /// Stops the command with `ticket`, wherever it is. Returns the socket to
    /// close, if it has one.
    pub(crate) fn cancel(&self, ticket: u64) -> Option<u64> {
        let mut state = self.state.lock().unwrap();
        match state.running.get(&ticket).and_then(|a| state.pools.get(a)) {
            Some(pool) => pool.cancel(ticket),
            None => {
                state.cancelled.insert(ticket);
                None
            }
        }
    }

    /// The sockets of checks in flight: what a connect that timed out closes.
    pub(crate) fn checking_sockets(&self) -> Vec<u64> {
        let state = self.state.lock().unwrap();
        state
            .checking
            .values()
            .filter_map(|s| *s.lock().unwrap())
            .collect()
    }

    /// Closes every pool and monitor.
    pub(crate) async fn close(&self) {
        let (pools, monitors, sockets) = {
            let mut state = self.state.lock().unwrap();
            state.closed = true;
            let sockets: Vec<u64> = state
                .checking
                .values()
                .filter_map(|s| *s.lock().unwrap())
                .collect();
            (
                state.pools.drain().map(|(_, p)| p).collect::<Vec<_>>(),
                state.monitors.drain().map(|(_, m)| m).collect::<Vec<_>>(),
                sockets,
            )
        };
        for pool in pools {
            pool.close().await;
        }
        for mut monitor in monitors {
            monitor.close().await;
        }
        for socket in sockets {
            let _ = self.net.close(socket).await;
        }
    }

    /// What the driver needs to know about the deployment, for JavaScript.
    pub(crate) fn summary(&self) -> Value {
        let state = self.state.lock().unwrap();
        let d = &state.description;
        let servers = d
            .servers
            .values()
            .map(|s| {
                let mut fields = vec![
                    ("address".to_string(), Value::String(s.address.clone())),
                    ("type".to_string(), Value::String(s.kind.name().to_string())),
                ];
                if let Some(error) = &s.error {
                    fields.push(("error".to_string(), Value::String(error.clone())));
                }
                Value::Object(fields)
            })
            .collect();
        let optional_number = |n: Option<i64>| n.map_or(Value::Null, |n| Value::Number(n as f64));
        Value::Object(vec![
            ("type".to_string(), Value::String(d.kind.name().to_string())),
            (
                "setName".to_string(),
                d.set_name.clone().map_or(Value::Null, Value::String),
            ),
            (
                "logicalSessionTimeoutMinutes".to_string(),
                optional_number(d.logical_session_timeout),
            ),
            ("servers".to_string(), Value::Array(servers)),
            (
                "maxBsonObjectSize".to_string(),
                Value::Number(state.limits.max_bson as f64),
            ),
            (
                "maxMessageSizeBytes".to_string(),
                Value::Number(state.limits.max_message as f64),
            ),
            (
                "maxWriteBatchSize".to_string(),
                Value::Number(state.limits.max_write_batch as f64),
            ),
        ])
    }
}

fn limits(hello: &Document<'_>, current: Limits) -> Limits {
    let get = |key: &str, default: i64| match hello.get(key) {
        Ok(Some(Element::Int32(n))) => i64::from(n),
        Ok(Some(Element::Int64(n))) => n,
        _ => default,
    };
    Limits {
        max_bson: get("maxBsonObjectSize", current.max_bson),
        max_message: get("maxMessageSizeBytes", current.max_message),
        max_write_batch: get("maxWriteBatchSize", current.max_write_batch),
    }
}

fn failure_message(failure: &Failure) -> String {
    match failure {
        Failure::Server { message, .. }
        | Failure::Lost(message)
        | Failure::Auth(message)
        | Failure::Unsupported(message)
        | Failure::Selection(message) => message.clone(),
        Failure::Refused(e) => e.to_string(),
        Failure::Closed => "the client is closed".into(),
        Failure::Cancelled => "cancelled".into(),
    }
}

/// How a command's outcome bears on SDAM, if at all: a lost connection, a
/// failure while opening one, or a reply carrying a state-change error.
fn app_error(result: &Result<Vec<u8>, Failed>, current_generation: u64) -> Option<AppError> {
    match result {
        Err(failed) => {
            let kind = match &failed.failure {
                Failure::Lost(message) => AppErrorKind::Network(message.clone()),
                // Stopped by the caller's signal or timeout: the server may only
                // have been slow, which SDAM treats as a timeout and leaves be.
                Failure::Cancelled => AppErrorKind::Timeout,
                // A handshake or authentication failure marks the server Unknown.
                Failure::Auth(message) | Failure::Server { message, .. }
                    if !failed.completed_handshake =>
                {
                    AppErrorKind::Command {
                        code: None,
                        message: message.clone(),
                        topology_version: None,
                    }
                }
                _ => return None,
            };
            Some(AppError {
                generation: failed.generation,
                completed_handshake: failed.completed_handshake,
                kind,
            })
        }
        Ok(reply) => {
            let doc = Document::new(reply).ok()?;
            let ok = matches!(doc.get("ok"), Ok(Some(Element::Double(n))) if n == 1.0)
                || matches!(doc.get("ok"), Ok(Some(Element::Int32(1))));
            let source = if ok {
                match doc.get("writeConcernError") {
                    Ok(Some(Element::Document(wce))) => wce,
                    _ => return None,
                }
            } else {
                doc
            };
            let code = match source.get("code") {
                Ok(Some(Element::Int32(c))) => Some(c),
                _ => None,
            };
            let message = match source.get("errmsg") {
                Ok(Some(Element::String(m))) => m.to_string(),
                _ => String::new(),
            };
            Some(AppError {
                generation: current_generation,
                completed_handshake: true,
                kind: AppErrorKind::Command {
                    code,
                    message,
                    topology_version: topology_version(&doc),
                },
            })
        }
    }
}

/// Why no server was suitable, for the error a selection timeout reports.
fn why_none(d: &TopologyDescription, selector: &Selector) -> String {
    let errors: Vec<String> = d
        .servers
        .values()
        .filter_map(|s| s.error.as_ref().map(|e| format!("{}: {e}", s.address)))
        .collect();
    let what = match (d.kind, selector.operation) {
        (TopologyType::Unknown, _) => "no server has answered yet".to_string(),
        (TopologyType::ReplicaSetNoPrimary, Operation::Write) => {
            "the replica set has no primary".to_string()
        }
        (_, Operation::Write) => "no server can take a write".to_string(),
        (_, Operation::Read) => format!(
            "no server matches read preference {}",
            selector.preference.mode.name()
        ),
    };
    if errors.is_empty() {
        what
    } else {
        format!("{what} ({})", errors.join("; "))
    }
}

/// Adds `$readPreference` where the server selection specification says a read
/// must carry one: to a mongos for any mode but primary, to a replica-set
/// member reached directly as `primaryPreferred` at least, and to a replica set
/// for any mode but primary. Never to a standalone, and never on a write.
fn with_read_preference(
    body: Vec<u8>,
    selector: &Selector,
    server: ServerType,
    topology: TopologyType,
) -> Result<Vec<u8>, Failure> {
    if selector.operation == Operation::Write || server == ServerType::Standalone {
        return Ok(body);
    }
    let preference = &selector.preference;
    let mode = match (topology, server) {
        (_, ServerType::Mongos)
        | (TopologyType::ReplicaSetNoPrimary | TopologyType::ReplicaSetWithPrimary, _)
            if preference.mode == Mode::Primary =>
        {
            return Ok(body);
        }
        (TopologyType::Single, _)
            if server != ServerType::Mongos && preference.mode == Mode::Primary =>
        {
            Mode::PrimaryPreferred
        }
        _ => preference.mode,
    };
    let bad = |e: es_runtime_bson::Error| Failure::Unsupported(e.to_string());
    let mut w = Writer::new();
    w.start_document().map_err(bad)?;
    w.key(kind::STRING, "mode").map_err(bad)?;
    w.string(mode.name()).map_err(bad)?;
    if !preference.tag_sets.is_empty() && preference.mode != Mode::Primary {
        w.key(kind::ARRAY, "tags").map_err(bad)?;
        w.start_document().map_err(bad)?;
        for (i, set) in preference.tag_sets.iter().enumerate() {
            w.index(kind::DOCUMENT, i);
            w.start_document().map_err(bad)?;
            for (k, v) in set {
                w.key(kind::STRING, k).map_err(bad)?;
                w.string(v).map_err(bad)?;
            }
            w.end_document().map_err(bad)?;
        }
        w.end_document().map_err(bad)?;
    }
    if let Some(seconds) = preference.max_staleness_seconds.filter(|&s| s > 0) {
        w.key(kind::INT64, "maxStalenessSeconds").map_err(bad)?;
        w.int64(seconds);
    }
    w.end_document().map_err(bad)?;
    let preference_doc = w.finish().map_err(bad)?;
    Ok(append_element(body, "$readPreference", &preference_doc))
}

/// `body` with a document element added at its end.
fn append_element(mut body: Vec<u8>, name: &str, document: &[u8]) -> Vec<u8> {
    body.pop();
    body.push(kind::DOCUMENT);
    body.extend_from_slice(name.as_bytes());
    body.push(0);
    body.extend_from_slice(document);
    body.push(0);
    let len = i32::try_from(body.len()).unwrap_or(i32::MAX);
    body[..4].copy_from_slice(&len.to_le_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector(mode: Mode, operation: Operation) -> Selector {
        Selector {
            preference: ReadPreference {
                mode,
                tag_sets: vec![vec![("dc".into(), "ny".into())]],
                max_staleness_seconds: Some(120),
            },
            operation,
            server: None,
        }
    }

    fn read_preference(body: &[u8]) -> Option<String> {
        match Document::new(body).unwrap().get("$readPreference").unwrap() {
            Some(Element::Document(d)) => match d.get("mode").unwrap() {
                Some(Element::String(m)) => Some(m.to_string()),
                _ => None,
            },
            _ => None,
        }
    }

    fn body() -> Vec<u8> {
        let mut w = Writer::new();
        w.start_document().unwrap();
        w.key(kind::INT32, "find").unwrap();
        w.int32(1);
        w.end_document().unwrap();
        w.finish().unwrap()
    }

    #[test]
    fn read_preference_is_added_only_where_the_specification_says() {
        let add = |mode, op, server, topology| {
            let out = with_read_preference(body(), &selector(mode, op), server, topology).unwrap();
            es_runtime_bson::validate(&out).unwrap();
            read_preference(&out)
        };
        use Mode::*;
        use ServerType as S;
        use TopologyType as T;
        assert_eq!(
            add(Secondary, Operation::Read, S::Standalone, T::Single),
            None
        );
        assert_eq!(
            add(Primary, Operation::Read, S::RsSecondary, T::Single).as_deref(),
            Some("primaryPreferred")
        );
        assert_eq!(
            add(
                Primary,
                Operation::Read,
                S::RsPrimary,
                T::ReplicaSetWithPrimary
            ),
            None
        );
        assert_eq!(
            add(
                Nearest,
                Operation::Read,
                S::RsSecondary,
                T::ReplicaSetWithPrimary
            )
            .as_deref(),
            Some("nearest")
        );
        assert_eq!(add(Primary, Operation::Read, S::Mongos, T::Sharded), None);
        assert_eq!(
            add(Secondary, Operation::Read, S::Mongos, T::Sharded).as_deref(),
            Some("secondary")
        );
        assert_eq!(
            add(
                Secondary,
                Operation::Write,
                S::RsPrimary,
                T::ReplicaSetWithPrimary
            ),
            None
        );
    }
}
