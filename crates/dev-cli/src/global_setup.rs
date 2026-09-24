//! Global setup: modules run once before any test file, in a process of their
//! own, and torn down after the last (DECISIONS D106).
//!
//! A process rather than this one, because what a global setup starts — a
//! database, a server — has to keep running while the test files do, and this
//! process is Rust waiting on children. The tests reach it the way they would
//! reach any other service, and what they need to find it by is handed over
//! with `provide` and read with `inject`, as JSON.
//!
//! **The end is the parent closing the process's stdin.** A parent that exits
//! early, or is killed, closes it too, so teardown still runs rather than a
//! server being left behind.

use std::path::{Path, PathBuf};
use std::process::{ExitCode, Stdio};

use es_runtime_cli_common::args::RunOptions;
use es_runtime_cli_common::{Config, Source};

use crate::test::TestConfig;
use crate::transform::TypeStripper;

/// A running global setup process, its `setup` functions done.
pub struct Running {
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    provided: PathBuf,
    relay: Option<tokio::task::JoinHandle<()>>,
}

/// Starts the global setup and waits for its `setup` functions to finish —
/// or `None` when the project has none, or the run starts no test.
pub async fn start(exe: &Path, config: &TestConfig) -> Result<Option<Running>, String> {
    if config.global_setup.is_empty() || config.list {
        return Ok(None);
    }
    let provided =
        std::env::temp_dir().join(format!("esdev-test-provided-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&provided);
    let mut command = tokio::process::Command::new(exe);
    command
        .arg("test")
        .arg(format!("--_global-setup-out={}", provided.display()))
        .args(
            config
                .global_setup
                .iter()
                .map(|module| format!("--global-setup={module}")),
        )
        .stdin(Stdio::piped());
    // Its own process group, so ^C at the terminal reaches only esdev, which
    // then ends the tests and closes stdin: teardown runs rather than being
    // interrupted with everything else.
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x0000_0200); // CREATE_NEW_PROCESS_GROUP
    // With a machine reporter on stdout, what the setup prints goes to stderr,
    // as what the tests print does.
    let quiet = !config.terminal_human();
    if quiet {
        command.stdout(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|err| format!("cannot start global setup: {err}"))?;
    let relay = child.stdout.take().map(|mut out| {
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut out, &mut tokio::io::stderr()).await;
        })
    });
    let stdin = child.stdin.take();
    // Ready is the file of what it provided appearing; ending first is setup
    // having failed, and it has already said why.
    while !provided.exists() {
        tokio::select! {
            status = child.wait() => {
                if let Some(relay) = relay {
                    let _ = relay.await;
                }
                let how = status.map_or_else(|err| err.to_string(), |status| status.to_string());
                return Err(format!("global setup failed ({how}), so no test ran"));
            }
            () = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
        }
    }
    Ok(Some(Running {
        child,
        stdin,
        provided,
        relay,
    }))
}

impl Running {
    /// The file of what `setup` provided, for the test files to read.
    pub fn provided(&self) -> &Path {
        &self.provided
    }

    /// What `setup` provided, as JSON.
    pub fn provided_json(&self) -> Option<String> {
        std::fs::read_to_string(&self.provided).ok()
    }

    /// Runs the teardowns, and waits for them.
    pub async fn stop(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        let status = self.child.wait().await;
        if let Some(relay) = self.relay.take() {
            let _ = relay.await;
        }
        let _ = std::fs::remove_file(&self.provided);
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(format!("global teardown failed ({status})")),
            Err(err) => Err(format!("global teardown failed: {err}")),
        }
    }
}

/// Starts it, or reports why it could not be and says so in the exit code.
pub async fn start_or_report(exe: &Path, config: &TestConfig) -> Result<Option<Running>, ExitCode> {
    start(exe, config).await.map_err(|err| {
        eprintln!("error: {err}");
        ExitCode::FAILURE
    })
}

/// Stops it, turning a teardown that failed into a failed run.
pub async fn stop_into(running: Option<Running>, code: ExitCode) -> ExitCode {
    let Some(running) = running else {
        return code;
    };
    match running.stop().await {
        Ok(()) => code,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// This process is the global setup: import each module in turn, run its
/// `setup`, hand over what it provided, wait for the tests, then run the
/// teardowns newest first.
pub async fn run(config: &TestConfig, out: &Path) -> ExitCode {
    crate::guest::test::configure_global_setup(out.to_path_buf());
    // Global setup is the suite's infrastructure, not code under test, so it
    // runs with esdev's own grant rather than a rehearsed one.
    let (capabilities, scopes) = match crate::test_capabilities(&[]) {
        Ok(granted) => granted,
        Err(err) => {
            es_runtime_cli_common::diagnostics::print_error(&err);
            return ExitCode::FAILURE;
        }
    };
    let transform = match crate::plugins::transform(
        &config.plugin_dir,
        &config.plugins,
        std::sync::Arc::new(TypeStripper::with_jsx(config.jsx.clone())),
    )
    .await
    {
        Ok(transform) => transform,
        Err(err) => {
            es_runtime_cli_common::diagnostics::print_error(&err);
            return ExitCode::FAILURE;
        }
    };
    let run = Config {
        source: Source::Inline(entry(&config.global_setup)),
        args: Vec::new(),
        capabilities,
        scopes,
        options: RunOptions::default(),
        transform: Some(transform),
        bundler_style_resolution: true,
        package_converter: Some(crate::commonjs::converter()),
        extensions: crate::guest::test_extensions(false),
        observer: None,
        inspector: None,
    };
    match es_runtime_cli_common::run("esdev", run).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            es_runtime_cli_common::diagnostics::print_error(&err);
            ExitCode::FAILURE
        }
    }
}

/// The module the global setup process runs.
///
/// Setups run in the order written, each module imported just before its
/// own; teardowns in reverse. A setup that throws still has the teardowns of
/// those before it run, since what they started is running.
fn entry(modules: &[String]) -> String {
    let urls = serde_json::to_string(modules).unwrap_or_else(|_| "[]".to_string());
    format!(
        r#"const ops = globalThis.__ops;
const provided = Object.create(null);
const context = Object.freeze({{
  provide(key, value) {{
    if (typeof key !== "string") throw new TypeError("provide needs a string key");
    const text = JSON.stringify(value);
    if (text === undefined) {{
      throw new TypeError(`provide(${{JSON.stringify(key)}}): a test file receives JSON, and ${{typeof value}} is not`);
    }}
    provided[key] = JSON.parse(text);
  }},
}});
const teardowns = [];
let failure;
try {{
  for (const url of {urls}) {{
    const module = await import(url);
    const setup = module.setup ?? module.default;
    if (setup !== undefined && typeof setup !== "function") {{
      throw new TypeError(`${{url}}: its setup is not a function`);
    }}
    const teardown = setup ? await setup(context) : undefined;
    if (typeof teardown === "function") teardowns.push(teardown);
    if (typeof module.teardown === "function") teardowns.push(module.teardown);
  }}
  ops.test_global_ready(JSON.stringify(provided));
  await ops.test_global_wait();
}} catch (err) {{
  failure = [err];
}}
for (const teardown of teardowns.reverse()) {{
  try {{
    await teardown();
  }} catch (err) {{
    (failure ??= []).push(err);
  }}
}}
if (failure) {{
  for (const err of failure.slice(1)) console.error(err);
  throw failure[0];
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_imports_each_module_by_its_url_in_order() {
        let source = entry(&["file:///p/a.ts".to_string(), "file:///p/b.ts".to_string()]);
        assert!(source.contains(r#"for (const url of ["file:///p/a.ts","file:///p/b.ts"])"#));
        assert!(source.contains("teardowns.reverse()"));
    }
}
