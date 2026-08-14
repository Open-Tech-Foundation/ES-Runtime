//! Installing a scaffolded project's dependencies, with a package manager the
//! user named.
//!
//! # Why this exists now and did not before
//!
//! D64 refused to install, and the reason it gave was exact: *"there is no
//! lockfile yet to say which package manager this project uses, and guessing
//! wrong leaves a `package-lock.json` in a bun project"*. That is an argument
//! against **guessing**, and it holds. It is not an argument against asking.
//!
//! So the rule is unchanged in the case it was written for — a non-interactive
//! run still writes files and stops — and the interactive one resolves the
//! objection at its root by getting the answer from the person who knows it.
//!
//! This is still not a package installer. It runs *theirs*: the command is the
//! one they would have typed, and everything about resolution, the registry,
//! the lockfile and the network belongs to it.

use std::path::Path;
use std::process::Command;

/// A package manager this machine has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Manager {
    /// The binary, and what `--install` spells.
    pub name: &'static str,
    /// What it calls "install everything in the manifest".
    pub command: &'static str,
}

/// Every package manager, in the order they are offered.
///
/// npm first because it is what a Node installation already has, so it is the
/// answer that is right most often. The rest are here because a project that
/// uses one and is installed with another ends up with two lockfiles.
pub const MANAGERS: &[Manager] = &[
    Manager {
        name: "npm",
        command: "install",
    },
    Manager {
        name: "bun",
        command: "install",
    },
    Manager {
        name: "pnpm",
        command: "install",
    },
    Manager {
        name: "yarn",
        command: "install",
    },
];

/// Looks a manager up by name.
pub fn by_name(name: &str) -> Option<Manager> {
    MANAGERS
        .iter()
        .copied()
        .find(|manager| manager.name.eq_ignore_ascii_case(name))
}

/// The managers actually on this machine.
///
/// Offering one that is not installed is offering an error message. The lookup
/// is a `--version` run rather than a `PATH` walk, because a name on `PATH` can
/// still be a broken shim — and a shim that fails here fails before anything
/// has been installed, which is the cheapest place for it to happen.
pub fn available() -> Vec<Manager> {
    MANAGERS
        .iter()
        .copied()
        .filter(|manager| {
            Command::new(manager.name)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
        .collect()
}

/// Runs `manager` in `dir`, inheriting the terminal.
///
/// Inherited rather than captured: an install prints progress, asks about
/// peer dependencies, and reports audit findings, and swallowing all of that to
/// re-print a summary would be worse at every part of it. What this adds is the
/// working directory and a sentence when it fails.
pub fn run(manager: Manager, dir: &Path) -> Result<(), String> {
    eprintln!("\n  {} {}", manager.name, manager.command);

    let status = Command::new(manager.name)
        .arg(manager.command)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("cannot run {}: {e}", manager.name))?;

    if status.success() {
        return Ok(());
    }
    // The project is written and correct; only the install failed. Saying so is
    // the difference between "start again" and "run one command".
    Err(format!(
        "{} {} failed.\n\n\
         The project is written and unaffected — run it again in {} once the \
         reason is dealt with.",
        manager.name,
        manager.command,
        dir.display(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manager_is_found_by_name_however_it_is_spelled() {
        assert_eq!(by_name("npm").map(|m| m.name), Some("npm"));
        assert_eq!(by_name("BUN").map(|m| m.name), Some("bun"));
        assert_eq!(by_name("cargo"), None);
    }

    /// Whatever this machine has, the answer is a subset of what is offered —
    /// so a name that reaches [`run`] is always one of the four.
    #[test]
    fn what_is_available_is_a_subset_of_what_is_offered() {
        for manager in available() {
            assert!(MANAGERS.contains(&manager), "{manager:?} is not offered");
        }
    }
}
