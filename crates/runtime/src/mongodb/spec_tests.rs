//! MongoDB's server discovery and monitoring and server selection test suites,
//! run against `sdam` and `select`.
//!
//! The suites are fetched by `mongodb-specs/fetch.sh` rather than committed
//! (they are CC BY-NC-SA). Without the checkout these tests say so and pass;
//! `tsr test:mongodb-specs` fetches it and sets `MONGODB_SPECS_REQUIRED`, which
//! turns an absent checkout into a failure.

use std::path::{Path, PathBuf};

use es_runtime_bson::{Document, Writer, kind};
use serde_json::Value as Json;

use super::sdam::{
    AppError, AppErrorKind, ServerDescription, ServerType, TopologyDescription, TopologyType,
    average_rtt, normalize, topology_version,
};
use super::select::{self, Mode, Operation, ReadPreference};

#[expect(clippy::print_stderr, reason = "a test that skips says why")]
fn specs() -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../mongodb-specs/upstream/source");
    if dir.is_dir() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("MONGODB_SPECS_REQUIRED").is_none(),
        "the MongoDB specification tests are not fetched: run mongodb-specs/fetch.sh"
    );
    eprintln!("skipped: the MongoDB specification tests are not fetched (mongodb-specs/fetch.sh)");
    None
}

fn json_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    files
}

fn read(path: &Path) -> Json {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

// ---- extended JSON to BSON, for the hello replies the tests feed in ---------

fn bson(doc: &Json) -> Vec<u8> {
    let mut w = Writer::new();
    document(&mut w, doc);
    w.finish().unwrap()
}

fn document(w: &mut Writer, doc: &Json) {
    w.start_document().unwrap();
    for (key, value) in doc.as_object().unwrap() {
        element(w, key, value);
    }
    w.end_document().unwrap();
}

fn element(w: &mut Writer, key: &str, value: &Json) {
    match value {
        Json::Null => w.key(kind::NULL, key).unwrap(),
        Json::Bool(b) => {
            w.key(kind::BOOLEAN, key).unwrap();
            w.boolean(*b);
        }
        Json::Number(n) => {
            if let Some(i) = n.as_i64().and_then(|i| i32::try_from(i).ok()) {
                w.key(kind::INT32, key).unwrap();
                w.int32(i);
            } else if let Some(i) = n.as_i64() {
                w.key(kind::INT64, key).unwrap();
                w.int64(i);
            } else {
                w.key(kind::DOUBLE, key).unwrap();
                w.double(n.as_f64().unwrap());
            }
        }
        Json::String(s) => {
            w.key(kind::STRING, key).unwrap();
            w.string(s).unwrap();
        }
        Json::Array(items) => {
            w.key(kind::ARRAY, key).unwrap();
            w.start_document().unwrap();
            for (i, item) in items.iter().enumerate() {
                element(w, &i.to_string(), item);
            }
            w.end_document().unwrap();
        }
        Json::Object(map) => {
            if let Some(Json::String(oid)) = map.get("$oid") {
                w.key(kind::OBJECT_ID, key).unwrap();
                w.raw(&es_runtime_bson::from_hex::<12>(oid).unwrap());
            } else if let Some(Json::String(n)) = map.get("$numberLong") {
                w.key(kind::INT64, key).unwrap();
                w.int64(n.parse().unwrap());
            } else if let Some(date) = map.get("$date") {
                w.key(kind::DATETIME, key).unwrap();
                w.int64(long_of(date));
            } else {
                w.key(kind::DOCUMENT, key).unwrap();
                document(w, value);
            }
        }
    }
}

fn long_of(value: &Json) -> i64 {
    match value {
        Json::Number(n) => n.as_i64().unwrap(),
        Json::Object(map) => map["$numberLong"].as_str().unwrap().parse().unwrap(),
        Json::String(s) => s.parse().unwrap(),
        other => panic!("not a number: {other}"),
    }
}

// ---- server discovery and monitoring -----------------------------------------

struct Uri {
    seeds: Vec<String>,
    set_name: Option<String>,
    direct: bool,
    load_balanced: bool,
}

fn parse_uri(uri: &str) -> Uri {
    let rest = uri.strip_prefix("mongodb://").unwrap();
    let (hosts, query) = rest.split_once('/').map_or((rest, ""), |(h, q)| (h, q));
    let query = query.split_once('?').map_or("", |(_, q)| q);
    let mut set_name = None;
    let mut direct = false;
    let mut load_balanced = false;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap();
        match k {
            "replicaSet" => set_name = Some(v.to_string()),
            "directConnection" => direct = v == "true",
            "loadBalanced" => load_balanced = v == "true",
            _ => {}
        }
    }
    Uri {
        seeds: hosts.split(',').map(normalize).collect(),
        set_name,
        direct,
        load_balanced,
    }
}

fn server_type(name: &str) -> ServerType {
    match name {
        "Standalone" => ServerType::Standalone,
        "Mongos" => ServerType::Mongos,
        "PossiblePrimary" => ServerType::PossiblePrimary,
        "RSPrimary" => ServerType::RsPrimary,
        "RSSecondary" => ServerType::RsSecondary,
        "RSArbiter" => ServerType::RsArbiter,
        "RSOther" => ServerType::RsOther,
        "RSGhost" => ServerType::RsGhost,
        _ => ServerType::Unknown,
    }
}

fn check_outcome(
    name: &str,
    phase: usize,
    topology: &TopologyDescription,
    outcome: &Json,
    failures: &mut Vec<String>,
) {
    let mut fail = |what: String| failures.push(format!("{name} phase {phase}: {what}"));
    let expected_type = outcome["topologyType"].as_str().unwrap();
    if topology.kind.name() != expected_type {
        fail(format!(
            "topology type {} != {expected_type}",
            topology.kind.name()
        ));
    }
    if outcome.get("setName").is_some()
        && topology.set_name.as_deref() != outcome["setName"].as_str()
    {
        fail(format!(
            "setName {:?} != {}",
            topology.set_name, outcome["setName"]
        ));
    }
    if let Some(lst) = outcome.get("logicalSessionTimeoutMinutes")
        && topology.logical_session_timeout != lst.as_i64()
    {
        fail(format!(
            "session timeout {:?} != {lst}",
            topology.logical_session_timeout
        ));
    }
    if let Some(v) = outcome.get("maxSetVersion")
        && topology.max_set_version != v.as_i64()
    {
        fail(format!(
            "maxSetVersion {:?} != {v}",
            topology.max_set_version
        ));
    }
    if let Some(e) = outcome.get("maxElectionId") {
        let expected = e
            .get("$oid")
            .and_then(Json::as_str)
            .map(|s| es_runtime_bson::from_hex::<12>(s).unwrap());
        if topology.max_election_id != expected {
            fail(format!(
                "maxElectionId {:?} != {e}",
                topology.max_election_id
            ));
        }
    }
    if let Some(c) = outcome.get("compatible")
        && topology.compatibility_error.is_none() != c.as_bool().unwrap()
    {
        fail(format!(
            "compatible {:?} != {c}",
            topology.compatibility_error
        ));
    }
    let servers = outcome["servers"].as_object().unwrap();
    let mut have: Vec<&String> = topology.servers.keys().collect();
    let mut want: Vec<&String> = servers.keys().collect();
    have.sort();
    want.sort();
    if have != want {
        fail(format!("servers {have:?} != {want:?}"));
        return;
    }
    for (address, expected) in servers {
        let server = &topology.servers[address];
        let want_type = expected["type"].as_str().unwrap();
        if server.kind != server_type(want_type) || server.kind.name() != want_type {
            fail(format!(
                "{address} type {} != {want_type}",
                server.kind.name()
            ));
        }
        if let Some(s) = expected.get("setName")
            && server.set_name.as_deref() != s.as_str()
        {
            fail(format!("{address} setName {:?} != {s}", server.set_name));
        }
        if let Some(v) = expected.get("setVersion")
            && server.set_version != v.as_i64()
        {
            fail(format!(
                "{address} setVersion {:?} != {v}",
                server.set_version
            ));
        }
        if let Some(e) = expected.get("electionId") {
            let want = e
                .get("$oid")
                .and_then(Json::as_str)
                .map(|s| es_runtime_bson::from_hex::<12>(s).unwrap());
            if server.election_id != want {
                fail(format!(
                    "{address} electionId {:?} != {e}",
                    server.election_id
                ));
            }
        }
        if let Some(tv) = expected.get("topologyVersion") {
            let want = if tv.is_null() {
                None
            } else {
                let doc = bson(&serde_json::json!({ "topologyVersion": tv }));
                topology_version(&Document::new(&doc).unwrap())
            };
            if server.topology_version != want {
                fail(format!(
                    "{address} topologyVersion {:?} != {tv}",
                    server.topology_version
                ));
            }
        }
        if let Some(generation) = expected.get("pool").and_then(|p| p.get("generation")) {
            let have = topology.generations.get(address).copied().unwrap_or(0);
            if Some(have) != generation.as_u64() {
                fail(format!("{address} pool generation {have} != {generation}"));
            }
        }
        if let Some(error) = expected.get("error").and_then(Json::as_str)
            && !server.error.as_deref().unwrap_or("").contains(error)
        {
            fail(format!(
                "{address} error {:?} lacks {error:?}",
                server.error
            ));
        }
    }
}

fn app_error(topology: &TopologyDescription, e: &Json) -> (String, AppError) {
    let address = normalize(e["address"].as_str().unwrap());
    let generation = e
        .get("generation")
        .and_then(Json::as_u64)
        .unwrap_or_else(|| topology.generations.get(&address).copied().unwrap_or(0));
    let completed_handshake = e["when"] == "afterHandshakeCompletes";
    let kind = match e["type"].as_str().unwrap() {
        "network" => AppErrorKind::Network("network error".into()),
        "timeout" => AppErrorKind::Timeout,
        _ => {
            let response = &e["response"];
            let doc = bson(response);
            AppErrorKind::Command {
                code: response
                    .get("code")
                    .and_then(Json::as_i64)
                    .map(|c| i32::try_from(c).unwrap()),
                message: response
                    .get("errmsg")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
                topology_version: topology_version(&Document::new(&doc).unwrap()),
            }
        }
    };
    (
        address,
        AppError {
            generation,
            completed_handshake,
            kind,
        },
    )
}

#[test]
fn server_discovery_and_monitoring() {
    let Some(root) = specs() else { return };
    let mut failures = Vec::new();
    let mut ran = 0;
    for dir in ["single", "rs", "sharded", "errors"] {
        for file in json_files(&root.join("server-discovery-and-monitoring/tests").join(dir)) {
            let test = read(&file);
            let uri = parse_uri(test["uri"].as_str().unwrap());
            if uri.load_balanced {
                continue;
            }
            let name = format!("{dir}/{}", file.file_name().unwrap().to_string_lossy());
            let mut topology = TopologyDescription::new(&uri.seeds, uri.direct, uri.set_name);
            for (i, phase) in test["phases"].as_array().unwrap().iter().enumerate() {
                for response in phase
                    .get("responses")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    let address = normalize(response[0].as_str().unwrap());
                    let hello = &response[1];
                    let description = if hello.as_object().unwrap().is_empty() {
                        ServerDescription::failed(
                            &address,
                            "network error during the check".into(),
                            None,
                        )
                    } else {
                        let bytes = bson(hello);
                        ServerDescription::from_hello(
                            &address,
                            &Document::new(&bytes).unwrap(),
                            Some(0.0),
                            0,
                        )
                    };
                    topology.update(description);
                }
                for error in phase
                    .get("applicationErrors")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    let (address, error) = app_error(&topology, error);
                    topology.application_error(&address, &error);
                }
                check_outcome(&name, i, &topology, &phase["outcome"], &mut failures);
            }
            ran += 1;
        }
    }
    assert!(ran > 100, "only {ran} SDAM tests found");
    assert!(
        failures.is_empty(),
        "{} SDAM failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

// ---- server selection -------------------------------------------------------

fn described(server: &Json) -> ServerDescription {
    let address = normalize(server["address"].as_str().unwrap());
    let mut d = ServerDescription::unknown(&address);
    d.kind = server_type(server["type"].as_str().unwrap());
    d.avg_rtt = server.get("avg_rtt_ms").and_then(Json::as_f64);
    d.max_wire = server
        .get("maxWireVersion")
        .and_then(Json::as_i64)
        .map_or(21, |w| i32::try_from(w).unwrap());
    if let Some(tags) = server.get("tags").and_then(Json::as_object) {
        d.tags = tags
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
            .collect();
    }
    if let Some(date) = server.get("lastWrite").and_then(|l| l.get("lastWriteDate")) {
        d.last_write_date = Some(long_of(date));
    }
    if let Some(time) = server.get("lastUpdateTime") {
        d.last_update_time = long_of(time);
    }
    d
}

fn topology_from(description: &Json) -> TopologyDescription {
    let servers: Vec<ServerDescription> = description["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(described)
        .collect();
    let mut topology = TopologyDescription::new(&[], false, None);
    topology.kind = match description["type"].as_str().unwrap() {
        "Single" => TopologyType::Single,
        "ReplicaSetNoPrimary" => TopologyType::ReplicaSetNoPrimary,
        "ReplicaSetWithPrimary" => TopologyType::ReplicaSetWithPrimary,
        "Sharded" => TopologyType::Sharded,
        _ => TopologyType::Unknown,
    };
    for server in servers {
        topology.servers.insert(server.address.clone(), server);
    }
    topology
}

fn preference(json: &Json) -> ReadPreference {
    ReadPreference {
        mode: json
            .get("mode")
            .and_then(Json::as_str)
            .map_or(Mode::Primary, |m| Mode::parse(m).unwrap()),
        tag_sets: json
            .get("tag_sets")
            .and_then(Json::as_array)
            .map(|sets| {
                sets.iter()
                    .map(|set| {
                        set.as_object()
                            .unwrap()
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                            .collect()
                    })
                    .collect()
            })
            .unwrap_or_default(),
        max_staleness_seconds: json.get("maxStalenessSeconds").and_then(Json::as_i64),
    }
}

fn addresses(list: &Json) -> Vec<String> {
    let mut out: Vec<String> = list
        .as_array()
        .map(|l| {
            l.iter()
                .map(|s| normalize(s["address"].as_str().unwrap()))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn run_selection(file: &Path, failures: &mut Vec<String>) {
    let test = read(file);
    let name = file.display().to_string();
    if test["topology_description"]["type"] == "LoadBalanced" {
        return;
    }
    let topology = topology_from(&test["topology_description"]);
    let operation = if test.get("operation").and_then(Json::as_str) == Some("write") {
        Operation::Write
    } else {
        Operation::Read
    };
    let heartbeat = test
        .get("heartbeatFrequencyMS")
        .and_then(Json::as_i64)
        .unwrap_or(10_000);
    let deprioritized = addresses(test.get("deprioritized_servers").unwrap_or(&Json::Null));
    let result = select::suitable(
        &topology,
        &preference(&test["read_preference"]),
        operation,
        heartbeat,
        &deprioritized,
    );
    match result {
        Err(e) if test.get("error").and_then(Json::as_bool) == Some(true) => {
            let _ = e;
        }
        Err(e) => failures.push(format!("{name}: unexpected error {e}")),
        Ok(_) if test.get("error").and_then(Json::as_bool) == Some(true) => {
            failures.push(format!("{name}: expected an error"));
        }
        Ok(found) => {
            let mut suitable: Vec<String> = found.iter().map(|s| s.address.clone()).collect();
            suitable.sort();
            if suitable != addresses(&test["suitable_servers"]) {
                failures.push(format!(
                    "{name}: suitable {suitable:?} != {:?}",
                    addresses(&test["suitable_servers"])
                ));
            }
            let mut window: Vec<String> = select::in_latency_window(&found, 15.0)
                .iter()
                .map(|s| s.address.clone())
                .collect();
            window.sort();
            if window != addresses(&test["in_latency_window"]) {
                failures.push(format!(
                    "{name}: window {window:?} != {:?}",
                    addresses(&test["in_latency_window"])
                ));
            }
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

#[test]
fn server_selection_and_max_staleness() {
    let Some(root) = specs() else { return };
    let mut files = Vec::new();
    walk(
        &root.join("server-selection/tests/server_selection"),
        &mut files,
    );
    walk(&root.join("max-staleness/tests"), &mut files);
    files.sort();
    let mut failures = Vec::new();
    for file in &files {
        run_selection(file, &mut failures);
    }
    assert!(
        files.len() > 100,
        "only {} selection tests found",
        files.len()
    );
    assert!(
        failures.is_empty(),
        "{} selection failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn round_trip_time_averaging() {
    let Some(root) = specs() else { return };
    for file in json_files(&root.join("server-selection/tests/rtt")) {
        let test = read(&file);
        let new = test["new_rtt_ms"].as_f64().unwrap();
        let got = match test["avg_rtt_ms"].as_f64() {
            Some(old) => average_rtt(old, new),
            None => new,
        };
        let want = test["new_avg_rtt"].as_f64().unwrap();
        assert!(
            (got - want).abs() < 1e-9,
            "{}: {got} != {want}",
            file.display()
        );
    }
}

#[test]
fn operation_count_selection_within_the_window() {
    let Some(root) = specs() else { return };
    // A fixed xorshift, so a run is reproducible.
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut random = move |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        usize::try_from(state % n as u64).unwrap()
    };
    for file in json_files(&root.join("server-selection/tests/in_window")) {
        let test = read(&file);
        let topology = topology_from(&test["topology_description"]);
        let counts: std::collections::HashMap<String, usize> = test["mocked_topology_state"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    normalize(s["address"].as_str().unwrap()),
                    usize::try_from(s["operation_count"].as_u64().unwrap()).unwrap(),
                )
            })
            .collect();
        // The suite's README: every iteration selects with mode "nearest".
        let nearest = ReadPreference {
            mode: Mode::Nearest,
            tag_sets: Vec::new(),
            max_staleness_seconds: None,
        };
        let found = select::suitable(&topology, &nearest, Operation::Read, 10_000, &[]).unwrap();
        let window = select::in_latency_window(&found, 15.0);
        let iterations = test["iterations"].as_u64().unwrap();
        let mut seen: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for _ in 0..iterations {
            let chosen = select::pick(
                &window,
                |a| counts.get(a).copied().unwrap_or(0),
                &mut random,
            )
            .unwrap();
            *seen.entry(chosen.address.clone()).or_default() += 1;
        }
        let tolerance = test["outcome"]["tolerance"].as_f64().unwrap();
        for (address, frequency) in test["outcome"]["expected_frequencies"].as_object().unwrap() {
            #[expect(clippy::cast_precision_loss, reason = "a few thousand iterations")]
            let got =
                seen.get(&normalize(address)).copied().unwrap_or(0) as f64 / iterations as f64;
            let want = frequency.as_f64().unwrap();
            assert!(
                (got - want).abs() <= tolerance,
                "{}: {address} {got} != {want}",
                file.display()
            );
        }
    }
}
