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

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

/// The one-line descriptions `--list` prints.
///
/// Beside the templates rather than inside them: a description is for somebody
/// choosing, and what they are choosing between is only visible from here.
const DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "api",
        "A JSON API — a hello-world server under a narrow grant. No deps",
    ),
    (
        "react",
        "React — a static site, or an app with a server of its own",
    ),
    (
        "lib",
        "A publishable TypeScript package — .d.ts included, no runtime deps",
    ),
    (
        "vanilla",
        "TypeScript and the DOM — no framework, nothing it ships depends on",
    ),
    (
        "micro-ui",
        "Micro apps with Micro-UI — framework-free UI with a tiny reactive core",
    ),
    (
        "spa",
        "An OTF Web single-page app — browser-only UI, static deploy, no server",
    ),
    (
        "fullstack",
        "An OTF Web fullstack app — loaders, API routes, SSR on demand",
    ),
    (
        "docs",
        "An OTF Web documentation site — MDX docs, sidebar, search, optional blog",
    ),
    (
        "library",
        "An OTF Web component library — publishable components, esdev-tested",
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
pub(crate) const RENAMED: &[(&str, &str)] = &[("_gitignore", ".gitignore")];

/// What `esdev create` was asked to do.
pub struct CreateConfig {
    /// The directory to write into.
    pub dir: String,
    /// Which template, or `None` to ask (or take the default).
    pub template: Option<String>,
    /// Which mode of that template, or `None` to ask (or take the default).
    /// Meaningless — and refused — for a template that has only one shape.
    pub mode: Option<String>,
    /// Which language an OTF template scaffolds, or `None` to ask (or take
    /// the default). Meaningless — and refused — for other templates.
    pub language: Option<String>,
    /// Plain CSS or Tailwind, or `None` to ask (or take the default). Taken
    /// by the templates with a page to style — `react`, `vanilla`, `micro-ui`,
    /// `spa` and `fullstack` ([`stylings`]); refused everywhere else.
    pub styling: Option<String>,
    /// Whether the `docs` template keeps its demo blog, or `None` to ask
    /// (or take the default). Refused for every other template.
    pub blog: Option<bool>,
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
}

/// The default template, when `--template` did not say and nobody was asked.
pub const DEFAULT_TEMPLATE: &str = "react";

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
    let mut answers = Answers::default();
    if let Some(named) = &config.template {
        answers.template = Some(named.clone());
    }
    // Steps that showed a menu, in order. Esc steps back to the nearest one
    // before the current step; a step answered by a flag never appears here,
    // so the retreat skips it — and with no prompted step behind, Esc cancels.
    let mut prompted: Vec<Step> = Vec::new();
    let mut step = 0;
    while step < STEPS.len() {
        let ask = Ask {
            scripted,
            back: STEPS[..step].iter().any(|s| prompted.contains(s)),
        };
        match STEPS[step] {
            Step::Template => {
                if answers.template.is_none() {
                    if scripted {
                        answers.template = Some(DEFAULT_TEMPLATE.to_string());
                    } else {
                        match ask_template() {
                            Some(template) => {
                                answers.template = Some(template);
                                prompted.push(Step::Template);
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
                if TEMPLATES.iter().all(|(name, _)| *name != template) {
                    return Err(format!(
                        "there is no {template} template.\n\n{}",
                        list().trim_end()
                    ));
                }
                step += 1;
            }
            Step::Mode => {
                let template = answers.template.as_deref().expect("template first");
                match resolve_mode(template, config.mode.as_deref(), ask)? {
                    Mode::Chosen(mode) => {
                        answers.mode = Some(mode);
                        if config.mode.is_none() && !scripted {
                            prompted.push(Step::Mode);
                        }
                        step += 1;
                    }
                    Mode::None => {
                        answers.mode = None;
                        step += 1;
                    }
                    Mode::Back => match step_back_index(Step::Mode, &prompted) {
                        Some(index) => step = index,
                        None => return Ok(String::new()),
                    },
                }
            }
            Step::Language => {
                let template = answers.template.as_deref().expect("template first");
                match resolve_choice(
                    template,
                    "language",
                    "language",
                    is_otf(template),
                    LANGUAGES,
                    Fallback::Preselected(DEFAULT_LANGUAGE),
                    config.language.as_deref(),
                    "Select a Language?",
                    ask,
                )? {
                    Choice::Chosen(language) => {
                        answers.language = Some(language);
                        if config.language.is_none() && !scripted {
                            prompted.push(Step::Language);
                        }
                        step += 1;
                    }
                    Choice::NotApplicable => {
                        answers.language = None;
                        step += 1;
                    }
                    Choice::Back => match step_back_index(Step::Language, &prompted) {
                        Some(index) => step = index,
                        None => return Ok(String::new()),
                    },
                }
            }
            Step::Styling => {
                let template = answers.template.as_deref().expect("template first");
                match resolve_choice(
                    template,
                    "styling",
                    "styling",
                    stylings(template).is_some(),
                    stylings(template).unwrap_or(STYLINGS),
                    Fallback::Unasked(UNSTYLED),
                    config.styling.as_deref(),
                    "Select a Styling Solution?",
                    ask,
                )? {
                    Choice::Chosen(styling) => {
                        answers.styling = Some(styling);
                        if config.styling.is_none() && !scripted {
                            prompted.push(Step::Styling);
                        }
                        step += 1;
                    }
                    Choice::NotApplicable => {
                        answers.styling = None;
                        step += 1;
                    }
                    Choice::Back => match step_back_index(Step::Styling, &prompted) {
                        Some(index) => step = index,
                        None => return Ok(String::new()),
                    },
                }
            }
            Step::Blog => {
                let template = answers.template.as_deref().expect("template first");
                match resolve_blog(template, config.blog, ask)? {
                    Blog::On => {
                        answers.blog = Some(true);
                        if config.blog.is_none() && !scripted {
                            prompted.push(Step::Blog);
                        }
                        step += 1;
                    }
                    Blog::Off => {
                        answers.blog = Some(false);
                        if config.blog.is_none() && !scripted {
                            prompted.push(Step::Blog);
                        }
                        step += 1;
                    }
                    Blog::NotApplicable => {
                        answers.blog = None;
                        step += 1;
                    }
                    Blog::Back => match step_back_index(Step::Blog, &prompted) {
                        Some(index) => step = index,
                        None => return Ok(String::new()),
                    },
                }
            }
            Step::Manager => match resolve_manager(config.manager.as_deref(), ask)? {
                ManagerChoice::Chosen(manager) => {
                    answers.manager = Some(manager);
                    if config.manager.is_none() && !scripted {
                        prompted.push(Step::Manager);
                    }
                    step += 1;
                }
                ManagerChoice::Back => match step_back_index(Step::Manager, &prompted) {
                    Some(index) => step = index,
                    None => return Ok(String::new()),
                },
            },
        }
    }
    let template = answers.template.expect("the loop answers it");
    let mode = answers.mode;
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
    let tailwind = answers.styling.as_deref() == Some("tailwind");
    let otf = match answers.language {
        Some(language) => Some(Otf {
            language,
            styling: answers.styling,
            blog: answers.blog,
        }),
        None => None,
    };

    let files = TEMPLATES
        .iter()
        .find(|(name, _)| *name == template)
        .map(|(_, files)| *files)
        .expect("validated above");
    let files = files_for(files, mode.as_deref());
    // OTF answers rewrite the file list — renames, patches, added and
    // withheld files — so from here the list is owned either way.
    let files: Vec<(String, Vec<u8>)> = match &otf {
        Some(otf) => apply_otf(&template, otf, &files),
        None => {
            let files = files
                .into_iter()
                .map(|(path, contents)| (path, contents.to_vec()))
                .collect();
            if tailwind {
                apply_tailwind(files)
            } else {
                files
            }
        }
    };

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
    let versions = resolve_registry_versions(&packages).await?;

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
    print!("{report}");
    let _ = std::io::Write::flush(&mut std::io::stdout());
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
        &template,
        installed,
        &manager,
        paint,
    ));
    Ok(report)
}

/// The lines printed after the project is written.
fn next_steps(
    dir: &str,
    template: &str,
    installed: Option<crate::install::Manager>,
    manager: &str,
    paint: crate::style::Palette,
) -> String {
    // The command that actually starts it, which is not the same for every
    // template: a library has nothing to run.
    let run = match template {
        "lib" | "library" => "test",
        _ => "dev",
    };
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

/// The pre-write questions, in order. Which of them appear depends on the
/// answers so far — a modeless template has no mode, a non-OTF one no axes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Template,
    Mode,
    Language,
    Styling,
    Blog,
    Manager,
}

const STEPS: &[Step] = &[
    Step::Template,
    Step::Mode,
    Step::Language,
    Step::Styling,
    Step::Blog,
    Step::Manager,
];

/// The nearest earlier step that showed a menu, or `None` when Esc cancels.
/// Pure, so the retreat rule tests without a terminal.
fn step_back_index(current: Step, prompted: &[Step]) -> Option<usize> {
    let position = STEPS.iter().position(|step| *step == current)?;
    prompted
        .iter()
        .rev()
        .filter_map(|step| STEPS.iter().position(|s| *s == *step))
        .find(|index| *index < position)
}

/// The answers so far. Every forward pass overwrites each one, so switching
/// template mid-run cannot leave a stale answer behind.
#[derive(Default)]
struct Answers {
    template: Option<String>,
    mode: Option<String>,
    language: Option<String>,
    styling: Option<String>,
    blog: Option<bool>,
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

/// What resolving `--mode` came to.
enum Mode {
    /// This template has modes, and this is the one.
    Chosen(String),
    /// This template has one shape.
    None,
    /// Esc: back to the question before, or out if there is none.
    Back,
}

/// The mode to write, from the flag, a question, or the default.
///
/// Naming a mode for a template that has none is refused rather than ignored: a
/// flag that silently does nothing is one somebody will keep passing, and keep
/// believing.
fn resolve_mode(template: &str, asked_for: Option<&str>, ask: Ask) -> Result<Mode, String> {
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

    if ask.scripted || !crate::prompt::interactive() {
        return Ok(Mode::Chosen(
            default_mode(template)
                .expect("a template with modes has a default")
                .to_string(),
        ));
    }

    let choices: Vec<crate::prompt::Choice<'_>> = modes
        .iter()
        .map(|(name, description)| crate::prompt::Choice {
            name,
            label: display_name(name),
            description,
            disabled: false,
        })
        .collect();
    match crate::prompt::select("Which Mode?", &choices, Some(0), ask.esc()) {
        Some(chosen) => Ok(Mode::Chosen(choices[chosen].name.to_string())),
        None => Ok(Mode::Back),
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

/// The templates that come from the OTF Web starter set rather than from this
/// repository's own stack. They build with the OTF toolchain (`otfw`, itself
/// an ES-runtime program) instead of `esdev build`, so they carry no
/// `esdev.json` — and they take the axes `create-web` asks about, which the
/// shared template → mode → install flow does not have.
const OTF_TEMPLATES: &[&str] = &["spa", "fullstack", "docs", "library"];

fn is_otf(template: &str) -> bool {
    OTF_TEMPLATES.contains(&template)
}

/// The languages an OTF template scaffolds. `create-web` ships JavaScript and
/// converts to TypeScript on request; the embedded files are the JavaScript
/// half, and the conversion is re-applied here rather than embedded twice.
const LANGUAGES: &[(&str, &str)] = &[
    ("js", "JavaScript — .jsx source files"),
    ("ts", "TypeScript — .tsx source files and a tsconfig.json"),
];

/// `create-web` starts its menu on JavaScript, so this does too.
const DEFAULT_LANGUAGE: &str = "js";

/// The stylesheets an OTF app template offers. Only `spa` and `fullstack`
/// ship `app/global.css` (`docs` is always Tailwind, a library has no
/// styles), so only they ask. Tailwind is a one-line prepend — the toolchain
/// compiles it — and a dependency, not a second project.
const STYLINGS: &[(&str, &str)] = &[
    ("css", "Plain CSS — a small starter stylesheet"),
    ("tailwind", "TailwindCSS v4, compiled by the toolchain"),
];

/// What an unattended run without `--styling` writes: the template as it
/// is, with nothing added.
///
/// **Styling has no default.** A framework is a dependency the project will
/// carry, and choosing one is the person's call, so the menu starts on
/// nothing and has to be answered. Only a run nobody can ask falls back — and
/// it falls back to adding nothing, not to a framework.
const UNSTYLED: &str = "css";

/// This repository's own templates that have a page to style. Their plain
/// choice writes no stylesheet at all — they are hello worlds, and a starter
/// stylesheet is one more file to delete (D100) — so Tailwind is the only
/// choice that adds anything.
const ESDEV_STYLED: &[&str] = &["react", "vanilla", "micro-ui"];

const ESDEV_STYLINGS: &[(&str, &str)] = &[
    ("css", "Plain CSS — no framework, nothing to install"),
    ("tailwind", "TailwindCSS v4, compiled by esdev"),
];

/// The styling choices a template offers, or `None` for one that takes no
/// styling.
fn stylings(template: &str) -> Option<&'static [(&'static str, &'static str)]> {
    if template == "spa" || template == "fullstack" {
        Some(STYLINGS)
    } else if ESDEV_STYLED.contains(&template) {
        Some(ESDEV_STYLINGS)
    } else {
        None
    }
}

/// Where the Tailwind choice puts its stylesheet in one of [`ESDEV_STYLED`].
const TAILWIND_STYLESHEET: &str = "src/styles.css";

/// Tailwind for one of this repository's own templates: a stylesheet that
/// imports it, the `<link>` that loads it, and the dependency that provides
/// it. esdev compiles it (D135); nothing else is configured.
///
/// Linked from `index.html`, which every one of them builds from — the
/// `react` modes included, since both render into it — so the stylesheet is
/// part of the build rather than an import some module has to remember.
fn apply_tailwind(mut files: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    for (path, bytes) in &mut files {
        let Ok(text) = std::str::from_utf8(bytes) else {
            continue;
        };
        let patched = match path.as_str() {
            "index.html" => text.replacen(
                "    <script type=\"module\"",
                &format!(
                    "    <link rel=\"stylesheet\" href=\"./{TAILWIND_STYLESHEET}\" />\n    <script type=\"module\""
                ),
                1,
            ),
            "package.json" => patch_tailwind_dep(text),
            _ => continue,
        };
        *bytes = patched.into_bytes();
    }
    files.push((
        TAILWIND_STYLESHEET.to_string(),
        b"@import \"tailwindcss\";\n".to_vec(),
    ));
    files
}

/// Whether the `docs` template keeps its demo blog. `create-web` starts on
/// yes; the embedded files are the blog-on-disk state with the config and
/// page unpatched, so "no" only withholds files and "yes" patches two.
const DEFAULT_BLOG: bool = true;

/// What an axis falls back to when its flag is absent.
#[derive(Clone, Copy)]
enum Fallback {
    /// A default in the ordinary sense: the menu starts on it, and a run
    /// nobody can ask takes it.
    Preselected(&'static str),
    /// No default: the menu starts on nothing and must be answered. A run
    /// nobody can ask still needs an answer, and takes this one.
    Unasked(&'static str),
}

impl Fallback {
    fn value(self) -> &'static str {
        match self {
            Fallback::Preselected(value) | Fallback::Unasked(value) => value,
        }
    }
}

/// What resolving one named choice came to. `NotApplicable` is a template
/// that does not take this axis at all.
#[derive(Debug)]
enum Choice {
    Chosen(String),
    NotApplicable,
    Back,
}

/// Flag, question, or default — the same precedence the mode uses, for an
/// axis only some templates take.
///
/// Naming one for a template that does not take it is refused rather than
/// ignored, for the same reason a stray `--mode` is: a flag that silently
/// does nothing is one somebody keeps passing, and keeps believing.
#[allow(clippy::too_many_arguments)]
fn resolve_choice(
    template: &str,
    axis: &str,
    flag: &str,
    takes: bool,
    options: &[(&'static str, &'static str)],
    default: Fallback,
    asked_for: Option<&str>,
    question: &str,
    ask: Ask,
) -> Result<Choice, String> {
    if !takes {
        return match asked_for {
            Some(value) => Err(format!(
                "the {template} template has no {axis}, so --{flag}={value} means nothing here."
            )),
            None => Ok(Choice::NotApplicable),
        };
    }
    if let Some(value) = asked_for {
        if !options.iter().any(|(name, _)| *name == value) {
            let valid: Vec<&str> = options.iter().map(|(name, _)| *name).collect();
            return Err(format!(
                "the {template} template has no {axis} {value}.\n\nValid {axis}s: {}.",
                valid.join(", ")
            ));
        }
        return Ok(Choice::Chosen(value.to_string()));
    }
    if ask.scripted || !crate::prompt::interactive() {
        return Ok(Choice::Chosen(default.value().to_string()));
    }
    let choices: Vec<crate::prompt::Choice<'_>> = options
        .iter()
        .map(|(name, description)| crate::prompt::Choice {
            name,
            label: display_name(name),
            description,
            disabled: false,
        })
        .collect();
    // An axis with a real default starts the menu on it; one without starts
    // on nothing, so Enter alone cannot answer it.
    let preselect = match default {
        Fallback::Preselected(value) => choices.iter().position(|choice| choice.name == value),
        Fallback::Unasked(_) => None,
    };
    match crate::prompt::select(question, &choices, preselect, ask.esc()) {
        Some(chosen) => Ok(Choice::Chosen(choices[chosen].name.to_string())),
        None => Ok(Choice::Back),
    }
}

/// Whether the `docs` template keeps its demo blog, resolved.
#[derive(Debug)]
enum Blog {
    On,
    Off,
    NotApplicable,
    Back,
}

/// The blog is a yes/no rather than a named choice, but the shape is the
/// same: flags, a question, a default — and refused outside `docs`.
fn resolve_blog(template: &str, asked_for: Option<bool>, ask: Ask) -> Result<Blog, String> {
    if template != "docs" {
        return match asked_for {
            Some(_) => Err(format!(
                "only the docs template has a blog, so --blog means nothing for {template}."
            )),
            None => Ok(Blog::NotApplicable),
        };
    }
    if let Some(on) = asked_for {
        return Ok(if on { Blog::On } else { Blog::Off });
    }
    if ask.scripted || !crate::prompt::interactive() {
        return Ok(if DEFAULT_BLOG { Blog::On } else { Blog::Off });
    }
    let choices = [
        crate::prompt::Choice {
            name: "Yes — add demo blog",
            label: "Yes — add demo blog",
            description: "Adds app/blog/, a sample post, and a Blog link in the navbar",
            disabled: false,
        },
        crate::prompt::Choice {
            name: "No — docs only",
            label: "No — docs only",
            description: "Documentation pages without a blog section",
            disabled: false,
        },
    ];
    let preselect = if DEFAULT_BLOG { 0 } else { 1 };
    match crate::prompt::select(
        "Include a Sample Blog?",
        &choices,
        Some(preselect),
        ask.esc(),
    ) {
        Some(0) => Ok(Blog::On),
        Some(_) => Ok(Blog::Off),
        None => Ok(Blog::Back),
    }
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

/// The extra answers an OTF Web template resolved to. `None` fields are axes
/// the template does not take.
#[derive(Debug)]
struct Otf {
    language: String,
    styling: Option<String>,
    blog: Option<bool>,
}

/// Applies the OTF answers to the embedded files: renames, patches, added
/// and withheld files. This is `create-web`'s `applyTypescript`,
/// `applyDocsBlog` and Tailwind prepend, re-applied to embedded bytes rather
/// than to a directory — the transforms are deterministic over file contents,
/// so they unit-test without touching the filesystem.
fn apply_otf(template: &str, otf: &Otf, files: &[(String, &[u8])]) -> Vec<(String, Vec<u8>)> {
    let ts = otf.language == "ts";
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    for (path, contents) in files {
        // The demo blog is files on disk plus two patches: withholding it is
        // withholding the files, and nothing else.
        if template == "docs" && otf.blog == Some(false) && path.starts_with("app/blog/") {
            continue;
        }
        if ts && path == "jsconfig.json" {
            continue;
        }
        let mut new_path = path.clone();
        let mut bytes = (*contents).to_vec();
        if ts {
            if let Some(renamed) = otf_rename(&new_path) {
                new_path = renamed;
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    let renamed_base = new_path.rsplit('/').next().unwrap_or(&new_path).to_string();
                    bytes = otf_patch_source(template, &renamed_base, text).into_bytes();
                }
            } else if template == "library" && new_path == "index.js" {
                new_path = "index.ts".to_string();
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    bytes = otf_patch_source(template, "index.ts", text).into_bytes();
                }
            } else if new_path == "README.md" {
                // READMEs name the file to edit; TypeScript renames it.
                if let Ok(text) = std::str::from_utf8(&bytes) {
                    bytes = otf_patch_source(template, "README.md", text).into_bytes();
                }
            }
        }
        if otf.styling.as_deref() == Some("tailwind")
            && new_path == "app/global.css"
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            bytes = format!("{OTF_TAILWIND_IMPORT}{text}").into_bytes();
        }
        if template == "docs" && otf.blog == Some(true) {
            if new_path == "otfw.config.js"
                && let Ok(text) = std::str::from_utf8(&bytes)
            {
                bytes = otf_patch_blog_config(text).into_bytes();
            } else if new_path == "app/docs/page.mdx"
                && let Ok(text) = std::str::from_utf8(&bytes)
            {
                bytes = otf_patch_blog_page(text).into_bytes();
            }
        }
        if template == "library"
            && ts
            && new_path == "package.json"
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            bytes = otf_patch_library_manifest(text).into_bytes();
        }
        // Tailwind is an `@import` the project resolves, so the project has
        // to depend on it: `create-web` leans on npm hoisting (its default
        // manager) to find the toolchain's copy, which a strict `node_modules`
        // layout does not provide. `docs` always imports it; apps only on the
        // Tailwind styling.
        if new_path == "package.json"
            && (template == "docs" || otf.styling.as_deref() == Some("tailwind"))
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            bytes = patch_tailwind_dep(text).into_bytes();
        }
        // The generated `tsconfig.json` needs something to act on it — the
        // same compiler dependency and `typecheck` script the esdev-native
        // templates carry in their own manifests. JavaScript mode keeps
        // `jsconfig.json` and gains neither.
        if ts
            && new_path == "package.json"
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            bytes = otf_patch_typescript(text).into_bytes();
        }
        // The overlay wins, same as modes: a generated file replaces an
        // embedded one rather than being written beside it.
        match out.iter_mut().find(|(existing, _)| *existing == new_path) {
            Some(entry) => entry.1 = bytes,
            None => out.push((new_path, bytes)),
        }
    }
    if ts {
        for (path, contents) in otf_typescript_files(template) {
            match out.iter_mut().find(|(existing, _)| *existing == path) {
                Some(entry) => entry.1 = contents,
                None => out.push((path, contents)),
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The TypeScript renames `create-web` applies: `.jsx` sources, and the
/// server files the toolchain treats as code rather than components.
fn otf_rename(path: &str) -> Option<String> {
    if let Some(stem) = path.strip_suffix(".jsx") {
        return Some(format!("{stem}.tsx"));
    }
    let base = path.rsplit('/').next().unwrap_or(path);
    if (base == "route.js" && path.contains("/api/"))
        || base == "_middleware.js"
        || base == "loader.js"
    {
        return path.strip_suffix(".js").map(|stem| format!("{stem}.ts"));
    }
    None
}

/// The source patches: references follow the renames, layouts type their
/// props, and the library's counter types its own.
fn otf_patch_source(template: &str, basename: &str, content: &str) -> String {
    let mut next = content
        .replace("app/page.jsx", "app/page.tsx")
        .replace("app/api/hello/route.js", "app/api/hello/route.ts")
        .replace("app/_middleware.js", "app/_middleware.ts")
        .replace("app/loader.js", "app/loader.ts")
        .replace("route.js", "route.ts");
    if template == "library" {
        next = next
            .replace("./src/Counter.jsx", "./src/Counter.tsx")
            .replace("../src/Counter.jsx", "../src/Counter.tsx");
    } else {
        next = next.replace(".jsx", ".tsx");
    }
    next = otf_patch_layout_types(&next, basename);
    next = otf_patch_component_types(&next, basename);
    next
}

/// Layouts take children; TypeScript wants to know of what.
fn otf_patch_layout_types(content: &str, basename: &str) -> String {
    if !basename.starts_with("layout.") {
        return content.to_string();
    }
    const PREFIX: &str = "export default function ";
    let Some(start) = content.find(PREFIX) else {
        return content.to_string();
    };
    let head = &content[..start + PREFIX.len()];
    let after = &content[start + PREFIX.len()..];
    let name_end = after
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(after.len());
    let name = &after[..name_end];
    let rest = &after[name_end..];
    let tail = if let Some(after_params) = rest.strip_prefix("({ children })") {
        format!("({{ children }}: {{ children: unknown }}){after_params}")
    } else if let Some(after_params) = rest.strip_prefix("(props)") {
        format!("(props: {{ children: unknown }}){after_params}")
    } else {
        return content.to_string();
    };
    format!("{head}{name}{tail}")
}

/// The library's counter types its one prop.
fn otf_patch_component_types(content: &str, basename: &str) -> String {
    if basename != "Counter.tsx" {
        return content.to_string();
    }
    content.replace(
        "export default function Counter({ initial = 0 })",
        "export default function Counter({ initial = 0 }: { initial?: number })",
    )
}

/// The library publishes its entry, and in TypeScript the entry is TypeScript.
fn otf_patch_library_manifest(content: &str) -> String {
    content
        .replace("\"./index.js\"", "\"./index.ts\"")
        .replace("\"index.js\"", "\"index.ts\"")
}

/// A project that imports `tailwindcss` depends on it, explicitly.
fn patch_tailwind_dep(content: &str) -> String {
    content.replace(
        "\"devDependencies\": {\n",
        "\"devDependencies\": {\n    \"tailwindcss\": \"{{registry:tailwindcss}}\",\n",
    )
}

/// A `tsconfig.json` without its compiler is a config nothing can act on:
/// TypeScript mode also gains the `typescript` dependency and the `typecheck`
/// script every esdev-native template ships. The script runs `esdev typecheck`
/// — the project's own `tsc` through its package manager, with a actionable
/// error when the compiler is not installed — rather than a bare `tsc`.
/// String surgery, like [`patch_tailwind_dep`], so the embedded manifest
/// keeps its formatting.
fn otf_patch_typescript(content: &str) -> String {
    let mut next = content.to_string();
    if !next.contains("\"typecheck\"") {
        next = next.replace(
            "\"scripts\": {\n",
            "\"scripts\": {\n    \"typecheck\": \"esdev typecheck\",\n",
        );
    }
    if !next.contains("\"typescript\"") {
        next = next.replace(
            "\"devDependencies\": {\n",
            "\"devDependencies\": {\n    \"typescript\": \"{{registry:typescript}}\",\n",
        );
    }
    next
}

/// The compiler macros are build-time, not runtime: their declarations ship
/// as a `.d.ts` beside the sources, exactly as `create-web` writes them.
const OTF_ENV_DTS: &str = "/** OTF Web compiler macros — provided at build time, not runtime. */
declare const $state: {
  <T>(initial: T): T;
  <T>(): T | undefined;
};
declare function $derived<T>(fn: () => T): T;
declare function $effect(fn: () => void | (() => void)): void;
";

/// The `tsconfig.json` beside it, with the same per-template includes.
fn otf_typescript_files(template: &str) -> Vec<(String, Vec<u8>)> {
    let (env_path, include, allow_js): (&str, Vec<&str>, bool) = match template {
        "spa" | "fullstack" => ("app/otfw-env.d.ts", vec!["app"], false),
        "library" => (
            "otfw-env.d.ts",
            vec!["src", "tests", "index.ts", "otfw-env.d.ts"],
            false,
        ),
        _ => ("app/otfw-env.d.ts", vec!["app", "otfw.config.js"], true),
    };
    let mut compiler = serde_json::json!({
        "lib": ["ESNext", "DOM", "DOM.Iterable"],
        "target": "ESNext",
        "module": "ESNext",
        "moduleResolution": "bundler",
        "jsx": "preserve",
        "jsxImportSource": "@opentf/web",
        "strict": true,
        "skipLibCheck": true,
        "noEmit": true,
        "isolatedModules": true,
        "moduleDetection": "force",
    });
    if allow_js {
        compiler["allowJs"] = serde_json::Value::Bool(true);
    }
    let tsconfig = serde_json::json!({
        "compilerOptions": compiler,
        "include": include,
    });
    let mut rendered = serde_json::to_string_pretty(&tsconfig).unwrap_or_default();
    rendered.push('\n');
    vec![
        ("tsconfig.json".to_string(), rendered.into_bytes()),
        (env_path.to_string(), OTF_ENV_DTS.as_bytes().to_vec()),
    ]
}

/// The Tailwind choice is a one-line prepend — the toolchain compiles it.
const OTF_TAILWIND_IMPORT: &str = "@import \"tailwindcss\";\n\n";

/// The demo blog is files on disk plus two patches; this is what "yes" adds.
const OTF_BLOG_NAV: &str = "nav: [\n      { label: \"Docs\", href: \"/docs\" },\n      { label: \"Blog\", href: \"/blog\" },\n    ],";
const OTF_BLOG_CONFIG: &str = "\n  // Sample blog — demo post under app/blog/. Remove this block and app/blog/ if unused.\n  blog: {\n    dir: \"blog\",\n    lastUpdated: true,\n  },";
const OTF_BLOG_DEMO_SECTION: &str = "\n## Blog (demo)\n\nThis starter includes a **demo blog** under `app/blog/` — one sample post plus a\n`/blog` link in the top navbar. Replace the placeholder post with your own MDX, or\nremove `app/blog/`, the `blog` block in `otfw.config.js`, and the Blog nav entry\nif you only need docs.\n";

fn otf_patch_blog_config(content: &str) -> String {
    let mut config = content.to_string();
    if !config.contains("href: \"/blog\"") {
        config = config.replace("nav: [{ label: \"Docs\", href: \"/docs\" }],", OTF_BLOG_NAV);
    }
    if !config.contains("blog:") {
        let trimmed = config.trim_end();
        let head = trimmed.strip_suffix("});").unwrap_or(trimmed);
        config = format!("{head}{OTF_BLOG_CONFIG}\n}});\n");
    }
    config
}

fn otf_patch_blog_page(content: &str) -> String {
    if content.contains("## Blog (demo)") {
        return content.to_string();
    }
    content.replace(
        "\n## Edit Content\n",
        &format!("{OTF_BLOG_DEMO_SECTION}\n## Edit Content\n"),
    )
}

/// How a menu value reads. Flags stay lowercase — what is chosen must match
/// what `--template=spa` spells — so the menu shows the proper form beside
/// the choice it stands for.
fn display_name(name: &'static str) -> &'static str {
    match name {
        "api" => "API",
        "docs" => "Docs",
        "fullstack" => "FullStack",
        "lib" => "Library",
        // The OTF starter's display is provisional: `lib` above already
        // reads "Library", so this needs its own (see OTF_GROUP below).
        "library" => "Component Library",
        "micro-ui" => "Micro-UI",
        "react" => "ReactJS",
        "spa" => "SPA",
        "vanilla" => "Vanilla",
        "static" => "Static",
        "js" => "JS",
        "ts" => "TS",
        "css" => "CSS",
        "tailwind" => "Tailwind",
        _ => name,
    }
}

/// The OTF starters in the order `create-web` asks them — the embedded
/// `TEMPLATES` list is alphabetical, which is for `--list`, not for choosing.
const OTF_ORDER: &[&str] = &["spa", "fullstack", "docs", "library"];

/// The group entry that stands in for the four OTF starters in the first
/// menu. A name no flag spells, so it never leaks into the scriptable path.
const OTF_GROUP: &str = "OTF Web";
const OTF_GROUP_DESCRIPTION: &str = "OTF Web starters — SPA, fullstack, docs or library";

/// The first menu: this repository's own templates, then the OTF Web group.
/// Pure, so a test can read what somebody choosing sees.
fn template_menu_top() -> Vec<(&'static str, &'static str)> {
    let mut menu: Vec<(&'static str, &'static str)> = TEMPLATES
        .iter()
        .filter(|(name, _)| !is_otf(name))
        .map(|(name, _)| {
            (
                *name,
                DESCRIPTIONS
                    .iter()
                    .find(|(template, _)| template == name)
                    .map_or("", |(_, description)| *description),
            )
        })
        .collect();
    menu.push((OTF_GROUP, OTF_GROUP_DESCRIPTION));
    menu
}

/// The second menu, behind the group entry: the four OTF starters.
fn template_menu_otf() -> Vec<(&'static str, &'static str)> {
    OTF_ORDER
        .iter()
        .map(|name| {
            (
                *name,
                DESCRIPTIONS
                    .iter()
                    .find(|(template, _)| template == name)
                    .map_or("", |(_, description)| *description),
            )
        })
        .collect()
}

/// Which template, asked on a terminal.
///
/// Two menus rather than nine lines: the OTF starters choose behind their
/// group entry, in `create-web`'s order. Neither menu has a default — the
/// template is the one answer worth choosing explicitly — while flags,
/// `-y` and unattended runs resolve exactly as before. Esc on the first
/// menu cancels the run; Esc on the second steps back to the first.
fn ask_template() -> Option<String> {
    let top: Vec<crate::prompt::Choice<'_>> = template_menu_top()
        .into_iter()
        .map(|(name, description)| crate::prompt::Choice {
            name,
            label: display_name(name),
            description,
            disabled: false,
        })
        .collect();
    loop {
        let chosen =
            crate::prompt::select("Which Template?", &top, None, crate::prompt::OnEsc::Cancel)?;
        if top[chosen].name != OTF_GROUP {
            return Some(top[chosen].name.to_string());
        }
        let otf: Vec<crate::prompt::Choice<'_>> = template_menu_otf()
            .into_iter()
            .map(|(name, description)| crate::prompt::Choice {
                name,
                label: display_name(name),
                description,
                disabled: false,
            })
            .collect();
        if let Some(chosen) = crate::prompt::select(
            "Which OTF Web Starter?",
            &otf,
            None,
            crate::prompt::OnEsc::Back,
        ) {
            return Some(otf[chosen].name.to_string());
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
        let question = format!("Install with {}?", wanted.name);
        let confirm = format!("install now with {}", wanted.name);
        let choices = [
            crate::prompt::Choice {
                name: wanted.name,
                label: "Yes",
                description: confirm.as_str(),
                disabled: false,
            },
            crate::prompt::Choice {
                name: "skip",
                label: "No",
                description: "write the files and stop",
                disabled: false,
            },
        ];
        // Esc lands on the same answer `No` does: the project is already on
        // disk by now, and cancelling the *install* question is not cancelling
        // the project. Either way the next steps say how to install it.
        let chosen = crate::prompt::select(
            question.as_str(),
            &choices,
            Some(0),
            crate::prompt::OnEsc::Cancel,
        )?;
        return if chosen == 0 { Some(wanted) } else { None };
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

    // Esc lands on the same answer `skip` does: the project is already on disk
    // by now, and cancelling the *install* question is not cancelling the
    // project. Either way the next steps say how to install it.
    let preselect = preferred
        .and_then(|manager| available.iter().position(|m| *m == manager))
        .or(Some(0));
    let chosen = crate::prompt::select(
        "Install the Dependencies?",
        &choices,
        preselect,
        crate::prompt::OnEsc::Cancel,
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

async fn registry_version(
    client: &reqwest::Client,
    registry: &str,
    package: &str,
) -> Result<String, RegistryFailure> {
    let response = client
        .get(registry_url(registry, package))
        .send()
        .await
        .map_err(|error| RegistryFailure {
            message: error.to_string(),
            retryable: true,
        })?;
    let status = response.status();
    if !status.is_success() {
        let retryable = status.is_server_error()
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            || status == reqwest::StatusCode::REQUEST_TIMEOUT;
        return Err(RegistryFailure {
            message: format!("registry returned HTTP {status}"),
            retryable,
        });
    }
    let metadata: serde_json::Value = response.json().await.map_err(|error| RegistryFailure {
        message: format!("invalid registry response: {error}"),
        retryable: true,
    })?;
    metadata
        .get("dist-tags")
        .and_then(|tags| tags.get("latest"))
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .ok_or_else(|| RegistryFailure {
            message: "registry response has no dist-tags.latest version".to_string(),
            retryable: false,
        })
}

fn registry_backoff(attempt: usize, retryable: bool) -> Option<Duration> {
    retryable
        .then(|| REGISTRY_BACKOFFS.get(attempt).copied())
        .flatten()
}

async fn registry_version_with_retries(
    client: &reqwest::Client,
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
                eprintln!(
                    "npm registry lookup for {package} failed on attempt {}/{}, {}; retrying in {} ms",
                    attempt + 1,
                    REGISTRY_BACKOFFS.len() + 1,
                    failure.message,
                    backoff.as_millis()
                );
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
) -> Result<HashMap<String, String>, String> {
    if packages.is_empty() {
        return Ok(HashMap::new());
    }
    eprintln!(
        "Resolving current versions for {} template packages from {}...",
        packages.len(),
        std::env::var("npm_config_registry")
            .or_else(|_| std::env::var("NPM_CONFIG_REGISTRY"))
            .unwrap_or_else(|_| DEFAULT_REGISTRY.to_string())
    );
    let registry = configured_registry()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("esdev/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("cannot create npm registry client: {error}"))?;
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
        eprintln!("  {package}@{version}");
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
            "src/App.tsx",
            "src/entry.client.tsx",
            "_gitignore",
        ] {
            assert!(
                paths.iter().any(|path| path == expected),
                "{expected} is not in the template: {paths:?}"
            );
        }

        let micro_ui = resolved("micro-ui", None);
        for expected in ["package.json", "esdev.json", "index.html", "src/main.ts"] {
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
        assert!(resolve_mode("react", Some("ssr"), ASK).is_err());
        assert!(resolve_mode("api", Some("static"), ASK).is_err());
        assert!(matches!(
            resolve_mode("react", Some("static"), ASK),
            Ok(Mode::Chosen(mode)) if mode == "static"
        ));
        assert!(matches!(resolve_mode("api", None, ASK), Ok(Mode::None)));
    }

    /// `-y` takes every default without asking: no menus, even where a
    /// terminal would have shown them. A test never has a terminal, so the
    /// scripted path is what pins this — the interactive one cannot run here.
    #[test]
    fn yes_resolves_every_default() {
        assert!(matches!(
            resolve_mode("react", None, SCRIPTED),
            Ok(Mode::Chosen(mode)) if mode == "static"
        ));
        assert!(matches!(
            resolve_choice(
                "spa",
                "language",
                "language",
                true,
                LANGUAGES,
                Fallback::Preselected(DEFAULT_LANGUAGE),
                None,
                "Select a Language?",
                SCRIPTED,
            ),
            Ok(Choice::Chosen(language)) if language == DEFAULT_LANGUAGE
        ));
        assert!(matches!(
            resolve_blog("docs", None, SCRIPTED),
            Ok(Blog::On) if DEFAULT_BLOG
        ));
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

        let docs = otf_written("docs", "ts", None, Some(false));
        assert_eq!(
            registry_packages(&docs).expect("valid placeholders"),
            BTreeSet::from([
                "@opentf/esrun-types".to_string(),
                "@opentf/web".to_string(),
                "@opentf/web-cli".to_string(),
                "@opentf/web-docs".to_string(),
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
            if is_otf(template) {
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
            let (_, files) = TEMPLATES
                .iter()
                .find(|(name, _)| *name == template)
                .expect("the template is embedded");
            let files = files_for(files, mode);
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
        const BRANDS: &[&str] = &["My Docs", "My Project", "OTF Web App"];
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
        use Step::{Blog, Language, Manager, Mode, Styling, Template};
        // Nothing behind: out.
        assert_eq!(step_back_index(Template, &[]), None);
        assert_eq!(step_back_index(Mode, &[]), None);
        // The nearest menu behind, skipping steps that never asked.
        assert_eq!(
            step_back_index(Mode, &[Template]),
            Some(0),
            "Esc at the mode returns to the template menu"
        );
        assert_eq!(
            step_back_index(Blog, &[Template, Language]),
            Some(2),
            "a modeless step never asked, so it is skipped"
        );
        assert_eq!(
            step_back_index(Styling, &[Template, Mode, Language, Styling]),
            Some(2),
            "a step's own earlier visit does not count as behind it"
        );
        assert_eq!(
            step_back_index(Language, &[Template, Template]),
            Some(0),
            "re-asked steps retreat the same way"
        );
        assert_eq!(
            step_back_index(Manager, &[Template, Blog]),
            Some(4),
            "the manager is the last question"
        );
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
    /// Scoped to the esdev-native set: the OTF templates are `otfw` projects
    /// until that migration lands, and are covered by their own tests.
    #[test]
    fn esdev_templates_ship_a_config_and_a_test() {
        for template in ["api", "react", "vanilla", "micro-ui", "lib"] {
            // `react` is two projects; the shared files (with the config and
            // the tests) are in both, so either mode proves the point.
            let mode = default_mode(template);
            let paths = resolved(template, mode);
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
        for (name, _) in TEMPLATES {
            let description = DESCRIPTIONS
                .iter()
                .find(|(template, _)| template == name)
                .map(|(_, description)| *description)
                .unwrap_or("");
            assert!(
                !description.is_empty(),
                "{name} has no description in DESCRIPTIONS"
            );
        }
    }

    /// Menus show the proper form while flags stay lowercase: `SPA`, not
    /// `spa` — and what is chosen still resolves to the flag spelling.
    #[test]
    fn menus_show_proper_names_for_lowercase_flags() {
        for (flag, label) in [
            ("api", "API"),
            ("docs", "Docs"),
            ("fullstack", "FullStack"),
            ("lib", "Library"),
            ("micro-ui", "Micro-UI"),
            ("react", "ReactJS"),
            ("spa", "SPA"),
            ("vanilla", "Vanilla"),
            ("static", "Static"),
            ("js", "JS"),
            ("ts", "TS"),
            ("css", "CSS"),
            ("tailwind", "Tailwind"),
            ("OTF Web", "OTF Web"),
            ("npm", "npm"),
        ] {
            assert_eq!(display_name(flag), label, "{flag} displays wrong");
        }
    }

    /// The first menu holds this repository's templates plus the OTF group —
    /// four more lines there is the flat list this grouping replaces.
    #[test]
    fn the_template_menu_groups_the_otf_starters() {
        let top = template_menu_top();
        let names: Vec<&str> = top.iter().map(|(name, _)| *name).collect();
        for otf in OTF_ORDER {
            assert!(!names.contains(otf), "{otf} leaked into the first menu");
        }
        assert_eq!(top.last().map(|(name, _)| *name), Some(OTF_GROUP));
        for (_, description) in &top {
            assert!(
                !description.is_empty(),
                "the menu shows an unexplained entry"
            );
        }
        // Behind the group: the four starters, in `create-web`'s order.
        let otf = template_menu_otf();
        let names: Vec<&str> = otf.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, OTF_ORDER);
    }

    /// The OTF Web starter set is embedded whole: apps, docs with its demo
    /// blog on disk, and the library without anything Bun-only.
    #[test]
    fn the_otf_templates_are_in_the_binary() {
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
        let docs = resolved("docs", None);
        for expected in [
            "package.json",
            "otfw.config.js",
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

    /// The OTF answers resolved from flags, without a terminal to ask on.
    /// Unit tests never see a TTY, so `None` here is the unattended default.
    #[test]
    fn the_otf_axes_default_and_refuse() {
        assert!(matches!(
            resolve_choice(
                "spa",
                "language",
                "language",
                true,
                LANGUAGES,
                Fallback::Preselected(DEFAULT_LANGUAGE),
                None,
                "Select a Language?",
                ASK,
            ),
            Ok(Choice::Chosen(language)) if language == DEFAULT_LANGUAGE
        ));
        assert!(matches!(
            resolve_choice(
                "spa",
                "styling",
                "styling",
                true,
                STYLINGS,
                Fallback::Unasked(UNSTYLED),
                None,
                "Select a Styling Solution?",
                ASK,
            ),
            Ok(Choice::Chosen(styling)) if styling == UNSTYLED
        ));
        assert!(matches!(
            resolve_blog("docs", None, ASK),
            Ok(Blog::On) if DEFAULT_BLOG
        ));
        assert!(matches!(
            resolve_choice(
                "api",
                "language",
                "language",
                false,
                LANGUAGES,
                Fallback::Preselected(DEFAULT_LANGUAGE),
                None,
                "Select a Language?",
                ASK,
            ),
            Ok(Choice::NotApplicable)
        ));

        assert!(matches!(
            resolve_choice(
                "fullstack",
                "language",
                "language",
                true,
                LANGUAGES,
                Fallback::Preselected(DEFAULT_LANGUAGE),
                Some("ts"),
                "Select a Language?",
                ASK,
            ),
            Ok(Choice::Chosen(language)) if language == "ts"
        ));
        assert!(matches!(
            resolve_blog("docs", Some(false), ASK),
            Ok(Blog::Off)
        ));
        assert!(
            resolve_choice(
                "api",
                "language",
                "language",
                false,
                LANGUAGES,
                Fallback::Preselected(DEFAULT_LANGUAGE),
                Some("elm"),
                "Select a Language?",
                ASK,
            )
            .is_err()
        );
        assert!(
            resolve_choice(
                "spa",
                "language",
                "language",
                true,
                LANGUAGES,
                Fallback::Preselected(DEFAULT_LANGUAGE),
                Some("elm"),
                "Select a Language?",
                ASK,
            )
            .is_err()
        );
        // Styling is not a docs axis, and the blog is not a spa one.
        assert!(
            resolve_choice(
                "docs",
                "styling",
                "styling",
                false,
                STYLINGS,
                Fallback::Unasked(UNSTYLED),
                Some("css"),
                "Select a Styling Solution?",
                ASK,
            )
            .is_err()
        );
        assert!(resolve_blog("spa", Some(true), ASK).is_err());
        assert!(resolve_blog("fullstack", Some(false), ASK).is_err());
        assert!(matches!(
            resolve_blog("spa", None, ASK),
            Ok(Blog::NotApplicable)
        ));
        assert!(matches!(
            resolve_blog("fullstack", None, ASK),
            Ok(Blog::NotApplicable)
        ));
    }

    /// The OTF answers applied, as `create` would write them.
    fn otf_written(
        template: &str,
        language: &str,
        styling: Option<&str>,
        blog: Option<bool>,
    ) -> Vec<(String, Vec<u8>)> {
        let (_, files) = TEMPLATES
            .iter()
            .find(|(name, _)| *name == template)
            .expect("the template is embedded");
        let files = files_for(files, None);
        apply_otf(
            template,
            &Otf {
                language: language.to_string(),
                styling: styling.map(str::to_string),
                blog,
            },
            &files,
        )
    }

    fn otf_text(files: &[(String, Vec<u8>)], path: &str) -> String {
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
        let files = otf_written("spa", "ts", Some("css"), None);
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"app/page.tsx"));
        assert!(paths.contains(&"app/layout.tsx"));
        assert!(paths.contains(&"tsconfig.json"));
        assert!(paths.contains(&"app/otfw-env.d.ts"));
        assert!(!paths.contains(&"app/page.jsx"));
        assert!(!paths.contains(&"jsconfig.json"));

        let layout = otf_text(&files, "app/layout.tsx");
        assert!(
            layout.contains("({ children }: { children: unknown })"),
            "the layout types its props: {layout}"
        );
        let page = otf_text(&files, "app/page.tsx");
        assert!(
            page.contains("app/page.tsx"),
            "references follow the rename: {page}"
        );
        let readme = otf_text(&files, "README.md");
        assert!(
            readme.contains("app/page.tsx"),
            "the README names the file that exists: {readme}"
        );
        let env = otf_text(&files, "app/otfw-env.d.ts");
        assert!(
            env.contains("declare const $state"),
            "macros declared: {env}"
        );

        let tsconfig: serde_json::Value =
            serde_json::from_str(&otf_text(&files, "tsconfig.json")).expect("valid JSON");
        assert_eq!(
            tsconfig["include"],
            serde_json::json!(["app"]),
            "apps include their tree"
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
        let docs = otf_written("docs", "ts", None, Some(false));
        let tsconfig: serde_json::Value =
            serde_json::from_str(&otf_text(&docs, "tsconfig.json")).expect("valid JSON");
        assert_eq!(
            tsconfig["include"],
            serde_json::json!(["app", "otfw.config.js"])
        );

        let library = otf_written("library", "ts", None, None);
        let paths: Vec<&str> = library.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"index.ts"));
        assert!(paths.contains(&"src/Counter.tsx"));
        assert!(paths.contains(&"otfw-env.d.ts"));
        assert!(!paths.contains(&"index.js"));
        let manifest: serde_json::Value =
            serde_json::from_str(&otf_text(&library, "package.json")).expect("valid JSON");
        assert_eq!(manifest["exports"]["."], serde_json::json!("./index.ts"));
        assert_eq!(manifest["files"], serde_json::json!(["index.ts", "src"]));
        let counter = otf_text(&library, "src/Counter.tsx");
        assert!(
            counter.contains("{ initial = 0 }: { initial?: number }"),
            "the counter types its prop: {counter}"
        );
        // The esdev test reads the manifest, so it needs no patch per mode.
        let test = otf_text(&library, "tests/counter.test.js");
        assert!(test.contains("runtime:test"));
        assert!(!test.contains("bun:test"));
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
            let files = otf_written(template, "ts", styling, blog);
            let manifest: serde_json::Value =
                serde_json::from_str(&otf_text(&files, "package.json")).expect("valid JSON");
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

            let js = otf_written(template, "js", styling, blog);
            let manifest: serde_json::Value =
                serde_json::from_str(&otf_text(&js, "package.json")).expect("valid JSON");
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
                serde_json::from_str(&otf_text(&files, "package.json"))
                    .expect("valid template package.json");
            assert_eq!(
                manifest["devDependencies"]["typescript"],
                registry_placeholder("typescript"),
                "{template} {mode:?}: TypeScript should resolve at scaffold time"
            );
        }
    }

    #[test]
    fn static_template_readmes_show_their_default_dev_ports() {
        for (template, mode, port) in [
            ("vanilla", None, 5173),
            ("react", Some("static"), 5173),
            ("react", Some("fullstack"), 8080),
        ] {
            let files = plain_written(template, mode);
            let readme = otf_text(&files, "README.md");
            assert!(
                readme.contains(&format!("http://localhost:{port}")),
                "{template} {mode:?}: README has the wrong dev URL: {readme}"
            );
        }
    }

    #[test]
    fn react_refresh_template_returns_a_source_map() {
        let files = plain_written("react", Some("static"));
        let plugin = otf_text(&files, "plugins/react-refresh.mjs");
        assert!(plugin.contains("sourcesContent: [code]"));
        assert!(plugin.contains("const mappings ="));
        assert!(plugin.contains("\";;;;\" +"));
    }

    #[test]
    fn both_react_modes_opt_into_oxc_react_compiler() {
        for mode in ["static", "fullstack"] {
            let files = plain_written("react", Some(mode));
            let config: serde_json::Value =
                serde_json::from_str(&otf_text(&files, "esdev.json")).expect("valid esdev.json");
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
            serde_json::from_str::<serde_json::Value>(&otf_text(files, "package.json"))
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
        for (template, language, styling, blog) in [
            ("spa", "js", Some("css"), None),
            ("spa", "ts", Some("css"), None),
            ("docs", "js", None, Some(true)),
            ("docs", "ts", None, Some(false)),
        ] {
            assert_eq!(
                scripts(&otf_written(template, language, styling, blog))["preview"],
                serde_json::json!("esdev preview --dir=dist"),
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
                scripts(&otf_written(template, language, styling, blog))
                    .get("preview")
                    .is_none(),
                "{template} {language}: nothing servable to preview"
            );
        }
    }

    /// JavaScript writes what is embedded: no config, no renames.
    #[test]
    fn javascript_writes_the_embedded_files() {
        let files = otf_written("spa", "js", Some("css"), None);
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&"app/page.jsx"));
        assert!(paths.contains(&"jsconfig.json"));
        assert!(!paths.contains(&"tsconfig.json"));
        assert!(!paths.iter().any(|path| path.ends_with(".tsx")));
    }

    /// Tailwind is a prepend, not a project shape — and docs already has it.
    #[test]
    fn tailwind_prepends_the_import() {
        let plain = otf_written("spa", "js", Some("css"), None);
        assert!(!otf_text(&plain, "app/global.css").contains("@import"));
        let plain_manifest: serde_json::Value =
            serde_json::from_str(&otf_text(&plain, "package.json")).expect("valid JSON");
        assert!(
            plain_manifest["devDependencies"]
                .get("tailwindcss")
                .is_none(),
            "plain CSS pulls no compiler"
        );
        let tw = otf_written("spa", "js", Some("tailwind"), None);
        assert!(otf_text(&tw, "app/global.css").starts_with("@import \"tailwindcss\";\n\nbody {"));
        // An `@import` the project resolves is a dependency the project
        // declares: npm hoisting (upstream's default manager) finds the
        // toolchain's copy, and a strict `node_modules` layout does not.
        let tw_manifest: serde_json::Value =
            serde_json::from_str(&otf_text(&tw, "package.json")).expect("valid JSON");
        assert_eq!(
            tw_manifest["devDependencies"]["tailwindcss"],
            serde_json::json!(registry_placeholder("tailwindcss"))
        );
        let docs = otf_written("docs", "js", None, Some(false));
        let docs_manifest: serde_json::Value =
            serde_json::from_str(&otf_text(&docs, "package.json")).expect("valid JSON");
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
        let bare = otf_written("docs", "js", None, Some(false));
        assert!(
            !bare.iter().any(|(path, _)| path.starts_with("app/blog/")),
            "no blog means no blog files"
        );
        let config = otf_text(&bare, "otfw.config.js");
        assert!(!config.contains("blog:"));

        let blogged = otf_written("docs", "js", None, Some(true));
        assert!(
            blogged
                .iter()
                .any(|(path, _)| path == "app/blog/hello-world/page.mdx"),
            "yes means the sample post"
        );
        let config = otf_text(&blogged, "otfw.config.js");
        assert!(config.contains("href: \"/blog\""));
        assert!(config.contains("dir: \"blog\""));
        let page = otf_text(&blogged, "app/docs/page.mdx");
        assert!(page.contains("## Blog (demo)"));
        assert!(page.contains("## Edit Content"));
    }

    fn plain_written(template: &str, mode: Option<&str>) -> Vec<(String, Vec<u8>)> {
        let (_, files) = TEMPLATES
            .iter()
            .find(|(name, _)| *name == template)
            .expect("the template is embedded");
        files_for(files, mode)
            .into_iter()
            .map(|(path, contents)| (path, contents.to_vec()))
            .collect()
    }

    /// The templates with a page take a styling; the rest refuse one.
    #[test]
    fn styling_is_offered_where_there_is_a_page() {
        for template in ["react", "vanilla", "micro-ui", "spa", "fullstack"] {
            let options = stylings(template).unwrap_or_else(|| panic!("{template}"));
            let names: Vec<&str> = options.iter().map(|(name, _)| *name).collect();
            assert_eq!(names, ["css", "tailwind"], "{template}");
        }
        for template in ["api", "lib", "docs", "library"] {
            assert!(stylings(template).is_none(), "{template}");
        }
        // Every template the list names is one that is embedded.
        for template in ESDEV_STYLED {
            assert!(
                TEMPLATES.iter().any(|(name, _)| name == template),
                "{template}"
            );
        }
    }

    /// Tailwind is three things — the stylesheet, the `<link>` in the head,
    /// the dependency — in every template and every mode that takes it.
    #[test]
    fn tailwind_adds_a_linked_stylesheet_and_its_dependency() {
        for (template, mode) in [
            ("vanilla", None),
            ("micro-ui", None),
            ("react", Some("static")),
            ("react", Some("fullstack")),
        ] {
            let files = apply_tailwind(plain_written(template, mode));
            assert_eq!(
                otf_text(&files, TAILWIND_STYLESHEET),
                "@import \"tailwindcss\";\n",
                "{template}"
            );
            let html = otf_text(&files, "index.html");
            let link = html
                .find("<link rel=\"stylesheet\" href=\"./src/styles.css\" />")
                .unwrap_or_else(|| panic!("{template}: no link in\n{html}"));
            assert!(link < html.find("</head>").expect("a head"), "{template}");
            let manifest: serde_json::Value =
                serde_json::from_str(&otf_text(&files, "package.json")).expect("still JSON");
            assert!(
                manifest.pointer("/devDependencies/tailwindcss").is_some(),
                "{template}: {manifest}"
            );
        }
    }

    /// Plain writes what it always wrote: no stylesheet, no dependency.
    #[test]
    fn plain_css_leaves_the_template_as_it_was() {
        let files = plain_written("vanilla", None);
        assert!(files.iter().all(|(path, _)| path != TAILWIND_STYLESHEET));
        assert!(!otf_text(&files, "package.json").contains("tailwindcss"));
    }
}
