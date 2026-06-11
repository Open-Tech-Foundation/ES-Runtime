//! Console output sinks.

use es_runtime_providers::{Console, ConsoleLevel};

/// The default [`Console`]: forwards guest output to `tracing` under the
/// `console` target, mapping each level to the matching `tracing` level
/// (ARCHITECTURE.md §8). The embedder's configured subscriber is the sink.
pub struct TracingConsole;

impl Console for TracingConsole {
    fn write(&self, level: ConsoleLevel, message: &str) {
        match level {
            ConsoleLevel::Debug => tracing::debug!(target: "console", "{message}"),
            ConsoleLevel::Warn => tracing::warn!(target: "console", "{message}"),
            ConsoleLevel::Error => tracing::error!(target: "console", "{message}"),
            // `log`, `info`, and any future level map to the info level.
            _ => tracing::info!(target: "console", "{message}"),
        }
    }
}

/// A [`Console`] that drops everything — makes `console` effectively deniable,
/// consistent with deny-by-default (DECISIONS.md D7).
pub struct NullConsole;

impl Console for NullConsole {
    fn write(&self, _level: ConsoleLevel, _message: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_console_drops_without_panicking() {
        NullConsole.write(ConsoleLevel::Error, "ignored");
    }

    #[test]
    fn tracing_console_does_not_panic() {
        // With no subscriber installed the events are dropped; this just checks
        // every level path is wired.
        let console = TracingConsole;
        for level in [
            ConsoleLevel::Debug,
            ConsoleLevel::Info,
            ConsoleLevel::Log,
            ConsoleLevel::Warn,
            ConsoleLevel::Error,
        ] {
            console.write(level, "hello");
        }
    }
}
