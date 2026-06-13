//! OS-backed [`Process`] — the real environment, working directory, and
//! platform, plus an exit-code cell. The standalone embedding's host process
//! view (DECISIONS D24).
//!
//! `args` are **supplied by the embedder** (the CLI knows which argv entries are
//! the user's, after the binary and the script/`-e` code); everything else is
//! read from the OS. `env` is snapshotted on each [`Process::env`] call.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use es_runtime_providers::{Process, ProviderError};

/// A [`Process`] reading the host environment/cwd/platform, with caller-provided
/// program arguments and a recorded exit code.
pub struct SystemProcess {
    args: Vec<String>,
    exit: Arc<ExitCell>,
}

#[derive(Default)]
struct ExitCell {
    requested: AtomicBool,
    code: AtomicI32,
}

impl SystemProcess {
    /// Builds a process view exposing `args` as the program arguments and the
    /// real OS environment/cwd/platform.
    pub fn new(args: Vec<String>) -> Self {
        SystemProcess {
            args,
            exit: Arc::new(ExitCell::default()),
        }
    }
}

impl Process for SystemProcess {
    fn env(&self) -> Vec<(String, String)> {
        std::env::vars().collect()
    }

    fn args(&self) -> Vec<String> {
        self.args.clone()
    }

    fn cwd(&self) -> Result<String, ProviderError> {
        let dir = std::env::current_dir()
            .map_err(|e| ProviderError::Other(format!("cannot read working directory: {e}")))?;
        Ok(dir.to_string_lossy().into_owned())
    }

    fn platform(&self) -> String {
        std::env::consts::OS.to_string()
    }

    fn exit(&self, code: i32) {
        self.exit.code.store(code, Ordering::SeqCst);
        self.exit.requested.store(true, Ordering::SeqCst);
    }

    fn requested_exit_code(&self) -> Option<i32> {
        self.exit
            .requested
            .load(Ordering::SeqCst)
            .then(|| self.exit.code.load(Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_args_and_records_exit() {
        let p = SystemProcess::new(vec!["a".into(), "b".into()]);
        assert_eq!(p.args(), ["a", "b"]);
        assert!(!p.platform().is_empty());
        assert!(p.cwd().is_ok());
        assert_eq!(p.requested_exit_code(), None);
        p.exit(3);
        assert_eq!(p.requested_exit_code(), Some(3));
    }
}
