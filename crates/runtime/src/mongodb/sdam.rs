//! Server discovery and monitoring: what the driver believes about each
//! server and about the deployment, and how each `hello` reply or operation
//! error changes it (MongoDB's SDAM specification).
//!
//! Pure: nothing here does I/O or reads a clock. The topology layer feeds it
//! outcomes and acts on what it returns — servers to start or stop watching,
//! pools to clear — so the specification's own test suite can drive it
//! directly.

use std::collections::BTreeMap;

use es_runtime_bson::{Document, Element};

/// The oldest and newest wire versions this driver speaks: MongoDB 7.0 up.
pub(crate) const CLIENT_MIN_WIRE: i32 = 21;
pub(crate) const CLIENT_MAX_WIRE: i32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServerType {
    Unknown,
    Standalone,
    Mongos,
    PossiblePrimary,
    RsPrimary,
    RsSecondary,
    RsArbiter,
    RsOther,
    RsGhost,
}

impl ServerType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            ServerType::Unknown => "Unknown",
            ServerType::Standalone => "Standalone",
            ServerType::Mongos => "Mongos",
            ServerType::PossiblePrimary => "PossiblePrimary",
            ServerType::RsPrimary => "RSPrimary",
            ServerType::RsSecondary => "RSSecondary",
            ServerType::RsArbiter => "RSArbiter",
            ServerType::RsOther => "RSOther",
            ServerType::RsGhost => "RSGhost",
        }
    }

    /// Whether it holds data an operation can read: what decides the
    /// deployment's session timeout and which servers get a pool.
    pub(crate) fn data_bearing(self) -> bool {
        matches!(
            self,
            ServerType::Standalone
                | ServerType::Mongos
                | ServerType::RsPrimary
                | ServerType::RsSecondary
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TopologyType {
    Unknown,
    Single,
    ReplicaSetNoPrimary,
    ReplicaSetWithPrimary,
    Sharded,
}

impl TopologyType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            TopologyType::Unknown => "Unknown",
            TopologyType::Single => "Single",
            TopologyType::ReplicaSetNoPrimary => "ReplicaSetNoPrimary",
            TopologyType::ReplicaSetWithPrimary => "ReplicaSetWithPrimary",
            TopologyType::Sharded => "Sharded",
        }
    }
}

/// `topologyVersion`: a server process id and a counter within it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TopologyVersion {
    pub process_id: [u8; 12],
    pub counter: i64,
}

/// Whether `incoming` is at least as fresh as `current`: a missing one on
/// either side, or another process, counts as fresher.
pub(crate) fn at_least_as_new(
    current: Option<TopologyVersion>,
    incoming: Option<TopologyVersion>,
) -> bool {
    match (current, incoming) {
        (Some(c), Some(i)) if c.process_id == i.process_id => i.counter >= c.counter,
        _ => true,
    }
}

/// What the driver knows about one server.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ServerDescription {
    pub address: String,
    pub kind: ServerType,
    pub error: Option<String>,
    /// The averaged round trip of its `hello`, in milliseconds.
    pub avg_rtt: Option<f64>,
    pub last_write_date: Option<i64>,
    /// When it was last checked, in milliseconds on the topology's clock.
    pub last_update_time: i64,
    pub min_wire: i32,
    pub max_wire: i32,
    pub me: Option<String>,
    /// Its `hosts`, `passives` and `arbiters`, normalized.
    pub members: Vec<String>,
    pub tags: Vec<(String, String)>,
    pub set_name: Option<String>,
    pub election_id: Option<[u8; 12]>,
    pub set_version: Option<i64>,
    pub primary: Option<String>,
    pub logical_session_timeout: Option<i64>,
    pub topology_version: Option<TopologyVersion>,
}

impl ServerDescription {
    pub(crate) fn unknown(address: &str) -> Self {
        ServerDescription {
            address: address.to_string(),
            kind: ServerType::Unknown,
            error: None,
            avg_rtt: None,
            last_write_date: None,
            last_update_time: 0,
            min_wire: 0,
            max_wire: 0,
            me: None,
            members: Vec::new(),
            tags: Vec::new(),
            set_name: None,
            election_id: None,
            set_version: None,
            primary: None,
            logical_session_timeout: None,
            topology_version: None,
        }
    }

    pub(crate) fn failed(
        address: &str,
        error: String,
        topology_version: Option<TopologyVersion>,
    ) -> Self {
        ServerDescription {
            error: Some(error),
            topology_version,
            ..Self::unknown(address)
        }
    }

    /// A description from a `hello` reply. `rtt` is this check's round trip;
    /// averaging it with earlier ones is the topology's business.
    pub(crate) fn from_hello(
        address: &str,
        hello: &Document<'_>,
        rtt: Option<f64>,
        now: i64,
    ) -> Self {
        let mut d = Self::unknown(address);
        d.last_update_time = now;
        if !truthy(hello, "ok") {
            d.error =
                Some(text(hello, "errmsg").unwrap_or_else(|| "the server's hello failed".into()));
            return d;
        }
        let set_name = text(hello, "setName");
        let writable = if field(hello, "isWritablePrimary").is_some() {
            truthy(hello, "isWritablePrimary")
        } else {
            truthy(hello, "ismaster")
        };
        d.kind = if text(hello, "msg").as_deref() == Some("isdbgrid") {
            ServerType::Mongos
        } else if set_name.is_some() {
            if truthy(hello, "hidden") {
                ServerType::RsOther
            } else if writable {
                ServerType::RsPrimary
            } else if truthy(hello, "secondary") {
                ServerType::RsSecondary
            } else if truthy(hello, "arbiterOnly") {
                ServerType::RsArbiter
            } else {
                ServerType::RsOther
            }
        } else if truthy(hello, "isreplicaset") {
            ServerType::RsGhost
        } else {
            ServerType::Standalone
        };
        d.avg_rtt = rtt;
        d.min_wire = int(hello, "minWireVersion").unwrap_or(0);
        d.max_wire = int(hello, "maxWireVersion").unwrap_or(0);
        d.me = text(hello, "me").map(|me| normalize(&me));
        for key in ["hosts", "passives", "arbiters"] {
            if let Some(Element::Array(list)) = field(hello, key) {
                for host in list.iter().flatten() {
                    if let Element::String(h) = host.1 {
                        d.members.push(normalize(h));
                    }
                }
            }
        }
        if let Some(Element::Document(tags)) = field(hello, "tags") {
            for (k, v) in tags.iter().flatten() {
                if let Element::String(v) = v {
                    d.tags.push((k.to_string(), v.to_string()));
                }
            }
        }
        d.set_name = set_name;
        if let Some(Element::ObjectId(id)) = field(hello, "electionId") {
            d.election_id = Some(id);
        }
        d.set_version = long(hello, "setVersion");
        d.primary = text(hello, "primary").map(|p| normalize(&p));
        d.logical_session_timeout = long(hello, "logicalSessionTimeoutMinutes");
        if let Some(Element::Document(last)) = field(hello, "lastWrite")
            && let Ok(Some(Element::DateTime(date))) = last.get("lastWriteDate")
        {
            d.last_write_date = Some(date);
        }
        d.topology_version = topology_version(hello);
        d
    }
}

/// The `topologyVersion` field of a reply, when it has one.
pub(crate) fn topology_version(doc: &Document<'_>) -> Option<TopologyVersion> {
    let Some(Element::Document(tv)) = field(doc, "topologyVersion") else {
        return None;
    };
    let Ok(Some(Element::ObjectId(process_id))) = tv.get("processId") else {
        return None;
    };
    Some(TopologyVersion {
        process_id,
        counter: long(&tv, "counter").unwrap_or(0),
    })
}

/// `host:port`, lower-cased, with the default port when none is given.
pub(crate) fn normalize(address: &str) -> String {
    let address = address.to_ascii_lowercase();
    if address.starts_with('[') {
        return if address.contains("]:") {
            address
        } else {
            format!("{address}:27017")
        };
    }
    if address.contains(':') {
        address
    } else {
        format!("{address}:27017")
    }
}

/// What a change asks the topology layer to do.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Effects {
    /// Servers newly in the topology: start watching them.
    pub added: Vec<String>,
    /// Servers no longer in it: stop watching them and drop their pools.
    pub removed: Vec<String>,
    /// Pools to clear.
    pub clear: Vec<String>,
    /// Servers to check again now, rather than at the next heartbeat.
    pub check: Vec<String>,
}

/// What the driver knows about the deployment.
#[derive(Clone, Debug)]
pub(crate) struct TopologyDescription {
    pub kind: TopologyType,
    pub set_name: Option<String>,
    pub max_election_id: Option<[u8; 12]>,
    pub max_set_version: Option<i64>,
    pub servers: BTreeMap<String, ServerDescription>,
    pub compatibility_error: Option<String>,
    pub logical_session_timeout: Option<i64>,
    /// Each server's pool generation, which a pool clear advances.
    pub generations: BTreeMap<String, u64>,
    seeds: usize,
}

impl TopologyDescription {
    /// The starting description, by the specification's table: a direct
    /// connection is Single, a named replica set ReplicaSetNoPrimary, anything
    /// else Unknown until a server says what it is.
    pub(crate) fn new(seeds: &[String], direct: bool, set_name: Option<String>) -> Self {
        let kind = if direct {
            TopologyType::Single
        } else if set_name.is_some() {
            TopologyType::ReplicaSetNoPrimary
        } else {
            TopologyType::Unknown
        };
        let servers = seeds
            .iter()
            .map(|s| (normalize(s), ServerDescription::unknown(&normalize(s))))
            .collect::<BTreeMap<_, _>>();
        let generations = servers.keys().map(|a| (a.clone(), 0)).collect();
        TopologyDescription {
            kind,
            set_name,
            max_election_id: None,
            max_set_version: None,
            servers,
            compatibility_error: None,
            logical_session_timeout: None,
            generations,
            seeds: seeds.len(),
        }
    }

    pub(crate) fn has_primary(&self) -> bool {
        self.servers
            .values()
            .any(|s| s.kind == ServerType::RsPrimary)
    }

    /// Replaces a server's description with `incoming` and applies the
    /// transition table. An outcome for a server no longer in the topology, or
    /// one older than what is known, changes nothing.
    pub(crate) fn update(&mut self, mut incoming: ServerDescription) -> Effects {
        let mut effects = Effects::default();
        let address = incoming.address.clone();
        let Some(current) = self.servers.get(&address) else {
            return effects;
        };
        if !at_least_as_new(current.topology_version, incoming.topology_version) {
            return effects;
        }
        // An Unknown server has no round trip; a known one keeps its average.
        if incoming.kind == ServerType::Unknown {
            incoming.avg_rtt = None;
        } else if let (Some(old), Some(new)) = (current.avg_rtt, incoming.avg_rtt) {
            incoming.avg_rtt = Some(average_rtt(old, new));
        }

        if self.kind == TopologyType::Single {
            if let Some(expected) = &self.set_name
                && incoming.set_name.as_ref() != Some(expected)
                && incoming.kind != ServerType::Unknown
            {
                incoming = ServerDescription::failed(
                    &address,
                    format!("the server is not a member of replica set {expected}"),
                    None,
                );
            }
            self.servers.insert(address, incoming);
            self.refresh();
            return effects;
        }

        let kind = incoming.kind;
        self.servers.insert(address.clone(), incoming.clone());
        use ServerType as S;
        use TopologyType as T;
        match (self.kind, kind) {
            (T::Unknown, S::Standalone) => {
                self.update_unknown_with_standalone(&address, &mut effects)
            }
            (T::Unknown, S::Mongos) => self.kind = T::Sharded,
            (T::Unknown | T::ReplicaSetNoPrimary | T::ReplicaSetWithPrimary, S::RsPrimary) => {
                self.kind = T::ReplicaSetWithPrimary;
                self.update_rs_from_primary(&incoming, &mut effects);
            }
            (T::Unknown, S::RsSecondary | S::RsArbiter | S::RsOther) => {
                self.kind = T::ReplicaSetNoPrimary;
                self.update_rs_without_primary(&incoming, &mut effects);
            }
            (T::ReplicaSetNoPrimary, S::RsSecondary | S::RsArbiter | S::RsOther) => {
                self.update_rs_without_primary(&incoming, &mut effects);
            }
            (T::ReplicaSetWithPrimary, S::RsSecondary | S::RsArbiter | S::RsOther) => {
                self.update_rs_with_primary_from_member(&incoming, &mut effects);
            }
            (T::Sharded, S::Unknown | S::Mongos)
            | (T::Unknown | T::ReplicaSetNoPrimary, S::Unknown | S::RsGhost) => {}
            (T::Sharded, _) | (T::ReplicaSetNoPrimary, S::Standalone | S::Mongos) => {
                self.remove(&address, &mut effects);
            }
            (T::ReplicaSetWithPrimary, S::Standalone | S::Mongos) => {
                self.remove(&address, &mut effects);
                self.check_if_has_primary();
            }
            (T::ReplicaSetWithPrimary, S::Unknown | S::RsGhost) => self.check_if_has_primary(),
            (_, S::PossiblePrimary) | (T::Single, _) => {}
        }
        self.refresh();
        effects
    }

    fn update_unknown_with_standalone(&mut self, address: &str, effects: &mut Effects) {
        if self.seeds == 1 {
            self.kind = TopologyType::Single;
        } else {
            self.remove(address, effects);
        }
    }

    fn update_rs_without_primary(&mut self, d: &ServerDescription, effects: &mut Effects) {
        if self.set_name.is_none() {
            self.set_name.clone_from(&d.set_name);
        } else if self.set_name != d.set_name {
            self.remove(&d.address, effects);
            return;
        }
        self.add_members(&d.members, effects);
        if let Some(primary) = &d.primary
            && let Some(server) = self.servers.get_mut(primary)
            && server.kind == ServerType::Unknown
        {
            server.kind = ServerType::PossiblePrimary;
        }
        if d.me.as_deref() != Some(d.address.as_str()) && d.me.is_some() {
            self.remove(&d.address, effects);
        }
    }

    fn update_rs_with_primary_from_member(&mut self, d: &ServerDescription, effects: &mut Effects) {
        if self.set_name != d.set_name
            || (d.me.is_some() && d.me.as_deref() != Some(d.address.as_str()))
        {
            self.remove(&d.address, effects);
            self.check_if_has_primary();
            return;
        }
        if !self.has_primary() {
            self.kind = TopologyType::ReplicaSetNoPrimary;
            if let Some(primary) = &d.primary
                && let Some(server) = self.servers.get_mut(primary)
                && server.kind == ServerType::Unknown
            {
                server.kind = ServerType::PossiblePrimary;
            }
        }
    }

    fn update_rs_from_primary(&mut self, d: &ServerDescription, effects: &mut Effects) {
        if self.set_name.is_none() {
            self.set_name.clone_from(&d.set_name);
        } else if self.set_name != d.set_name {
            self.remove(&d.address, effects);
            self.check_if_has_primary();
            return;
        }
        let stale = if d.max_wire >= 17 {
            // (electionId, setVersion) in order; a missing value is the least.
            let newer = match (d.election_id, self.max_election_id) {
                (Some(e), Some(max)) if e != max => e > max,
                (None, Some(_)) => false,
                (Some(_), None) => true,
                _ => {
                    d.set_version.unwrap_or(i64::MIN) >= self.max_set_version.unwrap_or(i64::MIN)
                        || (d.set_version.is_none() && self.max_set_version.is_none())
                }
            };
            if newer {
                self.max_election_id = d.election_id;
                self.max_set_version = d.set_version;
                false
            } else {
                true
            }
        } else {
            let mut stale = false;
            if let (Some(set_version), Some(election_id)) = (d.set_version, d.election_id) {
                if let (Some(max_v), Some(max_e)) = (self.max_set_version, self.max_election_id)
                    && (max_v > set_version || (max_v == set_version && max_e > election_id))
                {
                    stale = true;
                } else {
                    self.max_election_id = Some(election_id);
                }
            }
            if !stale
                && let Some(set_version) = d.set_version
                && self.max_set_version.is_none_or(|max| set_version > max)
            {
                self.max_set_version = Some(set_version);
            }
            stale
        };
        if stale {
            self.servers.insert(
                d.address.clone(),
                ServerDescription::failed(
                    &d.address,
                    "primary marked stale due to electionId/setVersion mismatch".into(),
                    None,
                ),
            );
            self.check_if_has_primary();
            return;
        }
        let others: Vec<String> = self
            .servers
            .values()
            .filter(|s| s.address != d.address && s.kind == ServerType::RsPrimary)
            .map(|s| s.address.clone())
            .collect();
        for address in others {
            self.servers.insert(
                address.clone(),
                ServerDescription::failed(
                    &address,
                    "primary marked stale due to discovery of newer primary".into(),
                    None,
                ),
            );
            effects.check.push(address);
        }
        self.add_members(&d.members, effects);
        let gone: Vec<String> = self
            .servers
            .keys()
            .filter(|a| !d.members.contains(a))
            .cloned()
            .collect();
        for address in gone {
            self.remove(&address, effects);
        }
        self.check_if_has_primary();
    }

    fn add_members(&mut self, members: &[String], effects: &mut Effects) {
        for member in members {
            if !self.servers.contains_key(member) {
                self.servers
                    .insert(member.clone(), ServerDescription::unknown(member));
                self.generations.entry(member.clone()).or_insert(0);
                effects.added.push(member.clone());
            }
        }
    }

    fn check_if_has_primary(&mut self) {
        self.kind = if self.has_primary() {
            TopologyType::ReplicaSetWithPrimary
        } else {
            TopologyType::ReplicaSetNoPrimary
        };
    }

    fn remove(&mut self, address: &str, effects: &mut Effects) {
        if self.servers.remove(address).is_some() {
            self.generations.remove(address);
            effects.removed.push(address.to_string());
        }
    }

    /// Recomputes what depends on every server: wire compatibility and the
    /// session timeout.
    fn refresh(&mut self) {
        self.compatibility_error = self.servers.values().find_map(|s| {
            if s.kind == ServerType::Unknown {
                None
            } else if s.min_wire > CLIENT_MAX_WIRE {
                Some(format!(
                    "Server at {} requires wire version {}, but this version of esrun only supports up to {CLIENT_MAX_WIRE}.",
                    s.address, s.min_wire
                ))
            } else if s.max_wire < CLIENT_MIN_WIRE {
                Some(format!(
                    "Server at {} reports wire version {}, but this version of esrun requires at least {CLIENT_MIN_WIRE} (MongoDB 7.0).",
                    s.address, s.max_wire
                ))
            } else {
                None
            }
        });
        let mut timeout: Option<Option<i64>> = None;
        for s in self.servers.values().filter(|s| s.kind.data_bearing()) {
            timeout = Some(match (timeout, s.logical_session_timeout) {
                (Some(None), _) | (_, None) => None,
                (None, Some(t)) => Some(t),
                (Some(Some(min)), Some(t)) => Some(min.min(t)),
            });
        }
        self.logical_session_timeout = timeout.flatten();
    }

    /// Applies an operation's failure on `address`, by the specification's
    /// error handling: an error from before a pool clear is stale and ignored; a
    /// network error, or a command error before the handshake completed, marks the
    /// server Unknown and clears its pool; a "not writable primary" or "node is
    /// recovering" error marks it Unknown and asks for a check, clearing the
    /// pool only when the node is shutting down.
    pub(crate) fn application_error(&mut self, address: &str, error: &AppError) -> Effects {
        let mut effects = Effects::default();
        let Some(current) = self.servers.get(address) else {
            return effects;
        };
        let generation = self.generations.get(address).copied().unwrap_or(0);
        if error.generation < generation {
            return effects;
        }
        match &error.kind {
            AppErrorKind::Command {
                code,
                message,
                topology_version,
            } => {
                let state_change =
                    is_recovering(*code, message) || is_not_writable_primary(*code, message);
                if !state_change {
                    if !error.completed_handshake {
                        self.network_unknown(address, message.clone(), &mut effects);
                    }
                    return effects;
                }
                // Stale if what the server says is not newer than what is known.
                if let (Some(cur), Some(inc)) = (current.topology_version, *topology_version)
                    && cur.process_id == inc.process_id
                    && cur.counter >= inc.counter
                {
                    return effects;
                }
                let changes = self.update(ServerDescription::failed(
                    address,
                    message.clone(),
                    *topology_version,
                ));
                effects.removed.extend(changes.removed);
                if matches!(code, Some(11600 | 91)) {
                    *self.generations.entry(address.to_string()).or_insert(0) += 1;
                    effects.clear.push(address.to_string());
                }
                effects.check.push(address.to_string());
            }
            AppErrorKind::Network(message) => {
                self.network_unknown(address, message.clone(), &mut effects)
            }
            // A timeout may be a slow operation rather than an unavailable server,
            // before the handshake as after it.
            AppErrorKind::Timeout => {}
        }
        effects
    }

    fn network_unknown(&mut self, address: &str, message: String, effects: &mut Effects) {
        let changes = self.update(ServerDescription::failed(address, message, None));
        effects.removed.extend(changes.removed);
        *self.generations.entry(address.to_string()).or_insert(0) += 1;
        effects.clear.push(address.to_string());
    }
}

/// An operation's failure, as SDAM classifies it.
pub(crate) struct AppError {
    /// The pool generation of the connection it happened on.
    pub generation: u64,
    pub completed_handshake: bool,
    pub kind: AppErrorKind,
}

pub(crate) enum AppErrorKind {
    Network(String),
    Timeout,
    Command {
        code: Option<i32>,
        message: String,
        topology_version: Option<TopologyVersion>,
    },
}

const RECOVERING: [i32; 5] = [11600, 11602, 13436, 189, 91];
const NOT_WRITABLE_PRIMARY: [i32; 3] = [10107, 13435, 10058];

pub(crate) fn is_recovering(code: Option<i32>, message: &str) -> bool {
    match code {
        Some(code) => RECOVERING.contains(&code),
        None => {
            message.contains("not master or secondary") || message.contains("node is recovering")
        }
    }
}

pub(crate) fn is_not_writable_primary(code: Option<i32>, message: &str) -> bool {
    match code {
        Some(code) => NOT_WRITABLE_PRIMARY.contains(&code),
        None => !is_recovering(None, message) && message.contains("not master"),
    }
}

/// The exponentially weighted average the server selection specification
/// gives: a fifth of the new measurement.
pub(crate) fn average_rtt(old: f64, new: f64) -> f64 {
    0.2 * new + 0.8 * old
}

fn field<'a>(doc: &Document<'a>, key: &str) -> Option<Element<'a>> {
    doc.get(key).ok().flatten()
}

fn truthy(doc: &Document<'_>, key: &str) -> bool {
    match field(doc, key) {
        Some(Element::Boolean(b)) => b,
        Some(Element::Int32(n)) => n != 0,
        Some(Element::Int64(n)) => n != 0,
        Some(Element::Double(n)) => n != 0.0,
        _ => false,
    }
}

fn text(doc: &Document<'_>, key: &str) -> Option<String> {
    match field(doc, key) {
        Some(Element::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn int(doc: &Document<'_>, key: &str) -> Option<i32> {
    long(doc, key).and_then(|n| i32::try_from(n).ok())
}

fn long(doc: &Document<'_>, key: &str) -> Option<i64> {
    match field(doc, key) {
        Some(Element::Int32(n)) => Some(i64::from(n)),
        Some(Element::Int64(n)) => Some(n),
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a server count sent as a double"
        )]
        Some(Element::Double(n)) => Some(n as i64),
        _ => None,
    }
}
