//! The import policy applied to what a build ships (DECISIONS D193).
//!
//! The runtime loader applies a policy as it resolves each module (D39), and a
//! build inlines every package, so a deployed bundle never asks the loader for
//! one. The same policy file is applied here instead, to every module the
//! bundler put into an output: what the build *ships*, not what is installed
//! and not the build tools that ran — none of their code is in the output, so
//! there is nothing of theirs to approve.
//!
//! What is judged is the bundle's **module graph** — every module parsed into
//! it, recorded by [`Graph`] — not the list of modules an output chunk
//! reports. The bundler drops a module from that list when it folds the
//! module away entirely: a package exporting a constant is inlined into its
//! importer and no longer "in" a chunk, though its code decided what shipped.
//! Judging the rendered list let a denied package through that way.
//!
//! A module the bundler made up rather than read — a plugin's virtual module —
//! is not a file and is skipped: it is the plugin's output, and the plugin is
//! a build tool.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use es_runtime_default_providers::{ImportPolicy, Unlisted, Verdict, package_of};
use rolldown::plugin::{HookNoopReturn, HookUsage, Plugin, PluginContext};

/// The modules parsed into a bundle's graph, collected by a bundler plugin.
///
/// A held dev bundler re-parses only what changed, so its graph accumulates
/// across rebuilds: an import removed in the loop stays judged until the
/// bundler is rebuilt, which can over-report and never misses.
#[derive(Clone, Debug, Default)]
pub struct Graph(Arc<Mutex<BTreeSet<String>>>);

impl Graph {
    /// The bundler plugin that records into this graph.
    pub fn plugin(&self) -> Arc<dyn rolldown::plugin::Pluginable> {
        Arc::new(Recorder(self.clone()))
    }

    fn ids(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[derive(Debug)]
struct Recorder(Graph);

impl Plugin for Recorder {
    fn name(&self) -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("esdev:import-policy")
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ModuleParsed
    }

    async fn module_parsed(
        &self,
        _ctx: &PluginContext,
        module_info: Arc<rolldown_common::ModuleInfo>,
        _normal_module: &rolldown_common::NormalModule,
    ) -> HookNoopReturn {
        if let Ok(mut ids) = (self.0).0.lock() {
            ids.insert(crate::adapter::guest_id(module_info.id.as_ref()).to_string());
        }
        Ok(())
    }
}

/// The policy at `path`, applied to what `graph` read into a bundle. `Ok`
/// carries the warnings to print when the policy reports rather than refuses
/// (`"unlisted": "warn"`); a refusal names every module refused, at once.
pub fn check(path: &Path, graph: &Graph, root: &Path) -> Result<Vec<String>, String> {
    let shown = shown_path(path, root);
    let policy = ImportPolicy::from_file(path)?;
    let mut denied = BTreeSet::new();
    let mut unlisted = BTreeSet::new();
    for id in &graph.ids() {
        // A query (`?raw`, `?url`) names a way of loading a file, not a
        // different file.
        let file = Path::new(id.split('?').next().unwrap_or(id));
        if !file.is_absolute() {
            continue;
        }
        let Ok(real) = es_runtime_default_providers::path::canonicalize(file) else {
            continue;
        };
        let name = package_of(&real).unwrap_or_else(|| shown_path(&real, root));
        match policy.judge(&real) {
            Verdict::Allowed => {}
            Verdict::Denied => {
                denied.insert(name);
            }
            Verdict::Unlisted => {
                unlisted.insert(name);
            }
        }
    }
    let strict = policy.unlisted() == Unlisted::Error;
    if denied.is_empty() && (unlisted.is_empty() || !strict) {
        return Ok(unlisted
            .into_iter()
            .map(|name| {
                format!(
                    "{name} is not in the import policy's allow list ({shown}); \
                     it shipped because the policy's \"unlisted\" is \"warn\""
                )
            })
            .collect());
    }
    let mut message = format!("the import policy ({shown}) refuses what this build ships:");
    if !denied.is_empty() {
        message.push_str(&format!("\n  denied: {}", join(&denied)));
    }
    if strict && !unlisted.is_empty() {
        message.push_str(&format!("\n  not in the allow list: {}", join(&unlisted)));
    }
    message.push_str(
        "\n\nA denied package has to go. One that is not listed is approved by adding it \
         to \"allow\"; `npm explain <package>` (or `pnpm why`) shows what brought it in.",
    );
    Err(message)
}

fn join(names: &BTreeSet<String>) -> String {
    names.iter().cloned().collect::<Vec<_>>().join(", ")
}

/// `path` as the reader of this project would write it: `./`-relative to the
/// root where it is inside it.
fn shown_path(path: &Path, root: &Path) -> String {
    let root = es_runtime_default_providers::path::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf());
    match path.strip_prefix(&root) {
        Ok(relative) => format!("./{}", relative.to_string_lossy().replace('\\', "/")),
        Err(_) => path.display().to_string(),
    }
}
