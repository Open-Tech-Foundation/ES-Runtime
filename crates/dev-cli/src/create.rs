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
//! which template, which *mode* if that template has more than one shape, the
//! axes the template takes (language, styling, blog), which package manager
//! the project uses, and whether to install — and away from one it writes
//! the files and says nothing, because every other command here is a flag
//! grammar that works unattended and `create` stays one whenever it cannot
//! see a person ([`crate::prompt::interactive`]). Esc steps back to the
//! nearest question that was asked; Esc on the first cancels, having written
//! nothing.
//!
//! **Everything a prompt asks has a flag**, so the interactive path is a
//! convenience over the scriptable one and never the only way to an answer.
//! `--template=api --install=bun` is the same run with nothing to type. The
//! manager question's flag is `--package-manager`; `--install=<manager>`
//! answers it implicitly, by naming the manager that installs.
//!
//! # A template is a scaffold, not a demo
//!
//! What each one writes is a project that runs and **one page**, or for the API
//! one route: the project's name, what it was built with and who it comes from,
//! the file to edit, and three links. Nothing else. A blog, a task store or a
//! counter is somebody else's application, and every line of it has to be read
//! and then deleted before the project can become the one it was created for
//! (D76).
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
//! A mode is one of the template's options (`mode`), and like every option its
//! files live in `_option/mode/<name>/`: what gets written is everything
//! outside `_option/` plus the chosen choices' overlays (D220, see
//! [`crate::template`]).
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

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use es_runtime_default_providers::TrustStore;
use es_runtime_default_providers::client::{HttpClient, Outbound};

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

/// The templates `create` can write: each one's files and its manifest, and
/// the catalog of groups some are chosen behind (D220).
///
/// Read from what is embedded, once; every manifest is validated when it is
/// read, and a test reads them all, so a broken one fails the build's tests
/// rather than somebody's `create`.
pub(crate) struct Library {
    templates: Vec<Entry>,
    catalog: crate::template::Catalog,
}

/// One template: its name, its files (manifest and overlays included) and
/// what its manifest says.
pub(crate) struct Entry {
    name: &'static str,
    files: Vec<(&'static str, &'static [u8])>,
    manifest: crate::template::Manifest,
}

impl Library {
    /// The templates built into this binary.
    fn embedded() -> Result<Library, String> {
        let mut templates = Vec::with_capacity(TEMPLATES.len());
        for (name, files) in TEMPLATES {
            let files: Vec<(&'static str, &'static [u8])> = files.to_vec();
            let manifest = crate::template::manifest(name, &files)?;
            templates.push(Entry {
                name,
                files,
                manifest,
            });
        }
        let catalog: crate::template::Catalog =
            serde_json::from_slice(CATALOG).map_err(|e| format!("catalog.json: {e}"))?;
        for group in &catalog.groups {
            if templates.iter().any(|entry| entry.name == group.name) {
                return Err(format!("catalog.json: {} is a template's name", group.name));
            }
            for member in &group.templates {
                let entry = templates
                    .iter()
                    .find(|entry| entry.name == member)
                    .ok_or_else(|| {
                        format!("catalog.json: {} has no template {member}", group.name)
                    })?;
                if entry.manifest.group.as_deref() != Some(group.name.as_str()) {
                    return Err(format!(
                        "catalog.json: {member} is in {}, and its template.json does not say so",
                        group.name
                    ));
                }
            }
        }
        for entry in &templates {
            if let Some(group) = &entry.manifest.group
                && !catalog
                    .groups
                    .iter()
                    .any(|g| &g.name == group && g.templates.iter().any(|t| t == entry.name))
            {
                return Err(format!(
                    "the {} template names the group {group}, which does not list it",
                    entry.name
                ));
            }
        }
        Ok(Library { templates, catalog })
    }

    fn get(&self, name: &str) -> Option<&Entry> {
        self.templates.iter().find(|entry| entry.name == name)
    }
}

/// The embedded library, read once.
fn library() -> &'static Library {
    static LIBRARY: std::sync::OnceLock<Library> = std::sync::OnceLock::new();
    LIBRARY.get_or_init(|| {
        Library::embedded().unwrap_or_else(|e| {
            panic!("the embedded templates are invalid — `the_embedded_templates_are_valid` says so: {e}")
        })
    })
}

impl Entry {
    /// The option called `name`, if this template asks it.
    fn option(&self, name: &str) -> Option<&crate::template::OptionSpec> {
        self.manifest
            .options
            .iter()
            .find(|option| option.name == name)
    }

    /// Every option at its fallback, except those `overrides` names.
    fn answers(&self, overrides: &[(&str, &str)]) -> Vec<(String, String)> {
        self.manifest
            .options
            .iter()
            .map(|option| {
                let value = overrides
                    .iter()
                    .find(|(name, _)| *name == option.name)
                    .map_or(option.fallback(), |(_, value)| *value);
                (option.name.clone(), value.to_string())
            })
            .collect()
    }

    /// The files written for `answers`.
    fn files(&self, answers: &[(String, String)]) -> Result<Vec<(String, Vec<u8>)>, String> {
        crate::template::assemble(&self.files, &self.manifest, answers)
    }
}

/// An option named on the command line: `--<name>=<value>`, `--<name>` (yes)
/// or `--no-<name>` (no), as it was spelled, for the message that refuses it.
#[derive(Debug, Clone)]
pub struct OptionFlag {
    pub name: String,
    pub value: String,
    pub spelled: String,
}

/// A file whose name in the template is not the name it is written under.
///
/// `.gitignore` is the whole list, and it is not cosmetic: a `.gitignore` in
/// the template directory would be applied *to the template*, so this
/// repository would stop tracking the very file it means to ship. npm's
/// packaging has the same problem and the same fix, which is why the convention
/// is one somebody scaffolding will already have seen.
pub(crate) const RENAMED: &[(&str, &str)] = &[("_gitignore", ".gitignore")];

/// What `esdev create` was asked to do.
pub struct CreateConfig {
    /// The directory to write into.
    pub dir: String,
    /// Which template, or `None` to ask. There is no default: an unattended
    /// run without `--template` is refused rather than guessed.
    pub template: Option<String>,
    /// The template's options named on the command line, in order; a later
    /// one wins. One the template does not ask is refused rather than
    /// ignored: a flag that silently does nothing is one somebody keeps
    /// passing, and keeps believing.
    pub options: Vec<OptionFlag>,
    /// Whether to write into a directory that already holds something.
    pub force: bool,
    /// Which package manager the project uses, or `None` to ask (or take
    /// the default). Rendered into the docs (`{{pm}}`) and printed in the
    /// next steps; `--install=<manager>` answers this implicitly.
    pub manager: Option<String>,
    /// Which package manager to install with, `Some(None)` for an explicit
    /// "do not install", and `None` to ask (or, unattended, not to).
    pub install: Option<Option<String>>,
    /// `-y`: take every default and never ask, even on a terminal.
    pub yes: bool,
    /// The outbound proxy for the registry lookups (D205).
    pub proxies: Option<std::sync::Arc<es_runtime_default_providers::Proxies>>,
}

/// What a bare `--install` means.
///
/// npm, because it is what a Node installation already has — the answer that
/// needs the least explaining when somebody did not name one.
pub const DEFAULT_MANAGER: &str = "npm";

/// Scaffolds a project and reports what to do next.
pub async fn create(config: &CreateConfig) -> Result<String, String> {
    // Everything below resolves before anything is written, so a person who
    // changes their mind at a prompt leaves no directory behind.
    let scripted = config.yes || !crate::prompt::interactive();
    if !scripted {
        crate::prompt::intro("esdev create");
    }
    // A flag's value is checked before any question: a package manager that is
    // not one is the caller's mistake whatever else is missing, and asking for
    // a template first would hide it behind a question it does not depend on.
    let named_managers = [
        config.manager.as_deref(),
        config.install.as_ref().and_then(|named| named.as_deref()),
    ];
    for named in named_managers.into_iter().flatten() {
        if crate::install::by_name(named).is_none() {
            return Err(unknown_manager(named));
        }
    }
    let library = library();
    let mut answers = Answers::default();
    if let Some(named) = &config.template {
        answers.template = Some(named.clone());
    }
    // Steps that showed a menu. Esc steps back to the nearest one before the
    // current step; a step answered by a flag never appears here, so the
    // retreat skips it — and with no prompted step behind, Esc cancels.
    let mut prompted: Vec<Step> = Vec::new();
    let mut step = 0;
    loop {
        // The questions after the template are that template's own, so the
        // list is rebuilt whenever the template may have changed.
        let entry = answers
            .template
            .as_deref()
            .and_then(|name| library.get(name));
        let steps = steps_for(entry);
        if step >= steps.len() {
            break;
        }
        let ask = Ask {
            scripted,
            back: steps[..step].iter().any(|s| prompted.contains(s)),
        };
        let current = steps[step];
        match current {
            Step::Template => {
                if answers.template.is_none() {
                    if scripted {
                        // No default template: guessing one writes a project
                        // nobody chose, so an unattended run without
                        // `--template` stops before writing anything.
                        return Err(format!(
                            "no template chosen.\n\nThere is no default template: pass --template=<name>, or choose on a terminal.\n\n{}",
                            list().trim_end()
                        ));
                    } else {
                        match ask_template(library) {
                            Some(template) => {
                                answers.template = Some(template);
                                prompt_once(&mut prompted, Step::Template);
                            }
                            // Esc on the first question, before anything has
                            // been written. Nothing to undo, nothing to
                            // report, and an exit status of zero: a person who
                            // changed their mind did not hit an error.
                            None => return Ok(String::new()),
                        }
                    }
                }
                let template = answers.template.as_deref().expect("answered above");
                let Some(entry) = library.get(template) else {
                    return Err(format!(
                        "there is no {template} template.\n\n{}",
                        list().trim_end()
                    ));
                };
                refuse_foreign_options(library, entry, &config.options)?;
                answers.options = vec![None; entry.manifest.options.len()];
                step += 1;
            }
            Step::Option(index) => {
                let entry = entry.expect("the template step resolves it");
                let option = &entry.manifest.options[index];
                let asked = config
                    .options
                    .iter()
                    .rev()
                    .find(|flag| flag.name == option.name)
                    .map(|flag| flag.value.as_str());
                match resolve_option(entry.name, option, asked, ask)? {
                    Some(value) => {
                        answers.options[index] = Some(value);
                        if asked.is_none() && !scripted {
                            prompt_once(&mut prompted, current);
                        }
                        step += 1;
                    }
                    None => match step_back_index(&steps, step, &prompted) {
                        Some(index) => step = index,
                        None => return Ok(String::new()),
                    },
                }
            }
            Step::Manager => match resolve_manager(config.manager.as_deref(), ask)? {
                ManagerChoice::Chosen(manager) => {
                    answers.manager = Some(manager);
                    if config.manager.is_none() && !scripted {
                        prompt_once(&mut prompted, Step::Manager);
                    }
                    step += 1;
                }
                ManagerChoice::Back => match step_back_index(&steps, step, &prompted) {
                    Some(index) => step = index,
                    None => return Ok(String::new()),
                },
            },
        }
    }
    let template = answers.template.expect("the loop answers it");
    let entry = library.get(&template).expect("validated in the loop");
    let chosen: Vec<(String, String)> = entry
        .manifest
        .options
        .iter()
        .zip(answers.options)
        .map(|(option, value)| (option.name.clone(), value.expect("the loop answers it")))
        .collect();
    let mode = chosen
        .iter()
        .find(|(name, _)| name == "mode")
        .map(|(_, value)| value.clone());
    // A flag that names the installer names the project's manager too:
    // installing with bun means the docs name bun. Validated here, before
    // anything is written, so an unknown name fails with no directory behind.
    let install_named = match &config.install {
        Some(Some(named)) => {
            Some(crate::install::by_name(named).ok_or_else(|| unknown_manager(named))?)
        }
        _ => None,
    };
    let manager: String = match install_named {
        Some(manager) => manager.name.to_string(),
        None => answers.manager.expect("the loop answers it"),
    };
    let files = entry.files(&chosen)?;

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
    let pending: Vec<_> = files
        .iter()
        .filter(|(path, _)| {
            let path = RENAMED
                .iter()
                .find(|(from, _)| from == path)
                .map_or(path.as_str(), |(_, to)| *to);
            !target.join(path).exists()
        })
        .map(|(path, contents)| (path.clone(), contents.clone()))
        .collect();
    let packages = registry_packages(&pending)?;
    let versions = resolve_registry_versions(&packages, config.proxies.clone()).await?;

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
        write(&destination, contents, &name, &manager, &versions)?;
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
    let paint = crate::style::Palette::stdout();
    if written == 0 {
        crate::prompt::outro("Nothing to write.");
        return Ok(format!(
            "nothing to write: {} already holds every file the {named} template has.\n",
            config.dir
        ));
    }
    let mut report = format!(
        "{} {} {}\n",
        paint.green("created"),
        paint.cyan(&config.dir),
        paint.dim(format_args!(
            "from the {named} template ({written} file{})",
            if written == 1 { "" } else { "s" }
        )),
    );
    if !skipped.is_empty() {
        report.push_str(&paint.dim(format_args!(
            "left alone, because they were already there: {}\n",
            skipped.join(", ")
        )));
    }
    // Reported before the install rather than after it, so the transcript reads
    // in the order things happened: what was written, then what installing it
    // printed. Returning the whole report at the end would put the install's
    // own output above the line announcing the project it installed into.
    // Inside the questions' gutter when they drew one, so the transcript stays
    // one column; on stdout as before when nothing was asked.
    if crate::prompt::is_open() {
        crate::prompt::success(report.trim_end());
    } else {
        print!("{report}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    report.clear();

    // Only now, with the project on disk: an install that fails leaves a
    // project that is complete and one command away, rather than half of one.
    // The flag's manager was validated before anything was written, so this
    // resolves rather than re-checks.
    let installed = match (&config.install, install_named) {
        (Some(Some(_)), Some(manager)) => {
            crate::install::run(manager, &target)?;
            Some(manager)
        }
        (Some(Some(_)), None) => unreachable!("validated before anything was written"),
        (Some(None), _) => None,
        (None, _) if crate::prompt::interactive() => {
            let preferred = crate::install::by_name(&manager);
            match ask_install(preferred) {
                Some(manager) => {
                    crate::install::run(manager, &target)?;
                    Some(manager)
                }
                None => None,
            }
        }
        (None, _) => None,
    };

    report.push_str(&next_steps(
        &config.dir,
        &entry.manifest.run,
        installed,
        &manager,
        paint,
    ));
    // create-afw's closing words, with the next steps under them.
    crate::prompt::outro("Done. Now run:");
    Ok(report)
}

/// The lines printed after the project is written.
fn next_steps(
    dir: &str,
    // The script that starts it, which is not the same for every template:
    // a library has nothing to run. The manifest's `run`.
    run: &str,
    installed: Option<crate::install::Manager>,
    manager: &str,
    paint: crate::style::Palette,
) -> String {
    // What installed is what runs: only a project nobody installed names the
    // chosen-or-default manager instead.
    let manager = installed.map_or(manager, |m| m.name);

    // Bold, because these are lines to type rather than lines to read.
    let mut steps = format!("\n  {}\n", paint.bold(format_args!("cd {dir}")));
    if installed.is_none() {
        steps.push_str(&format!(
            "  {}\n",
            paint.bold(format_args!("{manager} install"))
        ));
    }
    steps.push_str(&format!(
        "  {}\n",
        paint.bold(format_args!("{manager} run {run}"))
    ));
    steps
}

/// One pre-write question: the template, one of its options, or the manager.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Template,
    /// The template's option at this index in its manifest.
    Option(usize),
    Manager,
}

/// The questions in order, for the template chosen so far: before one is,
/// only the template and the manager.
fn steps_for(entry: Option<&Entry>) -> Vec<Step> {
    let mut steps = vec![Step::Template];
    if let Some(entry) = entry {
        steps.extend((0..entry.manifest.options.len()).map(Step::Option));
    }
    steps.push(Step::Manager);
    steps
}

fn prompt_once(prompted: &mut Vec<Step>, step: Step) {
    if !prompted.contains(&step) {
        prompted.push(step);
    }
}

/// The nearest earlier step that showed a menu, or `None` when Esc cancels.
/// Pure, so the retreat rule tests without a terminal.
fn step_back_index(steps: &[Step], current: usize, prompted: &[Step]) -> Option<usize> {
    (0..current.min(steps.len()))
        .rev()
        .find(|&index| prompted.contains(&steps[index]))
}

/// The answers so far. Every forward pass overwrites each one, so switching
/// template mid-run cannot leave a stale answer behind.
#[derive(Default)]
struct Answers {
    template: Option<String>,
    /// One per option of the template, in manifest order.
    options: Vec<Option<String>>,
    manager: Option<String>,
}

/// How one pre-write question resolves.
#[derive(Clone, Copy)]
struct Ask {
    /// `-y`, or away from a terminal: menus never appear, defaults win. This
    /// is what makes `-y`'s promise hold on a terminal, where `interactive()`
    /// alone is true and a menu would otherwise appear.
    scripted: bool,
    /// Esc steps back: an earlier step showed a menu.
    back: bool,
}

impl Ask {
    fn esc(&self) -> crate::prompt::OnEsc {
        if self.back {
            crate::prompt::OnEsc::Back
        } else {
            crate::prompt::OnEsc::Cancel
        }
    }
}

/// An option flag the chosen template does not ask is refused before
/// anything is asked or written, naming the templates that do ask it.
fn refuse_foreign_options(
    library: &Library,
    entry: &Entry,
    flags: &[OptionFlag],
) -> Result<(), String> {
    let Some(flag) = flags.iter().find(|flag| entry.option(&flag.name).is_none()) else {
        return Ok(());
    };
    let having: Vec<&str> = library
        .templates
        .iter()
        .filter(|other| other.option(&flag.name).is_some())
        .map(|other| other.name)
        .collect();
    let elsewhere = match having.as_slice() {
        [] => format!(
            "No template has a {} option; `esdev create --help` lists create's own flags.",
            flag.name
        ),
        [one] => format!("Only {one} does."),
        many => format!("{} do.", many.join(", ")),
    };
    Err(format!(
        "the {} template has no {}, so {} means nothing here.\n\n{elsewhere}",
        entry.name, flag.name, flag.spelled
    ))
}

/// One option's answer, from the flag, a question, or its fallback. `None`
/// is Esc: back to the question before, or out if there is none.
///
/// A value that is not one of the option's choices is refused with the
/// values that are.
fn resolve_option(
    template: &str,
    option: &crate::template::OptionSpec,
    asked_for: Option<&str>,
    ask: Ask,
) -> Result<Option<String>, String> {
    if let Some(value) = asked_for {
        if option.choice(value).is_none() {
            let valid: Vec<&str> = option.choices.iter().map(|c| c.value.as_str()).collect();
            return Err(format!(
                "the {template} template has no {} {value}.\n\nValid {}s: {}.",
                option.name,
                option.name,
                valid.join(", ")
            ));
        }
        return Ok(Some(value.to_string()));
    }
    if ask.scripted || !crate::prompt::interactive() {
        return Ok(Some(option.fallback().to_string()));
    }
    if option.kind == crate::template::Kind::Confirm {
        return Ok(
            crate::prompt::confirm(&option.question, option.fallback() == "yes", ask.esc())
                .map(|yes| if yes { "yes" } else { "no" }.to_string()),
        );
    }
    let choices: Vec<crate::prompt::Choice<'_>> = option
        .choices
        .iter()
        .map(|choice| crate::prompt::Choice {
            name: &choice.value,
            label: &choice.label,
            description: &choice.description,
            disabled: false,
        })
        .collect();
    // An option with a default starts the menu on it; one without starts on
    // nothing, so Enter alone cannot answer it (D135).
    let preselect = option
        .default
        .as_deref()
        .and_then(|value| choices.iter().position(|choice| choice.name == value));
    Ok(
        crate::prompt::select(&option.question, &choices, preselect, ask.esc())
            .map(|chosen| choices[chosen].name.to_string()),
    )
}

/// What resolving `--package-manager` came to.
#[derive(Debug)]
enum ManagerChoice {
    /// The project's package manager, canonical spelling.
    Chosen(String),
    /// Esc: back to the question before, or out if there is none.
    Back,
}

/// No such manager, from a flag that names one. Shared by the manager
/// question, the install, and `init`, so every command names the same set.
pub(crate) fn unknown_manager(named: &str) -> String {
    format!(
        "there is no {named} package manager.\n\nKnown: {}.",
        crate::install::MANAGERS
            .iter()
            .map(|m| m.name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Which package manager the project's docs name, from the flag, a question,
/// or the default. Every template takes it — a project has an install
/// command whatever it is built with — so there is no refusal, only an
/// unknown name.
fn resolve_manager(asked_for: Option<&str>, ask: Ask) -> Result<ManagerChoice, String> {
    if let Some(named) = asked_for {
        return match crate::install::by_name(named) {
            Some(manager) => Ok(ManagerChoice::Chosen(manager.name.to_string())),
            None => Err(unknown_manager(named)),
        };
    }
    if ask.scripted || !crate::prompt::interactive() {
        return Ok(ManagerChoice::Chosen(DEFAULT_MANAGER.to_string()));
    }
    match ask_manager(ask) {
        Some(manager) => Ok(ManagerChoice::Chosen(manager.to_string())),
        None => Ok(ManagerChoice::Back),
    }
}

/// Which package manager, asked on a terminal.
///
/// What this machine has is marked and choosable; what it does not have is
/// shown as not available and cannot be chosen, so the answer is always
/// something the install step can actually run. When detection finds nothing
/// at all the menu stays unrestricted — disabling everything would trap the
/// run — and the install step skips itself as before.
fn ask_manager(ask: Ask) -> Option<&'static str> {
    manager_menu(ask.esc())
}

/// The manager menu, shared with `init` — which has no step machine to step
/// back through, so Esc cancels the run there. `None` is Esc either way.
pub(crate) fn ask_manager_choice() -> Option<&'static str> {
    manager_menu(crate::prompt::OnEsc::Cancel)
}

/// One row of the manager menu: the base description plus install status, and
/// whether the row can be chosen. Pure over the detected set, so the status
/// and the disabled rule test without touching the machine.
struct ManagerEntry {
    name: &'static str,
    description: String,
    disabled: bool,
}

/// The base line each manager is described with, before the install status.
fn manager_base_description(name: &str) -> &'static str {
    match name {
        "npm" => "Node's own — already installed with Node",
        "bun" => "Fast all-in-one toolchain",
        "pnpm" => "Strict, disk-efficient installs",
        "yarn" => "Classic alternative",
        _ => "",
    }
}

/// The manager menu's rows for what `available` holds. An empty detection
/// leaves every row enabled rather than trapping the run with nothing to pick.
fn manager_entries(available: &[crate::install::Manager]) -> Vec<ManagerEntry> {
    let unrestricted = available.is_empty();
    crate::install::MANAGERS
        .iter()
        .map(|manager| {
            let installed = available.contains(manager);
            let description = if unrestricted {
                manager_base_description(manager.name).to_string()
            } else if installed {
                format!("{} · installed", manager_base_description(manager.name))
            } else {
                format!("{} · not installed", manager_base_description(manager.name))
            };
            ManagerEntry {
                name: manager.name,
                description,
                disabled: !unrestricted && !installed,
            }
        })
        .collect()
}

/// Where the manager menu starts: the default when it can be chosen, else the
/// first thing that can be. Pure, so the fallback tests directly.
fn manager_preselect(entries: &[ManagerEntry], default: &str) -> Option<usize> {
    entries
        .iter()
        .position(|entry| entry.name == default && !entry.disabled)
        .or_else(|| entries.iter().position(|entry| !entry.disabled))
}

fn manager_menu(esc: crate::prompt::OnEsc) -> Option<&'static str> {
    let available = crate::install::available();
    let entries = manager_entries(&available);
    let choices: Vec<crate::prompt::Choice<'_>> = entries
        .iter()
        .map(|entry| crate::prompt::Choice {
            name: entry.name,
            label: entry.name,
            description: entry.description.as_str(),
            disabled: entry.disabled,
        })
        .collect();
    let preselect = manager_preselect(&entries, DEFAULT_MANAGER);
    let chosen = crate::prompt::select("Which Package Manager?", &choices, preselect, esc)?;
    // Into `entries`, not `choices`: the name is `'static`, while `choices`
    // borrows the owned descriptions beside them.
    entries.get(chosen).map(|entry| entry.name)
}

/// The first menu: the templates outside any group, in `--list` order, then
/// one entry per group. Pure, so a test can read what somebody choosing sees.
fn template_menu_top(library: &Library) -> Vec<crate::prompt::Choice<'_>> {
    let mut menu: Vec<crate::prompt::Choice<'_>> = library
        .templates
        .iter()
        .filter(|entry| entry.manifest.group.is_none())
        .map(|entry| crate::prompt::Choice {
            name: entry.name,
            label: &entry.manifest.label,
            description: &entry.manifest.description,
            disabled: false,
        })
        .collect();
    menu.extend(
        library
            .catalog
            .groups
            .iter()
            .map(|group| crate::prompt::Choice {
                name: &group.name,
                label: &group.label,
                description: &group.description,
                disabled: false,
            }),
    );
    menu
}

/// The menu behind a group entry: its templates, in the catalog's order.
fn template_menu_group<'a>(
    library: &'a Library,
    group: &'a crate::template::Group,
) -> Vec<crate::prompt::Choice<'a>> {
    group
        .templates
        .iter()
        .filter_map(|name| library.get(name))
        .map(|entry| crate::prompt::Choice {
            name: entry.name,
            label: &entry.manifest.label,
            description: &entry.manifest.description,
            disabled: false,
        })
        .collect()
}

/// Which template, asked on a terminal.
///
/// The grouped templates choose behind their group's entry. Neither menu has
/// a default — the template is the one answer worth choosing explicitly —
/// while flags, `-y` and unattended runs resolve exactly as before. Esc on the
/// first menu cancels the run; Esc on a group's menu steps back to the first.
fn ask_template(library: &Library) -> Option<String> {
    let top = template_menu_top(library);
    loop {
        let chosen =
            crate::prompt::select("Which Template?", &top, None, crate::prompt::OnEsc::Cancel)?;
        let Some(group) = library
            .catalog
            .groups
            .iter()
            .find(|g| g.name == top[chosen].name)
        else {
            return Some(top[chosen].name.to_string());
        };
        let members = template_menu_group(library, group);
        if let Some(chosen) =
            crate::prompt::select(&group.question, &members, None, crate::prompt::OnEsc::Back)
        {
            return Some(members[chosen].name.to_string());
        }
    }
}

/// Whether to install, and with what.
///
/// When the project already named a manager that is here, this is a yes/no
/// for exactly that manager — the choice was made one question ago, and
/// listing every installer again is what made the second question read as the
/// first one repeated. Otherwise only what this machine actually has is
/// offered, since naming a package manager that is not installed is offering
/// an error message.
pub(crate) fn ask_install(
    preferred: Option<crate::install::Manager>,
) -> Option<crate::install::Manager> {
    let available = crate::install::available();
    if available.is_empty() {
        return None;
    }

    // The common path out of `create`: the manager menu only lets somebody
    // pick what is installed, so confirming here is confirming something that
    // runs. A preferred manager that is not here — a flag, or `init`'s
    // install-first order — falls through to the list below.
    if let Some(wanted) = preferred
        && install_confirms(Some(wanted), &available)
    {
        // Esc declines, as `No` does: the project is already on disk by now,
        // and declining the *install* is not cancelling the project. Either
        // way the next steps say how to install it.
        let question = format!("Install with {}?", wanted.name);
        let install = crate::prompt::confirm(&question, true, crate::prompt::OnEsc::Skip)?;
        return install.then_some(wanted);
    }

    let mut choices: Vec<crate::prompt::Choice<'_>> = available
        .iter()
        .map(|manager| crate::prompt::Choice {
            name: manager.name,
            label: manager.name,
            description: "",
            disabled: false,
        })
        .collect();
    choices.push(crate::prompt::Choice {
        name: "skip",
        label: "Skip",
        description: "write the files and stop",
        disabled: false,
    });

    // Esc declines, as `skip` does: the project is already on disk by now, and
    // declining the *install* is not cancelling the project. Either way the
    // next steps say how to install it.
    let preselect = preferred
        .and_then(|manager| available.iter().position(|m| *m == manager))
        .or(Some(0));
    let chosen = crate::prompt::select(
        "Install the Dependencies?",
        &choices,
        preselect,
        crate::prompt::OnEsc::Skip,
    )?;
    available.get(chosen).copied()
}

/// Whether the install step is a yes/no for the project's manager rather than
/// a list of installers: exactly when that manager is here to run. Pure, so
/// the two paths test without touching the machine.
fn install_confirms(
    preferred: Option<crate::install::Manager>,
    available: &[crate::install::Manager],
) -> bool {
    preferred.is_some_and(|wanted| available.contains(&wanted))
}

const REGISTRY_PREFIX: &str = "{{registry:";
const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";
const REGISTRY_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

fn registry_placeholder(package: &str) -> String {
    format!("{{{{registry:{package}}}}}")
}

/// Package names named by the not-yet-written manifests. Each unique name is
/// resolved once even if a package appears in both peer and dev dependencies.
fn registry_packages(files: &[(String, Vec<u8>)]) -> Result<BTreeSet<String>, String> {
    let mut packages = BTreeSet::new();
    for (path, contents) in files {
        if !path.ends_with("package.json") {
            continue;
        }
        let Ok(text) = std::str::from_utf8(contents) else {
            continue;
        };
        let mut remaining = text;
        while let Some(start) = remaining.find(REGISTRY_PREFIX) {
            let after_prefix = &remaining[start + REGISTRY_PREFIX.len()..];
            let Some(end) = after_prefix.find("}}") else {
                return Err(format!(
                    "{path} has an unterminated registry version placeholder"
                ));
            };
            let package = &after_prefix[..end];
            if package.is_empty() || package.chars().any(char::is_whitespace) {
                return Err(format!("{path} has an invalid registry package name"));
            }
            packages.insert(package.to_string());
            remaining = &after_prefix[end + 2..];
        }
    }
    Ok(packages)
}

#[derive(Debug)]
struct RegistryFailure {
    message: String,
    retryable: bool,
}

fn registry_url(registry: &str, package: &str) -> String {
    format!(
        "{}/{}",
        registry.trim_end_matches('/'),
        package.replace('/', "%2F")
    )
}

fn configured_registry() -> Result<String, String> {
    let configured = std::env::var("npm_config_registry")
        .or_else(|_| std::env::var("NPM_CONFIG_REGISTRY"))
        .unwrap_or_else(|_| DEFAULT_REGISTRY.to_string());
    let registry = configured.trim_end_matches('/');
    let parsed = url::Url::parse(registry)
        .map_err(|error| format!("invalid npm registry URL {configured:?}: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(format!("invalid npm registry URL {configured:?}"));
    }
    Ok(registry.to_string())
}

/// How long one registry lookup may take, connect to last byte.
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest package document a lookup reads. A popular package's full
/// metadata runs to megabytes; this is far beyond any of them.
const REGISTRY_LIMIT: usize = 64 * 1024 * 1024;

async fn registry_version(
    client: &HttpClient,
    registry: &str,
    package: &str,
) -> Result<String, RegistryFailure> {
    let failure = |message: String| RegistryFailure {
        message,
        retryable: true,
    };
    let url = url::Url::parse(&registry_url(registry, package))
        .map_err(|error| failure(format!("invalid registry URL: {error}")))?;
    let mut request = Outbound::get(url);
    request.headers = vec![(
        "user-agent".to_string(),
        concat!("esdev/", env!("CARGO_PKG_VERSION")).to_string(),
    )];
    let lookup = async {
        let response = client
            .send(request)
            .await
            .map_err(|error| failure(error.to_string()))?;
        let status = response.status;
        if !status.is_success() {
            let retryable = status.is_server_error()
                // 429 Too Many Requests, 408 Request Timeout.
                || matches!(status.as_u16(), 429 | 408);
            return Err(RegistryFailure {
                message: format!("registry returned HTTP {status}"),
                retryable,
            });
        }
        response
            .bytes(REGISTRY_LIMIT)
            .await
            .map_err(|error| failure(error.to_string()))
    };
    let body = tokio::time::timeout(REGISTRY_TIMEOUT, lookup)
        .await
        .map_err(|_| failure(format!("no answer within {}s", REGISTRY_TIMEOUT.as_secs())))??;
    let metadata: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| failure(format!("invalid registry response: {error}")))?;
    scaffold_version(&metadata).ok_or_else(|| RegistryFailure {
        message: "registry response has no dist-tags.latest version".to_string(),
        retryable: false,
    })
}

/// The version a new project is pinned to: `dist-tags.latest`, unless that is
/// a prerelease and the package has a stable release (D215).
///
/// `latest` is whatever a maintainer last published without a tag, and a
/// prerelease lands there by mistake — elysia's `2.0.0-rc.4` did in November
/// 2026, and every project scaffolded that week pinned a release candidate
/// whose API the template was not written for. npm installs `latest` either
/// way; a scaffolder writes the pin somebody keeps, so it takes the newest
/// stable, non-deprecated version instead. A package that has only ever
/// published prereleases keeps its `latest`: there is nothing stable to pin.
fn scaffold_version(metadata: &serde_json::Value) -> Option<String> {
    let latest = metadata
        .get("dist-tags")
        .and_then(|tags| tags.get("latest"))
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty())?;
    if stable(latest).is_some() {
        return Some(latest.to_string());
    }
    let newest_stable = metadata
        .get("versions")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(_, manifest)| manifest.get("deprecated").is_none())
        .filter_map(|(version, _)| stable(version).map(|parsed| (parsed, version)))
        .max_by_key(|(parsed, _)| *parsed)
        .map(|(_, version)| version.clone());
    Some(newest_stable.unwrap_or_else(|| latest.to_string()))
}

/// A stable semver version as its three numbers, or `None` for a prerelease
/// (`2.0.0-rc.4`) or anything that is not `major.minor.patch`. Build metadata
/// (`+abc`) does not make a version a prerelease, so it is ignored.
fn stable(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split('+').next()?;
    if core.contains('-') {
        return None;
    }
    let mut parts = core.split('.');
    let numbers = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(numbers)
}

fn registry_backoff(attempt: usize, retryable: bool) -> Option<Duration> {
    retryable
        .then(|| REGISTRY_BACKOFFS.get(attempt).copied())
        .flatten()
}

async fn registry_version_with_retries(
    client: &HttpClient,
    registry: &str,
    package: &str,
) -> Result<String, String> {
    for attempt in 0..=REGISTRY_BACKOFFS.len() {
        match registry_version(client, registry, package).await {
            Ok(version) => return Ok(version),
            Err(failure) => {
                let Some(backoff) = registry_backoff(attempt, failure.retryable) else {
                    let attempts = if failure.retryable {
                        format!(" after {} attempts", REGISTRY_BACKOFFS.len() + 1)
                    } else {
                        String::new()
                    };
                    return Err(format!(
                        "could not resolve {package} from the configured npm registry{attempts}: {}",
                        failure.message
                    ));
                };
                crate::prompt::warn(&format!(
                    "npm registry lookup for {package} failed on attempt {}/{}, {}; retrying in {} ms",
                    attempt + 1,
                    REGISTRY_BACKOFFS.len() + 1,
                    failure.message,
                    backoff.as_millis()
                ));
                tokio::time::sleep(backoff).await;
            }
        }
    }
    unreachable!("the retry loop returns on success or exhaustion")
}

/// Resolve the package placeholders before writing anything. Requests run in
/// parallel so a slow package does not hold up the other registry lookups.
async fn resolve_registry_versions(
    packages: &BTreeSet<String>,
    proxies: Option<std::sync::Arc<es_runtime_default_providers::Proxies>>,
) -> Result<HashMap<String, String>, String> {
    if packages.is_empty() {
        return Ok(HashMap::new());
    }
    crate::prompt::log("");
    crate::prompt::log(&format!(
        "Resolving current versions for {} template packages from {}...",
        packages.len(),
        std::env::var("npm_config_registry")
            .or_else(|_| std::env::var("NPM_CONFIG_REGISTRY"))
            .unwrap_or_else(|_| DEFAULT_REGISTRY.to_string())
    ));
    let registry = configured_registry()?;
    let client = HttpClient::new(&TrustStore::bundled()).with_proxy(proxies);
    let requests = packages.iter().map(|package| {
        let client = client.clone();
        let registry = registry.clone();
        async move {
            let result = registry_version_with_retries(&client, &registry, package).await;
            (package.clone(), result)
        }
    });
    let results = futures_util::future::join_all(requests).await;

    let mut versions = HashMap::new();
    let mut failures = Vec::new();
    for (package, result) in results {
        match result {
            Ok(version) => {
                versions.insert(package, version);
            }
            Err(error) => failures.push((package, error)),
        }
    }
    if let Some((_, error)) = failures.into_iter().min_by(|a, b| a.0.cmp(&b.0)) {
        return Err(error);
    }
    let mut resolved: Vec<_> = versions.iter().collect();
    resolved.sort_by(|a, b| a.0.cmp(b.0));
    for (package, version) in resolved {
        crate::prompt::log(&format!("  {package}@{version}"));
    }
    Ok(versions)
}

fn render_registry_versions(
    mut text: String,
    versions: &HashMap<String, String>,
) -> Result<String, String> {
    for (package, version) in versions {
        text = text.replace(&registry_placeholder(package), version);
    }
    if text.contains(REGISTRY_PREFIX) {
        return Err(
            "a template package version was not resolved from the npm registry".to_string(),
        );
    }
    Ok(text)
}

/// Writes one file, substituting the project's name, package manager, and the
/// versions resolved for this scaffold.
///
/// The substitution is attempted only on text. A template is free to hold a
/// favicon or a font, and running a search and replace over one would corrupt
/// it — so a file that is not UTF-8 is written exactly as it was embedded.
fn write(
    destination: &Path,
    contents: &[u8],
    name: &str,
    manager: &str,
    versions: &HashMap<String, String>,
) -> Result<(), String> {
    match std::str::from_utf8(contents) {
        Ok(text)
            if text.contains(PLACEHOLDER)
                || text.contains(PM_PLACEHOLDER)
                || text.contains(REGISTRY_PREFIX) =>
        {
            let rendered = render(text, name, manager);
            let rendered = render_registry_versions(rendered, versions)?;
            std::fs::write(destination, rendered)
        }
        _ => std::fs::write(destination, contents),
    }
    .map_err(|e| format!("cannot write {}: {e}", destination.display()))
}

/// The placeholders filled in, pure so the rendering tests without files.
/// Shared with `init`, whose bare project names the manager in the same docs.
pub(crate) fn render(text: &str, name: &str, manager: &str) -> String {
    text.replace(PLACEHOLDER, name)
        .replace(PM_PLACEHOLDER, manager)
}

/// The one thing a template can ask about the project being created.
pub(crate) const PLACEHOLDER: &str = "{{name}}";

/// The other thing: which package manager's commands the docs name. A single
/// placeholder, because `install` and `run <script>` spell the same under
/// npm, bun, pnpm and yarn — `bun test` excepted, which is Bun's own runner
/// rather than the manifest's script, so READMEs always write `run test`.
pub(crate) const PM_PLACEHOLDER: &str = "{{pm}}";

/// The project's name, from the directory it is being created in.
///
/// Sanitised, because this lands in `package.json` and npm's rules are narrower
/// than a filesystem's: a directory called `My App` would otherwise produce a
/// manifest that every package manager rejects, on the first command the user
/// runs.
pub(crate) fn package_name(target: &Path) -> String {
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
    for entry in &library().templates {
        let name = entry.name;
        let description = &entry.manifest.description;
        let count =
            |answers: &[(String, String)]| entry.files(answers).map_or(0, |files| files.len());
        // A template with modes has no single file count — the modes have one
        // each — so it names them instead, on the lines under it. Every other
        // option counts at its default.
        match entry.option("mode") {
            None => report.push_str(&format!(
                "  {name:<10} {description} ({} files)\n",
                count(&entry.answers(&[]))
            )),
            Some(modes) => {
                report.push_str(&format!("  {name:<10} {description}\n"));
                for mode in &modes.choices {
                    let files = count(&entry.answers(&[("mode", &mode.value)]));
                    let flag = format!("--mode={}", mode.value);
                    report.push_str(&format!(
                        "  {:<10}   {flag:<18} {} ({files} files)\n",
                        "", mode.description
                    ));
                }
            }
        }
        // The rest of its options, as their flags: this list is where a
        // template's options are documented, since they are the template's.
        let flags: Vec<String> = entry
            .manifest
            .options
            .iter()
            .filter(|option| option.name != "mode")
            .map(|option| match option.kind {
                crate::template::Kind::Confirm => {
                    format!("--{0}, --no-{0}", option.name)
                }
                crate::template::Kind::Choice => format!(
                    "--{}={}",
                    option.name,
                    option
                        .choices
                        .iter()
                        .map(|c| c.value.as_str())
                        .collect::<Vec<_>>()
                        .join("|")
                ),
            })
            .collect();
        if !flags.is_empty() {
            report.push_str(&format!("  {:<10}   {}\n", "", flags.join("  ")));
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The stylesheet the page templates share, and Tailwind replaces.
    const STYLESHEET: &str = "src/styles.css";

    /// The files of one template in one mode, as `create` would write them.
    fn resolved(template: &str, mode: Option<&str>) -> Vec<String> {
        plain_written(template, mode)
            .into_iter()
            .map(|(path, _)| path)
            .collect()
    }

    #[test]
    fn the_templates_are_in_the_binary() {
        let paths = resolved("react", Some("static"));

        for expected in [
            "package.json",
            "esdev.json",
            "index.html",
            "src/App.tsx",
            "src/entry.client.tsx",
            "src/styles.css",
            "_gitignore",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} is not in the template: {paths:?}"
            );
        }

        let micro_ui = resolved("micro-ui", None);
        for expected in [
            "package.json",
            "esdev.json",
            "index.html",
            "src/main.ts",
            "src/styles.css",
        ] {
            assert!(
                micro_ui.iter().any(|path| path == expected),
                "{expected} is not in the micro-ui template: {micro_ui:?}"
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
        assert!(
            !statik.iter().any(|path| path == "src/server.tsx"),
            "a static project has no server: {statik:?}"
        );

        assert!(full.contains(&"src/server.tsx".to_string()));
        assert!(
            !full.iter().any(|path| path == "src/prerender.tsx"),
            "a fullstack project renders per request, so it prerenders nothing: {full:?}"
        );

        // The shared half really is shared, rather than duplicated per mode.
        for both in ["src/App.tsx", "index.html", "src/render.tsx"] {
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
                !paths
                    .iter()
                    .any(|path| path.starts_with(crate::template::OPTION_PREFIX)),
                "{mode} leaked an _option/ path: {paths:?}"
            );
        }
    }

    /// Every mode must produce a project, which starts with the two files that
    /// say what it is and how to build it.
    #[test]
    fn every_mode_of_every_template_is_a_whole_project() {
        for entry in &library().templates {
            let Some(modes) = entry.option("mode") else {
                continue;
            };
            for mode in &modes.choices {
                let paths = resolved(entry.name, Some(&mode.value));
                for required in ["package.json", "esdev.json", "README.md"] {
                    assert!(
                        paths.iter().any(|path| path == required),
                        "{} ({}) has no {required}: {paths:?}",
                        entry.name,
                        mode.value
                    );
                }
            }
        }
    }

    /// The answers as asked: on a terminal menus appear, and Esc steps back.
    const ASK: Ask = Ask {
        scripted: false,
        back: false,
    };
    /// `-y`, or away from a terminal: menus never appear, defaults win.
    const SCRIPTED: Ask = Ask {
        scripted: true,
        back: false,
    };

    /// Naming a mode a template does not have is refused rather than ignored.
    #[test]
    fn a_mode_that_is_not_one_is_refused() {
        let react = library().get("react").expect("react");
        let mode = react.option("mode").expect("react has modes");
        assert!(resolve_option("react", mode, Some("ssr"), ASK).is_err());
        assert_eq!(
            resolve_option("react", mode, Some("static"), ASK).unwrap(),
            Some("static".to_string())
        );
        let flag = OptionFlag {
            name: "mode".to_string(),
            value: "static".to_string(),
            spelled: "--mode=static".to_string(),
        };
        assert!(refuse_foreign_options(library(), react, std::slice::from_ref(&flag)).is_ok());
        let api = library().get("api").expect("api");
        let refused = refuse_foreign_options(library(), api, &[flag]).unwrap_err();
        assert!(refused.contains("has no mode"), "{refused}");
        assert!(refused.contains("Only react does"), "{refused}");
    }

    /// `-y` takes every default without asking: no menus, even where a
    /// terminal would have shown them. A test never has a terminal, so the
    /// scripted path is what pins this — the interactive one cannot run here.
    #[test]
    fn yes_resolves_every_default() {
        let fallback = |template: &str, option: &str| {
            let entry = library().get(template).expect("the template");
            resolve_option(
                template,
                entry.option(option).expect("the option"),
                None,
                SCRIPTED,
            )
            .unwrap()
            .expect("an answer")
        };
        assert_eq!(fallback("react", "mode"), "static");
        assert_eq!(fallback("spa", "language"), "js");
        // No default: an unattended run adds nothing (D135).
        assert_eq!(fallback("spa", "styling"), "css");
        assert_eq!(fallback("docs", "blog"), "no");
        assert!(matches!(
            resolve_manager(None, SCRIPTED),
            Ok(ManagerChoice::Chosen(manager)) if manager == DEFAULT_MANAGER
        ));
    }

    /// The manager question resolves like the others: a flag names it however
    /// spelled, an unknown name is refused with the known set, and away from
    /// a terminal the default wins. It applies to every template, so there is
    /// no refusal for a template that does not take it.
    #[test]
    fn the_manager_resolves_from_flag_or_default() {
        assert!(matches!(
            resolve_manager(Some("BUN"), ASK),
            Ok(ManagerChoice::Chosen(manager)) if manager == "bun"
        ));
        assert!(matches!(
            resolve_manager(Some("pnpm"), ASK),
            Ok(ManagerChoice::Chosen(manager)) if manager == "pnpm"
        ));
        let err = resolve_manager(Some("cargo"), ASK).expect_err("cargo is not one");
        assert!(
            err.contains("npm, bun, pnpm, yarn"),
            "the error names the known set: {err}"
        );
        assert!(matches!(
            resolve_manager(None, SCRIPTED),
            Ok(ManagerChoice::Chosen(manager)) if manager == "npm"
        ));
    }

    /// The menu marks what the machine has and disables the rest, so the
    /// answer is always something the install step can run. A flag still names
    /// anything known — the detection only narrows the interactive menu.
    #[test]
    fn the_manager_menu_disables_what_is_not_installed() {
        use crate::install::MANAGERS;
        let npm = MANAGERS.iter().find(|m| m.name == "npm").copied().unwrap();
        let entries = manager_entries(&[npm]);
        assert_eq!(entries.len(), MANAGERS.len());
        let npm_entry = entries.iter().find(|e| e.name == "npm").unwrap();
        assert!(!npm_entry.disabled);
        assert!(npm_entry.description.contains("installed"));
        assert!(!npm_entry.description.contains("not installed"));
        for name in ["bun", "pnpm", "yarn"] {
            let entry = entries.iter().find(|e| e.name == name).unwrap();
            assert!(entry.disabled, "{name} is not here, so it cannot be picked");
            assert!(
                entry.description.contains("not installed"),
                "{name} says why it cannot be picked: {}",
                entry.description
            );
        }
        // Nothing detected leaves every row enabled rather than trapping the
        // run with nothing to pick.
        for entry in manager_entries(&[]) {
            assert!(!entry.disabled, "{} stays pickable", entry.name);
        }
    }

    /// The menu starts on the default when it can be chosen, else on the first
    /// thing that can be.
    #[test]
    fn the_manager_menu_starts_on_something_choosable() {
        use crate::install::MANAGERS;
        let bun = MANAGERS.iter().find(|m| m.name == "bun").copied().unwrap();
        let pnpm = MANAGERS.iter().find(|m| m.name == "pnpm").copied().unwrap();
        let entries = manager_entries(&[bun, pnpm]);
        let preselect = manager_preselect(&entries, DEFAULT_MANAGER);
        assert_eq!(
            preselect,
            Some(1),
            "npm is gone, so bun is first: {preselect:?}"
        );
        let npm = MANAGERS.iter().find(|m| m.name == "npm").copied().unwrap();
        let entries = manager_entries(&[npm, bun]);
        assert_eq!(manager_preselect(&entries, DEFAULT_MANAGER), Some(0));
    }

    /// The install step is a yes/no for the project's manager exactly when
    /// that manager is here; otherwise it lists what is, as before.
    #[test]
    fn the_install_step_confirms_the_chosen_manager_when_it_is_here() {
        use crate::install::MANAGERS;
        let npm = MANAGERS.iter().find(|m| m.name == "npm").copied().unwrap();
        let bun = MANAGERS.iter().find(|m| m.name == "bun").copied().unwrap();
        assert!(install_confirms(Some(npm), &[npm, bun]));
        assert!(!install_confirms(Some(bun), &[npm]));
        assert!(!install_confirms(None, &[npm]));
        assert!(!install_confirms(Some(npm), &[]));
    }

    /// Both placeholders render, and text without them is untouched.
    #[test]
    fn placeholders_render() {
        assert_eq!(
            render("# {{name}}\n\n```sh\n{{pm}} install\n```\n", "shop", "pnpm"),
            "# shop\n\n```sh\npnpm install\n```\n"
        );
        assert_eq!(render("plain", "shop", "pnpm"), "plain");
    }

    #[test]
    fn package_versions_resolve_into_the_manifest() {
        let mut versions = HashMap::new();
        versions.insert("@types/react".to_string(), "19.3.0".to_string());
        let manifest = format!(
            "{{\"devDependencies\":{{\"@types/react\":\"{}\"}}}}",
            registry_placeholder("@types/react")
        );
        assert_eq!(
            render_registry_versions(manifest, &versions).expect("resolved"),
            r#"{"devDependencies":{"@types/react":"19.3.0"}}"#
        );
        assert!(render_registry_versions(registry_placeholder("typescript"), &versions).is_err());
    }

    #[test]
    fn a_stable_latest_is_pinned_as_it_is() {
        let metadata = serde_json::json!({
            "dist-tags": { "latest": "1.4.31", "next": "2.0.0-beta.32" },
            "versions": { "1.4.31": {}, "2.0.0-beta.32": {} },
        });
        assert_eq!(scaffold_version(&metadata).as_deref(), Some("1.4.31"));
    }

    /// elysia, November 2026: a release candidate tagged `latest` by mistake.
    /// The newest stable is pinned instead — by version order, not by which
    /// was published last, and never a deprecated one.
    #[test]
    fn a_prerelease_latest_pins_the_newest_stable_release() {
        let metadata = serde_json::json!({
            "dist-tags": { "latest": "2.0.0-rc.4", "rc": "2.0.0-rc.3" },
            "versions": {
                "1.4.9": {},
                "1.4.31": {},
                "1.4.30": {},
                "1.5.0": { "deprecated": "published by mistake" },
                "2.0.0-rc.3": {},
                "2.0.0-rc.4": {},
            },
        });
        assert_eq!(scaffold_version(&metadata).as_deref(), Some("1.4.31"));
    }

    /// With no stable release at all there is nothing better to pin.
    #[test]
    fn a_package_with_only_prereleases_keeps_its_latest() {
        let metadata = serde_json::json!({
            "dist-tags": { "latest": "0.1.0-alpha.2" },
            "versions": { "0.1.0-alpha.1": {}, "0.1.0-alpha.2": {} },
        });
        assert_eq!(
            scaffold_version(&metadata).as_deref(),
            Some("0.1.0-alpha.2")
        );
        assert_eq!(
            scaffold_version(&serde_json::json!({ "versions": {} })),
            None
        );
    }

    #[test]
    fn stable_versions_are_three_numbers_without_a_prerelease() {
        assert_eq!(stable("1.4.31"), Some((1, 4, 31)));
        assert_eq!(stable("1.4.31+build.7"), Some((1, 4, 31)));
        assert_eq!(stable("2.0.0-rc.4"), None);
        assert_eq!(stable("2.0.0-rc.4+build"), None);
        assert_eq!(stable("1.4"), None);
        assert_eq!(stable("1.4.x"), None);
        assert_eq!(stable("1.4.31.0"), None);
    }

    #[test]
    fn registry_placeholders_cover_every_embedded_dependency() {
        for (template, files) in TEMPLATES {
            for (path, contents) in *files {
                if !path.ends_with("package.json") {
                    continue;
                }
                let manifest: serde_json::Value = serde_json::from_slice(contents)
                    .unwrap_or_else(|error| panic!("{template}/{path}: {error}"));
                for section in ["dependencies", "devDependencies", "peerDependencies"] {
                    let Some(dependencies) = manifest.get(section).and_then(|v| v.as_object())
                    else {
                        continue;
                    };
                    for (package, version) in dependencies {
                        assert_eq!(
                            version.as_str(),
                            Some(registry_placeholder(package).as_str()),
                            "{template}/{path} {section}.{package} must resolve at scaffold time"
                        );
                    }
                }
            }
        }
        assert_eq!(
            registry_url(DEFAULT_REGISTRY, "@types/react"),
            "https://registry.npmjs.org/@types%2Freact"
        );

        let docs = afw_written("docs", "ts", None, Some(false));
        assert_eq!(
            registry_packages(&docs).expect("valid placeholders"),
            BTreeSet::from([
                "@opentf/esdev-plugin-afw".to_string(),
                "@opentf/esrun-types".to_string(),
                "@opentf/afw".to_string(),
                "@opentf/afw-cli".to_string(),
                "@opentf/afw-docs".to_string(),
                "tailwindcss".to_string(),
                "typescript".to_string(),
            ])
        );
    }

    #[test]
    fn registry_lookups_use_three_exponential_backoffs() {
        assert_eq!(registry_backoff(0, true), Some(Duration::from_millis(500)));
        assert_eq!(registry_backoff(1, true), Some(Duration::from_secs(1)));
        assert_eq!(registry_backoff(2, true), Some(Duration::from_secs(2)));
        assert_eq!(registry_backoff(3, true), None);
        assert_eq!(registry_backoff(0, false), None);
    }

    /// The docs name the chosen manager, never a fixed one: every README and
    /// every docs prose file carries `{{pm}}`, and no hardcoded install verb
    /// survives beside it. (`bun test` is deliberately absent everywhere — it
    /// is Bun's own runner, not the manifest's script, so docs write
    /// `run test`.)
    #[test]
    fn docs_name_the_manager_not_a_fixed_one() {
        const VERBS: &[&str] = &[
            "npm install",
            "npm run",
            "npm test",
            "npm start",
            "pnpm install",
            "pnpm run",
            "pnpm test",
            "pnpm start",
            "bun install",
            "bun run",
            "yarn install",
            "yarn run",
            "npx ",
        ];
        for (template, files) in TEMPLATES {
            for (path, contents) in *files {
                let is_readme = *path == "README.md";
                let is_prose = path.ends_with(".mdx");
                let is_manifest = path.ends_with("package.json");
                if !(is_readme || is_prose || is_manifest) {
                    continue;
                }
                let text = String::from_utf8_lossy(contents);
                for verb in VERBS {
                    assert!(
                        !text.contains(verb),
                        "{template}/{path} names a fixed manager: {verb}"
                    );
                }
                if is_readme {
                    assert!(
                        text.contains(PM_PLACEHOLDER),
                        "{template}/{path} names no manager at all"
                    );
                }
            }
        }
    }

    /// The ignores agree: esdev builds stage beside the project, so every
    /// esdev-built template ignores the staging prefix — and no template
    /// ignores a directory no tool here writes (`target/`, `.cache/`).
    #[test]
    fn gitignores_agree_on_what_the_toolchain_leaves_behind() {
        for (template, files) in TEMPLATES {
            let Some((_, contents)) = files.iter().find(|(path, _)| *path == "_gitignore") else {
                panic!("{template} ships no ignore file");
            };
            let text = String::from_utf8_lossy(contents);
            assert!(text.contains("node_modules/"), "{template}: dependencies");
            assert!(text.contains("dist/"), "{template}: build output");
            assert!(
                !text.contains("target/") && !text.contains(".cache/"),
                "{template}: nothing here writes those"
            );
            if library()
                .get(template)
                .is_some_and(|entry| entry.manifest.group.as_deref() == Some("AFW"))
            {
                continue;
            }
            assert!(
                text.contains(".esdev-build-"),
                "{template}: a killed build leaves staging behind"
            );
        }
    }

    /// Every README points at the file to edit: a backticked path under
    /// `src/` or `app/`, not just commands to run. `react` is two projects
    /// with two READMEs, so both modes are checked.
    #[test]
    fn readmes_point_at_the_file_to_edit() {
        let mut checked = 0;
        for (template, mode) in [
            ("api", None),
            ("lib", None),
            ("vanilla", None),
            ("micro-ui", None),
            ("react", Some("static")),
            ("react", Some("fullstack")),
            ("spa", None),
            ("fullstack", None),
            ("docs", None),
            ("library", None),
        ] {
            let files = plain_written(template, mode);
            let (_, readme) = files
                .iter()
                .find(|(path, _)| path == "README.md")
                .unwrap_or_else(|| panic!("{template} {mode:?} has no README"));
            let readme = String::from_utf8_lossy(readme);
            assert!(
                readme.contains("`src/") || readme.contains("`app/"),
                "{template} {mode:?}: names no file to edit"
            );
            checked += 1;
        }
        assert_eq!(checked, 10, "a template escaped the list");
    }

    /// No scaffolded page ships someone else's brand: every document titles
    /// itself with the project being created.
    #[test]
    fn documents_title_themselves_with_the_project() {
        const BRANDS: &[&str] = &["My Docs", "My Project", "AFW App"];
        let mut documents = 0;
        for (template, files) in TEMPLATES {
            for (path, contents) in *files {
                let text = String::from_utf8_lossy(contents);
                for brand in BRANDS {
                    assert!(
                        !text.contains(brand),
                        "{template}/{path} ships a leftover brand: {brand}"
                    );
                }
                if *path == "index.html" {
                    documents += 1;
                    assert!(
                        text.contains(PLACEHOLDER),
                        "{template}/{path} hardcodes its title"
                    );
                }
            }
        }
        assert!(documents > 0, "no document checked anything");
    }

    /// Esc steps back to the nearest earlier step that showed a menu — past
    /// steps answered by flags, and away from the first step, it cancels.
    #[test]
    fn esc_retreats_to_the_last_menu() {
        use Step::{Manager, Option as Opt, Template};
        let steps = [Template, Opt(0), Opt(1), Opt(2), Manager];
        // Nothing behind: out.
        assert_eq!(step_back_index(&steps, 0, &[]), None);
        assert_eq!(step_back_index(&steps, 1, &[]), None);
        // The nearest menu behind, skipping steps that never asked.
        assert_eq!(
            step_back_index(&steps, 1, &[Template]),
            Some(0),
            "Esc at the first option returns to the template menu"
        );
        assert_eq!(
            step_back_index(&steps, 3, &[Template, Opt(1)]),
            Some(2),
            "an option answered by a flag never asked, so it is skipped"
        );
        assert_eq!(
            step_back_index(&steps, 2, &[Template, Opt(0), Opt(1)]),
            Some(1),
            "a step's own earlier visit does not count as behind it"
        );
        assert_eq!(
            step_back_index(&steps, 4, &[Template, Opt(2)]),
            Some(3),
            "the manager is the last question"
        );
        // The questions are the template's own, between the two fixed ones.
        let docs = library().get("docs").expect("docs");
        assert_eq!(steps_for(Some(docs)), [Template, Opt(0), Opt(1), Manager]);
        assert_eq!(steps_for(None), [Template, Manager]);
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

    /// The type definitions track the runtime. The scaffolder resolves the
    /// registry's current release and writes that exact version into the new
    /// project's manifest.
    #[test]
    fn the_types_package_is_never_pinned_in_a_template() {
        for (name, files) in TEMPLATES {
            for (path, contents) in *files {
                if !path.ends_with("package.json") {
                    continue;
                }
                let manifest: serde_json::Value = serde_json::from_slice(contents)
                    .unwrap_or_else(|e| panic!("{name}/{path}: {e}"));
                let Some(version) = manifest
                    .get("devDependencies")
                    .and_then(|deps| deps.get("@opentf/esrun-types"))
                    .and_then(serde_json::Value::as_str)
                else {
                    continue;
                };
                assert_eq!(
                    version,
                    registry_placeholder("@opentf/esrun-types"),
                    "{name}/{path} must resolve the type definitions at scaffold time"
                );
            }
        }
    }

    /// Every template carries them, so a new one that forgets is caught here
    /// rather than by whoever scaffolds it and finds no `runtime:` completions.
    #[test]
    fn every_template_depends_on_the_type_definitions() {
        for (name, files) in TEMPLATES {
            let found = files.iter().any(|(path, contents)| {
                path.ends_with("package.json")
                    && String::from_utf8_lossy(contents).contains("@opentf/esrun-types")
            });
            assert!(found, "{name} has no @opentf/esrun-types dev dependency");
        }
    }

    /// The templates esdev itself can build and test. A fresh scaffold's
    /// `npm test` and `npm run typecheck` must mean something, so each one
    /// ships a `tsconfig.json` and at least one test file.
    ///
    /// Scoped to this repository's own stack: the AFW starters are covered
    /// by their own tests.
    #[test]
    fn esdev_templates_ship_a_config_and_a_test() {
        for template in ["api", "react", "vanilla", "micro-ui", "lib"] {
            // `react` is two projects; the shared files (with the config and
            // the tests) are in both, so either mode proves the point.
            let paths = resolved(template, None);
            assert!(
                paths.iter().any(|path| path == "tsconfig.json"),
                "{template} ships no tsconfig.json, so `npm run typecheck` checks nothing"
            );
            assert!(
                paths.iter().any(|path| path.contains(".test.")),
                "{template} ships no test, so a fresh `npm test` fails"
            );
        }
    }

    /// The check runs through esdev, not bare `tsc`: `esdev typecheck` uses the
    /// project's own compiler through its package manager and reports an
    /// actionable error when it is not installed, while a bare `tsc` is a
    /// command the scaffold never ensured was on `PATH`.
    ///
    /// `react` is checked in both modes since each carries its own manifest.
    #[test]
    fn esdev_templates_typecheck_through_esdev() {
        for (template, mode) in [
            ("api", None),
            ("lib", None),
            ("vanilla", None),
            ("micro-ui", None),
            ("react", Some("static")),
            ("react", Some("fullstack")),
        ] {
            let files = plain_written(template, mode);
            let (_, manifest) = files
                .iter()
                .find(|(path, _)| path == "package.json")
                .unwrap_or_else(|| panic!("{template} {mode:?} has no package.json"));
            let manifest: serde_json::Value = serde_json::from_slice(manifest)
                .unwrap_or_else(|e| panic!("{template} {mode:?}: {e}"));
            assert_eq!(
                manifest["scripts"]["typecheck"],
                serde_json::json!("esdev typecheck"),
                "{template} {mode:?}: typecheck bypasses esdev"
            );
        }
    }

    /// A scaffolder must not choose the user's license. Caught here rather
    /// than by whoever publishes and finds the registry took them at their
    /// template's word.
    #[test]
    fn no_template_names_a_license() {
        for (template, files) in TEMPLATES {
            for (path, contents) in *files {
                if !path.ends_with("package.json") {
                    continue;
                }
                let manifest: serde_json::Value = serde_json::from_slice(contents)
                    .unwrap_or_else(|e| panic!("{template}/{path}: {e}"));
                assert!(
                    manifest.get("license").is_none(),
                    "{template}/{path} chooses a license for the user"
                );
            }
        }
    }

    #[test]
    fn the_list_names_every_embedded_template() {
        let listed = list();
        for (name, _) in TEMPLATES {
            assert!(listed.contains(name), "{name} is missing from:\n{listed}");
        }
    }

    /// The template menu shows a description beside every name; one without
    /// reads as an option the scaffolder forgot to explain.
    #[test]
    fn every_template_has_a_description() {
        for entry in &library().templates {
            assert!(
                !entry.manifest.description.is_empty(),
                "{} has no description",
                entry.name
            );
            assert!(
                !entry.manifest.label.is_empty(),
                "{} has no label",
                entry.name
            );
            for option in &entry.manifest.options {
                assert!(
                    !option.question.is_empty(),
                    "{} {}: no question",
                    entry.name,
                    option.name
                );
                for choice in &option.choices {
                    assert!(
                        !choice.label.is_empty() && !choice.description.is_empty(),
                        "{} {} {}: unexplained",
                        entry.name,
                        option.name,
                        choice.value
                    );
                }
            }
        }
        // Every template's manifest and the catalog read, and agree.
        assert!(Library::embedded().is_ok());
    }

    /// Menus show the proper form while flags stay lowercase: `SPA`, not
    /// `spa` — and what is chosen still resolves to the flag spelling.
    #[test]
    fn menus_show_proper_names_for_lowercase_flags() {
        let label = |template: &str| {
            library()
                .get(template)
                .expect("the template")
                .manifest
                .label
                .clone()
        };
        for (flag, shown) in [
            ("api", "API"),
            ("docs", "Docs"),
            ("fullstack", "FullStack"),
            ("lib", "Library"),
            ("library", "Component Library"),
            ("micro-ui", "Micro-UI"),
            ("react", "React"),
            ("spa", "SPA"),
            ("vanilla", "Vanilla"),
        ] {
            assert_eq!(label(flag), shown, "{flag} displays wrong");
        }
        let choice = |template: &str, option: &str, value: &str| {
            library()
                .get(template)
                .and_then(|entry| entry.option(option))
                .and_then(|option| option.choice(value))
                .expect("the choice")
                .label
                .clone()
        };
        assert_eq!(choice("react", "mode", "static"), "Static");
        assert_eq!(choice("spa", "language", "js"), "JS");
        assert_eq!(choice("spa", "language", "ts"), "TS");
        assert_eq!(choice("spa", "styling", "css"), "CSS");
        assert_eq!(choice("vanilla", "styling", "tailwind"), "Tailwind");
        assert_eq!(library().catalog.groups[0].label, "AFW");
    }

    /// The first menu holds this repository's templates plus the AFW group —
    /// four more lines there is the flat list this grouping replaces.
    #[test]
    fn the_template_menu_groups_the_afw_starters() {
        const AFW: [&str; 4] = ["spa", "fullstack", "docs", "library"];
        let top = template_menu_top(library());
        let names: Vec<&str> = top.iter().map(|choice| choice.name).collect();
        for afw in AFW {
            assert!(!names.contains(&afw), "{afw} leaked into the first menu");
        }
        assert_eq!(names.last(), Some(&"AFW"));
        for choice in &top {
            assert!(
                !choice.description.is_empty(),
                "the menu shows an unexplained entry"
            );
        }
        // Behind the group: the four starters, simplest first.
        let group = &library().catalog.groups[0];
        let names: Vec<&str> = template_menu_group(library(), group)
            .iter()
            .map(|choice| choice.name)
            .collect();
        assert_eq!(names, AFW);
    }

    /// The AFW starter set is embedded whole: apps, docs with its demo
    /// blog on disk, and the library without anything Bun-only.
    #[test]
    fn the_afw_templates_are_in_the_binary() {
        for expected in [
            "package.json",
            "index.html",
            "jsconfig.json",
            "app/layout.jsx",
            "app/page.jsx",
            "app/global.css",
            "README.md",
            "_gitignore",
        ] {
            for template in ["spa", "fullstack"] {
                assert!(
                    resolved(template, None).iter().any(|path| path == expected),
                    "{template} has no {expected}"
                );
            }
        }
        // With its demo blog, which is opt-in.
        let docs: Vec<String> = written("docs", &[("blog", "yes")])
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        for expected in [
            "package.json",
            "afw.config.js",
            "app/docs/page.mdx",
            "app/docs/_meta.js",
            "app/blog/page.jsx",
            "app/blog/hello-world/page.mdx",
        ] {
            assert!(
                docs.iter().any(|path| path == expected),
                "docs has no {expected}"
            );
        }
        let library = resolved("library", None);
        for expected in [
            "package.json",
            "index.js",
            "src/Counter.jsx",
            "tests/counter.test.js",
            "jsconfig.json",
        ] {
            assert!(
                library.iter().any(|path| path == expected),
                "library has no {expected}"
            );
        }
        for forbidden in ["bunfig.toml", "test-setup.js"] {
            assert!(
                !library.iter().any(|path| path == forbidden),
                "library embeds {forbidden}, which needs Bun to run"
            );
        }
    }

    /// The AFW answers resolved from flags, without a terminal to ask on.
    /// Unit tests never see a TTY, so `None` here is the unattended default.
    #[test]
    fn the_afw_axes_default_and_refuse() {
        let answer = |template: &str, option: &str, asked: Option<&str>| {
            let entry = library().get(template).expect("the template");
            resolve_option(
                template,
                entry.option(option).expect("the option"),
                asked,
                ASK,
            )
        };
        // Unit tests never see a terminal, so an unsaid option is its fallback.
        assert_eq!(answer("spa", "language", None).unwrap(), Some("js".into()));
        assert_eq!(answer("spa", "styling", None).unwrap(), Some("css".into()));
        assert_eq!(answer("docs", "blog", None).unwrap(), Some("no".into()));
        assert_eq!(
            answer("fullstack", "language", Some("ts")).unwrap(),
            Some("ts".into())
        );
        assert_eq!(
            answer("docs", "blog", Some("no")).unwrap(),
            Some("no".into())
        );
        let elm = answer("spa", "language", Some("elm")).unwrap_err();
        assert!(
            elm.contains("no language elm") && elm.contains("js, ts"),
            "{elm}"
        );

        // An option the template does not ask is refused, naming who asks it.
        let refused = |template: &str, name: &str, value: &str, spelled: &str| {
            let flag = OptionFlag {
                name: name.into(),
                value: value.into(),
                spelled: spelled.into(),
            };
            refuse_foreign_options(library(), library().get(template).expect("t"), &[flag])
        };
        assert!(refused("api", "language", "elm", "--language=elm").is_err());
        assert!(
            refused("docs", "styling", "css", "--styling=css")
                .unwrap_err()
                .contains("has no styling")
        );
        assert!(
            refused("spa", "blog", "yes", "--blog")
                .unwrap_err()
                .contains("Only docs does")
        );
        assert!(
            refused("fullstack", "blog", "no", "--no-blog")
                .unwrap_err()
                .contains("--no-blog")
        );
        assert!(refused("spa", "language", "ts", "--language=ts").is_ok());
        // A flag no template knows says so, and where create's own are.
        let typo = refused("api", "forse", "yes", "--forse").unwrap_err();
        assert!(typo.contains("No template has a forse option"), "{typo}");
    }

    /// The AFW answers applied, as `create` would write them.
    fn afw_written(
        template: &str,
        language: &str,
        styling: Option<&str>,
        blog: Option<bool>,
    ) -> Vec<(String, Vec<u8>)> {
        let mut overrides = vec![("language", language)];
        if let Some(styling) = styling {
            overrides.push(("styling", styling));
        }
        if let Some(blog) = blog {
            overrides.push(("blog", if blog { "yes" } else { "no" }));
        }
        written(template, &overrides)
    }

    /// What `create` writes for `template` with `overrides` and every other
    /// option at its fallback.
    fn written(template: &str, overrides: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        let entry = library().get(template).expect("the template is embedded");
        entry
            .files(&entry.answers(overrides))
            .expect("the template assembles")
    }

    fn afw_text(files: &[(String, Vec<u8>)], path: &str) -> String {
        let (_, contents) = files
            .iter()
            .find(|(written, _)| written == path)
            .unwrap_or_else(|| panic!("{path} was not written"));
        String::from_utf8(contents.clone()).expect("template text is UTF-8")
    }

    /// TypeScript is renames plus patches: sources move, references follow,
    /// layouts type their props, and the config arrives beside them.
    #[test]
    fn typescript_renames_patches_and_configures() {
        let files = afw_written("spa", "ts", Some("css"), None);
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"app/page.tsx"));
        assert!(paths.contains(&"app/layout.tsx"));
        assert!(paths.contains(&"tests/counter.test.ts"));
        assert!(paths.contains(&"tsconfig.json"));
        assert!(paths.contains(&"app/afw-env.d.ts"));
        assert!(!paths.contains(&"app/page.jsx"));
        assert!(!paths.contains(&"tests/counter.test.js"));
        assert!(!paths.contains(&"jsconfig.json"));

        let layout = afw_text(&files, "app/layout.tsx");
        assert!(
            layout.contains("({ children }: { children: unknown })"),
            "the layout types its props: {layout}"
        );
        let page = afw_text(&files, "app/page.tsx");
        assert!(
            page.contains("app/page.tsx"),
            "references follow the rename: {page}"
        );
        let readme = afw_text(&files, "README.md");
        assert!(
            readme.contains("app/page.tsx"),
            "the README names the file that exists: {readme}"
        );
        let env = afw_text(&files, "app/afw-env.d.ts");
        assert!(
            env.contains("declare const $state"),
            "macros declared: {env}"
        );

        let tsconfig: serde_json::Value =
            serde_json::from_str(&afw_text(&files, "tsconfig.json")).expect("valid JSON");
        assert_eq!(
            tsconfig["include"],
            serde_json::json!(["app", "tests"]),
            "apps include their tree and their tests"
        );
        assert!(
            tsconfig["compilerOptions"].get("allowJs").is_none(),
            "a TypeScript app is not half JavaScript"
        );
    }

    /// The docs tree keeps its config include, and the library publishes its
    /// entry — in TypeScript, the TypeScript entry.
    #[test]
    fn typescript_configures_docs_and_library() {
        let docs = afw_written("docs", "ts", None, Some(false));
        let tsconfig: serde_json::Value =
            serde_json::from_str(&afw_text(&docs, "tsconfig.json")).expect("valid JSON");
        assert_eq!(
            tsconfig["include"],
            serde_json::json!(["app", "afw.config.js"])
        );

        let library = afw_written("library", "ts", None, None);
        let paths: Vec<&str> = library.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"index.ts"));
        assert!(paths.contains(&"src/Counter.tsx"));
        assert!(paths.contains(&"afw-env.d.ts"));
        assert!(!paths.contains(&"index.js"));
        let manifest: serde_json::Value =
            serde_json::from_str(&afw_text(&library, "package.json")).expect("valid JSON");
        assert_eq!(manifest["exports"]["."], serde_json::json!("./index.ts"));
        assert_eq!(manifest["files"], serde_json::json!(["index.ts", "src"]));
        let counter = afw_text(&library, "src/Counter.tsx");
        assert!(
            counter.contains("{ initial = 0 }: { initial?: number }"),
            "the counter types its prop: {counter}"
        );
        // The esdev test reads the manifest, so it needs no patch per mode.
        let test = afw_text(&library, "tests/counter.test.ts");
        assert!(test.contains("runtime:test"));
        assert!(!test.contains("bun:test"));
    }

    /// The four AFW starters are native esdev projects: a project-local
    /// esdev.json naming the framework plugin in its rendering mode, and
    /// scripts that run on esdev (or, for a fullstack deploy, esrun).
    #[test]
    fn afw_templates_are_native_esdev_projects() {
        for (template, mode, styling, blog) in [
            ("spa", "spa", Some("css"), None),
            ("fullstack", "ssr", Some("css"), None),
            ("docs", "ssg", None, Some(true)),
        ] {
            let files = afw_written(template, "js", styling, blog);
            let config: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "esdev.json")).expect("valid esdev.json");
            let plugins = config["plugins"].as_array().expect("a plugins array");
            assert!(
                plugins.iter().any(|plugin| {
                    plugin.get("module").and_then(|m| m.as_str())
                        == Some("@opentf/esdev-plugin-afw")
                        && plugin
                            .get("options")
                            .and_then(|o| o.get("mode"))
                            .and_then(|m| m.as_str())
                            == Some(mode)
                }),
                "{template}: the framework plugin with mode {mode}"
            );
            let manifest: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "package.json"))
                    .expect("valid package.json");
            for (script, command) in manifest["scripts"].as_object().expect("scripts") {
                let command = command.as_str().unwrap_or_default();
                assert!(
                    command.starts_with("esdev ") || command.starts_with("esrun "),
                    "{template}: {script} runs outside esdev and esrun: {command}"
                );
            }
            assert!(
                manifest["devDependencies"]
                    .get("@opentf/esdev-plugin-afw")
                    .is_some(),
                "{template}: the plugin is installed"
            );
            // The client entry mounts the generated route map, with the
            // generated loader patterns beside it.
            let entry = afw_text(&files, "entry.js");
            assert!(
                entry.contains("loaderRoutes") && entry.contains("mountApp"),
                "{template}: the entry mounts generated routes: {entry}"
            );
        }
        // The library compiles without route discovery and tests in the DOM.
        let files = afw_written("library", "js", None, None);
        let config: serde_json::Value =
            serde_json::from_str(&afw_text(&files, "esdev.json")).expect("valid esdev.json");
        let options = &config["plugins"][0]["options"];
        assert_eq!(options["target"], serde_json::json!("csr"));
        assert_eq!(options["routes"], serde_json::json!(false));
        assert_eq!(
            config["test"]["setup"],
            serde_json::json!(["@opentf/afw-test/setup"])
        );
    }

    /// In TypeScript the server-function sample is TypeScript too, typed the
    /// way the Vite scaffolder types it, and its importers follow the rename.
    #[test]
    fn fullstack_server_functions_are_typed_in_typescript() {
        let files = afw_written("fullstack", "ts", Some("css"), None);
        let functions = afw_text(&files, "app/todos/todos.server.ts");
        assert!(functions.contains("const todos: string[]"), "{functions}");
        assert!(
            functions.contains("({ title }: { title: string })"),
            "{functions}"
        );
        let page = afw_text(&files, "app/todos/page.tsx");
        assert!(page.contains("from \"./todos.server.ts\""), "{page}");
        assert!(page.contains("event: SubmitEvent"), "{page}");
        assert!(page.contains("as HTMLInputElement"), "{page}");
        let loader = afw_text(&files, "app/todos/loader.ts");
        assert!(
            loader.contains("{ locals }: { locals: Record<string, unknown> }"),
            "{loader}"
        );
        assert!(
            !files
                .iter()
                .any(|(name, _)| name.ends_with("todos.server.js")),
            "the JavaScript sample shipped beside the TypeScript one"
        );
    }

    /// The fullstack server is the generated handler (`@afw/server`): API
    /// routes, server functions, loader data and SSR pages come through it, so
    /// adding one is adding a file under `app/` — `server.js` names none of
    /// them. What stays the app's is how static files are served. The server
    /// target's entry imports nothing that needs a DOM.
    #[test]
    fn fullstack_server_is_the_generated_handler() {
        let files = afw_written("fullstack", "js", Some("css"), None);
        assert!(
            !files.iter().any(|(name, _)| name.ends_with("bootstrap.js")),
            "no DOM shim entry ships"
        );
        let config: serde_json::Value =
            serde_json::from_str(&afw_text(&files, "esdev.json")).expect("valid esdev.json");
        assert_eq!(
            config["build"]["targets"]["server"]["entry"],
            serde_json::json!("server.js")
        );
        assert_eq!(config["dev"]["run"], serde_json::json!("server"));
        let server = afw_text(&files, "server.js");
        for expected in [
            "from \"@afw/server\"",
            "createHandler",
            "createPageRenderer({ shell })",
            "async function files(request)",
        ] {
            assert!(
                server.contains(expected),
                "server.js has {expected}: {server}"
            );
        }
        for hand_wired in ["registerRoutes", "createApiHandler", "\"/app/"] {
            assert!(
                !server.contains(hand_wired),
                "server.js still wires {hand_wired} by hand"
            );
        }
        // A server function sample: the page calls it, and its loader calls it
        // directly.
        let functions = afw_text(&files, "app/todos/todos.server.js");
        assert!(
            functions.contains("query(") && functions.contains("action("),
            "{functions}"
        );
        assert!(afw_text(&files, "app/todos/page.jsx").contains("from \"./todos.server.js\""));
        assert!(
            afw_text(&files, "app/todos/loader.js").contains("listTodos(undefined, { locals })")
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&afw_text(&files, "package.json")).expect("valid package.json");
        let start = manifest["scripts"]["start"].as_str().unwrap_or_default();
        assert!(
            start.starts_with("esrun ") && start.contains("dist/server.js"),
            "production serves the bundle: {start}"
        );
        assert!(
            start.contains("--allow-imports"),
            "the server lazy-loads its routes chunk: {start}"
        );
    }

    /// Withholding the demo blog withholds its files plus the plugin entry
    /// that would index posts that do not exist; keeping it keeps the entry.
    #[test]
    fn docs_blog_off_drops_the_posts_plugin() {
        let without = afw_written("docs", "js", None, Some(false));
        let paths: Vec<&str> = without.iter().map(|(path, _)| path.as_str()).collect();
        assert!(
            !paths.iter().any(|path| path.starts_with("app/blog/")),
            "blog files withheld"
        );
        let config: serde_json::Value =
            serde_json::from_str(&afw_text(&without, "esdev.json")).expect("valid esdev.json");
        let plugins = config["plugins"].as_array().expect("a plugins array");
        assert!(
            !plugins
                .iter()
                .any(|plugin| plugin.get("export").and_then(|e| e.as_str())
                    == Some("blogPostsPlugin")),
            "no posts plugin without posts"
        );
        assert!(
            plugins.iter().any(
                |plugin| plugin.get("export").and_then(|e| e.as_str()) == Some("docsNavPlugin")
            ),
            "docs navigation stays"
        );

        let with = afw_written("docs", "js", None, Some(true));
        let config: serde_json::Value =
            serde_json::from_str(&afw_text(&with, "esdev.json")).expect("valid esdev.json");
        assert!(
            config["plugins"]
                .as_array()
                .expect("a plugins array")
                .iter()
                .any(|plugin| plugin.get("export").and_then(|e| e.as_str())
                    == Some("blogPostsPlugin")),
            "the demo blog is indexed"
        );
    }

    /// Test files are not renamed, but their imports follow the renames.
    #[test]
    fn afw_test_imports_follow_ts_renames() {
        let spa = afw_written("spa", "ts", Some("css"), None);
        assert!(
            afw_text(&spa, "tests/counter.test.ts").contains("../app/page.tsx"),
            "the SPA test imports what exists"
        );
        let fullstack = afw_written("fullstack", "ts", Some("css"), None);
        let api = afw_text(&fullstack, "tests/api.test.ts");
        assert!(
            api.contains("../app/api/hello/route.ts"),
            "route import: {api}"
        );
        assert!(api.contains("../app/loader.ts"), "loader import: {api}");
        let library = afw_written("library", "ts", None, None);
        let test = afw_text(&library, "tests/counter.test.ts");
        // The entry is TypeScript; the test's bundler-style import still
        // names `.js`, which the test runner resolves to `index.ts`.
        assert!(
            test.contains("\"../index.js\""),
            "the library test imports the entry: {test}"
        );
        // Middleware runs the pipeline typed: the untyped parameters are
        // implicit `any` under `strict` (TS7006).
        let middleware = afw_text(&fullstack, "app/_middleware.ts");
        assert!(
            middleware
                .contains("import type { MiddlewareContext, NextFn } from \"@opentf/afw/server\""),
            "the middleware names its types: {middleware}"
        );
        assert!(
            middleware.contains("request: Request")
                && middleware.contains("context: MiddlewareContext")
                && middleware.contains("next: NextFn"),
            "the middleware types its parameters: {middleware}"
        );
        // Route and loader handlers are typed the way the framework and the
        // template tests call them: request in, context in.
        let route = afw_text(&fullstack, "app/api/hello/route.ts");
        assert!(
            route.contains("export function GET(_request: Request)"),
            "the route types its request: {route}"
        );
        let loader = afw_text(&fullstack, "app/loader.ts");
        assert!(
            loader.contains("params: Record<string, string>"),
            "the loader types its context: {loader}"
        );
    }

    /// Extension-full source imports (`./src/Counter.tsx`) are TS5097 without
    /// `allowImportingTsExtensions` — which libraries need to publish sources,
    /// and spas need to consume them.
    #[test]
    fn afw_tsconfigs_allow_importing_ts_extensions() {
        for (template, styling, blog) in [
            ("spa", Some("css"), None),
            ("fullstack", Some("css"), None),
            ("docs", None, Some(false)),
            ("library", None, None),
        ] {
            let files = afw_written(template, "ts", styling, blog);
            let tsconfig: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "tsconfig.json")).expect("valid JSON");
            assert_eq!(
                tsconfig["compilerOptions"]["allowImportingTsExtensions"],
                serde_json::json!(true),
                "{template}: extension imports refused"
            );
            assert_eq!(
                tsconfig["compilerOptions"]["types"],
                serde_json::json!(["@opentf/esrun-types"]),
                "{template}: the runtime: modules resolve through the type package"
            );
        }
    }

    /// The library test renders the component it publishes: props, clicks,
    /// and cleanup — not the manifest that points at it.
    #[test]
    fn library_tests_render_the_component() {
        let files = afw_written("library", "js", None, None);
        let test = afw_text(&files, "tests/counter.test.js");
        for expected in [
            "import { Counter }",
            "render(Counter",
            ".click()",
            "unmount()",
            "isConnected",
        ] {
            assert!(test.contains(expected), "the test {expected}");
        }
    }

    /// The docs release prerenders through its own target: ssg.js runs after
    /// every target (`then: run`) and skips without a staging area, so the
    /// dev loop never prerenders.
    #[test]
    fn docs_ssg_target_prerenders_on_release() {
        let files = afw_written("docs", "js", None, Some(true));
        let config: serde_json::Value =
            serde_json::from_str(&afw_text(&files, "esdev.json")).expect("valid esdev.json");
        assert_eq!(
            config["build"]["targets"]["site-ssg"],
            serde_json::json!({
                "entry": "ssg.js",
                "out": ".ssg/ssg.js",
                "then": "run",
            })
        );
        let ssg = afw_text(&files, "ssg.js");
        assert!(
            ssg.contains("no staging area") && ssg.contains("exit(0)"),
            "development skips the prerender: {ssg}"
        );
        assert!(
            ssg.contains("writePrerenderReport"),
            "the finish plugin gets its handoff"
        );
    }

    /// A `tsconfig.json` nothing can act on is a broken promise: TypeScript
    /// mode gains the compiler dependency and the `typecheck` script the
    /// esdev-native templates ship, and JavaScript mode gains neither.
    #[test]
    fn typescript_gains_its_compiler_and_typecheck() {
        for (template, styling, blog) in [
            ("spa", Some("css"), None),
            ("fullstack", Some("css"), None),
            ("docs", None, Some(false)),
            ("library", None, None),
        ] {
            let files = afw_written(template, "ts", styling, blog);
            let manifest: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "package.json")).expect("valid JSON");
            assert_eq!(
                manifest["scripts"]["typecheck"],
                serde_json::json!("esdev typecheck"),
                "{template}: no typecheck script"
            );
            assert!(
                manifest["devDependencies"].get("typescript").is_some(),
                "{template}: no typescript dependency: {manifest}"
            );
            assert_eq!(
                manifest["devDependencies"]["typescript"],
                registry_placeholder("typescript"),
                "{template}: TypeScript should resolve at scaffold time"
            );

            let js = afw_written(template, "js", styling, blog);
            let manifest: serde_json::Value =
                serde_json::from_str(&afw_text(&js, "package.json")).expect("valid JSON");
            assert!(
                manifest["scripts"].get("typecheck").is_none(),
                "{template}: JavaScript gained a typecheck script"
            );
            assert!(
                manifest["devDependencies"].get("typescript").is_none(),
                "{template}: JavaScript gained a compiler: {manifest}"
            );
        }
    }

    #[test]
    fn native_typescript_templates_resolve_the_registry_release() {
        for (template, mode) in [
            ("api", None),
            ("vanilla", None),
            ("micro-ui", None),
            ("lib", None),
            ("react", Some("static")),
            ("react", Some("fullstack")),
        ] {
            let files = plain_written(template, mode);
            let manifest: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "package.json"))
                    .expect("valid template package.json");
            assert_eq!(
                manifest["devDependencies"]["typescript"],
                registry_placeholder("typescript"),
                "{template} {mode:?}: TypeScript should resolve at scaffold time"
            );
        }
    }

    /// Every template's development port is the one it says everywhere.
    ///
    /// A template with a server of its own uses **3000** — what Bun.serve,
    /// Hono and Next.js default to, so it is the number a developer coming to
    /// a server app expects — in `dev.app.port`, in the README's URL, in the
    /// `start` script's `--allow-listen`, and as the `PORT` fallback in its
    /// code: four places, one number, or the README sends you to a port
    /// nothing listens on. A frontend pins nothing, so it gets the dev loop's
    /// default (5173, Vite's) and moves when that is taken, and its README
    /// says 5173.
    #[test]
    fn every_template_names_one_dev_port() {
        let mut checked = 0;
        for (template, _) in TEMPLATES {
            let shapes: Vec<Option<&str>> =
                match library().get(template).and_then(|e| e.option("mode")) {
                    Some(modes) => modes
                        .choices
                        .iter()
                        .map(|mode| Some(mode.value.as_str()))
                        .collect(),
                    None => vec![None],
                };
            for mode in shapes {
                let files = plain_written(template, mode);
                let text = |path: &str| {
                    files
                        .iter()
                        .find(|(written, _)| written == path)
                        .map(|(_, contents)| String::from_utf8_lossy(contents).into_owned())
                };
                let Some(config) = text("esdev.json") else {
                    continue;
                };
                let config: serde_json::Value =
                    serde_json::from_str(&config).expect("esdev.json is JSON");
                let app_port = config["dev"]["app"]["port"].as_u64();
                let readme = text("README.md").unwrap_or_default();
                let shown = readme.split("http://localhost:").nth(1).map(|rest| {
                    rest.chars()
                        .take_while(char::is_ascii_digit)
                        .collect::<String>()
                });
                let at = format!("{template} {mode:?}");
                match app_port {
                    Some(port) => {
                        assert_eq!(port, 3000, "{at}: a server's dev port");
                        if let Some(shown) = &shown {
                            assert_eq!(shown, "3000", "{at}: the README's URL");
                        }
                        let manifest: serde_json::Value = serde_json::from_str(
                            &text("package.json").expect("a server template has a manifest"),
                        )
                        .expect("package.json is JSON");
                        if let Some(start) = manifest["scripts"]["start"].as_str()
                            && let Some(grant) = start.split("--allow-listen=").nth(1)
                        {
                            assert!(grant.starts_with("3000"), "{at}: start grants {start}");
                        }
                        for (path, contents) in &files {
                            let source = String::from_utf8_lossy(contents);
                            if let Some(rest) = source.split("env.PORT ??").nth(1) {
                                let fallback: String = rest
                                    .chars()
                                    .skip_while(|c| !c.is_ascii_digit())
                                    .take_while(char::is_ascii_digit)
                                    .collect();
                                assert_eq!(fallback, "3000", "{at}: {path}'s PORT fallback");
                            }
                        }
                    }
                    None => {
                        assert!(
                            config["dev"]["server"]["port"].is_null(),
                            "{at}: a frontend pins no dev port"
                        );
                        if let Some(shown) = &shown {
                            assert_eq!(shown, "5173", "{at}: the README's URL");
                        }
                    }
                }
                checked += 1;
            }
        }
        assert!(checked >= 12, "only {checked} templates checked");
    }

    #[test]
    fn react_refresh_template_returns_a_source_map() {
        let files = plain_written("react", Some("static"));
        let plugin = afw_text(&files, "plugins/react-refresh.mjs");
        assert!(plugin.contains("sourcesContent: [code]"));
        assert!(plugin.contains("const mappings ="));
        assert!(plugin.contains("\";;;;\" +"));
    }

    /// Fast Refresh is a project plugin that installs itself in a hot build
    /// (D148): declared at the top level, with no target naming a scheme.
    #[test]
    fn react_refresh_is_a_project_plugin_reading_ctx_hot() {
        for mode in ["static", "fullstack"] {
            let files = plain_written("react", Some(mode));
            let config: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "esdev.json")).expect("valid esdev.json");
            assert_eq!(
                config["plugins"][0], "./plugins/react-refresh.mjs",
                "{mode}"
            );
            for (name, target) in config["build"]["targets"].as_object().expect("targets") {
                assert!(target.get("plugins").is_none(), "{mode} {name}");
                assert!(target.get("refresh").is_none(), "{mode} {name}");
            }
        }
        let plugin = afw_text(
            &plain_written("react", Some("static")),
            "plugins/react-refresh.mjs",
        );
        assert!(plugin.contains("if (!ctx.hot) return null;"), "{plugin}");
    }

    #[test]
    fn both_react_modes_opt_into_oxc_react_compiler() {
        for mode in ["static", "fullstack"] {
            let files = plain_written("react", Some(mode));
            let config: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "esdev.json")).expect("valid esdev.json");
            assert_eq!(config["jsx"]["importSource"], "react", "{mode}");
            assert_eq!(config["jsx"]["reactCompiler"], true, "{mode}");
        }
    }

    /// Every template that builds a static site has a `preview` script through
    /// `esdev preview`. Servers are run, not served — `esdev preview` refuses
    /// them by design — so the production check there is `start`/`serve`, and
    /// a library builds a package, not a site, so neither gets one.
    #[test]
    fn every_servable_template_has_a_preview_script() {
        fn scripts(files: &[(String, Vec<u8>)]) -> serde_json::Value {
            serde_json::from_str::<serde_json::Value>(&afw_text(files, "package.json"))
                .map(|manifest| manifest["scripts"].clone())
                .expect("valid JSON")
        }
        for (template, mode) in [
            ("vanilla", None),
            ("micro-ui", None),
            ("react", Some("static")),
        ] {
            assert_eq!(
                scripts(&plain_written(template, mode))["preview"],
                serde_json::json!("esdev preview"),
                "{template} {mode:?}: wrong preview script"
            );
        }
        // A client-side router owns every route, so its preview asks for the
        // index fallback; a site of pages is served as it is.
        for (template, language, styling, blog, script) in [
            ("spa", "js", Some("css"), None, "esdev preview --spa"),
            ("spa", "ts", Some("css"), None, "esdev preview --spa"),
            ("docs", "js", None, Some(true), "esdev preview"),
            ("docs", "ts", None, Some(false), "esdev preview"),
        ] {
            assert_eq!(
                scripts(&afw_written(template, language, styling, blog))["preview"],
                serde_json::json!(script),
                "{template} {language}: wrong preview script"
            );
        }
        // Servers are run (`start`/`serve`), and packages are published:
        // neither has a site to preview.
        for (template, mode) in [("api", None), ("react", Some("fullstack")), ("lib", None)] {
            assert!(
                scripts(&plain_written(template, mode))
                    .get("preview")
                    .is_none(),
                "{template} {mode:?}: nothing servable to preview"
            );
        }
        for (template, language, styling, blog) in [
            ("fullstack", "js", Some("css"), None),
            ("fullstack", "ts", Some("css"), None),
            ("library", "js", None, None),
            ("library", "ts", None, None),
        ] {
            assert!(
                scripts(&afw_written(template, language, styling, blog))
                    .get("preview")
                    .is_none(),
                "{template} {language}: nothing servable to preview"
            );
        }
    }

    /// JavaScript writes what is embedded: no config, no renames.
    #[test]
    fn javascript_writes_the_embedded_files() {
        let files = afw_written("spa", "js", Some("css"), None);
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"app/page.jsx"));
        assert!(paths.contains(&"jsconfig.json"));
        assert!(!paths.contains(&"tsconfig.json"));
        assert!(!paths.iter().any(|path| path.ends_with(".tsx")));
    }

    /// Tailwind is a prepend, not a project shape — and docs already has it.
    #[test]
    fn tailwind_prepends_the_import() {
        let plain = afw_written("spa", "js", Some("css"), None);
        assert!(!afw_text(&plain, "app/global.css").contains("@import"));
        let plain_manifest: serde_json::Value =
            serde_json::from_str(&afw_text(&plain, "package.json")).expect("valid JSON");
        assert!(
            plain_manifest["devDependencies"]
                .get("tailwindcss")
                .is_none(),
            "plain CSS pulls no compiler"
        );
        let tw = afw_written("spa", "js", Some("tailwind"), None);
        assert!(afw_text(&tw, "app/global.css").starts_with("@import \"tailwindcss\";\n\nbody {"));
        // An `@import` the project resolves is a dependency the project
        // declares: npm hoisting (upstream's default manager) finds the
        // toolchain's copy, and a strict `node_modules` layout does not.
        let tw_manifest: serde_json::Value =
            serde_json::from_str(&afw_text(&tw, "package.json")).expect("valid JSON");
        assert_eq!(
            tw_manifest["devDependencies"]["tailwindcss"],
            serde_json::json!(registry_placeholder("tailwindcss"))
        );
        let docs = afw_written("docs", "js", None, Some(false));
        let docs_manifest: serde_json::Value =
            serde_json::from_str(&afw_text(&docs, "package.json")).expect("valid JSON");
        assert_eq!(
            docs_manifest["devDependencies"]["tailwindcss"],
            serde_json::json!(registry_placeholder("tailwindcss")),
            "docs always imports it"
        );
    }

    /// Withholding the blog withholds its files; keeping it patches the two
    /// files that point at it.
    #[test]
    fn the_blog_is_files_plus_two_patches() {
        let bare = afw_written("docs", "js", None, Some(false));
        assert!(
            !bare.iter().any(|(path, _)| path.starts_with("app/blog/")),
            "no blog means no blog files"
        );
        let config = afw_text(&bare, "afw.config.js");
        assert!(!config.contains("blog:"));
        // afw-docs builds its search index only for a provider it knows.
        assert!(config.contains(r#"provider: "afw""#), "{config}");

        let blogged = afw_written("docs", "js", None, Some(true));
        assert!(
            blogged
                .iter()
                .any(|(path, _)| path == "app/blog/hello-world/page.mdx"),
            "yes means the sample post"
        );
        let config = afw_text(&blogged, "afw.config.js");
        assert!(config.contains("href: \"/blog\""));
        assert!(config.contains("dir: \"blog\""));
        let page = afw_text(&blogged, "app/docs/page.mdx");
        assert!(page.contains("## Blog (demo)"));
        assert!(page.contains("## Edit Content"));
    }

    fn plain_written(template: &str, mode: Option<&str>) -> Vec<(String, Vec<u8>)> {
        match mode {
            Some(mode) => written(template, &[("mode", mode)]),
            None => written(template, &[]),
        }
    }

    /// The templates with a page take a styling; the rest refuse one.
    #[test]
    fn styling_is_offered_where_there_is_a_page() {
        let styling = |template: &str| {
            library()
                .get(template)
                .expect("the template")
                .option("styling")
                .map(|option| {
                    option
                        .choices
                        .iter()
                        .map(|c| c.value.clone())
                        .collect::<Vec<_>>()
                })
        };
        for template in [
            "react",
            "vanilla",
            "micro-ui",
            "preact",
            "lit",
            "spa",
            "fullstack",
        ] {
            assert_eq!(
                styling(template).as_deref(),
                Some(&["css".to_string(), "tailwind".to_string()][..]),
                "{template}"
            );
            // No default: the menu starts on nothing (D135).
            let option = library().get(template).unwrap().option("styling").unwrap();
            assert!(option.default.is_none(), "{template} defaults its styling");
        }
        for template in ["api", "lib", "docs", "library"] {
            assert!(styling(template).is_none(), "{template}");
        }
    }

    /// Tailwind replaces the shared stylesheet in place — its contents, never
    /// a second file beside it — and the `<link>` already in the head stays
    /// the one, in every template and every mode that takes it.
    #[test]
    fn tailwind_adds_a_linked_stylesheet_and_its_dependency() {
        for (template, mode) in [
            ("vanilla", None),
            ("micro-ui", None),
            ("react", Some("static")),
            ("react", Some("fullstack")),
        ] {
            let mut answers = vec![("styling", "tailwind")];
            if let Some(mode) = mode {
                answers.push(("mode", mode));
            }
            let files = written(template, &answers);
            assert_eq!(
                afw_text(&files, STYLESHEET),
                "@import \"tailwindcss\";\n",
                "{template}"
            );
            assert_eq!(
                files.iter().filter(|(path, _)| path == STYLESHEET).count(),
                1,
                "{template}: two stylesheets"
            );
            let html = afw_text(&files, "index.html");
            assert_eq!(
                html.matches("<link rel=\"stylesheet\" href=\"./src/styles.css\" />")
                    .count(),
                1,
                "{template}: the link duplicated or missing in\n{html}"
            );
            let link = html
                .find("<link rel=\"stylesheet\" href=\"./src/styles.css\" />")
                .unwrap();
            assert!(link < html.find("</head>").expect("a head"), "{template}");
            let manifest: serde_json::Value =
                serde_json::from_str(&afw_text(&files, "package.json")).expect("still JSON");
            assert!(
                manifest.pointer("/devDependencies/tailwindcss").is_some(),
                "{template}: {manifest}"
            );
        }
    }

    /// Plain writes the template as it is: the shared stylesheet, linked, with
    /// no framework import and no compiler dependency.
    #[test]
    fn plain_css_leaves_the_template_as_it_was() {
        let files = plain_written("vanilla", None);
        let css = afw_text(&files, STYLESHEET);
        assert!(
            css.contains("font-family"),
            "the starter styles shipped: {css}"
        );
        assert!(!css.contains("@import"), "plain CSS pulls no import");
        let html = afw_text(&files, "index.html");
        assert!(
            html.contains("<link rel=\"stylesheet\" href=\"./src/styles.css\" />"),
            "the page loads it: {html}"
        );
        assert!(!afw_text(&files, "package.json").contains("tailwindcss"));
    }

    /// One look, shared: the page templates ship byte for byte the same
    /// stylesheet, so the three starters cannot drift into three designs.
    /// `react` is checked in both modes since each carries its own manifest
    /// but the same shared files.
    #[test]
    fn page_templates_share_one_stylesheet() {
        let mut contents = Vec::new();
        for (template, mode) in [
            ("vanilla", None),
            ("micro-ui", None),
            ("react", Some("static")),
            ("react", Some("fullstack")),
        ] {
            let files = plain_written(template, mode);
            contents.push(afw_text(&files, STYLESHEET));
            let html = afw_text(&files, "index.html");
            assert!(
                html.contains("<link rel=\"stylesheet\" href=\"./src/styles.css\" />"),
                "{template} {mode:?}: the page does not load it"
            );
        }
        for css in &contents[1..] {
            assert_eq!(css, &contents[0], "the stylesheets drifted apart");
        }
    }
}
