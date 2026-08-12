//! `--trace-permissions` — what the run actually reached for, and the `esrun`
//! command line that would grant exactly that (DECISIONS.md D59).
//!
//! The gap this closes is the last-mile one D59 was written about: `esrun`
//! grants everything by default and can be narrowed to nothing, but nothing
//! helped a developer arrive at the right flags — so in practice they ship the
//! default and the differentiator evaporates. Running the program once under
//! this prints the line to deploy with.
//!
//! What it observes is the capability check itself, in `engine`, which is the
//! only place that knows what a program *reached for* rather than what it was
//! given. Everything here is the other half: turning those observations into a
//! command line, which is a question about `esrun`'s flag grammar and belongs in
//! the binary a developer runs, not in the one that serves production.

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use es_runtime_cli_common::{Capability, CapabilityObserver};

/// One capability, as this run used it.
#[derive(Default)]
struct Used {
    /// Held at least once when something asked for it.
    granted: bool,
    /// Refused at least once — the program wanted it and did not have it.
    denied: bool,
    /// What asked, by op name (`fs_read`, `process_env`) or `import` for the
    /// module loader. Ordered and deduplicated: the same op firing in a loop is
    /// one fact, not ten thousand.
    ops: BTreeSet<String>,
}

/// Records every capability check and reports the grant line at the end.
pub struct PermissionTrace {
    /// Keyed by the flag name (`read`, `imports`, …) rather than the capability,
    /// because that is the vocabulary the report is written in and the one the
    /// user will type. Capabilities with no flag — the clock, entropy, timers —
    /// are dropped on the way in: no flag revokes them, so naming them in a
    /// report about flags would only suggest one does.
    used: Mutex<HashMap<&'static str, Used>>,
    /// The entry, as it was named on the command line, for the printed line.
    entry: String,
    /// Reported already. The run has several ways to end and each of them says
    /// so, which is the point — but the report is printed once.
    reported: AtomicBool,
}

impl PermissionTrace {
    /// A trace for a run of `entry`.
    pub fn new(entry: String) -> PermissionTrace {
        PermissionTrace {
            used: Mutex::new(HashMap::new()),
            entry,
            reported: AtomicBool::new(false),
        }
    }

    /// The report, as text. Separate from printing it so a test can read it.
    fn report(&self) -> String {
        let used = self.used.lock().unwrap_or_else(|e| e.into_inner());

        // Fixed order — the one `Capability::HOST_FACING` is written in — so two
        // runs of the same program produce the same report, and a diff between
        // two programs is about the programs.
        let rows: Vec<(&'static str, &Used)> = Capability::HOST_FACING
            .iter()
            .filter_map(|capability| capability.flag_name())
            .filter_map(|name| used.get(name).map(|entry| (name, entry)))
            .collect();

        let mut out = String::from("\nesdev: the permissions this run used\n\n");
        if rows.is_empty() {
            out.push_str("  none — it reached past the isolate for nothing at all\n");
        }
        for (name, entry) in &rows {
            let ops: Vec<&str> = entry.ops.iter().map(String::as_str).collect();
            let status = if entry.denied && !entry.granted {
                "  (denied — the program asked and was refused)"
            } else if entry.denied {
                "  (also denied somewhere — a worker's grants are set at the spawn)"
            } else {
                ""
            };
            out.push_str(&format!("  {name:<9} {}{status}\n", ops.join(", ")));
        }

        // The line to deploy with lists what was **granted and used**. A denied
        // capability is deliberately left out: the program asked for it and was
        // refused, and whether that was the right answer is the developer's
        // call, not a trace's.
        let grants: Vec<String> = rows
            .iter()
            .filter(|(_, entry)| entry.granted)
            .map(|(name, _)| format!("--allow-{name}"))
            .collect();
        out.push_str(&format!(
            "\n  esrun --deny-all {}{}\n",
            grants
                .iter()
                .map(|grant| format!("{grant} "))
                .collect::<String>(),
            self.entry
        ));
        out.push_str(
            "\n  Scopes are not traced: --allow-read grants every path this way. Narrow each\n  \
             grant by hand (--allow-read=./data) once you know what the program needs.\n",
        );
        out
    }
}

impl CapabilityObserver for PermissionTrace {
    fn observed(&self, op: &str, capability: Capability, granted: bool) {
        // A capability with no flag cannot be denied and cannot be granted back,
        // so it is not part of the question being asked here.
        let Some(name) = capability.flag_name() else {
            return;
        };
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        let entry = used.entry(name).or_default();
        if granted {
            entry.granted = true;
        } else {
            entry.denied = true;
        }
        if !entry.ops.contains(op) {
            entry.ops.insert(op.to_string());
        }
    }

    fn run_finished(&self) {
        if self.reported.swap(true, Ordering::SeqCst) {
            return;
        }
        // stderr: this is a report about the run, not the run's output, so a
        // program being piped somewhere keeps its stdout to itself.
        eprint!("{}", self.report());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace() -> PermissionTrace {
        PermissionTrace::new("app.mjs".to_string())
    }

    #[test]
    fn a_run_that_touched_nothing_gets_the_tightest_line() {
        let report = trace().report();
        assert!(
            report.contains("reached past the isolate for nothing"),
            "{report}"
        );
        assert!(report.contains("esrun --deny-all app.mjs"), "{report}");
    }

    #[test]
    fn a_granted_capability_becomes_its_allow_flag() {
        let trace = trace();
        trace.observed("fs_read", Capability::FileRead, true);
        trace.observed("fetch", Capability::Net, true);
        let report = trace.report();
        assert!(
            report.contains("esrun --deny-all --allow-read --allow-net app.mjs"),
            "{report}"
        );
    }

    #[test]
    fn the_flags_are_ordered_the_same_way_every_run() {
        let trace = trace();
        // Observed in the reverse of the order they are reported in.
        trace.observed("http_serve", Capability::NetListen, true);
        trace.observed("fetch", Capability::Net, true);
        trace.observed("import", Capability::FileSystem, true);
        trace.observed("fs_read", Capability::FileRead, true);
        let report = trace.report();
        assert!(
            report.contains("--allow-read --allow-imports --allow-net --allow-listen"),
            "{report}"
        );
    }

    #[test]
    fn a_denied_capability_is_named_but_not_granted() {
        let trace = trace();
        trace.observed("fs_read", Capability::FileRead, true);
        trace.observed("system_spawn", Capability::Run, false);
        let report = trace.report();
        assert!(report.contains("run       system_spawn"), "{report}");
        assert!(report.contains("asked and was refused"), "{report}");
        // The deploy line grants what worked, never what was refused: whether a
        // refusal was correct is not a trace's call.
        assert!(
            report.contains("esrun --deny-all --allow-read app.mjs"),
            "{report}"
        );
    }

    #[test]
    fn an_op_firing_in_a_loop_is_one_fact() {
        let trace = trace();
        for _ in 0..1000 {
            trace.observed("fs_read", Capability::FileRead, true);
        }
        trace.observed("fs_stat", Capability::FileRead, true);
        let report = trace.report();
        assert!(report.contains("read      fs_read, fs_stat"), "{report}");
    }

    #[test]
    fn capabilities_no_flag_can_revoke_are_left_out() {
        let trace = trace();
        // Nothing revokes the clock, so a report about flags must not imply one
        // does.
        trace.observed("now", Capability::Clock, true);
        let report = trace.report();
        assert!(report.contains("nothing at all"), "{report}");
    }

    #[test]
    fn the_report_is_printed_once_however_the_run_ended() {
        let trace = trace();
        trace.run_finished();
        trace.run_finished();
        assert!(
            trace.reported.load(Ordering::SeqCst),
            "the first report must latch"
        );
    }
}
