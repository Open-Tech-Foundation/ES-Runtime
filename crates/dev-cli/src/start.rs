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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{Event, RecursiveMode, Watcher};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc};

use crate::build::{BuildRequest, Dev, ProjectBuild};
use crate::config::{Output, Project};
use crate::devserver::{DevServer, RELOAD_PATH};

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

/// What `esdev start` was asked to do.
pub struct StartConfig {
    /// The project, and everything it builds.
    pub project: Project,
    /// How long a child gets to drain before it is killed — `--shutdown-grace`,
    /// the same number production uses.
    pub grace: Duration,
}

/// Runs the dev loop until the user interrupts it.
pub async fn start(config: StartConfig) -> Result<(), String> {
    let project = Arc::new(config.project);
    let serve = serve_dir(&project)?;

    // Bound before the first build, so a port already in use is an error at the
    // top rather than after a build the developer then has to watch happen
    // again.
    let (listener, port) = bind(project.start.port)?;
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
    let (tx, mut rx) = mpsc::unbounded_channel::<()>();
    // Held for the duration: dropping the watcher stops the thread behind it.
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        if let Ok(event) = res
            && crate::watch::is_change(&event.kind)
            && event
                .paths
                .iter()
                .any(|path| is_source(path, &scope, &ignored))
        {
            let _ = tx.send(());
        }
    })
    .map_err(|e| format!("cannot start the file watcher: {e}"))?;
    watcher
        .watch(&root, RecursiveMode::Recursive)
        .map_err(|e| format!("cannot watch {}: {e}", root.display()))?;

    match &serve {
        Some(dir) => eprintln!(
            "esdev: serving {} on http://127.0.0.1:{port}",
            dir.strip_prefix(&root).unwrap_or(dir).display()
        ),
        None => eprintln!("esdev: reload endpoint on http://127.0.0.1:{port}{RELOAD_PATH}"),
    }
    if project.start.port.is_none() && port != DEFAULT_PORT {
        eprintln!("esdev: {DEFAULT_PORT} was taken; use --port to pin one");
    }
    eprintln!("esdev: watching {}", root.display());

    let run = project.start.run.clone();
    let watched = project.start.watch.clone();
    let permissions = project.permissions.clone();
    let output = match &run {
        Some(name) => Some(running_output(&project, name)?),
        None => None,
    };
    let exe = std::env::current_exe().map_err(|e| format!("cannot find the esdev binary: {e}"))?;

    // The first build is allowed to fail like any other: the loop below is what
    // a developer fixes it in.
    let built = rebuild(&project, &watched, port).await;
    let mut child = match (&output, built) {
        (Some(output), true) => spawn(&exe, output, &permissions, &root)?,
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
        match woken {
            Woken::Interrupted => {
                if let Some(process) = &mut child {
                    crate::watch::stop(process, config.grace).await;
                }
                return Ok(());
            }
            Woken::Exited => continue,
            Woken::Changed => {}
        }

        // **Rebuilt before anything is stopped.** A syntax error mid-edit is
        // the most ordinary event there is, and the server you were about to
        // fix it on should still be answering — including while the build runs,
        // which is why the stop is here and not above.
        if !rebuild(&project, &watched, port).await {
            continue;
        }
        if let Some(process) = &mut child {
            crate::watch::stop(process, config.grace).await;
        }
        if let Some(output) = &output {
            child = spawn(&exe, output, &permissions, &root)?;
        }
        // After the restart, not before: a page told to reload while the server
        // is still coming back gets a connection refused and stays blank.
        let _ = reload.send(());
    }
}

/// Why the loop woke up.
enum Woken {
    /// A watched file changed.
    Changed,
    /// The server exited on its own.
    Exited,
    /// ^C, or the watcher went away.
    Interrupted,
}

impl From<Option<()>> for Woken {
    fn from(change: Option<()>) -> Self {
        match change {
            Some(()) => Self::Changed,
            None => Self::Interrupted,
        }
    }
}

/// Builds the project in dev mode, reporting whether it worked.
///
/// The error is printed rather than returned, because in a loop a failed build
/// is a message and not an exit: the developer is mid-edit, and the tool's job
/// is to still be there when they finish.
async fn rebuild(project: &Arc<Project>, watched: &[String], port: u16) -> bool {
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
        dev: Some(Dev { reload_port: port }),
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
) -> Result<Option<Child>, String> {
    let child = Command::new(exe)
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
async fn wait_for_change(rx: &mut mpsc::UnboundedReceiver<()>) -> Option<()> {
    crate::watch::coalesce(rx).await
}

/// Where the output that `start.run` names lands.
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
