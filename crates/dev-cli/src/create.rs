//! `esdev create` — a project that already works.
//!
//! Everything the last four increments built is only reachable if somebody can
//! get to a working project without assembling one: an `esdev.json` with the
//! right targets, an `index.html` whose script tag names the entry, a server
//! that reads its template from beside itself, and a permission line that is
//! narrow from the first run rather than widened to `--allow-all` on the way to
//! a demo. None of that is hard to write and all of it is tedious to write
//! correctly, which is exactly what a scaffolder is for.
//!
//! # It asks, or it writes files and stops
//!
//! Which of those depends on whether anybody is there. On a terminal it asks
//! up to three questions — which template, which *mode* if that template has
//! more than one shape, and whether to install — and away from one it writes
//! the files and says nothing, because every other command here is a flag
//! grammar that works unattended and `create` stays one whenever it cannot see
//! a person ([`crate::prompt::interactive`]).
//!
//! **Everything a prompt asks has a flag**, so the interactive path is a
//! convenience over the scriptable one and never the only way to an answer.
//! `--template=api --install=bun` is the same run with nothing to type.
//!
//! # A template can have modes
//!
//! Some stacks are two projects wearing one name. `react` is: an app with a
//! server of its own is not the same project as a site that deploys to a static
//! host — different files, different `esdev.json`, a different set of
//! capabilities, and in one case none at all. Scaffolding the union of them and
//! leaving the user to delete half is how a starter ends up shipping a server
//! nobody runs and a permission nobody needs.
//!
//! So a template directory may hold `_mode/<name>/`, and what gets written is
//! everything outside `_mode/` plus one mode's files with that prefix stripped.
//! A mode may add files the shared part does not have (`src/server.tsx`), and
//! may replace one it does (`package.json`, `esdev.json`, `README.md`) — the
//! overlay wins, because that is what makes a mode able to say something
//! different about the same project.
//!
//! D64 refused to install at all, and the reason it gave was exact: there is no
//! lockfile yet to say which package manager this project uses, and guessing
//! wrong leaves a `package-lock.json` in a bun project. That is an argument
//! against **guessing**, and it still holds — a non-interactive run installs
//! nothing. Asking resolves the objection at its root, by getting the answer
//! from the person who knows it. See [`crate::install`].
//!
//! # It never overwrites
//!
//! `esdev build --lib` empties its output directory because the build owns it
//! (D59). This owns nothing: it writes into a directory the user named, which
//! may be their home directory or a project they have been working in for a
//! year. So a non-empty target is refused unless `--force` says otherwise, and
//! even then an existing file is left alone rather than replaced — `--force`
//! means "write among what is there", never "write over it".

use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

/// The one-line descriptions `--list` prints.
///
/// Beside the templates rather than inside them: a description is for somebody
/// choosing, and what they are choosing between is only visible from here.
const DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "api",
        "A JSON API — routing, validation, error handling. No dependencies",
    ),
    (
        "react",
        "React + react-router — a static site, or an app with a server of its own",
    ),
    (
        "lib",
        "A publishable TypeScript package — module tree, .d.ts, no dependencies",
    ),
    (
        "vanilla",
        "TypeScript and the DOM — no framework, no dependencies",
    ),
];

/// Where a template keeps the files that belong to one mode and not the others.
///
/// A directory rather than a naming convention on each file, so a mode is
/// something you can read by listing one directory — and so adding a file to a
/// mode is putting it where the others are rather than remembering a suffix.
const MODE_PREFIX: &str = "_mode/";

/// The templates that come in more than one shape, and what the shapes are.
///
/// The first mode listed is the default: what `--mode` unsaid resolves to away
/// from a terminal, and what the menu starts on when there is one. `static` is
/// first for `react` deliberately — it is the one with nothing to deploy but
/// files, so it is the smaller thing to be handed when nobody expressed a
/// preference.
const MODES: &[(&str, &[(&str, &str)])] = &[(
    "react",
    &[
        (
            "static",
            "No server — prerendered HTML or a single-page app, on any static host",
        ),
        (
            "fullstack",
            "A server of its own — rendered per request, under named capabilities",
        ),
    ],
)];

/// The modes a template has, or `None` when it has one shape.
fn modes(template: &str) -> Option<&'static [(&'static str, &'static str)]> {
    MODES
        .iter()
        .find(|(name, _)| *name == template)
        .map(|(_, modes)| *modes)
}

/// The mode taken when nothing said and nobody was asked.
fn default_mode(template: &str) -> Option<&'static str> {
    modes(template)
        .and_then(|modes| modes.first())
        .map(|(name, _)| *name)
}

/// The files one mode of a template is written from.
///
/// Everything outside `_mode/`, plus the chosen mode's files with the prefix
/// stripped. The overlay is applied second and wins, so a mode can replace a
/// shared file as well as add one.
fn files_for<'a>(files: &'a [TemplateFile], mode: Option<&str>) -> Vec<(String, &'a [u8])> {
    let overlay = mode.map(|mode| format!("{MODE_PREFIX}{mode}/"));
    let mut written: Vec<(String, &[u8])> = Vec::new();

    for (path, contents) in files {
        let path = match path.strip_prefix(MODE_PREFIX) {
            // A mode's file, for whichever mode. It is written only if it is
            // this one's, and then under the path it has inside the mode.
            Some(_) => match overlay.as_ref().and_then(|p| path.strip_prefix(p.as_str())) {
                Some(within) => within.to_string(),
                None => continue,
            },
            None => (*path).to_string(),
        };
        match written.iter_mut().find(|(existing, _)| *existing == path) {
            Some(entry) => entry.1 = contents,
            None => written.push((path, contents)),
        }
    }

    written.sort_by(|a, b| a.0.cmp(&b.0));
    written
}

/// A file whose name in the template is not the name it is written under.
///
/// `.gitignore` is the whole list, and it is not cosmetic: a `.gitignore` in
/// the template directory would be applied *to the template*, so this
/// repository would stop tracking the very file it means to ship. npm's
/// packaging has the same problem and the same fix, which is why the convention
/// is one somebody scaffolding will already have seen.
const RENAMED: &[(&str, &str)] = &[("_gitignore", ".gitignore")];

/// What `esdev create` was asked to do.
pub struct CreateConfig {
    /// The directory to write into.
    pub dir: String,
    /// Which template, or `None` to ask (or take the default).
    pub template: Option<String>,
    /// Which mode of that template, or `None` to ask (or take the default).
    /// Meaningless — and refused — for a template that has only one shape.
    pub mode: Option<String>,
    /// Whether to write into a directory that already holds something.
    pub force: bool,
    /// Which package manager to install with, `Some(None)` for an explicit
    /// "do not install", and `None` to ask (or, unattended, not to).
    pub install: Option<Option<String>>,
}

/// The default template, when `--template` did not say and nobody was asked.
pub const DEFAULT_TEMPLATE: &str = "react";

/// What a bare `--install` means.
///
/// npm, because it is what a Node installation already has — the answer that
/// needs the least explaining when somebody did not name one.
pub const DEFAULT_MANAGER: &str = "npm";

/// Scaffolds a project and reports what to do next.
pub fn create(config: &CreateConfig) -> Result<String, String> {
    // Asked before anything is written, so a person who changes their mind at
    // the prompt leaves no directory behind.
    let template = match &config.template {
        Some(named) => named.clone(),
        // Esc at the first question, before anything has been written. Nothing
        // to undo, nothing to report, and an exit status of zero: a person who
        // changed their mind did not hit an error.
        None if crate::prompt::interactive() => match ask_template() {
            Some(template) => template,
            None => return Ok(String::new()),
        },
        None => DEFAULT_TEMPLATE.to_string(),
    };

    let files = TEMPLATES
        .iter()
        .find(|(name, _)| *name == template)
        .map(|(_, files)| *files)
        .ok_or_else(|| format!("there is no {template} template.\n\n{}", list().trim_end()))?;

    // After the template, because which modes exist depends on which template
    // it is — and still before anything is written, for the same reason.
    let mode = match resolve_mode(&template, config.mode.as_deref())? {
        Mode::Chosen(mode) => Some(mode),
        Mode::None => None,
        Mode::Cancelled => return Ok(String::new()),
    };
    let files = files_for(files, mode.as_deref());

    let target = PathBuf::from(&config.dir);
    if target.is_file() {
        return Err(format!(
            "{} is a file.\n\n`esdev create` writes a project into a directory.",
            config.dir
        ));
    }
    if !config.force
        && let Ok(mut existing) = std::fs::read_dir(&target)
        && existing.next().is_some()
    {
        return Err(format!(
            "{} is not empty.\n\n\
             `esdev create` will not write into a directory that already holds \
             something unless you say so: --force writes among what is there, and \
             still never replaces a file.",
            config.dir
        ));
    }

    let name = package_name(&target);
    let mut written = 0usize;
    let mut skipped = Vec::new();
    for (path, contents) in &files {
        let path = RENAMED
            .iter()
            .find(|(from, _)| from == path)
            .map_or(path.as_str(), |(_, to)| *to);
        let destination = target.join(path);
        if destination.exists() {
            skipped.push(path.to_string());
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        write(&destination, contents, &name)?;
        written += 1;
    }

    // Nothing written is not an error — `--force` on a directory that already
    // holds the whole project is a no-op somebody asked for — but reporting it
    // as "created" would be a lie about a command whose entire job is to write
    // files.
    // The mode is part of the template's name in every message, because
    // "the react template" is two different projects and a report that does not
    // say which one is a report that cannot be checked.
    let named = match &mode {
        Some(mode) => format!("{template} ({mode})"),
        None => template.clone(),
    };
    if written == 0 {
        return Ok(format!(
            "nothing to write: {} already holds every file the {named} template has.\n",
            config.dir
        ));
    }
    let mut report = format!(
        "created {} from the {named} template ({written} file{})\n",
        config.dir,
        if written == 1 { "" } else { "s" },
    );
    if !skipped.is_empty() {
        report.push_str(&format!(
            "left alone, because they were already there: {}\n",
            skipped.join(", ")
        ));
    }
    // Reported before the install rather than after it, so the transcript reads
    // in the order things happened: what was written, then what installing it
    // printed. Returning the whole report at the end would put the install's
    // own output above the line announcing the project it installed into.
    print!("{report}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    report.clear();

    // Only now, with the project on disk: an install that fails leaves a
    // project that is complete and one command away, rather than half of one.
    let installed = match &config.install {
        Some(Some(named)) => {
            let manager = crate::install::by_name(named).ok_or_else(|| {
                format!(
                    "there is no {named} package manager.\n\nKnown: {}.",
                    crate::install::MANAGERS
                        .iter()
                        .map(|m| m.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            crate::install::run(manager, &target)?;
            Some(manager)
        }
        Some(None) => None,
        None if crate::prompt::interactive() => match ask_install() {
            Some(manager) => {
                crate::install::run(manager, &target)?;
                Some(manager)
            }
            None => None,
        },
        None => None,
    };

    report.push_str(&next_steps(&config.dir, &template, installed));
    Ok(report)
}

/// The lines printed after the project is written.
fn next_steps(dir: &str, template: &str, installed: Option<crate::install::Manager>) -> String {
    // The command that actually starts it, which is not the same for every
    // template: a library has nothing to run.
    let run = match template {
        "lib" => "test",
        _ => "dev",
    };
    let manager = installed.map_or("npm", |m| m.name);

    let mut steps = format!("\n  cd {dir}\n");
    if installed.is_none() {
        steps.push_str(&format!("  {manager} install\n"));
    }
    steps.push_str(&format!("  {manager} run {run}\n"));
    steps
}

/// What resolving `--mode` came to.
enum Mode {
    /// This template has modes, and this is the one.
    Chosen(String),
    /// This template has one shape.
    None,
    /// Esc at the question.
    Cancelled,
}

/// The mode to write, from the flag, a question, or the default.
///
/// Naming a mode for a template that has none is refused rather than ignored: a
/// flag that silently does nothing is one somebody will keep passing, and keep
/// believing.
fn resolve_mode(template: &str, asked_for: Option<&str>) -> Result<Mode, String> {
    let Some(modes) = modes(template) else {
        return match asked_for {
            Some(mode) => Err(format!(
                "the {template} template has no modes, so --mode={mode} means nothing here.\n\n\
                 Modes exist where one template is really two projects; {}.",
                what_has_modes()
            )),
            None => Ok(Mode::None),
        };
    };

    if let Some(mode) = asked_for {
        if !modes.iter().any(|(name, _)| *name == mode) {
            return Err(format!(
                "the {template} template has no {mode} mode.\n\n{}",
                describe_modes(template).trim_end()
            ));
        }
        return Ok(Mode::Chosen(mode.to_string()));
    }

    if !crate::prompt::interactive() {
        return Ok(Mode::Chosen(
            default_mode(template)
                .expect("a template with modes has a default")
                .to_string(),
        ));
    }

    let choices: Vec<crate::prompt::Choice<'_>> = modes
        .iter()
        .map(|(name, description)| crate::prompt::Choice { name, description })
        .collect();
    match crate::prompt::select("Which mode?", &choices, 0) {
        Some(chosen) => Ok(Mode::Chosen(choices[chosen].name.to_string())),
        None => Ok(Mode::Cancelled),
    }
}

/// The templates that have modes, for an error message that has to name them.
fn what_has_modes() -> String {
    let names: Vec<&str> = MODES.iter().map(|(name, _)| *name).collect();
    match names.as_slice() {
        [] => "no template here has any".to_string(),
        [one] => format!("only {one} does"),
        many => format!("{} do", many.join(", ")),
    }
}

/// One template's modes, as a list somebody can choose from.
fn describe_modes(template: &str) -> String {
    let Some(modes) = modes(template) else {
        return String::new();
    };
    let width = modes.iter().map(|(name, _)| name.len()).max().unwrap_or(0);
    let mut report = format!("Modes of {template}:\n");
    for (name, description) in modes {
        report.push_str(&format!("  {name:<width$}  {description}\n"));
    }
    report
}

/// Which template, asked on a terminal.
fn ask_template() -> Option<String> {
    let choices: Vec<crate::prompt::Choice<'_>> = TEMPLATES
        .iter()
        .map(|(name, _)| crate::prompt::Choice {
            name,
            description: DESCRIPTIONS
                .iter()
                .find(|(template, _)| template == name)
                .map_or("", |(_, description)| description),
        })
        .collect();
    let default = choices
        .iter()
        .position(|choice| choice.name == DEFAULT_TEMPLATE)
        .unwrap_or(0);

    let chosen = crate::prompt::select("Which template?", &choices, default)?;
    Some(choices[chosen].name.to_string())
}

/// Whether to install, and with what.
///
/// Only what this machine actually has is offered: naming a package manager
/// that is not installed is offering an error message.
fn ask_install() -> Option<crate::install::Manager> {
    let available = crate::install::available();
    if available.is_empty() {
        return None;
    }

    let mut choices: Vec<crate::prompt::Choice<'_>> = available
        .iter()
        .map(|manager| crate::prompt::Choice {
            name: manager.name,
            description: "",
        })
        .collect();
    choices.push(crate::prompt::Choice {
        name: "skip",
        description: "write the files and stop",
    });

    // Esc lands on the same answer `skip` does: the project is already on disk
    // by now, and cancelling the *install* question is not cancelling the
    // project. Either way the next steps say how to install it.
    let chosen = crate::prompt::select("Install the dependencies?", &choices, 0)?;
    available.get(chosen).copied()
}

/// Writes one file, substituting the project's name into it.
///
/// The substitution is attempted only on text. A template is free to hold a
/// favicon or a font, and running a search and replace over one would corrupt
/// it — so a file that is not UTF-8 is written exactly as it was embedded.
fn write(destination: &Path, contents: &[u8], name: &str) -> Result<(), String> {
    match std::str::from_utf8(contents) {
        Ok(text) if text.contains(PLACEHOLDER) => {
            std::fs::write(destination, text.replace(PLACEHOLDER, name))
        }
        _ => std::fs::write(destination, contents),
    }
    .map_err(|e| format!("cannot write {}: {e}", destination.display()))
}

/// The one thing a template can ask about the project being created.
const PLACEHOLDER: &str = "{{name}}";

/// The project's name, from the directory it is being created in.
///
/// Sanitised, because this lands in `package.json` and npm's rules are narrower
/// than a filesystem's: a directory called `My App` would otherwise produce a
/// manifest that every package manager rejects, on the first command the user
/// runs.
fn package_name(target: &Path) -> String {
    let raw = target
        .file_name()
        .or_else(|| target.parent().and_then(Path::file_name))
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = raw
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'a'..='z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect();
    let cleaned = cleaned.trim_matches(['-', '.', '_']).to_string();
    if cleaned.is_empty() {
        "app".to_string()
    } else {
        cleaned
    }
}

/// The templates, as `--list` prints them.
pub fn list() -> String {
    let mut report = String::from("Templates:\n");
    for (name, files) in TEMPLATES {
        let description = DESCRIPTIONS
            .iter()
            .find(|(template, _)| template == name)
            .map_or("", |(_, description)| *description);

        // A template with modes has no single file count — the modes have one
        // each — so it names them instead, on the lines under it.
        match modes(name) {
            None => report.push_str(&format!(
                "  {name:<10} {description} ({} files)\n",
                files.len()
            )),
            Some(modes) => {
                report.push_str(&format!("  {name:<10} {description}\n"));
                for (mode, mode_description) in modes {
                    let count = files_for(files, Some(mode)).len();
                    let flag = format!("--mode={mode}");
                    report.push_str(&format!(
                        "  {:<10}   {flag:<18} {mode_description} ({count} files)\n",
                        ""
                    ));
                }
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The files of one template in one mode, as `create` would write them.
    fn resolved(template: &str, mode: Option<&str>) -> Vec<String> {
        let (_, files) = TEMPLATES
            .iter()
            .find(|(name, _)| *name == template)
            .expect("the template is embedded");
        files_for(files, mode)
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    }

    /// The point of the build script: a template that is not in the binary is
    /// a `create` that cannot work, and nothing else would notice.
    #[test]
    fn the_templates_are_in_the_binary() {
        assert_eq!(DEFAULT_TEMPLATE, "react");
        let paths = resolved(DEFAULT_TEMPLATE, default_mode(DEFAULT_TEMPLATE));

        for expected in [
            "package.json",
            "esdev.json",
            "index.html",
            "src/routes.tsx",
            "src/entry.client.tsx",
            "_gitignore",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} is not in the template: {paths:?}"
            );
        }
    }

    /// The whole point of a mode: what you get is one project, not the union of
    /// two with the other half left for you to delete.
    #[test]
    fn a_mode_writes_its_own_files_and_not_the_others() {
        let statik = resolved("react", Some("static"));
        let full = resolved("react", Some("fullstack"));

        assert!(statik.contains(&"src/prerender.tsx".to_string()));
        assert!(statik.contains(&"src/paths.ts".to_string()));
        assert!(
            !statik.iter().any(|path| path == "src/server.tsx"),
            "a static project has no server: {statik:?}"
        );
        assert!(
            !statik
                .iter()
                .any(|path| path.starts_with("src/http/headers")),
            "a static project sets no response headers: {statik:?}"
        );

        assert!(full.contains(&"src/server.tsx".to_string()));
        assert!(full.contains(&"src/http/headers.ts".to_string()));
        assert!(
            !full.iter().any(|path| path == "src/prerender.tsx"),
            "a fullstack project renders per request, so it prerenders nothing: {full:?}"
        );

        // The shared half really is shared, rather than duplicated per mode.
        for both in ["src/routes.tsx", "index.html", "styles/app.css"] {
            assert!(statik.contains(&both.to_string()) && full.contains(&both.to_string()));
        }
    }

    /// A mode replaces a shared file rather than being written beside it —
    /// otherwise the two esdev.json files would race and one would win by sort
    /// order.
    #[test]
    fn a_mode_writes_each_path_once() {
        for mode in ["static", "fullstack"] {
            let paths = resolved("react", Some(mode));
            let mut sorted = paths.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                paths.len(),
                "{mode} writes a path twice: {paths:?}"
            );
            // And no overlay path escapes with its prefix still on it.
            assert!(
                !paths.iter().any(|path| path.starts_with(MODE_PREFIX)),
                "{mode} leaked a _mode/ path: {paths:?}"
            );
        }
    }

    /// Every mode must produce a project, which starts with the two files that
    /// say what it is and how to build it.
    #[test]
    fn every_mode_of_every_template_is_a_whole_project() {
        for (template, modes) in MODES {
            for (mode, _) in *modes {
                let paths = resolved(template, Some(mode));
                for required in ["package.json", "esdev.json", "README.md"] {
                    assert!(
                        paths.iter().any(|path| path == required),
                        "{template} ({mode}) has no {required}: {paths:?}"
                    );
                }
            }
        }
    }

    /// Naming a mode a template does not have is refused rather than ignored.
    #[test]
    fn a_mode_that_is_not_one_is_refused() {
        assert!(resolve_mode("react", Some("ssr")).is_err());
        assert!(resolve_mode("api", Some("static")).is_err());
        assert!(matches!(
            resolve_mode("react", Some("static")),
            Ok(Mode::Chosen(mode)) if mode == "static"
        ));
        assert!(matches!(resolve_mode("api", None), Ok(Mode::None)));
    }

    /// What a *running* template leaves behind is not the template. Embedding
    /// an installed `node_modules` would put tens of megabytes of somebody
    /// else's code in this binary, and nothing about the build would complain.
    #[test]
    fn nothing_a_local_build_left_behind_is_embedded() {
        for (_, files) in TEMPLATES {
            for (path, _) in *files {
                assert!(
                    !path.starts_with("node_modules/")
                        && !path.starts_with("dist/")
                        && !path.ends_with("bun.lock")
                        && !path.ends_with("package-lock.json"),
                    "{path} should not be embedded"
                );
            }
        }
    }

    #[test]
    fn a_directory_name_becomes_a_package_name() {
        assert_eq!(package_name(Path::new("my-app")), "my-app");
        assert_eq!(package_name(Path::new("/tmp/My App")), "my-app");
        assert_eq!(package_name(Path::new("Weather_2026")), "weather_2026");
        // Nothing usable left is still a valid manifest.
        assert_eq!(package_name(Path::new("///")), "app");
    }

    #[test]
    fn the_list_names_every_embedded_template() {
        let listed = list();
        for (name, _) in TEMPLATES {
            assert!(listed.contains(name), "{name} is missing from:\n{listed}");
        }
    }
}
