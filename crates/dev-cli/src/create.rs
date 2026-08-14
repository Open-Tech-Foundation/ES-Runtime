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
//! # It writes files and stops
//!
//! No install, no `git init`, no prompts.
//!
//! **No install** because there is no lockfile yet to say which package manager
//! this project uses, and guessing wrong leaves a `package-lock.json` in a bun
//! project — the sort of mess that outlives the guess. A package installer is a
//! stated non-goal of this project besides.
//!
//! **No prompts** because every other command here is a flag grammar that works
//! in a script, and a scaffolder that stops to ask cannot be one of them.
//!
//! # It never overwrites
//!
//! `esdev build --lib` empties its output directory because the build owns it
//! (D59). This owns nothing: it writes into a directory the user named, which
//! may be their home directory or a project they have been working in for a
//! year. So a non-empty target is refused unless `--force` says otherwise, and
//! even then an existing file is left alone rather than replaced — `--force`
//! means "write among what is there", never "write over it".

use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/templates.rs"));

/// The one-line descriptions `--list` prints.
///
/// Beside the templates rather than inside them: a description is for somebody
/// choosing, and what they are choosing between is only visible from here.
const DESCRIPTIONS: &[(&str, &str)] = &[(
    "react",
    "React + react-router — server-rendered, hydrated, prerenderable to static HTML",
)];

/// A file whose name in the template is not the name it is written under.
///
/// `.gitignore` is the whole list, and it is not cosmetic: a `.gitignore` in
/// the template directory would be applied *to the template*, so this
/// repository would stop tracking the very file it means to ship. npm's
/// packaging has the same problem and the same fix, which is why the convention
/// is one somebody scaffolding will already have seen.
const RENAMED: &[(&str, &str)] = &[("_gitignore", ".gitignore")];

/// What `esdev create` was asked to do.
pub struct CreateConfig {
    /// The directory to write into.
    pub dir: String,
    /// Which template.
    pub template: String,
    /// Whether to write into a directory that already holds something.
    pub force: bool,
}

/// The default template, when `--template` did not say.
pub const DEFAULT_TEMPLATE: &str = "react";

/// Scaffolds a project and reports what to do next.
pub fn create(config: &CreateConfig) -> Result<String, String> {
    let files = TEMPLATES
        .iter()
        .find(|(name, _)| *name == config.template)
        .map(|(_, files)| *files)
        .ok_or_else(|| {
            format!(
                "there is no {} template.\n\n{}",
                config.template,
                list().trim_end()
            )
        })?;

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
    let mut written = 0usize;
    let mut skipped = Vec::new();
    for (path, contents) in files {
        let path = RENAMED
            .iter()
            .find(|(from, _)| from == path)
            .map_or(*path, |(_, to)| *to);
        let destination = target.join(path);
        if destination.exists() {
            skipped.push(path.to_string());
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        write(&destination, contents, &name)?;
        written += 1;
    }

    // Nothing written is not an error — `--force` on a directory that already
    // holds the whole project is a no-op somebody asked for — but reporting it
    // as "created" would be a lie about a command whose entire job is to write
    // files.
    if written == 0 {
        return Ok(format!(
            "nothing to write: {} already holds every file the {} template has.\n",
            config.dir, config.template
        ));
    }
    let mut report = format!(
        "created {} from the {} template ({written} file{})\n",
        config.dir,
        config.template,
        if written == 1 { "" } else { "s" },
    );
    if !skipped.is_empty() {
        report.push_str(&format!(
            "left alone, because they were already there: {}\n",
            skipped.join(", ")
        ));
    }
    report.push_str(&format!(
        "\n  cd {}\n  npm install\n  npm run dev\n",
        config.dir
    ));
    Ok(report)
}

/// Writes one file, substituting the project's name into it.
///
/// The substitution is attempted only on text. A template is free to hold a
/// favicon or a font, and running a search and replace over one would corrupt
/// it — so a file that is not UTF-8 is written exactly as it was embedded.
fn write(destination: &Path, contents: &[u8], name: &str) -> Result<(), String> {
    match std::str::from_utf8(contents) {
        Ok(text) if text.contains(PLACEHOLDER) => {
            std::fs::write(destination, text.replace(PLACEHOLDER, name))
        }
        _ => std::fs::write(destination, contents),
    }
    .map_err(|e| format!("cannot write {}: {e}", destination.display()))
}

/// The one thing a template can ask about the project being created.
const PLACEHOLDER: &str = "{{name}}";

/// The project's name, from the directory it is being created in.
///
/// Sanitised, because this lands in `package.json` and npm's rules are narrower
/// than a filesystem's: a directory called `My App` would otherwise produce a
/// manifest that every package manager rejects, on the first command the user
/// runs.
fn package_name(target: &Path) -> String {
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
        report.push_str(&format!(
            "  {name:<10} {description} ({} files)\n",
            files.len()
        ));
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the build script: a template that is not in the binary is
    /// a `create` that cannot work, and nothing else would notice.
    #[test]
    fn the_templates_are_in_the_binary() {
        let (name, files) = TEMPLATES
            .iter()
            .find(|(name, _)| *name == DEFAULT_TEMPLATE)
            .expect("the default template is embedded");
        assert_eq!(*name, "react");

        for expected in [
            "package.json",
            "esdev.json",
            "index.html",
            "src/server.tsx",
            "src/entry.client.tsx",
            "_gitignore",
        ] {
            assert!(
                files.iter().any(|(path, _)| *path == expected),
                "{expected} is not in the template: {:?}",
                files.iter().map(|(path, _)| *path).collect::<Vec<_>>()
            );
        }
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

    #[test]
    fn the_list_names_every_embedded_template() {
        let listed = list();
        for (name, _) in TEMPLATES {
            assert!(listed.contains(name), "{name} is missing from:\n{listed}");
        }
    }
}
