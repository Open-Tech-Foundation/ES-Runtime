//! Linking: from an entry's exports to one declaration file.
//!
//! Four passes, in this order, each depending on the last:
//!
//! 1. **Public API.** What the entry exports, by name, followed through
//!    re-exports and `export *` to the declaration that actually defines it.
//! 2. **Reachability.** Everything those declarations name, transitively. A
//!    private helper type is *included* — a public type that mentions it is
//!    meaningless without it — but is not exported.
//! 3. **Naming.** Two modules may both declare `Options`. One keeps the name;
//!    the rest are suffixed, and every site that named them is rewritten.
//! 4. **Emission.** External imports, then declarations, then a single export
//!    block.
//!
//! The pass that is easy to get subtly wrong is the third, and the reason is
//! worth stating: a rename is only sound if *every* site was found. That is why
//! the sites come from semantic analysis rather than a syntax walk
//! ([`super::analyze`]), and why a rename never happens at all unless two
//! declarations genuinely collide.

use std::collections::{HashMap, HashSet};

use super::analyze::{Decl, ExportSource, Imported};
use super::{Graph, ModuleId, Resolution};

/// Where a name ultimately comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Target {
    /// A declaration in a module of this library.
    Local { module: ModuleId, name: String },
    /// A name from a package, which stays an import.
    External {
        specifier: String,
        imported: Imported,
    },
}

/// A symbol that will appear in the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Symbol {
    module: ModuleId,
    name: String,
}

/// Links the graph rooted at `entry` into one declaration file.
pub fn link(graph: &Graph, entry: ModuleId) -> Result<String, String> {
    let public = public_api(graph, entry)?;
    let (order, externals) = reachable(graph, &public);
    let names = assign_names(graph, &order, &externals);
    Ok(emit(graph, entry, &public, &order, &externals, &names))
}

/// The entry's exports, in a stable order, each resolved to its origin.
fn public_api(graph: &Graph, entry: ModuleId) -> Result<Vec<(String, Target)>, String> {
    let mut api = Vec::new();
    for name in exported_names(graph, entry, &mut HashSet::new()) {
        let Some(target) = resolve_export(graph, entry, &name, &mut HashSet::new()) else {
            // An ambiguous `export *` is *excluded* by TypeScript rather than
            // being an error, and following that is better than inventing a
            // stricter rule: the same package built by tsc would not export it
            // either.
            continue;
        };
        api.push((name, target));
    }
    if api.is_empty() {
        return Err(format!(
            "{} exports nothing, so there is no declaration file to build from it.",
            graph.module(entry).label
        ));
    }
    Ok(api)
}

/// Every name a module exports, including through `export *`.
fn exported_names(graph: &Graph, module: ModuleId, seen: &mut HashSet<ModuleId>) -> Vec<String> {
    if !seen.insert(module) {
        return Vec::new();
    }
    let analysis = &graph.module(module).analysis;
    let mut names: Vec<String> = Vec::new();
    for export in &analysis.exports {
        if !names.contains(&export.exported) {
            names.push(export.exported.clone());
        }
    }
    for specifier in &analysis.star_exports {
        // An external `export *` contributes names nobody here can enumerate;
        // it is re-emitted verbatim instead (see `emit`).
        if let Some(Resolution::Internal(target)) = graph.resolution(module, specifier) {
            for name in exported_names(graph, target, seen) {
                // `default` is never re-exported by `export *`, in TypeScript as
                // in JavaScript.
                if name != "default" && !names.contains(&name) {
                    names.push(name);
                }
            }
        }
    }
    names
}

/// What a module's export of `name` resolves to, following re-exports.
fn resolve_export(
    graph: &Graph,
    module: ModuleId,
    name: &str,
    seen: &mut HashSet<(ModuleId, String)>,
) -> Option<Target> {
    if !seen.insert((module, name.to_string())) {
        return None;
    }
    let analysis = &graph.module(module).analysis;
    // An explicit export wins over any `export *`, which is TypeScript's rule
    // and the reason the two are checked in this order rather than together.
    for export in &analysis.exports {
        if export.exported != name {
            continue;
        }
        return match &export.from {
            ExportSource::Local(local) => resolve_binding(graph, module, local, seen),
            ExportSource::Reexport {
                specifier,
                imported,
            } => match graph.resolution(module, specifier) {
                Some(Resolution::Internal(target)) => resolve_export(graph, target, imported, seen),
                _ => Some(Target::External {
                    specifier: specifier.clone(),
                    imported: Imported::Named(imported.clone()),
                }),
            },
        };
    }
    // Then the star exports. Two of them offering the same name is an ambiguity
    // TypeScript resolves by excluding it — so a second, *different* answer
    // means no answer, rather than the first one silently winning.
    let mut found: Option<Target> = None;
    for specifier in &analysis.star_exports {
        let Some(Resolution::Internal(target)) = graph.resolution(module, specifier) else {
            continue;
        };
        if let Some(resolved) = resolve_export(graph, target, name, seen) {
            match &found {
                None => found = Some(resolved),
                Some(existing) if *existing == resolved => {}
                Some(_) => return None,
            }
        }
    }
    found
}

/// What a top-level name in a module refers to, following imports.
fn resolve_binding(
    graph: &Graph,
    module: ModuleId,
    name: &str,
    seen: &mut HashSet<(ModuleId, String)>,
) -> Option<Target> {
    let analysis = &graph.module(module).analysis;
    if let Some(import) = analysis.imports.iter().find(|i| i.local == name) {
        return match graph.resolution(module, &import.specifier) {
            Some(Resolution::Internal(target)) => {
                let wanted = match &import.imported {
                    Imported::Named(n) => n.clone(),
                    Imported::Default => "default".to_string(),
                };
                resolve_export(graph, target, &wanted, seen)
            }
            _ => Some(Target::External {
                specifier: import.specifier.clone(),
                imported: import.imported.clone(),
            }),
        };
    }
    // Declared here — or not bound at all, which for a site means a global
    // (`Promise`, `Uint8Array`), and a global needs nothing done to it.
    analysis
        .decls
        .iter()
        .any(|d| d.names.iter().any(|n| n == name))
        .then(|| Target::Local {
            module,
            name: name.to_string(),
        })
}

/// Everything the public API needs, and the externals it leaves behind.
///
/// Declarations come back in **dependency order** — a module's symbols after
/// the symbols they name — which is not required by TypeScript (declarations
/// hoist) but makes the output readable top to bottom.
fn reachable(graph: &Graph, public: &[(String, Target)]) -> (Vec<Symbol>, Vec<Target>) {
    let mut queue: Vec<Symbol> = Vec::new();
    let mut externals: Vec<Target> = Vec::new();
    let mut seen: HashSet<Symbol> = HashSet::new();

    let mut enqueue =
        |target: &Target, queue: &mut Vec<Symbol>, externals: &mut Vec<Target>| match target {
            Target::Local { module, name } => {
                let symbol = Symbol {
                    module: *module,
                    name: name.clone(),
                };
                if seen.insert(symbol.clone()) {
                    queue.push(symbol);
                }
            }
            external => {
                if !externals.contains(external) {
                    externals.push(external.clone());
                }
            }
        };

    for (_, target) in public {
        enqueue(target, &mut queue, &mut externals);
    }

    let mut order: Vec<Symbol> = Vec::new();
    let mut index = 0;
    while index < queue.len() {
        let symbol = queue[index].clone();
        index += 1;
        for decl in decls_of(graph, &symbol) {
            for site in &decl.sites {
                // A site naming the declaration itself, or a type parameter, or
                // a global, resolves to nothing new.
                if let Some(target) =
                    resolve_binding(graph, symbol.module, &site.name, &mut HashSet::new())
                {
                    enqueue(&target, &mut queue, &mut externals);
                }
            }
        }
        order.push(symbol);
    }

    // Dependencies first: the graph's finish order for modules, and source
    // order within one.
    order.sort_by_key(|symbol| {
        let module_rank = graph
            .order
            .iter()
            .position(|id| *id == symbol.module)
            .unwrap_or(usize::MAX);
        let decl_rank = graph
            .module(symbol.module)
            .analysis
            .decls
            .iter()
            .position(|d| d.names.contains(&symbol.name))
            .unwrap_or(usize::MAX);
        (module_rank, decl_rank)
    });
    (order, externals)
}

/// The declarations binding a symbol's name. Usually one — TypeScript's
/// declaration merging makes several possible (`interface Foo` twice, or an
/// `interface` beside a `namespace` of the same name), and all of them have to
/// travel together or the merge is lost.
fn decls_of<'a>(graph: &'a Graph, symbol: &Symbol) -> Vec<&'a Decl> {
    graph
        .module(symbol.module)
        .analysis
        .decls
        .iter()
        .filter(|decl| decl.names.contains(&symbol.name))
        .collect()
}

/// The name each symbol takes in the bundle.
///
/// First come, first served: the earliest declaration keeps the name it was
/// written with, and a later collision is suffixed. `Options` and `Options$1`
/// rather than `Options$foo` and `Options$bar` — the same shape rollup produces,
/// and it does not encode a file path into a published type name.
fn assign_names(graph: &Graph, order: &[Symbol], externals: &[Target]) -> HashMap<Target, String> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut names: HashMap<Target, String> = HashMap::new();

    // Externals first: their names are pinned by the package they come from, so
    // an alias on an external import is a second name for the same thing, while
    // a rename of a local declaration is free.
    for external in externals {
        let Target::External {
            specifier,
            imported,
        } = external
        else {
            continue;
        };
        let wanted = match imported {
            Imported::Named(name) => name.clone(),
            Imported::Default => default_name(specifier),
        };
        let name = unique(&wanted, &mut taken);
        names.insert(external.clone(), name);
    }

    for symbol in order {
        let target = Target::Local {
            module: symbol.module,
            name: symbol.name.clone(),
        };
        let name = unique(&symbol.name, &mut taken);
        names.insert(target, name);
    }
    let _ = graph;
    names
}

/// A local name for a package's default export, derived from the specifier —
/// `some-pkg` becomes `some_pkg`, which is at least recognisable.
fn default_name(specifier: &str) -> String {
    let stem = specifier.rsplit('/').next().unwrap_or(specifier);
    let cleaned: String = stem
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect();
    if cleaned.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn unique(wanted: &str, taken: &mut HashSet<String>) -> String {
    if taken.insert(wanted.to_string()) {
        return wanted.to_string();
    }
    for suffix in 1.. {
        let candidate = format!("{wanted}${suffix}");
        if taken.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!("an unbounded search for a free name always ends")
}

/// Writes the bundle.
fn emit(
    graph: &Graph,
    entry: ModuleId,
    public: &[(String, Target)],
    order: &[Symbol],
    externals: &[Target],
    names: &HashMap<Target, String>,
) -> String {
    let mut out = String::new();

    // Imports, grouped by specifier so a package is named once.
    let mut specifiers: Vec<&String> = Vec::new();
    for external in externals {
        if let Target::External { specifier, .. } = external
            && !specifiers.contains(&specifier)
        {
            specifiers.push(specifier);
        }
    }
    for specifier in specifiers {
        let mut named: Vec<String> = Vec::new();
        let mut default: Option<String> = None;
        for external in externals {
            let Target::External {
                specifier: from,
                imported,
            } = external
            else {
                continue;
            };
            if from != specifier {
                continue;
            }
            let local = &names[external];
            match imported {
                Imported::Named(original) if original == local => named.push(original.clone()),
                Imported::Named(original) => named.push(format!("{original} as {local}")),
                Imported::Default => default = Some(local.clone()),
            }
        }
        let mut clause = String::new();
        if let Some(default) = default {
            clause.push_str(&default);
        }
        if !named.is_empty() {
            if !clause.is_empty() {
                clause.push_str(", ");
            }
            clause.push_str(&format!("{{ {} }}", named.join(", ")));
        }
        let keyword = if type_only(graph, specifier) {
            "import type"
        } else {
            "import"
        };
        out.push_str(&format!("{keyword} {clause} from \"{specifier}\";\n"));
    }
    if !out.is_empty() {
        out.push('\n');
    }

    // `export * from "react"` — nothing here can enumerate what it brings, so
    // it is passed through and the consumer's resolver does what it would have
    // done anyway.
    let mut passthrough: Vec<String> = Vec::new();
    for module in &graph.order {
        for specifier in &graph.module(*module).analysis.star_exports {
            if !matches!(
                graph.resolution(*module, specifier),
                Some(Resolution::Internal(_))
            ) && !passthrough.contains(specifier)
            {
                passthrough.push(specifier.clone());
            }
        }
    }
    for specifier in &passthrough {
        out.push_str(&format!("export * from \"{specifier}\";\n"));
    }
    if !passthrough.is_empty() {
        out.push('\n');
    }

    for symbol in order {
        for decl in decls_of(graph, symbol) {
            out.push_str(&rewrite(graph, symbol.module, decl, names));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }

    // One export block at the end, naming the public API and nothing else.
    let mut named: Vec<String> = Vec::new();
    let mut default: Option<String> = None;
    for (exported, target) in public {
        let Some(local) = names.get(target) else {
            continue;
        };
        if exported == "default" {
            default = Some(local.clone());
        } else if local == exported {
            named.push(local.clone());
        } else {
            named.push(format!("{local} as {exported}"));
        }
    }
    if !named.is_empty() {
        out.push_str(&format!("\nexport {{ {} }};\n", named.join(", ")));
    }
    if let Some(default) = default {
        out.push_str(&format!("export default {default};\n"));
    }
    let _ = entry;
    out
}

/// Whether every import of `specifier` anywhere in the library was an
/// `import type`.
///
/// Decided per package rather than per name because that is the granularity an
/// import statement has. Erring towards the plain `import` is the safe
/// direction: a value import of a type is legal in a declaration file, while a
/// type import of a value is not.
fn type_only(graph: &Graph, specifier: &str) -> bool {
    let mut seen = false;
    for module in &graph.order {
        for import in &graph.module(*module).analysis.imports {
            if import.specifier == specifier {
                seen = true;
                if !import.type_only {
                    return false;
                }
            }
        }
    }
    seen
}

/// One declaration's text, with every renamed site spliced.
///
/// Applied back to front so an earlier range is still valid after a later one
/// changed length — the reason the sites are ranges rather than offsets.
fn rewrite(
    graph: &Graph,
    module: ModuleId,
    decl: &Decl,
    names: &HashMap<Target, String>,
) -> String {
    let mut text = decl.text.clone();
    let mut sites = decl.sites.clone();
    sites.sort_by_key(|site| std::cmp::Reverse(site.range.start));
    for site in sites {
        let Some(target) = resolve_binding(graph, module, &site.name, &mut HashSet::new()) else {
            continue;
        };
        let Some(name) = names.get(&target) else {
            continue;
        };
        if *name != site.name {
            text.replace_range(site.range, name);
        }
    }
    text
}
