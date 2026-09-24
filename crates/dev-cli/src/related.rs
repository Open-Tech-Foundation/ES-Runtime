//! Which test files a change affects: `--changed` and `--related`
//! (DECISIONS D107).
//!
//! A test file is affected when it, or anything it imports — followed through
//! static imports, re-exports and `import()` of a literal, as the runtime
//! would resolve them — is one of the files. A few files change what every
//! test means (the project's config, its dependencies, a setup module), and a
//! change to one of them affects every test file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use es_runtime_cli_common::SourceResolver;
use oxc::allocator::Allocator;
use oxc::ast::ast::{Expression, Statement};
use oxc::ast_visit::Visit;
use oxc::parser::Parser;
use oxc::span::SourceType;

/// Files whose change affects every test: what configures the run, and what
/// decides which dependency is installed.
const EVERYTHING: &[&str] = &[
    "esdev.json",
    "package.json",
    "tsconfig.json",
    "jsconfig.json",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
];

/// Whether a file of this name decides what every test means. A tsconfig
/// another one `extends` — `tsconfig.base.json` — carries the `paths` an
/// import resolves through as surely as `tsconfig.json` does.
fn configures_every_test(name: &str) -> bool {
    EVERYTHING.contains(&name)
        || ((name.starts_with("tsconfig.") || name.starts_with("jsconfig."))
            && name.ends_with(".json"))
}

/// The files git reports changed: uncommitted ones, or — given `since`, a
/// commit or branch — everything that differs from where this branch left it,
/// uncommitted changes included. Untracked files count: a new test is a change.
pub fn changed_files(root: &Path, since: Option<&str>) -> Result<Vec<PathBuf>, String> {
    let git = |args: &[&str]| -> Result<String, String> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|err| format!("--changed reads git, and git could not be run: {err}"))?;
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(format!(
                "--changed reads git, and `git {}` failed: {why}",
                args.join(" ")
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let top = PathBuf::from(git(&["rev-parse", "--show-toplevel"])?.trim());
    let mut names = String::new();
    match since {
        Some(since) => names.push_str(&git(&["diff", "--name-only", "--merge-base", since])?),
        // Against HEAD: staged and unstaged at once. A repository with no
        // commit yet has no HEAD; everything in it is staged or untracked.
        None => match git(&["diff", "--name-only", "HEAD"]) {
            Ok(text) => names.push_str(&text),
            Err(_) => names.push_str(&git(&["diff", "--name-only", "--cached"])?),
        },
    }
    names.push_str(&git(&[
        "ls-files",
        "--others",
        "--exclude-standard",
        "--full-name",
    ])?);
    let mut files: Vec<PathBuf> = names
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| canonical(&top.join(line)))
        .collect();
    files.sort();
    files.dedup();
    Ok(files)
}

/// `path`, with symlinks resolved when it exists — the form resolved imports
/// come back in — and as given when it was deleted.
pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Which of `tests` the `changed` files affect. `setup` are the modules every
/// test file runs with, whose own imports affect every file too.
pub fn affected(
    root: &Path,
    source: &crate::settings::Source,
    tests: &[PathBuf],
    setup: &[PathBuf],
    changed: &[PathBuf],
) -> Result<Vec<PathBuf>, String> {
    let changed: HashSet<PathBuf> = changed.iter().map(|path| canonical(path)).collect();
    if changed.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(configures_every_test)
    }) {
        return Ok(tests.to_vec());
    }
    // A deleted file cannot be resolved to: a test that imports something that
    // no longer resolves is affected when a file was deleted.
    let deleted = changed.iter().any(|path| !path.exists());
    let mut graph = Graph::new(root, source)?;
    if setup
        .iter()
        .any(|module| graph.reaches(&canonical(module), &changed, deleted))
    {
        return Ok(tests.to_vec());
    }
    Ok(tests
        .iter()
        .filter(|test| graph.reaches(&canonical(test), &changed, deleted))
        .cloned()
        .collect())
}

/// The modules each module imports, read on demand and remembered.
struct Graph {
    resolver: SourceResolver,
    imports: HashMap<PathBuf, Imports>,
}

#[derive(Default, Clone)]
struct Imports {
    files: Vec<PathBuf>,
    /// Whether an import names something that did not resolve.
    unresolved: bool,
}

impl Graph {
    /// The graph as a run of the project resolves it: its aliases included,
    /// or a test that imports `@/db` would never be reached by a change to it.
    fn new(root: &Path, source: &crate::settings::Source) -> Result<Self, String> {
        Ok(Self {
            resolver: SourceResolver::new(root)?.with_alias(source.resolution().alias),
            imports: HashMap::new(),
        })
    }

    /// Whether `from`, or anything it imports, is one of `changed`.
    fn reaches(&mut self, from: &Path, changed: &HashSet<PathBuf>, deleted: bool) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from.to_path_buf()];
        while let Some(module) = stack.pop() {
            if !seen.insert(module.clone()) {
                continue;
            }
            if changed.contains(&module) {
                return true;
            }
            let imports = self.imports_of(&module);
            if deleted && imports.unresolved {
                return true;
            }
            stack.extend(imports.files);
        }
        false
    }

    fn imports_of(&mut self, module: &Path) -> Imports {
        if let Some(known) = self.imports.get(module) {
            return known.clone();
        }
        let found = self.read(module);
        self.imports.insert(module.to_path_buf(), found.clone());
        found
    }

    /// What `module` imports. A dependency in node_modules is where the walk
    /// stops: it changes by being reinstalled, which a lockfile says.
    fn read(&self, module: &Path) -> Imports {
        let mut imports = Imports::default();
        if module
            .components()
            .any(|part| part.as_os_str() == "node_modules")
        {
            return imports;
        }
        let Ok(source_type) = SourceType::from_path(module) else {
            return imports;
        };
        let Ok(text) = std::fs::read_to_string(module) else {
            return imports;
        };
        let Ok(referrer) = url::Url::from_file_path(module) else {
            return imports;
        };
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &text, source_type.with_module(true)).parse();
        let mut specifiers = Specifiers::default();
        specifiers.visit_program(&parsed.program);
        for specifier in specifiers.found {
            // What the runtime provides, rather than a file.
            if ["runtime:", "node:", "esdev:", "http:", "https:", "data:"]
                .iter()
                .any(|scheme| specifier.starts_with(scheme))
            {
                continue;
            }
            match self
                .resolver
                .resolve(&specifier, referrer.as_str())
                .and_then(|url| url::Url::parse(&url).ok())
                .and_then(|url| url.to_file_path().ok())
            {
                Some(path) => imports.files.push(canonical(&path)),
                None => imports.unresolved = true,
            }
        }
        imports
    }
}

/// The specifiers a module loads at run time: `import` and `export … from`
/// declarations that are not type-only, and `import()` of a string.
#[derive(Default)]
struct Specifiers {
    found: Vec<String>,
}

impl<'a> Visit<'a> for Specifiers {
    fn visit_statement(&mut self, statement: &Statement<'a>) {
        match statement {
            Statement::ImportDeclaration(import) if !import.import_kind.is_type() => {
                self.found.push(import.source.value.to_string());
            }
            Statement::ExportFromDeclaration(export) if !export.export_kind.is_type() => {
                self.found.push(export.source.value.to_string());
            }
            Statement::ExportAllDeclaration(export) if !export.export_kind.is_type() => {
                self.found.push(export.source.value.to_string());
            }
            _ => {}
        }
        oxc::ast_visit::walk::walk_statement(self, statement);
    }

    fn visit_expression(&mut self, expression: &Expression<'a>) {
        if let Expression::ImportExpression(import) = expression
            && let Expression::StringLiteral(literal) = &import.source
        {
            self.found.push(literal.value.to_string());
        }
        oxc::ast_visit::walk::walk_expression(self, expression);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Source;

    /// A project on disk: `a.test.ts → lib/a.ts → lib/shared.ts`,
    /// `b.test.ts → lib/b.ts`, `types.test.ts` importing only a type.
    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-related-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        let write = |file: &str, text: &str| std::fs::write(dir.join(file), text).unwrap();
        write("lib/shared.ts", "export const shared = 1;\n");
        write(
            "lib/a.ts",
            "export { shared } from \"./shared\";\nexport const a = 1;\n",
        );
        write(
            "lib/b.ts",
            "export const b = () => import(\"./lazy.ts\");\n",
        );
        write("lib/lazy.ts", "export default 1;\n");
        write("lib/typed.ts", "export type T = number;\n");
        write(
            "a.test.ts",
            "import { test } from \"runtime:test\";\nimport { a } from \"./lib/a\";\n",
        );
        write("b.test.ts", "import { b } from \"./lib/b.ts\";\n");
        write("types.test.ts", "import type { T } from \"./lib/typed\";\n");
        canonical(&dir)
    }

    fn names(root: &Path, files: &[PathBuf]) -> Vec<String> {
        files
            .iter()
            .map(|file| file.strip_prefix(root).unwrap().display().to_string())
            .collect()
    }

    fn tests(root: &Path) -> Vec<PathBuf> {
        ["a.test.ts", "b.test.ts", "types.test.ts"]
            .map(|file| root.join(file))
            .to_vec()
    }

    #[test]
    fn a_change_affects_every_test_that_reaches_it() {
        let root = project("reach");
        let hit = |changed: &str| {
            names(
                &root,
                &affected(
                    &root,
                    &Source::default(),
                    &tests(&root),
                    &[],
                    &[root.join(changed)],
                )
                .unwrap(),
            )
        };
        // Through a re-export, two levels down.
        assert_eq!(hit("lib/shared.ts"), ["a.test.ts"]);
        // Through `import()` of a literal.
        assert_eq!(hit("lib/lazy.ts"), ["b.test.ts"]);
        // The test file itself.
        assert_eq!(hit("b.test.ts"), ["b.test.ts"]);
        // A type-only import is not loaded, so it is no dependency.
        assert!(hit("lib/typed.ts").is_empty());
        // Nothing imports it.
        assert!(hit("README.md").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn config_lockfiles_and_setup_modules_affect_every_test() {
        let root = project("everything");
        for file in ["package.json", "esdev.json", "pnpm-lock.yaml"] {
            let hit = affected(
                &root,
                &Source::default(),
                &tests(&root),
                &[],
                &[root.join(file)],
            )
            .unwrap();
            assert_eq!(hit.len(), 3, "{file}");
        }
        let setup = [root.join("lib/a.ts")];
        let hit = affected(
            &root,
            &Source::default(),
            &tests(&root),
            &setup,
            &[root.join("lib/shared.ts")],
        )
        .unwrap();
        assert_eq!(hit.len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A test that imports a module through the project's alias is reached
    /// by a change to it — the graph resolves as the run does.
    #[test]
    fn a_change_reaches_a_test_through_an_alias() {
        let root = project("alias");
        std::fs::write(
            root.join("aliased.test.ts"),
            "import { shared } from \"@lib/shared\";\n",
        )
        .unwrap();
        let source = Source {
            root: root.clone(),
            alias: vec![("@lib".to_string(), root.join("lib").display().to_string())],
            ..Source::default()
        };
        let tests = [root.join("aliased.test.ts")];
        let hit = affected(&root, &source, &tests, &[], &[root.join("lib/shared.ts")]).unwrap();
        assert_eq!(names(&root, &hit), ["aliased.test.ts"]);
        let unaliased = affected(
            &root,
            &Source::default(),
            &tests,
            &[],
            &[root.join("lib/shared.ts")],
        )
        .unwrap();
        assert!(unaliased.is_empty(), "the alias is what connects them");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tsconfig_that_is_extended_affects_every_test() {
        assert!(configures_every_test("tsconfig.base.json"));
        assert!(configures_every_test("jsconfig.json"));
        assert!(!configures_every_test("tsconfig.ts"));
    }

    #[test]
    fn a_deleted_import_affects_the_test_that_still_imports_it() {
        let root = project("deleted");
        std::fs::remove_file(root.join("lib/shared.ts")).unwrap();
        let hit = affected(
            &root,
            &Source::default(),
            &tests(&root),
            &[],
            &[root.join("lib/shared.ts")],
        )
        .unwrap();
        assert_eq!(names(&root, &hit), ["a.test.ts"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
