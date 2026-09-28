//! `esdev start` — the dev loop: build, run, rebuild, reload.
//!
//! # It is `esdev build` on a loop, plus the two things a loop needs
//!
//! That framing is the design, not a summary of it. A dev build and a release
//! build differ in exactly two ways — `NODE_ENV` is `"development"` and nothing
//! is content-hashed — and in nothing else, because a dev and prod that
//! disagree about how a module *resolves* is the failure this whole toolchain
//! is arranged to prevent. Everything else here is the loop around it: watch,
//! rebuild, restart, tell the browser.
//!
//! # What runs the app is the app
//!
//! For a fullstack or backend project, `dev.run` names the target whose
//! output is the server, and that output is run as a child process with esdev's
//! normal development permissions. **It is the same file production runs**, on
//! the same runtime, under the same capability model — there is no development
//! server standing in for it, no middleware wrapping it, and no second code
//! path that only exists on a developer's machine.
//!
//! A frontend-only project has no such target, and there esdev serves the
//! output directory itself ([`crate::devserver`]) — because telling somebody to
//! write a server before they can look at their page is not parity with
//! anything.
//!
//! # A restart is a SIGTERM, and a rebuild that fails changes nothing
//!
//! The restart policy is `--watch`'s, for `--watch`'s reasons: a fresh process
//! cannot carry anything forward, and `SIGTERM` is the graceful stop production
//! gets, so a request in flight when a file is saved is answered rather than
//! dropped.
//!
//! What is new here is the build in front of it, and the rule that goes with
//! it: **a failed build leaves everything running.** A syntax error mid-edit is
//! the most ordinary event in a dev loop, and the right response to it is a
//! message and the server you already had — not a dead port and a browser that
//! cannot load the page that would tell you what you broke.
//!
//! # Two ports, and neither of them fights for one
//!
//! There are two: esdev's own endpoint ([`bind`]) and the port the application
//! binds ([`app_port`]). Both follow the same rule — **a port that was named is
//! a promise and a port that was not is a convenience** — and only one of them
//! has a flag.
//!
//! `--port` is **the port you open**, which is the application's whenever the
//! project has a server of its own. esdev's endpoint is plumbing: it carries one
//! message to the page, nobody types its address, and it takes a free port
//! quietly. A frontend project has no server of its own, so there esdev *is*
//! what is being opened and `--port` is this listener's.
//!
//! Before this, `--port` was the endpoint's in every case and only the endpoint
//! moved. Two projects open in two terminals both ran their server on whatever
//! `dev.app.port` named, so the second one died on a bound port — on a number
//! the developer had not chosen and had no reason to be thinking about, with the
//! one flag named after ports pointing somewhere else.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ignore::Match;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, RecursiveMode, Watcher};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc};

use crate::build::{BuildRequest, Dev, ProjectBuild};
use crate::config::Output;
use crate::devserver::{DevServer, Update};
use crate::settings::Settings;

/// The port the endpoint binds when the config does not say.
///
/// Vite's, deliberately: a developer who has seen a dev server before has seen
/// this number, and there is nothing to gain by being novel about it.
const DEFAULT_PORT: u16 = 5173;

/// Binds the endpoint, and reports the port it actually got.
///
/// **A port that was named is a promise, and a port that was not is a
/// convenience.** So the two cases are deliberately not the same:
///
/// * `--port=8080`, or `"port": 8080` in `esdev.json`, binds *that* port or
///   fails. Something is already there, and moving quietly to another one would
///   leave a bookmark, a proxy rule or a second terminal pointing at whatever
///   that something is. The message names what to do about it.
/// * Nothing named binds [`DEFAULT_PORT`] if it can, and any free port if it
///   cannot. A second project in a second terminal is an ordinary afternoon,
///   and refusing to start over a number nobody chose is the tool inventing a
///   problem. The port it settled on is printed, because it is now the only
///   place the URL exists.
///
/// `--port=0` asks for the second behaviour explicitly, and is what a script
/// that reads the printed URL should pass.
fn bind(wanted: Option<u16>) -> Result<(std::net::TcpListener, u16), String> {
    let listener = match wanted {
        Some(port) => std::net::TcpListener::bind(crate::devserver::address(port)).map_err(|e| {
            format!(
                "cannot bind 127.0.0.1:{port}: {e}\n\n                 Something is already listening there. Stop it, or start on \
                 another port with `--port=<n>` — or drop the flag and let \
                 esdev pick a free one."
            )
        })?,
        None => std::net::TcpListener::bind(crate::devserver::address(DEFAULT_PORT))
            .or_else(|_| std::net::TcpListener::bind(crate::devserver::address(0)))
            .map_err(|e| format!("cannot bind a port on 127.0.0.1: {e}"))?,
    };
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read the port just bound: {e}"))?
        .port();
    Ok((listener, port))
}

/// Where the application's own server listens in development.
#[derive(Debug)]
struct AppPort {
    /// The port, handed to the child as `PORT`.
    port: u16,
    /// The port the project asked for, when nothing named one and it was busy.
    /// A port that *was* named and then moved would be a broken promise, so
    /// there is no such case: it is an error instead.
    moved_from: Option<u16>,
}

/// Settles the port the application will listen on.
///
/// # Why esdev has an opinion about this at all
///
/// Because otherwise two projects fight over one number. The application reads
/// `PORT` and falls back to its configured `dev.app.port`, so a second
/// project can move to a free one and receive it through `PORT`.
///
/// So the same rule the endpoint follows applies here: `--port=3000` is a
/// **promise** and fails if something holds it, and an unnamed port is a
/// **convenience** — the project's own is tried first, and if it is busy a free
/// one is taken and printed.
///
/// `dev.app.port` is the app's declared default. It only moves the port when
/// that setting says the server follows the `PORT` environment variable; a
/// project binding some other way needs no opinion from esdev.
fn app_port(listen: Option<u16>, wanted: Option<u16>) -> Result<Option<AppPort>, String> {
    let Some(listen) = listen else {
        return match wanted {
            None => Ok(None),
            Some(port) => Err(format!(
                "a pinned app port ({port}) needs `dev.app.port` in {} to say which port the app uses by default.\n\n                 Add `\"app\": {{ \"port\": 8080 }}` under `dev`; the server must read `PORT` to use a moved port.",
                crate::config::FILE_NAME,
            )),
        };
    };

    let port = match wanted {
        // Named, so it is a promise: something else holding it is an error
        // rather than a reason to quietly serve on a different address.
        Some(port) => {
            free(port).map_err(|e| {
                format!(
                    "cannot start the app on port {port}: {e}\n\n                     Something is already listening there. Stop it or choose another port with `--port=<n>` or change `dev.app.port`; remove the pin to let esdev choose."
                )
            })?;
            port
        }
        None => match free(listen) {
            Ok(()) => listen,
            // Taken. A second project in a second terminal is an ordinary
            // afternoon, and refusing to start over a number that came with the
            // template is the tool inventing a problem.
            Err(_) => {
                any_free().map_err(|e| format!("cannot find a free port for the app: {e}"))?
            }
        },
    };

    Ok(Some(AppPort {
        port,
        // Only an unnamed port can have moved. A named port is a promise, so
        // reporting it as a fallback would misdescribe the user's choice.
        moved_from: (wanted.is_none() && port != listen).then_some(listen),
    }))
}

/// Whether a port can be listened on, by listening on it and letting go.
///
/// Racy by construction — something can take it between here and the child's
/// own bind — and that is the same race every tool that picks a port runs. The
/// alternative is binding it here and passing the socket down, which would make
/// esdev part of how the application listens, and the whole point is that it is
/// not.
///
/// `0.0.0.0` rather than loopback, because that is what the templates bind and a
/// port is only free if it is free the way the child will ask for it.
fn free(port: u16) -> std::io::Result<()> {
    std::net::TcpListener::bind(("0.0.0.0", port)).map(drop)
}

/// A port nothing is listening on, chosen by the operating system.
fn any_free() -> std::io::Result<u16> {
    std::net::TcpListener::bind(("0.0.0.0", 0))?
        .local_addr()
        .map(|addr| addr.port())
}

/// Ready timing in human units: whole milliseconds under a second, one
/// decimal over it. Pure, so the banner's shape tests without a listener.
/// Shared with `run --watch`, which times its restarts in the same words.
pub(crate) fn format_duration(elapsed: std::time::Duration) -> String {
    if elapsed.as_secs() == 0 {
        format!("{}ms", elapsed.as_millis())
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

/// One line of loop narration, from what was sent to the page and whether
/// the server is a new process. Pure, so the vocabulary tests without one.
fn cycle_summary(update: &Update, replaced_the_server: bool) -> String {
    let summary = match update {
        Update::Patch { changed_ids, .. } => {
            let n = changed_ids.len();
            format!("hot-swapped {n} module{}", if n == 1 { "" } else { "s" })
        }
        Update::Css => "swapped stylesheet".to_string(),
        Update::Reload if replaced_the_server => "restarted server".to_string(),
        Update::Reload => "reloaded".to_string(),
        // Never routed here: failures send before the cycle line. Named
        // anyway, so the vocabulary stays total if that ever changes.
        Update::Error { .. } => "errored".to_string(),
    };
    match (update, replaced_the_server) {
        (Update::Reload, _) => summary,
        (_, true) => format!("restarted server, {summary}"),
        (_, false) => summary,
    }
}

/// What `esdev start` was asked to do.
pub struct StartConfig {
    /// The project, and everything it builds, with the flags applied.
    pub project: Settings,
    /// Whether a change is patched into the running page rather than reloading
    /// it. On unless `--no-hot`.
    pub hot: bool,
    /// How long a child gets to drain before it is killed — `--shutdown-grace`,
    /// the same number production uses.
    pub grace: Duration,
}

/// Runs the dev loop until the user interrupts it.
pub async fn start(config: StartConfig) -> Result<(), String> {
    let started = std::time::Instant::now();
    let project = Arc::new(config.project);
    let serve = serve_dir(&project)?;

    // **`--port` is the port you open**, and which process that is depends on
    // the project. A project with a server of its own opens *that*, and esdev's
    // endpoint beside it is plumbing — it carries one message to the page and
    // nobody types its address. A frontend project has no such server, so esdev
    // is the one being opened and the flag is this listener's.
    //
    // The alternative — `--port` for the endpoint and a second flag for the
    // application — gives the name everybody's dev server uses to the one thing
    // here that is not a dev server.
    let opens_its_own = project.start.run.is_some();

    // Bound before the first build, so a port already in use is an error at the
    // top rather than after a build the developer then has to watch happen
    // again.
    let (listener, port) = bind(if opens_its_own {
        // Nothing to pin it to and nothing asking for one. On a project with a
        // server of its own this endpoint carries one message to the page, its
        // address is written into the page by the build, and no human ever types
        // it — so it takes a free port and says nothing about which.
        None
    } else {
        project.start.port
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot bind 127.0.0.1:{port}: {e}"))?;
    // A handful of slots: every open page holds one reload stream, and a burst
    // of them is a burst of the same word.
    let (reload, _) = broadcast::channel(16);
    let (error, _) = tokio::sync::watch::channel(None::<String>);
    tokio::spawn(crate::devserver::serve(
        listener,
        Arc::new(DevServer {
            serve: serve.clone(),
            reload: reload.clone(),
            error: error.clone(),
        }),
    ));

    let root = project.source.root.clone();
    let ignored = output_dirs(&project);
    let ignore_rules = IgnoreRules::load(&root)?;
    let watch_roots = watch_roots(&project);
    let scopes = watch_roots.clone();
    // The *paths*, not just the fact of a change: a stylesheet can be swapped
    // into a running page and everything else has to reload it, and only the
    // path says which this was.
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    // Held for the duration: dropping the watcher stops the thread behind it.
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && crate::watch::is_change(&event.kind)
        {
            for path in &event.paths {
                if ignore_rules.refresh(path) {
                    continue;
                }
                let rules = ignore_rules.current();
                if scopes
                    .iter()
                    .any(|scope| is_source(path, scope, &ignored, rules.as_ref()))
                {
                    let _ = tx.send(path.clone());
                }
            }
        }
    })
    .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    for path in &watch_roots {
        let mode = if path.is_dir() {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        watcher
            .watch(path, mode)
            .map_err(|e| format!("cannot watch {}: {e}", path.display()))?;
    }

    let paint = crate::style::Palette::stderr();
    let tag = paint.dim("esdev:");
    // Only where the flag exists to act on. On a project that runs its own
    // server this listener has no flag and no reader, so a note about which port
    // it landed on is noise about plumbing.
    if !opens_its_own && project.start.port.is_none() && port != DEFAULT_PORT {
        eprintln!("{tag} {DEFAULT_PORT} was taken; use --port to pin one");
    }

    let run = project.start.run.clone();
    let watched = project.start.watch.clone();
    let output = match &run {
        Some(name) => Some(running_output(&project, name)?),
        None => None,
    };
    // Only for a project that runs a server of its own. A frontend project has
    // no child to give a port to, and esdev is already serving its output.
    let app = match &output {
        Some(_) => app_port(project.start.listen, project.start.port)?,
        None => None,
    };
    if let Some(app) = &app {
        // The reason before the result, so the line a developer's eye lands on
        // is the URL rather than an aside about a port they are leaving behind.
        // The URL itself prints with the banner below, once the first build
        // has run — one line for it, not two.
        if let Some(asked) = app.moved_from {
            eprintln!("{tag} {asked} was taken; use --port to pin one");
        }
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the esdev binary: {e}"))?;

    // The first build is allowed to fail like any other: the loop below is what
    // a developer fixes it in.
    let built = match rebuild(&project, &watched, port, config.hot).await {
        Ok(()) => true,
        Err(err) => {
            eprintln!("esdev: {err}");
            error.send_replace(Some(crate::devserver::strip_ansi(&err)));
            false
        }
    };

    // The banner goes last: ready means listening *and* built, with the first
    // build's own lines above it. One block with no per-line tags — the
    // process is the brand — and the URL as an affordance: it names what to
    // open, never plumbing (no served directory, no watch path, no ws://
    // line). A project with a server of its own names the app's URL instead
    // of this endpoint's; without one known, the endpoint's is the only URL
    // there is. The app sentence stays recognizable: the test helper reads it.
    eprintln!(
        "esdev {} — ready in {}",
        env!("CARGO_PKG_VERSION"),
        format_duration(started.elapsed())
    );
    eprintln!();
    match &app {
        Some(app) => eprintln!(
            "  {}  the app is on {}",
            paint.dim("→"),
            paint.cyan(format_args!("http://localhost:{}/", app.port))
        ),
        None => eprintln!(
            "  {}  Local:   {}",
            paint.dim("→"),
            paint.cyan(format_args!("http://localhost:{port}/"))
        ),
    }
    let mut child = match (&output, built) {
        (Some(output), true) => spawn(&exe, output, &root, app.as_ref().map(|a| a.port))?,
        _ => None,
    };

    loop {
        let woken = match &mut child {
            Some(process) => {
                tokio::select! {
                    status = process.wait() => {
                        report_exit(status);
                        child = None;
                        // Not a reason to rebuild: nothing changed. The
                        // watcher outlives the program, because a program that
                        // exited is one the developer is about to fix.
                        Woken::Exited
                    }
                    change = wait_for_change(&mut rx) => Woken::from(change),
                    () = interrupt() => Woken::Interrupted,
                }
            }
            None => {
                tokio::select! {
                    change = wait_for_change(&mut rx) => Woken::from(change),
                    () = interrupt() => Woken::Interrupted,
                }
            }
        };
        // The paths come out of the match rather than being counted and thrown
        // away: the loop's tail decides what to tell the page, and only what
        // changed says whether a stylesheet swap will do.
        let changed = match woken {
            Woken::Interrupted => {
                if let Some(process) = &mut child {
                    crate::watch::stop(process, config.grace).await;
                }
                return Ok(());
            }
            Woken::Exited => continue,
            Woken::Changed(paths) => paths,
        };

        // What the server was reading, before the build replaces any of it.
        let before = output.as_deref().map(fingerprint);

        // **Before the rebuild, not after.** A hot update is computed by
        // scanning what changed against the graph the page is running; a full
        // build consumes exactly that change first, so asking afterwards gets
        // an honest answer to the wrong question — nothing changed since the
        // last build — and every save falls back to a reload.
        //
        // The full build still happens, and has to: what is on disk is what a
        // hard refresh and every page opened after this one will load, and a
        // patch updates neither.
        let cycle = std::time::Instant::now();
        let hot = if config.hot && !changed.is_empty() {
            crate::build::hot_update(&changed).await
        } else {
            None
        };

        // **Rebuilt before anything is stopped.** A syntax error mid-edit is
        // the most ordinary event there is, and the server you were about to
        // fix it on should still be answering — including while the build runs,
        // which is why the stop is here and not above.
        if let Err(err) = rebuild(&project, &watched, port, config.hot).await {
            eprintln!("esdev: {err}");
            // Keep this as current state as well as notifying connected pages:
            // clients that open or reconnect during the failure need the same
            // message. No cycle line: the codeframe above is the message.
            error.send_replace(Some(crate::devserver::strip_ansi(&err)));
            continue;
        }

        // **Restarted only if the build changed something it reads.** Editing a
        // stylesheet or a browser component rebuilds the client bundle and
        // leaves `server.js` byte for byte identical, and stopping a healthy
        // server to start the same one again costs every open connection, every
        // warm cache the process had, and a window where requests are refused —
        // to deliver nothing. A child that is not running is started whatever
        // the answer, because the developer is fixing the reason it stopped.
        let restarting = child.is_none() || before != output.as_deref().map(fingerprint);
        if restarting {
            if let Some(process) = &mut child {
                crate::watch::stop(process, config.grace).await;
            }
            if let Some(output) = &output {
                child = spawn(&exe, output, &root, app.as_ref().map(|a| a.port))?;
            }
        }
        // **Waited for, not assumed.** `spawn` returns when the process starts,
        // not when it is listening, and the page's very next act is to fetch
        // something from it — the patch, or the document. A page that arrives in
        // that window gets a connection refused, and a hot update that cannot be
        // fetched is a page that reloads: the one edit Fast Refresh exists for,
        // answered by exactly what it exists to avoid.
        if restarting && let Some(app) = &app {
            wait_until_listening(app.port).await;
        }

        // After the restart, not before: a page told to reload while the server
        // is still coming back gets a connection refused and stays blank. Sent
        // either way — the browser has new bundles to fetch whether or not the
        // server moved.
        //
        // A restart makes the question moot: the process the page is talking to
        // is a new one, so whatever it had is stale however narrow the edit was.
        // `restarting` alone will not do — a project with no server of its own
        // has no child, so it reads as "restarting" on every pass, and a
        // stylesheet edit would reload the page it could have swapped.
        let replaced_the_server = restarting && output.is_some();
        // A hot patch is tried first, and a restarted server is not a reason to
        // skip it. The page's state lives in the page: a stateless server coming
        // back as a new process invalidates nothing the browser is holding, and
        // in a fullstack project *every* component edit rebuilds the server
        // bundle — so treating a restart as a reload would mean Fast Refresh
        // never fired for the one project shape it was written for.
        //
        // The patch is still sent after the restart, because the page fetches it
        // from the application's own server and a server still coming back
        // refuses the connection.
        let update = match hot {
            Some(hot) => Update::Patch {
                // Relative, so it is fetched from whatever origin the page is
                // on. Absolute-to-esdev was tried and is worse: a module script
                // is fetched under CORS *and* under the page's CSP, and the
                // template runs its production policy in development on purpose
                // — `script-src 'self'` refuses another origin, exactly as it
                // should. What the application serves, the application serves.
                url: format!("/{}/{}", crate::html::ASSET_DIR, hot.filename),
                changed_ids: hot.changed_ids,
            },
            // A server that was replaced and no patch to offer: the browser has
            // new bundles to fetch either way.
            None if replaced_the_server => Update::Reload,
            // rolldown could not express this change as a patch, or there is no
            // graph to compute one against yet.
            None => update_for(&changed),
        };
        // One line per pass, like the banner: what the save did and how long
        // it took. Untagged, like the banner — the loop's older event lines
        // keep their `esdev:` tags; these two lines are the new vocabulary.
        eprintln!(
            "{} {} in {}",
            paint.green("✓"),
            cycle_summary(&update, replaced_the_server),
            format_duration(cycle.elapsed())
        );
        error.send_replace(None);
        let _ = reload.send(update);
    }
}

/// Why the loop woke up.
enum Woken {
    /// Watched files changed, and these are they — a stylesheet can be swapped
    /// into the running page and anything else cannot, so the paths travel with
    /// the wake rather than being counted and thrown away.
    Changed(Vec<PathBuf>),
    /// The server exited on its own.
    Exited,
    /// ^C, or the watcher went away.
    Interrupted,
}

impl From<Option<Vec<PathBuf>>> for Woken {
    fn from(change: Option<Vec<PathBuf>>) -> Self {
        match change {
            Some(changed) => Self::Changed(changed),
            None => Self::Interrupted,
        }
    }
}

/// Builds the project in dev mode, reporting the failure.
///
/// The error is returned rather than printed: in a loop a failed build is a
/// message and not an exit — the terminal prints it, and so does the page as
/// an overlay — because the developer is mid-edit either way, and the tool's
/// job is to still be there when they finish.
async fn rebuild(
    project: &Arc<Settings>,
    watched: &[String],
    port: u16,
    hot: bool,
) -> Result<(), String> {
    let targets = if watched.is_empty() {
        None
    } else {
        Some(watched.to_vec())
    };
    let request = BuildRequest::Project(Box::new(ProjectBuild {
        settings: Arc::clone(project),
        targets,
        dev: Some(Dev {
            reload_port: port,
            hot,
            outdir: PathBuf::from(project.start.devdir()),
        }),
    }));
    // Unprefixed: callers print it with the terminal's `esdev:` tag, and the
    // loop sends it to the page stripped of paint.
    crate::build::run(request).await
}

/// Starts the application's server as a child process.
///
/// Under esdev's normal development permissions. Production permissions belong
/// on the `esrun` command that deploys the built program.
fn spawn(
    exe: &Path,
    output: &Path,
    root: &Path,
    port: Option<u16>,
) -> Result<Option<Child>, String> {
    let mut command = Command::new(exe);
    // Set rather than merely allowed: the child reads `PORT` and falls back to
    // whatever number it was written with, and that fallback is the one two
    // projects collide on. Nothing is overridden that the developer chose — a
    // `PORT` already in the environment is what [`app_port`] would have found
    // busy, or is the port it settled on.
    if let Some(port) = port {
        command.env("PORT", port.to_string());
    }
    let child = command
        .arg(output)
        .current_dir(root)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", output.display()))?;
    Ok(Some(child))
}

fn report_exit(status: std::io::Result<std::process::ExitStatus>) {
    match status {
        Ok(status) if status.success() => eprintln!("esdev: the server exited"),
        Ok(status) => eprintln!("esdev: the server exited ({status})"),
        Err(e) => eprintln!("esdev: cannot wait for the server: {e}"),
    }
}

/// Resolves after ^C or `SIGTERM`: either way the server is stopped first,
/// so it does not outlive the dev loop that started it.
async fn interrupt() {
    crate::watch::stopped().await;
}

/// Blocks until a change arrives, then swallows the burst that follows it.
async fn wait_for_change(rx: &mut mpsc::UnboundedReceiver<PathBuf>) -> Option<Vec<PathBuf>> {
    crate::watch::coalesce(rx).await
}

/// What to tell the page about a burst of changes.
///
/// **A stylesheet is the one thing that can be replaced in a page that is
/// already running.** Its content is not addressed by anything the document
/// holds — no component owns it, no state depends on it — so re-fetching it and
/// swapping the `<link>` is indistinguishable from having built it that way,
/// and it costs none of what a reload costs: scroll position, an open dialog,
/// whatever was typed into a form.
///
/// Anything else is a reload, and mixtures are too. A burst containing a
/// stylesheet *and* a component is a burst whose module graph moved, and
/// swapping only the styles would leave a page half updated — which is worse
/// than reloading, because it looks like it worked.
fn update_for(changed: &[PathBuf]) -> Update {
    if !changed.is_empty() && changed.iter().all(|path| is_stylesheet(path)) {
        Update::Css
    } else {
        Update::Reload
    }
}

/// Blocks until something accepts on `port`, or long enough that waiting is
/// clearly not the answer.
///
/// A connect, not a health check: what the page needs is a socket that answers,
/// and a server that binds and then fails to route is a different problem with a
/// different message. The bound is generous because a slow first request is
/// better than a reload, and finite because a child that never binds must not
/// wedge the dev loop — the page is told either way, and a page that reloads
/// into a dead server shows the browser's own error, which is the truth.
async fn wait_until_listening(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether a path is a stylesheet, by extension.
///
/// `.module.css` counts: a CSS Module's class names are derived from its
/// *path*, so editing its contents renames nothing and what comes out is the
/// same stylesheet with different rules in it.
fn is_stylesheet(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("css"))
}

/// Where the output that `dev.run` names lands.
/// A fingerprint of everything the running server might read.
///
/// Its own output, and whatever else the build left **beside** it — the
/// template a server splices its render into, a manifest it loads at startup —
/// because a server reads from its own directory and the runtime resolves a
/// relative path against the entry module's.
///
/// The client asset directory is left out, and it is the whole point: it is
/// where every stylesheet and browser bundle lands, so including it would make
/// every CSS edit look like a reason to restart. Nothing in there is read by
/// the server; the browser fetches it over HTTP, from a URL that has not
/// changed.
///
/// **Contents, not timestamps.** Every rebuild rewrites `server.js` whether or
/// not a byte of it changed, so a modification time would say "different" every
/// time and this would be the unconditional restart it replaces. Reading a
/// megabyte twice is nothing beside the build that just produced it.
fn fingerprint(output: &Path) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut entries: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    let Some(dir) = output.parent() else {
        return 0;
    };
    collect(dir, &mut entries);
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut hasher);
    hasher.finish()
}

/// Walks `dir`, skipping the client assets. Unreadable is empty: a directory
/// the build has not written yet is a fingerprint that changes once it has.
fn collect(dir: &Path, into: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == crate::html::ASSET_DIR)
            {
                continue;
            }
            collect(&path, into);
        } else if let Ok(bytes) = std::fs::read(&path) {
            into.push((path, bytes));
        }
    }
}

fn running_output(project: &Settings, name: &str) -> Result<PathBuf, String> {
    let target = project
        .targets
        .iter()
        .find(|target| target.name == name)
        .ok_or_else(|| format!("no target called {name}"))?;
    if target.is_html() {
        return Err(format!(
            "`start`'s `run` names {name}, which builds an HTML file.\n\n\
             There is nothing to run: a document is served, not executed. Leave \
             `run` out and esdev serves the output itself."
        ));
    }
    // Under the dev directory: the loop runs the development build, which is
    // what the last rebuild wrote — never the deployment in `dist/`.
    Ok(project
        .source
        .root
        .join(project.start.devdir())
        .join(crate::build::output_path(target)))
}

/// The directory to serve when no target is run.
///
/// The HTML target's output, because that is what a frontend-only project has
/// and there is nothing else it could sensibly mean. Two of them is a
/// multi-page app, where the directory is shared and either answer is the same
/// one — but nothing is guessed if the config already said.
fn serve_dir(project: &Settings) -> Result<Option<PathBuf>, String> {
    if project.start.run.is_some() {
        return Ok(None);
    }
    if let Some(serve) = &project.start.serve {
        return Ok(Some(project.source.root.join(serve)));
    }
    let mut html = project.targets.iter().filter(|target| target.is_html());
    let Some(first) = html.next() else {
        return Err(format!(
            "there is nothing for `esdev start` to run or serve.\n\n\
             Name the target whose output is your server:\n\n  \
             \"start\": {{ \"run\": \"{}\" }}\n\n\
             …or give it a directory to serve: \"start\": {{ \"serve\": \"dist\" }}.",
            project
                .targets
                .first()
                .map_or("server", |target| target.name.as_str())
        ));
    };
    let Output::Dir(dir) = &first.output else {
        return Err("an HTML target writes a directory".to_string());
    };
    // The development build, like everything else the loop reads: a `serve`
    // directory names somewhere as it stands and is left alone.
    Ok(Some(
        project.source.root.join(project.start.devdir()).join(dir),
    ))
}

/// Every directory the build writes into.
///
/// That is one directory: the dev loop mirrors every target output underneath
/// it, so ignoring it ignores them all. `dist` and `target` are ignored by
/// name already ([`crate::watch`]), but an output directory is whatever the
/// config called it, and only the config knows.
///
/// The watcher has to ignore these or it never settles: a rebuild writes files,
/// the watcher sees them, and it rebuilds.
fn output_dirs(project: &Settings) -> Vec<PathBuf> {
    vec![project.source.root.join(project.start.devdir())]
}

/// Whether a changed path is one to rebuild for.
///
/// A wider net than `--watch`'s, because a build has more inputs than a run
/// does: an `index.html`, a stylesheet and an image in `public` are all things
/// a target names, and a save that appears to do nothing is worse than a
/// rebuild that costs milliseconds.
fn is_source(path: &Path, root: &Path, outputs: &[PathBuf], gitignore: Option<&Gitignore>) -> bool {
    if outputs.iter().any(|output| path.starts_with(output)) {
        return false;
    }
    if gitignore.is_some_and(|rules| {
        matches!(
            rules.matched_path_or_any_parents(path, path.is_dir()),
            Match::Ignore(_)
        )
    }) {
        return false;
    }
    crate::watch::is_interesting(path, root) || crate::watch::is_asset(path, root)
}

/// The project's `.gitignore`, kept current while the loop runs.
///
/// Read once, the rules were whatever the file said when `esdev start` began:
/// a directory added to it afterwards — screenshots, a cache, a browser profile
/// a script writes into — kept triggering rebuilds, and every rebuild reloads
/// the page, until the loop was restarted. The watcher already sees the file
/// change, so that is when the rules are read again.
struct IgnoreRules {
    root: PathBuf,
    file: PathBuf,
    rules: std::sync::RwLock<Option<Gitignore>>,
}

impl IgnoreRules {
    fn load(root: &Path) -> Result<Self, String> {
        Ok(Self {
            root: root.to_path_buf(),
            file: root.join(".gitignore"),
            rules: std::sync::RwLock::new(load_gitignore(root)?),
        })
    }

    /// Whether `path` is the `.gitignore` itself, re-reading it if so. The
    /// file is not a build input, so its own change is never a rebuild. A file
    /// that no longer parses leaves the rules it had; the mistake is the
    /// user's to fix, and dropping every rule would rebuild on everything.
    fn refresh(&self, path: &Path) -> bool {
        if path != self.file {
            return false;
        }
        if let Ok(rules) = load_gitignore(&self.root)
            && let Ok(mut held) = self.rules.write()
        {
            *held = rules;
        }
        true
    }

    fn current(&self) -> std::sync::RwLockReadGuard<'_, Option<Gitignore>> {
        self.rules
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn load_gitignore(root: &Path) -> Result<Option<Gitignore>, String> {
    let path = root.join(".gitignore");
    if !path.is_file() {
        return Ok(None);
    }
    let mut builder = GitignoreBuilder::new(root);
    if let Some(err) = builder.add(&path) {
        return Err(format!("cannot read {}: {err}", path.display()));
    }
    builder
        .build()
        .map(Some)
        .map_err(|err| format!("cannot parse {}: {err}", path.display()))
}

/// The project tree plus every explicitly scoped read path in the config.
///
/// A scoped read path can name an external config, certificate or data file
/// that should rebuild the running program when it changes. Only scoped
/// `--allow-read=...` entries add roots; an unscoped grant has no finite path
/// to register.
fn watch_roots(project: &Settings) -> Vec<PathBuf> {
    let mut roots = vec![project.source.root.clone()];
    for configured in &project.watch_paths {
        let paths: Vec<&str> = match configured.strip_prefix("--allow-read=") {
            Some(paths) => paths.split(',').map(str::trim).collect(),
            None => vec![configured],
        };
        for path in paths.into_iter().filter(|path| !path.is_empty()) {
            let path = Path::new(path);
            let path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                project.source.root.join(path)
            };
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    /// Ready timing reads in human units: milliseconds under a second, one
    /// decimal over it.
    #[test]
    fn ready_timing_reads_in_human_units() {
        assert_eq!(format_duration(std::time::Duration::from_millis(0)), "0ms");
        assert_eq!(
            format_duration(std::time::Duration::from_millis(182)),
            "182ms"
        );
        assert_eq!(
            format_duration(std::time::Duration::from_millis(999)),
            "999ms"
        );
        assert_eq!(
            format_duration(std::time::Duration::from_millis(1000)),
            "1.0s"
        );
        assert_eq!(
            format_duration(std::time::Duration::from_millis(2340)),
            "2.3s"
        );
    }

    /// One loop pass reads as one line: what was sent, and whether the
    /// server is new — a restart never passes as a plain reload.
    #[test]
    fn a_loop_pass_reads_as_one_line() {
        let patch = |n: usize| Update::Patch {
            url: "/_assets/1.js".to_string(),
            changed_ids: (0..n).map(|i| format!("m{i}")).collect(),
        };
        assert_eq!(cycle_summary(&patch(3), false), "hot-swapped 3 modules");
        assert_eq!(cycle_summary(&patch(1), false), "hot-swapped 1 module");
        assert_eq!(
            cycle_summary(&patch(2), true),
            "restarted server, hot-swapped 2 modules"
        );
        assert_eq!(cycle_summary(&Update::Css, false), "swapped stylesheet");
        assert_eq!(
            cycle_summary(&Update::Css, true),
            "restarted server, swapped stylesheet"
        );
        assert_eq!(cycle_summary(&Update::Reload, false), "reloaded");
        assert_eq!(cycle_summary(&Update::Reload, true), "restarted server");
        assert_eq!(
            cycle_summary(
                &Update::Error {
                    message: "boom".to_string()
                },
                false
            ),
            "errored"
        );
    }

    /// A stylesheet is the one thing that can be replaced in a page that is
    /// already running, so it is the one thing that does not cost a reload.
    #[test]
    fn a_stylesheet_only_burst_is_swapped_rather_than_reloaded() {
        assert!(matches!(
            update_for(&paths(&["styles/app.css"])),
            Update::Css
        ));
        // Several, which is what an `@import` chain saved at once looks like.
        assert!(matches!(
            update_for(&paths(&["styles/app.css", "src/app/Callout.module.css"])),
            Update::Css
        ));
        // A CSS Module counts: its class names come from its *path*, so editing
        // its contents renames nothing and the output is the same stylesheet.
        assert!(matches!(
            update_for(&paths(&["src/app/Callout.module.css"])),
            Update::Css
        ));
        assert!(matches!(
            update_for(&paths(&["styles/APP.CSS"])),
            Update::Css
        ));
    }

    /// Anything else moved the module graph, and so did a burst that merely
    /// *contained* something else — swapping only the styles there would leave
    /// a page half updated, which is worse than reloading it, because it looks
    /// like it worked.
    #[test]
    fn anything_but_a_stylesheet_reloads() {
        assert!(matches!(
            update_for(&paths(&["src/app/Home.tsx"])),
            Update::Reload
        ));
        assert!(matches!(
            update_for(&paths(&["index.html"])),
            Update::Reload
        ));
        assert!(matches!(
            update_for(&paths(&["styles/app.css", "src/app/Home.tsx"])),
            Update::Reload
        ));
        // A file with no extension at all, which is not a stylesheet by any
        // reading and must not be treated as one by an `unwrap_or(true)`.
        assert!(matches!(update_for(&paths(&["Makefile"])), Update::Reload));
        // And a wake carrying nothing is not an invitation to swap nothing.
        assert!(matches!(update_for(&[]), Update::Reload));
    }

    /// The app gets the declared default port when it is free.
    #[test]
    fn a_free_declared_port_is_the_one_the_app_gets() {
        let port = any_free().expect("a free port");
        let app = app_port(Some(port), None)
            .expect("settled")
            .expect("a movable port");
        assert_eq!(app.port, port);
        assert_eq!(app.moved_from, None);
    }

    /// The collision this exists for: a second project moves to a free port
    /// and the app learns it through `PORT`.
    #[test]
    fn a_taken_declared_port_moves() {
        let held = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("hold a port");
        let taken = held.local_addr().expect("its address").port();
        let app = app_port(Some(taken), None)
            .expect("settled")
            .expect("a movable port");
        assert_ne!(app.port, taken);
        assert_eq!(app.moved_from, Some(taken));
    }

    /// A port that was named is the port that was asked for, so it is not
    /// reported as a fallback from the configured default.
    #[test]
    fn a_named_port_is_not_reported_as_a_move() {
        let free = any_free().expect("a free port");
        let app = app_port(Some(8080), Some(free))
            .expect("settled")
            .expect("a movable port");
        assert_eq!(app.port, free);
        assert_eq!(app.moved_from, None, "a deliberate port read as a fallback");
    }

    /// A named port is a promise, so a busy one is an error rather than a
    /// quiet move to an address nobody is pointing at.
    #[test]
    fn a_named_port_that_is_taken_is_an_error() {
        let held = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("hold a port");
        let taken = held.local_addr().expect("its address").port();
        let refused = app_port(Some(8080), Some(taken)).expect_err("refused");
        assert!(refused.contains("--port"), "{refused}");
    }

    /// Projects without a declared default are left alone unless the user
    /// explicitly pins a port, which cannot be honored safely without it.
    #[test]
    fn a_project_that_does_not_say_where_it_listens_is_left_alone() {
        assert!(app_port(None, None).expect("settled").is_none());
    }

    /// Pinning a port without declaring the app's normal port gives a useful
    /// error rather than silently doing nothing.
    #[test]
    fn naming_a_port_a_project_cannot_use_says_what_is_missing() {
        let refused = app_port(None, Some(3000)).expect_err("refused");
        assert!(refused.contains("dev.app.port"), "{refused}");
        assert!(refused.contains("PORT"), "{refused}");
    }

    /// A directory added to `.gitignore` while `esdev start` runs is ignored
    /// from then on, not only after a restart.
    #[test]
    fn a_gitignore_edited_while_the_loop_runs_is_read_again() {
        let root = std::env::temp_dir().join(format!("esdev-ignore-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("shots")).expect("create project");
        std::fs::write(root.join(".gitignore"), "node_modules/\n").expect("write .gitignore");
        let shot = root.join("shots/01.png");
        let rules = IgnoreRules::load(&root).expect("load");
        assert!(is_source(&shot, &root, &[], rules.current().as_ref()));

        std::fs::write(root.join(".gitignore"), "node_modules/\nshots/\n")
            .expect("edit .gitignore");
        assert!(
            rules.refresh(&root.join(".gitignore")),
            "the file's own change is taken, not rebuilt for"
        );
        assert!(!is_source(&shot, &root, &[], rules.current().as_ref()));
        assert!(!rules.refresh(&shot));

        // A file that no longer parses keeps the rules it had.
        std::fs::write(root.join(".gitignore"), "shots/\n[").expect("break .gitignore");
        rules.refresh(&root.join(".gitignore"));
        assert!(!is_source(&shot, &root, &[], rules.current().as_ref()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_build_s_own_output_is_not_a_change() {
        let root = Path::new("/p");
        let outputs = vec![PathBuf::from("/p/dist"), PathBuf::from("/p/.dev")];
        assert!(!is_source(
            Path::new("/p/dist/server.js"),
            root,
            &outputs,
            None
        ));
        assert!(!is_source(
            Path::new("/p/dist/assets/main.js"),
            root,
            &outputs,
            None
        ));
        assert!(!is_source(
            Path::new("/p/.dev/index.html"),
            root,
            &outputs,
            None
        ));

        assert!(is_source(
            Path::new("/p/src/server.ts"),
            root,
            &outputs,
            None
        ));
        assert!(is_source(Path::new("/p/index.html"), root, &outputs, None));
        assert!(is_source(
            Path::new("/p/public/styles.css"),
            root,
            &outputs,
            None
        ));
    }

    /// The dev loop mirrors every target output under one directory, so the
    /// watcher ignores that directory rather than each output in turn — and a
    /// save can never rebuild into the deployment.
    #[test]
    fn the_dev_directory_is_what_the_watcher_ignores() {
        let project = crate::settings::Settings::from_project(
            crate::config::parse(
                r#"{ "targets": {
                   "server": { "entry": "src/s.ts", "out": "build/server.js" },
                   "web": { "entry": "index.html", "outdir": "public_html" } } }"#,
                PathBuf::from("/p"),
                "esdev.json",
            )
            .expect("parsed")
            .expect("a config"),
        );

        assert_eq!(output_dirs(&project), vec![PathBuf::from("/p/.dev")]);
    }

    /// A named dev directory is the same directory by another name, for the
    /// watcher and for everything the loop runs or serves.
    #[test]
    fn a_named_dev_directory_is_used_everywhere() {
        let project = crate::settings::Settings::from_project(
            crate::config::parse(
                r#"{ "targets": {
                   "server": { "entry": "src/s.ts", "out": "dist/server.js" },
                   "web": { "entry": "index.html", "outdir": "dist" } },
                 "start": { "run": "server", "devdir": "tmp-dev" } }"#,
                PathBuf::from("/p"),
                "esdev.json",
            )
            .expect("parsed")
            .expect("a config"),
        );

        assert_eq!(output_dirs(&project), vec![PathBuf::from("/p/tmp-dev")]);
        assert_eq!(
            running_output(&project, "server").expect("output"),
            PathBuf::from("/p/tmp-dev/dist/server.js")
        );
    }

    /// What the loop runs and serves is the development build: the server
    /// bundle it restarts, and the directory it serves a frontend from, both
    /// mirrored under the dev directory rather than read from the deployment.
    #[test]
    fn the_loop_runs_and_serves_the_development_builds() {
        let backend = crate::settings::Settings::from_project(
            crate::config::parse(
                r#"{ "targets": { "server": { "entry": "src/s.ts", "out": "dist/server.js" } },
                 "start": { "run": "server" } }"#,
                PathBuf::from("/p"),
                "esdev.json",
            )
            .expect("parsed")
            .expect("a config"),
        );
        assert_eq!(
            running_output(&backend, "server").expect("output"),
            PathBuf::from("/p/.dev/dist/server.js")
        );
        assert!(serve_dir(&backend).expect("served").is_none());

        let frontend = crate::settings::Settings::from_project(
            crate::config::parse(
                r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
                PathBuf::from("/p"),
                "esdev.json",
            )
            .expect("parsed")
            .expect("a config"),
        );
        assert_eq!(
            serve_dir(&frontend).expect("served"),
            Some(PathBuf::from("/p/.dev/dist"))
        );
    }

    #[test]
    fn configured_watch_paths_are_additional_roots() {
        let project = crate::settings::Settings::from_project(
            crate::config::parse(
                r#"{ "build": { "targets": { "web": { "entry": "index.html", "outdir": "dist" } } },
                "dev": { "watch": { "paths": ["./config,local", "/etc/example"] } } }"#,
                PathBuf::from("/p"),
                "esdev.json",
            )
            .expect("config")
            .expect("project"),
        );
        assert_eq!(
            watch_roots(&project),
            vec![
                PathBuf::from("/p"),
                PathBuf::from("/p/config,local"),
                PathBuf::from("/etc/example")
            ]
        );
    }
}
