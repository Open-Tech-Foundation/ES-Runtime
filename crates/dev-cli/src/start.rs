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
//! For a fullstack or backend project, `start.run` names the target whose
//! output is the server, and that output is run as a child process under the
//! grants the config gives it. **It is the same file production runs**, on the
//! same runtime, under the same capability model — there is no development
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
//! `esdev.json` granted, so the second one died on a bound port — on a number
//! the developer had not chosen and had no reason to be thinking about, with the
//! one flag named after ports pointing somewhere else.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, RecursiveMode, Watcher};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc};

use crate::build::{BuildRequest, Dev, ProjectBuild};
use crate::config::{Output, Project};
use crate::devserver::{DevServer, HMR_PATH, Update};

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

/// The grant a project's application port is written as.
const LISTEN: &str = "--allow-listen=";

/// Where the application's own server listens in development, and the grant
/// that lets it.
#[derive(Debug)]
struct AppPort {
    /// The port, handed to the child as `PORT`.
    port: u16,
    /// The port the project asked for, when nothing named one and it was busy.
    /// A port that *was* named and then moved would be a broken promise, so
    /// there is no such case: it is an error instead.
    moved_from: Option<u16>,
    /// The project's permissions with the `listen` grant pointed at `port`.
    permissions: Vec<String>,
}

/// Settles the port the application will listen on.
///
/// # Why esdev has an opinion about this at all
///
/// Because otherwise two projects fight over one number. The application reads
/// `PORT` and falls back to whatever it was written with — 8080, usually — and
/// its `listen` grant names that same port, so a second project started in a
/// second terminal dies on a bound address. Nobody chose 8080; it came with the
/// template.
///
/// So the same rule the endpoint follows applies here: `--port=3000` is a
/// **promise** and fails if something holds it, and an unnamed port is a
/// **convenience** — the project's own is tried first, and if it is busy a free
/// one is taken and printed.
///
/// # It only does this for a project shaped to be told
///
/// Two things have to be true, and both are things the project already says:
/// the `listen` grant narrows to exactly one port, and `env` grants `PORT`.
/// Without the first there is no port to move; without the second the child
/// cannot be told which port it got, and setting the variable would move the
/// grant out from under a server still binding its old number. A project that
/// is not shaped that way is left entirely alone — which is what every backend
/// that binds a socket by some other name needs.
///
/// # The grant moves with it
///
/// The rewritten flag is the same grant with a different number, not a wider
/// one: `--allow-listen=8080` becomes `--allow-listen=8137`. The property this
/// project protects — that development runs under the deployment's grant, so a
/// capability nobody tested is never added on the way to production — is about
/// *which* capabilities, and this changes none of them. The move is printed, so
/// what is running is never a port only esdev knows about.
fn app_port(permissions: &[String], wanted: Option<u16>) -> Result<Option<AppPort>, String> {
    let granted = permissions
        .iter()
        .position(|flag| flag.starts_with(LISTEN))
        .and_then(|at| {
            permissions[at][LISTEN.len()..]
                .parse::<u16>()
                .ok()
                .map(|port| (at, port))
        });
    let tells_the_child = permissions.iter().any(|flag| {
        flag == "--allow-env" || flag.strip_prefix("--allow-env=").is_some_and(names_port)
    });

    let Some((at, granted)) = granted.filter(|_| tells_the_child) else {
        return match wanted {
            None => Ok(None),
            Some(port) => Err(format!(
                "--app-port={port} needs the project to say where its server listens, and                  {} does not.\n\n                 Two things make a port movable, and both are grants you already write:                  `\"listen\": [\"8080\"]`, one port and no more, so there is a port to                  move — and `\"env\": [\"PORT\"]`, so the server can be told which one                  it got.",
                crate::config::FILE_NAME
            )),
        };
    };

    let port = match wanted {
        // Named, so it is a promise: something else holding it is an error
        // rather than a reason to quietly serve on a different address.
        Some(port) => {
            free(port).map_err(|e| {
                format!(
                    "cannot start the app on port {port}: {e}\n\n                     Something is already listening there. Stop it, or name another                      with `--app-port=<n>` — or drop the flag and let esdev pick."
                )
            })?;
            port
        }
        None => match free(granted) {
            Ok(()) => granted,
            // Taken. A second project in a second terminal is an ordinary
            // afternoon, and refusing to start over a number that came with the
            // template is the tool inventing a problem.
            Err(_) => {
                any_free().map_err(|e| format!("cannot find a free port for the app: {e}"))?
            }
        },
    };

    let mut permissions = permissions.to_vec();
    permissions[at] = format!("{LISTEN}{port}");
    Ok(Some(AppPort {
        port,
        // Only an unnamed port can have moved. `--port=3000` on a project
        // granting 8080 is not 8080 being taken — it is the port that was asked
        // for, and reporting it as a fallback would read as a warning about
        // something the developer did on purpose.
        moved_from: (wanted.is_none() && port != granted).then_some(granted),
        permissions,
    }))
}

/// Whether an `--allow-env` scope list includes `PORT`.
fn names_port(scopes: &str) -> bool {
    scopes.split(',').any(|name| name.trim() == "PORT")
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

/// What `esdev start` was asked to do.
pub struct StartConfig {
    /// The project, and everything it builds.
    pub project: Project,
    /// Whether a change is patched into the running page rather than reloading
    /// it. On unless `--no-hot`.
    pub hot: bool,
    /// How long a child gets to drain before it is killed — `--shutdown-grace`,
    /// the same number production uses.
    pub grace: Duration,
}

/// Runs the dev loop until the user interrupts it.
pub async fn start(config: StartConfig) -> Result<(), String> {
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
    tokio::spawn(crate::devserver::serve(
        listener,
        Arc::new(DevServer {
            serve: serve.clone(),
            reload: reload.clone(),
        }),
    ));

    let root = project.dir.clone();
    let ignored = output_dirs(&project);
    let scope = root.clone();
    // The *paths*, not just the fact of a change: a stylesheet can be swapped
    // into a running page and everything else has to reload it, and only the
    // path says which this was.
    let (tx, mut rx) = mpsc::unbounded_channel::<PathBuf>();
    // Held for the duration: dropping the watcher stops the thread behind it.
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && crate::watch::is_change(&event.kind)
        {
            for path in event
                .paths
                .iter()
                .filter(|path| is_source(path, &scope, &ignored))
            {
                let _ = tx.send(path.clone());
            }
        }
    })
    .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;

    let paint = crate::style::Palette::stderr();
    let tag = paint.dim("esdev:");
    // Only where the flag exists to act on. On a project that runs its own
    // server this listener has no flag and no reader, so a note about which port
    // it landed on is noise about plumbing.
    if !opens_its_own && project.start.port.is_none() && port != DEFAULT_PORT {
        eprintln!("{tag} {DEFAULT_PORT} was taken; use --port to pin one");
    }
    match &serve {
        Some(dir) => eprintln!(
            "{tag} serving {} on {}",
            dir.strip_prefix(&root).unwrap_or(dir).display(),
            paint.cyan(format_args!("http://127.0.0.1:{port}"))
        ),
        None => eprintln!(
            "{tag} update channel on {}",
            paint.cyan(format_args!("ws://127.0.0.1:{port}{HMR_PATH}"))
        ),
    }
    eprintln!("{tag} watching {}", paint.dim(root.display()));

    let run = project.start.run.clone();
    let watched = project.start.watch.clone();
    let output = match &run {
        Some(name) => Some(running_output(&project, name)?),
        None => None,
    };
    // Only for a project that runs a server of its own. A frontend project has
    // no child to give a port to, and esdev is already serving its output.
    let app = match &output {
        Some(_) => app_port(&project.permissions, project.start.port)?,
        None => None,
    };
    let permissions = app.as_ref().map_or_else(
        || project.permissions.clone(),
        |app| app.permissions.clone(),
    );
    if let Some(app) = &app {
        // The reason before the result, so the line a developer's eye lands on
        // is the URL rather than an aside about a port they are leaving behind.
        if let Some(asked) = app.moved_from {
            eprintln!("{tag} {asked} was taken; use --port to pin one");
        }
        eprintln!(
            "{tag} the app is on {}",
            paint.cyan(format_args!("http://localhost:{}", app.port))
        );
    }
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the esdev binary: {e}"))?;

    // The first build is allowed to fail like any other: the loop below is what
    // a developer fixes it in.
    let built = rebuild(&project, &watched, port, config.hot).await;
    let mut child = match (&output, built) {
        (Some(output), true) => spawn(
            &exe,
            output,
            &permissions,
            &root,
            app.as_ref().map(|a| a.port),
        )?,
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
        let hot = if config.hot && !changed.is_empty() {
            crate::build::hot_update(&changed).await
        } else {
            None
        };

        // **Rebuilt before anything is stopped.** A syntax error mid-edit is
        // the most ordinary event there is, and the server you were about to
        // fix it on should still be answering — including while the build runs,
        // which is why the stop is here and not above.
        if !rebuild(&project, &watched, port, config.hot).await {
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
                child = spawn(
                    &exe,
                    output,
                    &permissions,
                    &root,
                    app.as_ref().map(|a| a.port),
                )?;
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

/// Builds the project in dev mode, reporting whether it worked.
///
/// The error is printed rather than returned, because in a loop a failed build
/// is a message and not an exit: the developer is mid-edit, and the tool's job
/// is to still be there when they finish.
async fn rebuild(project: &Arc<Project>, watched: &[String], port: u16, hot: bool) -> bool {
    let targets = if watched.is_empty() {
        None
    } else {
        Some(watched.to_vec())
    };
    let request = BuildRequest::Project(Box::new(ProjectBuild {
        project: Arc::clone(project),
        targets,
        minify: false,
        defines: Vec::new(),
        conditions: Vec::new(),
        dev: Some(Dev {
            reload_port: port,
            hot,
        }),
    }));
    match crate::build::run(request).await {
        Ok(()) => true,
        Err(err) => {
            eprintln!("esdev: {err}");
            false
        }
    }
}

/// Starts the application's server as a child process.
///
/// Under the config's `permissions`, spelled as the flags they are — so what
/// runs in development is what the deploy line will say, and a capability the
/// program turns out to need is discovered here rather than in production.
fn spawn(
    exe: &Path,
    output: &Path,
    permissions: &[String],
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
        .args(permissions)
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

/// Resolves after ^C.
async fn interrupt() {
    let _ = tokio::signal::ctrl_c().await;
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

/// Where the output that `start.run` names lands.
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

fn running_output(project: &Project, name: &str) -> Result<PathBuf, String> {
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
    Ok(project.dir.join(crate::build::output_path(target)))
}

/// The directory to serve when no target is run.
///
/// The HTML target's output, because that is what a frontend-only project has
/// and there is nothing else it could sensibly mean. Two of them is a
/// multi-page app, where the directory is shared and either answer is the same
/// one — but nothing is guessed if the config already said.
fn serve_dir(project: &Project) -> Result<Option<PathBuf>, String> {
    if project.start.run.is_some() {
        return Ok(None);
    }
    if let Some(serve) = &project.start.serve {
        return Ok(Some(project.dir.join(serve)));
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
    Ok(Some(project.dir.join(dir)))
}

/// Every directory the build writes into.
///
/// The watcher has to ignore these or it never settles: a rebuild writes files,
/// the watcher sees them, and it rebuilds. `dist` and `target` are ignored by
/// name already ([`crate::watch`]), but an output directory is whatever the
/// config called it, and only the config knows.
fn output_dirs(project: &Project) -> Vec<PathBuf> {
    project
        .targets
        .iter()
        .filter_map(|target| match &target.output {
            Output::Dir(dir) => Some(project.dir.join(dir)),
            Output::File(file) => Path::new(file)
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|parent| project.dir.join(parent)),
        })
        .collect()
}

/// Whether a changed path is one to rebuild for.
///
/// A wider net than `--watch`'s, because a build has more inputs than a run
/// does: an `index.html`, a stylesheet and an image in `public` are all things
/// a target names, and a save that appears to do nothing is worse than a
/// rebuild that costs milliseconds.
fn is_source(path: &Path, root: &Path, outputs: &[PathBuf]) -> bool {
    if outputs.iter().any(|output| path.starts_with(output)) {
        return false;
    }
    crate::watch::is_interesting(path, root) || crate::watch::is_asset(path, root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(list: &[&str]) -> Vec<String> {
        list.iter().map(ToString::to_string).collect()
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
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

    /// The ordinary shape: one port granted, `PORT` readable, nothing holding
    /// it. The app gets the port the project asked for, and the grant still
    /// names exactly that port.
    #[test]
    fn a_free_granted_port_is_the_one_the_app_gets() {
        let granted = any_free().expect("a free port");
        let permissions = flags(&[
            "--deny-all",
            "--allow-read=./dist",
            "--allow-env=PORT",
            &format!("--allow-listen={granted}"),
        ]);

        let app = app_port(&permissions, None)
            .expect("settled")
            .expect("a movable port");
        assert_eq!(app.port, granted);
        assert_eq!(app.moved_from, None);
        assert!(
            app.permissions
                .contains(&format!("--allow-listen={granted}"))
        );
        // Nothing else about the grant moved.
        assert!(app.permissions.contains(&"--allow-read=./dist".to_string()));
        assert_eq!(app.permissions.len(), permissions.len());
    }

    /// The collision this exists for: a second project whose granted port is
    /// held by the first. It moves, it says so, and its grant follows it.
    #[test]
    fn a_taken_port_moves_and_takes_its_grant_with_it() {
        let held = std::net::TcpListener::bind(("0.0.0.0", 0)).expect("hold a port");
        let taken = held.local_addr().expect("its address").port();
        let permissions = flags(&["--allow-env=PORT", &format!("--allow-listen={taken}")]);

        let app = app_port(&permissions, None)
            .expect("settled")
            .expect("a movable port");
        assert_ne!(app.port, taken);
        assert_eq!(app.moved_from, Some(taken));
        assert!(
            app.permissions
                .contains(&format!("--allow-listen={}", app.port))
        );
        assert!(
            !app.permissions.contains(&format!("--allow-listen={taken}")),
            "the old port is still granted: {:?}",
            app.permissions
        );
    }

    /// A port that was named is the port that was asked for, whatever the grant
    /// says — so it is not reported as a fallback from one.
    #[test]
    fn a_named_port_is_not_reported_as_a_move() {
        let free = any_free().expect("a free port");
        let permissions = flags(&["--allow-env=PORT", "--allow-listen=8080"]);

        let app = app_port(&permissions, Some(free))
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
        let permissions = flags(&["--allow-env=PORT", "--allow-listen=8080"]);

        let refused = app_port(&permissions, Some(taken)).expect_err("refused");
        assert!(refused.contains("--app-port"), "{refused}");
    }

    /// The two halves that make a port movable. Without either of them the
    /// project is left exactly as it was — a backend that binds by some other
    /// name is not something esdev should be rewriting.
    #[test]
    fn a_project_that_does_not_say_where_it_listens_is_left_alone() {
        // No `listen` grant at all.
        assert!(
            app_port(&flags(&["--allow-env=PORT"]), None)
                .expect("settled")
                .is_none()
        );
        // A grant that is not one port: a host, or several.
        assert!(
            app_port(
                &flags(&["--allow-env=PORT", "--allow-listen=8080,9090"]),
                None
            )
            .expect("settled")
            .is_none()
        );
        assert!(
            app_port(&flags(&["--allow-env=PORT", "--allow-listen"]), None)
                .expect("settled")
                .is_none()
        );
        // No way to tell the child which port it got.
        assert!(
            app_port(&flags(&["--allow-listen=8080"]), None)
                .expect("settled")
                .is_none()
        );
        assert!(
            app_port(&flags(&["--allow-env=HOME", "--allow-listen=8080"]), None)
                .expect("settled")
                .is_none()
        );
        // An unnarrowed env grant covers PORT, so that one is movable.
        assert!(
            app_port(&flags(&["--allow-env", "--allow-listen=8080"]), None)
                .expect("settled")
                .is_some()
        );
    }

    /// Asking for a port on a project that cannot be told about one is refused
    /// with the two grants that would make it work, rather than accepted and
    /// silently ignored.
    #[test]
    fn naming_a_port_a_project_cannot_use_says_what_is_missing() {
        let refused = app_port(&flags(&["--allow-listen=8080"]), Some(3000)).expect_err("refused");
        assert!(refused.contains("listen"), "{refused}");
        assert!(refused.contains("PORT"), "{refused}");
    }

    #[test]
    fn the_build_s_own_output_is_not_a_change() {
        let root = Path::new("/p");
        let outputs = vec![PathBuf::from("/p/dist"), PathBuf::from("/p/.dev")];
        assert!(!is_source(Path::new("/p/dist/server.js"), root, &outputs));
        assert!(!is_source(
            Path::new("/p/dist/assets/main.js"),
            root,
            &outputs
        ));
        assert!(!is_source(Path::new("/p/.dev/index.html"), root, &outputs));

        assert!(is_source(Path::new("/p/src/server.ts"), root, &outputs));
        assert!(is_source(Path::new("/p/index.html"), root, &outputs));
        assert!(is_source(Path::new("/p/public/styles.css"), root, &outputs));
    }

    /// A build target's output directory is whatever the config called it, so
    /// the watcher's fixed list of machine-written names cannot be the whole
    /// answer.
    #[test]
    fn the_output_directories_come_from_the_config() {
        let project = crate::config::parse(
            r#"{ "targets": {
                   "server": { "entry": "src/s.ts", "out": "build/server.js" },
                   "web": { "entry": "index.html", "outdir": "public_html" } } }"#,
            PathBuf::from("/p"),
            "esdev.json",
        )
        .expect("parsed")
        .expect("a config");

        let dirs = output_dirs(&project);
        assert!(dirs.contains(&PathBuf::from("/p/build")), "{dirs:?}");
        assert!(dirs.contains(&PathBuf::from("/p/public_html")), "{dirs:?}");
    }
}
