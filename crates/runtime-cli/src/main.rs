//! `esrun` — a standalone CLI that runs JavaScript on the ES-Runtime.
//!
//! This is the thin executable wrapper around the embeddable `runtime` library:
//! it wires the default tokio providers (system clock, OS entropy, reqwest
//! networking, a stdout/stderr console), constructs a [`Runtime`], evaluates the
//! given source, and drives it to completion on the [`Driver`]. The runtime
//! itself owns no loop and no I/O — everything host-facing is injected here, so
//! this file *is* the standalone embedding (SPEC.md §8).
//!
//! ```text
//! esrun script.js          # run a file
//! esrun -e "console.log(1)" # run an inline snippet
//! esrun --version | --help
//! ```

// A CLI's whole job is to talk to the terminal.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;
use std::sync::Arc;

use es_runtime_common::{CapabilitySet, Limits};
use es_runtime_default_providers::Driver;
use es_runtime_default_providers::{OsEntropy, ReqwestTransport, SystemClock, TokioTimers};
use es_runtime_providers::{Console, ConsoleLevel};
use es_runtime_runtime::{HostProviders, Runtime, V8Engine};

const USAGE: &str = "\
esrun — run JavaScript on the ES-Runtime

USAGE:
    esrun <file.js>          Run a JavaScript file
    esrun -e <code>          Run an inline snippet
    esrun --help             Show this help
    esrun --version          Show the version

The full WinterTC surface is available (console, URL, fetch, crypto, streams,
encoding, timers, events). All host capabilities are granted.";

/// A console that prints to the process's stdout/stderr, like Node/Deno.
struct StdoutConsole;

impl Console for StdoutConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        match level {
            ConsoleLevel::Warn | ConsoleLevel::Error => eprintln!("{message}"),
            _ => println!("{message}"),
        }
    }
}

/// What to run, parsed from argv.
enum Source {
    File(String),
    Inline(String),
}

fn parse_args() -> Result<Source, String> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        return Err(format!("missing script argument\n\n{USAGE}"));
    };
    match first.as_str() {
        "-h" | "--help" => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        "-V" | "--version" => {
            println!("esrun {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        "-e" | "--eval" => {
            let code = args
                .next()
                .ok_or_else(|| "-e/--eval requires a code argument".to_string())?;
            Ok(Source::Inline(code))
        }
        flag if flag.starts_with('-') => Err(format!("unknown option: {flag}\n\n{USAGE}")),
        path => Ok(Source::File(path.to_string())),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("esrun: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let (source, label) = match parse_args()? {
        Source::File(path) => {
            let code =
                std::fs::read_to_string(&path).map_err(|e| format!("cannot read {path}: {e}"))?;
            (code, path)
        }
        Source::Inline(code) => (code, "<eval>".to_string()),
    };

    // Default providers — the standalone embedding's host surface.
    let clock = Arc::new(SystemClock::new());
    let timers = Arc::new(TokioTimers);
    let net = Arc::new(ReqwestTransport::new().map_err(|e| format!("http transport: {e}"))?);
    let providers = HostProviders::new(
        clock.clone(),
        Arc::new(StdoutConsole),
        net,
        Arc::new(OsEntropy),
    );

    let engine = V8Engine::new(Limits::default()).map_err(|e| format!("engine: {e}"))?;
    let mut runtime = Runtime::new(Box::new(engine), providers).map_err(|e| e.to_string())?;
    // A trusted local script: grant the full capability set.
    runtime.set_capabilities(CapabilitySet::all());

    // Wrap in an async IIFE so top-level `await` works — the engine evaluates a
    // classic script, which disallows it; real ES-module top-level await is a
    // later feature. A runtime throw becomes an unhandled rejection, reported
    // below. Syntax errors still surface here, labelled with the source.
    let wrapped = format!("(async () => {{\n{source}\n}})();");
    runtime
        .eval(&wrapped)
        .map_err(|e| format!("{label}: {e}"))?;

    let driver = Driver::new(clock, timers);
    let rejections = driver.run_to_completion(&mut runtime).await;
    if !rejections.is_empty() {
        for message in &rejections {
            eprintln!("Uncaught (in promise): {message}");
        }
        return Err(format!(
            "{} unhandled promise rejection(s)",
            rejections.len()
        ));
    }
    Ok(())
}
