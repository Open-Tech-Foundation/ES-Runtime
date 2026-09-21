//! `esdev init` — start a bare project, or adopt an existing directory.
//!
//! `create` scaffolds a starter into an empty directory; `init` is for
//! everything else. A directory with nothing to adopt gets the bare minimal
//! setup — an internal template, never listed, just enough that `esdev start`
//! works on the first run. A directory with a project in it gets the one file
//! it is missing: a working `esdev.json`, plus its types installed. Either
//! way the transcript says what was written and what to run next.
//!
//! What it never does is `create`'s whole job: no template choice, no modes,
//! no demo blog — and like `create` it never overwrites. An `esdev.json`
//! already there is refused outright, and `--force` means "write among what
//! is there", never "write over it".
//!
//! Every question has a flag, so the interactive path stays a convenience over
//! the scriptable one: `--name`, `--language` and `--entry` answer what would
//! otherwise be asked, and `-y` takes every default without asking at all.

use std::path::{Path, PathBuf};

/// What `esdev init [dir]` was asked to do.
pub struct InitConfig {
    /// Where, `.` for here. Created when it does not exist.
    pub dir: String,
    /// `--name`: the new project's package name. New projects only.
    pub name: Option<String>,
    /// `--language=js|ts`: the new project's language. New projects only.
    pub language: Option<String>,
    /// `--entry`: the adopted file. Existing projects only.
    pub entry: Option<String>,
    /// `--install[=<manager>]` / `--no-install`: installing a new project's
    /// dependencies, exactly as `create` spells them.
    pub install: Option<Option<String>>,
    /// `--force`: write a new project among what is there. Never replaces.
    pub force: bool,
    /// `-y`: take every default; never ask, even on a terminal.
    pub yes: bool,
}

/// File extensions that make a directory a project worth adopting.
const SOURCE_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx", "html"];

/// Directories an adoption scan never descends into: machine-written, not
/// source.
const SKIP_DIRS: &[&str] = &["node_modules", ".git", "dist", "target", ".dev", ".cache"];

/// Entry candidates for an adopted project, in the order they become the
/// default. A document first: a directory holding one is a frontend project
/// before it is anything else.
const ENTRY_CANDIDATES: &[&str] = &[
    "index.html",
    "src/index.ts",
    "src/index.js",
    "src/index.mjs",
    "src/server.ts",
    "src/server.js",
    "src/main.ts",
    "src/main.js",
    "index.ts",
    "index.js",
    "server.ts",
    "server.js",
];

/// Starts a bare project in an empty directory, or adopts the project in a
/// directory that has one. Returns what to print: what was written, and what
/// to run next.
pub fn init(config: &InitConfig) -> Result<String, String> {
    let target = resolve_target(&config.dir)?;
    if target.join("esdev.json").is_file() {
        return Err(
            "esdev.json already exists here — edit it rather than rewriting it.\n\n\
             `esdev init` adopts or starts; it never replaces."
                .to_string(),
        );
    }
    if is_new(&target) {
        init_new(config, &target)
    } else {
        init_existing(config, &target)
    }
}

/// The directory, created when named and missing. A file by that name is a
/// refusal, not a project. `.` stays the working directory itself, so reports
/// name somewhere readable rather than a trailing `/.`.
fn resolve_target(dir: &str) -> Result<PathBuf, String> {
    if dir == "." || dir.is_empty() {
        return std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"));
    }
    let target = std::env::current_dir()
        .map_err(|e| format!("cannot read working directory: {e}"))?
        .join(dir);
    if target.is_file() {
        return Err(format!(
            "{dir} is a file.\n\n`esdev init` writes a project into a directory."
        ));
    }
    if !target.exists() {
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("cannot create {}: {e}", target.display()))?;
    }
    Ok(target)
}

/// Whether there is nothing here to adopt: no manifest, no source.
fn is_new(target: &Path) -> bool {
    if target.join("package.json").is_file() {
        return false;
    }
    !has_sources(target, 2)
}

/// Whether source files live under `dir`, a few levels down at most.
fn has_sources(dir: &Path, depth: u8) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in read.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if depth > 0 && !SKIP_DIRS.contains(&name.as_str()) && has_sources(&path, depth - 1) {
                return true;
            }
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| SOURCE_EXTENSIONS.contains(&e))
        {
            return true;
        }
    }
    false
}

/// Starts a bare project: the internal template, named and written.
#[allow(clippy::too_many_lines)]
fn init_new(config: &InitConfig, target: &Path) -> Result<String, String> {
    // The other flow's only flag has no meaning where there is nothing to
    // adopt: refusing names what to drop rather than silently ignoring it.
    if let Some(entry) = &config.entry {
        return Err(format!(
            "--entry={entry} names an adopted file, and a new project starts from src/index.\n\n\
             Drop it to start bare."
        ));
    }
    if !is_empty(target) && !config.force {
        return Err(format!(
            "{} is not empty.\n\n\
             `esdev init` starts a bare project here; pass --force to write among what is \
             there (it still never replaces a file), or adopt what is there by removing the \
             argument.",
            target.display()
        ));
    }
    let interactive = !config.yes && crate::prompt::interactive();
    let fallback_name = crate::create::package_name(target);
    let name = match &config.name {
        // Sanitised like a directory-derived name: this lands in
        // `package.json`, and npm's rules are narrower than a terminal's.
        Some(name) => crate::create::package_name(Path::new(name)),
        None if interactive => match crate::prompt::ask_text("Project name", &fallback_name) {
            Some(answer) => crate::create::package_name(Path::new(&answer)),
            // End of input is not a decision, but a default is: take it and
            // carry on, since nothing has been written yet either way.
            None => fallback_name,
        },
        None => fallback_name,
    };
    let language = match config.language.as_deref() {
        Some(language) if language == "js" || language == "ts" => language.to_string(),
        Some(language) => {
            return Err(format!(
                "--language={language} is not a language this writes — js or ts."
            ));
        }
        None if interactive => {
            let choices = [
                crate::prompt::Choice {
                    name: "js",
                    label: "JavaScript",
                    description: "",
                },
                crate::prompt::Choice {
                    name: "ts",
                    label: "TypeScript",
                    description: "",
                },
            ];
            match crate::prompt::select("Language?", &choices, None, crate::prompt::OnEsc::Cancel) {
                // Explicitly asked, so there is no default to fall back on:
                // a cancel cancels, having written nothing.
                Some(0) => "js".to_string(),
                Some(_) => "ts".to_string(),
                None => return Ok(String::new()),
            }
        }
        // Scripted runs take JavaScript: no compiler to install, nothing to
        // configure beyond the files themselves.
        None => "js".to_string(),
    };
    let install = match &config.install {
        Some(Some(name)) => Some(crate::install::by_name(name).ok_or_else(|| {
            format!(
                "there is no {name} package manager.\n\n\
                 Install with one of: {}.",
                crate::install::MANAGERS
                    .iter()
                    .map(|m| m.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })?),
        Some(None) => None,
        None if interactive => crate::create::ask_install(),
        None => None,
    };

    let files = bare(&language);
    let mut written = 0;
    for (path, contents) in &files {
        let destination = target.join(path);
        // Never replaces, even under `--force`: it means "write among".
        if destination.exists() {
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        write(&destination, contents, &name)?;
        written += 1;
    }
    let mut report = format!(
        "initialized {} as a bare {language} project ({written} files)\n",
        target.display()
    );

    // Only now, with the project on disk.
    if let Some(manager) = install {
        crate::install::run(manager, target)?;
    }
    let paint = crate::style::Palette::stdout();
    report.push_str(&next_steps(
        config,
        target,
        install.is_some(),
        "npm run dev",
        &paint,
    ));
    Ok(report)
}

/// The bare template for `language`: the shared files plus its overlay,
/// overlay winning. Paths are project-relative, with `_gitignore` already
/// renamed.
fn bare(language: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (path, contents) in crate::create::BARE_FILES {
        let path = *path;
        let (overlay, rest) = match path.split_once('/') {
            Some((overlay, rest)) => (overlay, rest),
            None => continue,
        };
        if overlay != "shared" && overlay != language {
            continue;
        }
        let path = if rest == "_gitignore" {
            ".gitignore".to_string()
        } else {
            rest.to_string()
        };
        // First wins, and shared comes first: an overlay replaces a shared
        // file rather than doubling it.
        if !seen.contains(&path) {
            seen.push(path.clone());
            files.push((path, contents.to_vec()));
        }
    }
    files.sort();
    files
}

/// Writes one file, substituting the project's name into it.
///
/// Text only, like `create`: a binary would be corrupted by a search and
/// replace, so anything that is not UTF-8 is written as embedded.
fn write(destination: &Path, contents: &[u8], name: &str) -> Result<(), String> {
    match std::str::from_utf8(contents) {
        Ok(text) if text.contains(crate::create::PLACEHOLDER) => {
            std::fs::write(destination, text.replace(crate::create::PLACEHOLDER, name))
        }
        _ => std::fs::write(destination, contents),
    }
    .map_err(|e| format!("cannot write {}: {e}", destination.display()))
}

/// Adopts the project in `target`: the one file it is missing.
fn init_existing(config: &InitConfig, target: &Path) -> Result<String, String> {
    // The new project's flags have no meaning where there is nothing to
    // start: refusing names what to drop rather than silently ignoring it.
    if config.name.is_some() {
        return Err(
            "--name names a new project's package, and this project has one — drop it.".to_string(),
        );
    }
    if config.language.is_some() {
        return Err(
            "--language chooses a new project's language, and this project's files already say — drop it."
                .to_string(),
        );
    }
    if config.install.is_some() {
        return Err(
            "--install decides a new project's install, and there is nothing to install here — drop it."
                .to_string(),
        );
    }
    if config.force {
        return Err(
            "--force writes a new project among what is there, and there is nothing to force past here — drop it."
                .to_string(),
        );
    }
    let interactive = !config.yes && crate::prompt::interactive();
    let entry = match &config.entry {
        Some(entry) => {
            if !target.join(entry).is_file() {
                return Err(format!(
                    "there is no {entry} here.\n\n\
                     Name the file to adopt with --entry, or run with nothing to adopt from scratch."
                ));
            }
            entry.clone()
        }
        None => match detect_entry(target) {
            Some(found) if interactive => match crate::prompt::ask_text("Entry", &found) {
                Some(answer) if !answer.trim().is_empty() => {
                    if !target.join(&answer).is_file() {
                        return Err(format!("there is no {answer} here."));
                    }
                    answer
                }
                _ => found,
            },
            Some(found) => found,
            None if interactive => match crate::prompt::ask_text("Entry (e.g. src/index.ts)", "") {
                Some(answer) if !answer.trim().is_empty() => {
                    if !target.join(&answer).is_file() {
                        return Err(format!("there is no {answer} here."));
                    }
                    answer
                }
                _ => {
                    return Err("no entry found — name the file to adopt with --entry.".to_string());
                }
            },
            None => {
                return Err("no entry found — name the file to adopt with --entry.".to_string());
            }
        },
    };
    let manifest = manifest_for(&entry);
    std::fs::write(target.join("esdev.json"), manifest)
        .map_err(|e| format!("cannot write {}: {e}", target.join("esdev.json").display()))?;
    let kind = if crate::config::is_html_entry(&entry) {
        "web"
    } else {
        "server"
    };
    let mut report = format!(
        "adopted {}: wrote esdev.json ({kind} target from {entry})\n",
        target.display()
    );

    // Best effort, and reported as such: without a network (or without a
    // manifest at all) there is nothing to install from, and the file above
    // is still the adoption. The standalone command stays the explicit tool.
    // Captured rather than inherited, so the transcript reads in the order
    // things happened: what was written, then what installing the types said.
    match install_types(target) {
        Ok(said) if !said.trim().is_empty() => {
            report.push_str(said.trim());
            report.push('\n');
        }
        Ok(_) => {
            report.push_str("  types installed\n");
        }
        Err(()) => {
            report.push_str(
                "  types not installed — run `esdev --install-types` after installing dependencies\n",
            );
        }
    }
    let paint = crate::style::Palette::stdout();
    // No install step: adopting installs nothing, and the types line above
    // already said how that half went.
    report.push_str(&next_steps(config, target, true, "esdev start", &paint));
    Ok(report)
}

/// The first entry candidate present, if any.
fn detect_entry(target: &Path) -> Option<String> {
    ENTRY_CANDIDATES
        .iter()
        .find(|candidate| target.join(candidate).is_file())
        .map(ToString::to_string)
}

/// An `esdev.json` for one entry: a served document, or a run server.
fn manifest_for(entry: &str) -> String {
    if crate::config::is_html_entry(entry) {
        return format!(
            "{{\n  \"targets\": {{\n    \"web\": {{\n      \"entry\": \"{entry}\",\n      \"outdir\": \"dist\"\n    }}\n  }}\n}}\n"
        );
    }
    let stem = Path::new(entry)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("index");
    format!(
        "{{\n  \"targets\": {{\n    \"server\": {{\n      \"entry\": \"{entry}\",\n      \"out\": \"dist/{stem}.js\"\n    }}\n  }},\n  \"start\": {{\n    \"run\": \"server\"\n  }}\n}}\n"
    )
}

/// Installs the project's types by re-executing this binary in `target`:
/// `esdev --install-types` reads the manifest there rather than this one's.
/// Returns what it said on success, so the transcript reads in order.
fn install_types(target: &Path) -> Result<String, ()> {
    let exe = std::env::current_exe().map_err(|_| ())?;
    let out = std::process::Command::new(exe)
        .arg("--install-types")
        .current_dir(target)
        .output()
        .map_err(|_| ())?;
    if !out.status.success() {
        return Err(());
    }
    let mut report = String::from_utf8_lossy(&out.stdout).into_owned();
    let errors = String::from_utf8_lossy(&out.stderr);
    if !errors.trim().is_empty() {
        report.push_str(&errors);
    }
    Ok(report)
}

/// Whether `dir` holds nothing at all. An unreadable directory counts as
/// empty: the write that follows fails with where, if so.
fn is_empty(dir: &Path) -> bool {
    match std::fs::read_dir(dir) {
        Ok(mut read) => read.next().is_none(),
        Err(_) => true,
    }
}

/// What to run next, printed after the report.
///
/// Bold, because these are lines to type rather than lines to read — the same
/// reason `create` bolds its own. `git init` is information, never an action:
/// it is only there when the directory is not a repository yet.
fn next_steps(
    config: &InitConfig,
    target: &Path,
    installed: bool,
    dev: &str,
    paint: &crate::style::Palette,
) -> String {
    let mut steps = String::new();
    if config.dir != "." {
        steps.push_str(&format!(
            "\n  {}",
            paint.bold(format_args!("cd {}", config.dir))
        ));
    }
    if !installed {
        steps.push_str(&format!("\n  {}", paint.bold(format_args!("npm install"))));
    }
    steps.push_str(&format!("\n  {}", paint.bold(format_args!("{dev}"))));
    if !target.join(".git").exists() {
        steps.push_str(&format!(
            "\n  {}",
            paint.bold(format_args!("git init  (to put it in version control)"))
        ));
    }
    steps.push('\n');
    steps
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory holding a fixture project.
    fn root(name: &str, files: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("esdev-init-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the fixture");
        for file in files {
            let path = dir.join(file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("create parents");
            }
            std::fs::write(&path, "").expect("write the fixture");
        }
        dir
    }

    /// A manifest or source makes a project; anything else is a blank slate —
    /// and machine-written directories never count as source.
    #[test]
    fn new_means_nothing_to_adopt() {
        for (name, files, expected) in [
            ("empty", &[][..], true),
            ("readme", &["README.md"], true),
            ("manifest", &["package.json"], false),
            ("server", &["src/server.ts"], false),
            ("page", &["index.html"], false),
            ("built", &["dist/index.js"], true),
            ("deps", &["node_modules/pkg/index.js"], true),
        ] {
            let dir = root(name, files);
            assert_eq!(is_new(&dir), expected, "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// A document comes before any module: a directory holding one is a
    /// frontend project first.
    #[test]
    fn a_document_is_the_default_entry() {
        let dir = root("entries", &["index.html", "src/index.ts", "src/server.js"]);
        assert_eq!(detect_entry(&dir).as_deref(), Some("index.html"));
        let _ = std::fs::remove_dir_all(&dir);

        let dir = root("modules", &["src/server.js", "src/index.ts"]);
        assert_eq!(detect_entry(&dir).as_deref(), Some("src/index.ts"));
        let _ = std::fs::remove_dir_all(&dir);

        let dir = root("none", &["README.md"]);
        assert_eq!(detect_entry(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A document is served, a module is run: the manifest says which.
    #[test]
    fn the_manifest_matches_the_entry_kind() {
        let web = manifest_for("index.html");
        assert!(web.contains("\"entry\": \"index.html\""), "{web}");
        assert!(web.contains("\"outdir\": \"dist\""), "{web}");
        assert!(!web.contains("run"), "{web}");

        let server = manifest_for("src/server.ts");
        assert!(server.contains("\"entry\": \"src/server.ts\""), "{server}");
        assert!(server.contains("\"out\": \"dist/server.js\""), "{server}");
        assert!(server.contains("\"run\": \"server\""), "{server}");

        // Parses as the config it will be read as.
        for manifest in [web, server] {
            let project = crate::config::parse(&manifest, PathBuf::from("/p"), "esdev.json")
                .expect("parsed")
                .expect("a config");
            assert_eq!(project.targets.len(), 1);
        }
    }

    /// The bare template is shared files plus one overlay, overlay winning —
    /// and `_gitignore` arrives renamed.
    #[test]
    fn bare_is_shared_plus_one_overlay() {
        for language in ["js", "ts"] {
            let files = bare(language);
            let names: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
            assert!(names.contains(&".gitignore"), "{names:?}");
            assert!(names.contains(&"package.json"), "{names:?}");
            assert!(names.contains(&"esdev.json"), "{names:?}");
            let other = if language == "js" { "ts" } else { "js" };
            assert!(
                !names.iter().any(|name| name.starts_with(other)),
                "{names:?}"
            );
        }
        let js = bare("js");
        assert!(js.iter().any(|(path, _)| path == "src/index.js"));
        assert!(!js.iter().any(|(path, _)| path == "tsconfig.json"));
        let ts = bare("ts");
        assert!(ts.iter().any(|(path, _)| path == "src/index.ts"));
        assert!(ts.iter().any(|(path, _)| path == "tsconfig.json"));
    }
}
