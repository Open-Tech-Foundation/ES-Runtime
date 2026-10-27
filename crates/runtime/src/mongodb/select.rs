//! Server selection: which servers may take an operation, by topology, read
//! preference, staleness and tags, and which of those are near enough to use
//! (MongoDB's server selection specification). Pure, like `sdam`.

use super::sdam::{ServerDescription, ServerType, TopologyDescription, TopologyType};

/// The smallest `maxStalenessSeconds` a replica set accepts.
const SMALLEST_MAX_STALENESS_SECONDS: i64 = 90;
/// How often an idle primary writes a no-op to the oplog.
const IDLE_WRITE_PERIOD_MS: i64 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Primary,
    PrimaryPreferred,
    Secondary,
    SecondaryPreferred,
    Nearest,
}

impl Mode {
    pub(crate) fn parse(text: &str) -> Option<Mode> {
        match text.to_ascii_lowercase().as_str() {
            "primary" => Some(Mode::Primary),
            "primarypreferred" => Some(Mode::PrimaryPreferred),
            "secondary" => Some(Mode::Secondary),
            "secondarypreferred" => Some(Mode::SecondaryPreferred),
            "nearest" => Some(Mode::Nearest),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Mode::Primary => "primary",
            Mode::PrimaryPreferred => "primaryPreferred",
            Mode::Secondary => "secondary",
            Mode::SecondaryPreferred => "secondaryPreferred",
            Mode::Nearest => "nearest",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReadPreference {
    pub mode: Mode,
    /// Tried in order; `[{}]`, the default, matches every server.
    pub tag_sets: Vec<Vec<(String, String)>>,
    pub max_staleness_seconds: Option<i64>,
}

impl ReadPreference {
    pub(crate) fn primary() -> Self {
        ReadPreference {
            mode: Mode::Primary,
            tag_sets: Vec::new(),
            max_staleness_seconds: None,
        }
    }
}

/// What the operation is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Read,
    Write,
}

/// The servers suitable for the operation, before the latency window. An error
/// is a read preference that cannot be used here.
pub(crate) fn suitable<'a>(
    topology: &'a TopologyDescription,
    preference: &ReadPreference,
    operation: Operation,
    heartbeat_ms: i64,
    deprioritized: &[String],
) -> Result<Vec<&'a ServerDescription>, String> {
    let preference = if operation == Operation::Write {
        &ReadPreference::primary()
    } else {
        preference
    };
    // -1 means no maximum; any other value must be positive.
    if let Some(max) = preference
        .max_staleness_seconds
        .filter(|&s| s != -1 && s <= 0)
    {
        return Err(format!(
            "maxStalenessSeconds must be positive, or -1 for none, not {max}"
        ));
    }
    if let Some(max) = preference.max_staleness_seconds.filter(|&s| s > 0) {
        if preference.mode == Mode::Primary {
            return Err("maxStalenessSeconds cannot be used with mode primary".into());
        }
        if matches!(
            topology.kind,
            TopologyType::ReplicaSetNoPrimary | TopologyType::ReplicaSetWithPrimary
        ) && (max < SMALLEST_MAX_STALENESS_SECONDS
            || max * 1000 < heartbeat_ms + IDLE_WRITE_PERIOD_MS)
        {
            return Err(format!(
                "maxStalenessSeconds must be at least {SMALLEST_MAX_STALENESS_SECONDS} and at least heartbeatFrequencyMS + 10000 ms"
            ));
        }
    }
    if preference.mode == Mode::Primary && preference.tag_sets.iter().any(|t| !t.is_empty()) {
        return Err("tag sets cannot be used with mode primary".into());
    }
    let all: Vec<&ServerDescription> = topology.servers.values().collect();
    let found = select_from(
        topology,
        &all,
        preference,
        operation,
        heartbeat_ms,
        deprioritized,
    );
    if found.is_empty() && !deprioritized.is_empty() {
        return Ok(select_from(
            topology,
            &all,
            preference,
            operation,
            heartbeat_ms,
            &[],
        ));
    }
    Ok(found)
}

fn select_from<'a>(
    topology: &TopologyDescription,
    all: &[&'a ServerDescription],
    preference: &ReadPreference,
    operation: Operation,
    heartbeat_ms: i64,
    deprioritized: &[String],
) -> Vec<&'a ServerDescription> {
    let servers: Vec<&ServerDescription> = all
        .iter()
        .copied()
        .filter(|s| !deprioritized.contains(&s.address))
        .collect();
    let of = |kind: ServerType| servers.iter().copied().filter(move |s| s.kind == kind);
    match topology.kind {
        TopologyType::Unknown => Vec::new(),
        TopologyType::Single => servers
            .into_iter()
            .filter(|s| s.kind != ServerType::Unknown)
            .collect(),
        TopologyType::Sharded => of(ServerType::Mongos).collect(),
        TopologyType::ReplicaSetNoPrimary | TopologyType::ReplicaSetWithPrimary => {
            let primary: Vec<&ServerDescription> = of(ServerType::RsPrimary).collect();
            if operation == Operation::Write {
                return primary;
            }
            let secondaries = || {
                let candidates: Vec<&ServerDescription> = of(ServerType::RsSecondary).collect();
                eligible(topology, candidates, preference, heartbeat_ms)
            };
            match preference.mode {
                Mode::Primary => primary,
                Mode::PrimaryPreferred if !primary.is_empty() => primary,
                Mode::PrimaryPreferred | Mode::Secondary => secondaries(),
                Mode::SecondaryPreferred => {
                    let found = secondaries();
                    if found.is_empty() { primary } else { found }
                }
                Mode::Nearest => {
                    let candidates: Vec<&ServerDescription> = servers
                        .iter()
                        .copied()
                        .filter(|s| {
                            matches!(s.kind, ServerType::RsPrimary | ServerType::RsSecondary)
                        })
                        .collect();
                    eligible(topology, candidates, preference, heartbeat_ms)
                }
            }
        }
    }
}

/// Candidates fresh enough for `maxStalenessSeconds`, then the first tag set
/// that matches any of them.
fn eligible<'a>(
    topology: &TopologyDescription,
    candidates: Vec<&'a ServerDescription>,
    preference: &ReadPreference,
    heartbeat_ms: i64,
) -> Vec<&'a ServerDescription> {
    let fresh: Vec<&ServerDescription> = match preference.max_staleness_seconds.filter(|&s| s > 0) {
        None => candidates,
        Some(max) => candidates
            .into_iter()
            .filter(|s| staleness(topology, s, heartbeat_ms) <= max * 1000)
            .collect(),
    };
    if preference.tag_sets.is_empty() {
        return fresh;
    }
    for tag_set in &preference.tag_sets {
        let matching: Vec<&ServerDescription> = fresh
            .iter()
            .copied()
            .filter(|s| tag_set.iter().all(|t| s.tags.contains(t)))
            .collect();
        if !matching.is_empty() {
            return matching;
        }
    }
    Vec::new()
}

/// A secondary's estimated staleness in milliseconds; zero for anything else.
fn staleness(topology: &TopologyDescription, s: &ServerDescription, heartbeat_ms: i64) -> i64 {
    if s.kind != ServerType::RsSecondary {
        return 0;
    }
    let last_write = s.last_write_date.unwrap_or(0);
    if let Some(p) = topology
        .servers
        .values()
        .find(|p| p.kind == ServerType::RsPrimary)
    {
        (s.last_update_time - last_write) - (p.last_update_time - p.last_write_date.unwrap_or(0))
            + heartbeat_ms
    } else {
        let max = topology
            .servers
            .values()
            .filter(|x| x.kind == ServerType::RsSecondary)
            .filter_map(|x| x.last_write_date)
            .max()
            .unwrap_or(last_write);
        max - last_write + heartbeat_ms
    }
}

/// The suitable servers within `local_threshold_ms` of the fastest.
pub(crate) fn in_latency_window<'a>(
    suitable: &[&'a ServerDescription],
    local_threshold_ms: f64,
) -> Vec<&'a ServerDescription> {
    let fastest = suitable
        .iter()
        .filter_map(|s| s.avg_rtt)
        .fold(f64::INFINITY, f64::min);
    suitable
        .iter()
        .copied()
        .filter(|s| {
            s.avg_rtt
                .is_none_or(|rtt| rtt <= fastest + local_threshold_ms)
        })
        .collect()
}

/// Of two servers drawn at random from the window, the one running fewer
/// operations — the specification's choice over plain random selection, which
/// sends a slow server as much work as a fast one.
pub(crate) fn pick<'a>(
    window: &[&'a ServerDescription],
    operations: impl Fn(&str) -> usize,
    random: &mut dyn FnMut(usize) -> usize,
) -> Option<&'a ServerDescription> {
    match window.len() {
        0 => None,
        1 => Some(window[0]),
        n => {
            let a = random(n);
            let mut b = random(n - 1);
            if b >= a {
                b += 1;
            }
            let (a, b) = (window[a], window[b]);
            Some(if operations(&b.address) < operations(&a.address) {
                b
            } else {
                a
            })
        }
    }
}
