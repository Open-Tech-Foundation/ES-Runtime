//! `esdev preview` — serve the built output, the way it will be served.
//!
//! # Why a command rather than "use any static server"
//!
//! Because the last thing anybody checks before deploying is whether the
//! *deployment* works, and the dev loop cannot answer that. `esdev start`
//! serves a build that differs from the release one in the two ways it is meant
//! to ([`crate::build`]): `NODE_ENV` is `"development"`, and nothing is
//! content-hashed. So the bundle the browser gets there is not the bundle that
//! ships, and the failures that only appear in a release build — a package that
//! ships a different module under `production`, an asset referenced by a name
//! that hashing changed, a `<link>` the document points at with the wrong path
//! — are exactly the ones the dev loop is blind to.
//!
//! What this does is deliberately small: it serves a directory. It does not
//! build (that is `esdev build`, and running it for you would make "preview the
//! thing I am about to ship" mean "ship something I have not seen"), it patches
//! nothing, and it holds no watcher. The one piece of behaviour it keeps from
//! the dev server is the **index fallback** — a reload on `/about` has to reach
//! the router in the bundle, and a preview that 404s there would be answering a
//! question about itself rather than about the build.
//!
//! # It is not a production server
//!
//! It binds loopback, like every other endpoint this binary opens
//! ([`crate::devserver`]), and there is no flag to widen it. What ships is the
//! *output*, served by whatever you deploy behind — this is a way to look at it
//! before that happens, not a smaller version of it.

use std::path::PathBuf;

use crate::config::{Output, Project};

/// What `esdev preview` was asked to do.
pub struct PreviewConfig {
    /// The directory to serve, from `--dir`. `None` reads the project's config.
    pub dir: Option<String>,
    /// The port to bind, from `--port`.
    pub port: Option<u16>,
    /// `--config`, for a project whose file is not `./esdev.json`.
    pub config: Option<String>,
}

/// The port a preview opens when none was named.
///
/// Not the dev loop's ([`crate::start::DEFAULT_PORT`]), on purpose: previewing
/// while the dev server runs is an ordinary thing to want — comparing the two is
/// most of what a preview is for — and two commands defaulting to one port would
/// make that a port conflict every time.
pub const DEFAULT_PORT: u16 = 4173;

/// Serves the built output until interrupted.
pub async fn run(config: PreviewConfig) -> Result<(), String> {
    let root =
        std::env::current_dir().map_err(|e| format!("cannot read working directory: {e}"))?;
    let dir = match &config.dir {
        Some(dir) => root.join(dir),
        None => {
            let project = crate::config::load(config.config.as_deref())?.ok_or_else(|| {
                format!(
                    "there is no {} here, so nothing says what was built.\n\n\
                     Name the directory: `esdev preview --dir=dist`.",
                    crate::config::FILE_NAME
                )
            })?;
            directory(&project)?
        }
    };

    if !dir.is_dir() {
        return Err(format!(
            "{} is not there.\n\n\
             A preview serves what a build wrote; it does not build. Run \
             `esdev build` first.",
            dir.display()
        ));
    }
    if !dir.join("index.html").is_file() {
        return Err(format!(
            "{} has no index.html.\n\n\
             A preview serves a built site. A project whose output is a server \
             bundle is run rather than served: `esrun {}`.",
            dir.display(),
            dir.join("server.js").display()
        ));
    }

    let (listener, port) = bind(config.port)?;
    // The listener was bound with std, and tokio refuses a blocking socket.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot prepare the listener: {e}"))?;
    // Nothing ever sends on it — a preview has no rebuild to announce — but the
    // server is the dev server, and the dev server has a channel.
    let (reload, _) = tokio::sync::broadcast::channel(1);
    let server = std::sync::Arc::new(crate::devserver::DevServer {
        serve: Some(dir.clone()),
        reload,
    });

    let paint = crate::style::Palette::stderr();
    eprintln!(
        "{} {} {}",
        paint.green("preview"),
        paint.dim("→"),
        paint.cyan(format!("http://127.0.0.1:{port}")),
    );
    eprintln!("{}", paint.dim(format!("  serving {}", dir.display())));

    tokio::select! {
        () = crate::devserver::serve(listener, server) => Ok(()),
        _ = tokio::signal::ctrl_c() => Ok(()),
    }
}

/// The directory a project's build wrote.
///
/// The same answer `esdev start` gives when it has no server to run
/// ([`crate::start`]), and deliberately the same rules: `start.serve` if the
/// file says, otherwise the one HTML target's output. A project **with** a
/// server target is the case that differs — the dev loop runs that server, and
/// a preview cannot, because what it would be running is a production artifact
/// that belongs under `esrun`.
fn directory(project: &Project) -> Result<PathBuf, String> {
    if let Some(serve) = &project.start.serve {
        return Ok(project.dir.join(serve));
    }
    let mut html = project.targets.iter().filter(|target| target.is_html());
    let Some(first) = html.next() else {
        let server = project
            .start
            .run
            .as_deref()
            .or_else(|| project.targets.first().map(|target| target.name.as_str()));
        return Err(match server {
            Some(name) => format!(
                "this project builds no site to preview — \"{name}\" writes a server.\n\n\
                 A server bundle is run rather than served, and it is run by the \
                 binary that will run it in production:\n\n  \
                 esrun --allow-listen=8080 dist/server.js"
            ),
            None => "this project builds nothing to preview.".to_string(),
        });
    };
    if let Some(second) = html.next() {
        return Err(format!(
            "this project builds two sites — \"{}\" and \"{}\" — so which one to \
             preview is not decided.\n\n\
             Name it: `esdev preview --dir=<path>`.",
            first.name, second.name
        ));
    }
    let Output::Dir(dir) = &first.output else {
        return Err("an HTML target writes a directory".to_string());
    };
    Ok(project.dir.join(dir))
}

/// Binds the port, or says which one it took instead.
///
/// The same rule as the dev loop's: a port that was **named** is the one you
/// get or the command fails, because a preview on a port you did not ask for is
/// a URL you will not type. Nothing named takes the default, or any free port if
/// something else already has it.
fn bind(wanted: Option<u16>) -> Result<(std::net::TcpListener, u16), String> {
    let listener = match wanted {
        Some(port) => {
            std::net::TcpListener::bind(crate::devserver::address(port)).map_err(|e| {
                format!(
                    "cannot bind 127.0.0.1:{port}: {e}\n\n\
                 Something is already listening there — the dev loop, or another \
                 preview. Preview on a different port, or stop it."
                )
            })?
        }
        None => std::net::TcpListener::bind(crate::devserver::address(DEFAULT_PORT))
            .or_else(|_| std::net::TcpListener::bind(crate::devserver::address(0)))
            .map_err(|e| format!("cannot bind a port: {e}"))?,
    };
    let port = listener
        .local_addr()
        .map_err(|e| format!("cannot read the port: {e}"))?
        .port();
    Ok((listener, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(text: &str) -> Project {
        crate::config::parse(text, PathBuf::from("/p"), "esdev.json")
            .expect("parse")
            .expect("a project")
    }

    #[test]
    fn the_one_html_targets_output_is_what_gets_served() {
        let found = directory(&project(
            r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }"#,
        ))
        .expect("a directory");
        assert_eq!(found, PathBuf::from("/p/dist"));
    }

    /// `serve` is the answer the config already has for "which directory is the
    /// site", and a preview that ignored it would disagree with the dev loop
    /// about the same project.
    #[test]
    fn start_serve_wins() {
        let found = directory(&project(
            r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" } },
                 "start": { "serve": "public" } }"#,
        ))
        .expect("a directory");
        assert_eq!(found, PathBuf::from("/p/public"));
    }

    /// A server bundle is run, not served — and by `esrun`, which is what will
    /// run it in production.
    #[test]
    fn a_project_with_only_a_server_is_told_to_run_it() {
        let err = directory(&project(
            r#"{ "targets": { "api": { "entry": "src/api.ts", "out": "dist/api.js" } } }"#,
        ))
        .expect_err("refused");
        assert!(err.contains("esrun"), "{err}");
        assert!(err.contains("\"api\""), "{err}");
    }

    /// Two sites and no way to choose between them.
    #[test]
    fn two_html_targets_are_refused_rather_than_guessed_between() {
        let err = directory(&project(
            r#"{ "targets": { "web": { "entry": "index.html", "outdir": "dist" },
                              "docs": { "entry": "docs.html", "outdir": "dist/docs" } } }"#,
        ))
        .expect_err("refused");
        assert!(err.contains("--dir"), "{err}");
    }
}
