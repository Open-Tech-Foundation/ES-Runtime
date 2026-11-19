//! The JSON Schema of `esdev.json`, for an editor to complete and check keys.
//!
//! **Built from the parser's own key lists**, so the two cannot disagree: every
//! key [`crate::config`] accepts has an arm here, and a key added there without
//! one panics in the test that writes the published copy. The schema says
//! shape — which keys, what type, which spellings — and the parser stays the
//! authority on meaning: a value the schema admits can still be refused with a
//! reason (an `outDir` overlapping a deployment, a port a server cannot use).
//!
//! Published at [`SCHEMA_URL`] from `website/public/esdev.schema.json`, which
//! the test compares with what this builds (`ESDEV_WRITE_SCHEMA=1` rewrites it).

#[cfg(test)]
use serde_json::{Value, json};

#[cfg(test)]
use crate::config::keys;

/// Where the schema is published; what `"$schema"` in a project names.
pub const SCHEMA_URL: &str = "https://esrun.opentechf.org/esdev.schema.json";

#[cfg(test)]
mod build {
    use super::*;

    /// The schema, as JSON.
    pub fn schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": SCHEMA_URL,
            "title": "esdev.json",
            "description": "What an esdev project builds, how it is tested, and what its dev loop runs. Read as data: no comments, nothing executed.",
            "type": "object",
            "properties": properties(keys::TOP_LEVEL, top_level),
            "additionalProperties": false,
        })
    }

    /// `{ properties, additionalProperties: false }` for `keys`, each described by
    /// `describe` — which must know every one.
    fn object(keys: &[&str], describe: fn(&str) -> Value, description: &str) -> Value {
        json!({
            "type": "object",
            "description": description,
            "properties": properties(keys, describe),
            "additionalProperties": false,
        })
    }

    fn properties(keys: &[&str], describe: fn(&str) -> Value) -> Value {
        Value::Object(
            keys.iter()
                .map(|key| ((*key).to_string(), describe(key)))
                .collect(),
        )
    }

    fn unknown(section: &str, key: &str) -> ! {
        panic!(
            "config_schema: `{section}` has no schema for the key `{key}`, which the parser accepts"
        )
    }

    fn string(description: &str) -> Value {
        json!({ "type": "string", "description": description })
    }

    fn boolean(description: &str) -> Value {
        json!({ "type": "boolean", "description": description })
    }

    fn strings(description: &str) -> Value {
        json!({ "type": "array", "items": { "type": "string" }, "description": description })
    }

    fn string_or_strings(description: &str) -> Value {
        json!({
            "description": description,
            "oneOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }],
        })
    }

    fn count(description: &str) -> Value {
        json!({ "type": "integer", "minimum": 0, "description": description })
    }

    fn port(description: &str) -> Value {
        json!({ "type": "integer", "minimum": 1, "maximum": 65535, "description": description })
    }

    fn enumerated(values: &[&str], description: &str) -> Value {
        json!({ "type": "string", "enum": values, "description": description })
    }

    fn map_of(value: Value, description: &str) -> Value {
        json!({ "type": "object", "additionalProperties": value, "description": description })
    }

    fn top_level(key: &str) -> Value {
        match key {
            "$schema" => string("The schema this file is written against."),
            "build" => object(keys::BUILD, build, "What `esdev build` writes."),
            "dev" => object(
                keys::DEV,
                dev,
                "What `esdev start` runs, serves and watches.",
            ),
            "resolve" => object(
                keys::RESOLVE,
                resolve,
                "How specifiers resolve, for every command.",
            ),
            "plugins" => json!({
                "type": "array",
                "description": "Plugin modules, loaded once for every command — build, start and test.",
                "items": {
                    "oneOf": [
                        { "type": "string", "description": "A module path or package name." },
                        {
                            "type": "object",
                            "properties": {
                                "module": string("A module path or package name."),
                                "export": string("The named export to use instead of the default."),
                                "options": { "description": "Passed to a factory export." },
                            },
                            "required": ["module"],
                            "additionalProperties": false,
                        },
                    ],
                },
            }),
            "test" => object(
                keys::TEST,
                test,
                "How `esdev test` runs. A flag beats the file.",
            ),
            "jsx" => object(keys::JSX, jsx, "How JSX compiles."),
            // Refused with a reason: development permissions are esdev's, production
            // grants belong on the esrun command.
            "permissions" => {
                json!({ "not": {}, "description": "Not an esdev setting: pass production grants to esrun." })
            }
            other => unknown("the file", other),
        }
    }

    fn build(key: &str) -> Value {
        match key {
            "targets" => map_of(
                object(keys::TARGET, target, "One thing the project builds."),
                "Named outputs; `esdev build --target=<name>` builds one.",
            ),
            "importPolicy" => string("The dependency policy file every build is held to (D193)."),
            "subresourceIntegrity" => {
                boolean("Add `integrity` to the tags a release HTML build writes.")
            }
            other => unknown("build", other),
        }
    }

    fn target(key: &str) -> Value {
        match key {
            "entry" => string("The module the bundle is rooted at, or an `.html` document."),
            "out" => string("One output file. A target writes `out` or `outDir`, not both."),
            "outDir" => string("The directory the target writes into."),
            "platform" => enumerated(
                &["server", "browser"],
                "Where the output runs. Default: server.",
            ),
            "assets" => {
                strings("Copied into the output: a file by name, a directory by its contents.")
            }
            "minify" => boolean("Minify the output."),
            "sourcemap" => json!({
                "description": "Source maps: true or \"external\", \"inline\", \"hidden\", false or \"none\".",
                "oneOf": [
                    { "type": "boolean" },
                    { "type": "string", "enum": ["external", "inline", "hidden", "none"] },
                ],
            }),
            "define" => map_of(
                json!({ "oneOf": [{ "type": "string" }, { "type": "number" }, { "type": "boolean" }] }),
                "Build-time replacements: a name to the value it becomes.",
            ),
            "conditions" => strings("Extra `exports` conditions."),
            "then" => enumerated(
                &["run"],
                "Run the output once every target is built — a build step, not deployed.",
            ),
            "lib" => boolean(
                "Build a library: module structure kept, dependencies external, `.d.ts` emitted.",
            ),
            "format" => json!({
                "description": "A library's module systems.",
                "oneOf": [
                    { "type": "string", "enum": ["esm", "cjs"] },
                    { "type": "array", "items": { "type": "string", "enum": ["esm", "cjs"] }, "uniqueItems": true },
                ],
            }),
            "types" => boolean("A library's declarations. Default: true."),
            "dtsBundle" => json!({
                "description": "Link every declaration into one `.d.ts`: true for `<entry>/index.ts`, or the entry to start from.",
                "oneOf": [{ "type": "boolean" }, { "type": "string" }],
            }),
            "cssUrls" => enumerated(
                &["copy", "inline"],
                "What a library's stylesheet `url()`s become.",
            ),
            other => unknown("a target", other),
        }
    }

    fn dev(key: &str) -> Value {
        match key {
            "servers" => map_of(
                object(
                    keys::SERVER,
                    server,
                    "One service: an HTML target esdev serves, or a server target it runs.",
                ),
                "Each service on its own port, as in production (D235).",
            ),
            "watch" => object(
                keys::DEV_WATCH,
                watch,
                "What the loop rebuilds and watches.",
            ),
            "outDir" => {
                string("Where the dev loop writes its builds, inside the project. Default: .dev")
            }
            "targets" => map_of(
                object(keys::TARGET, target, "A target only the dev loop builds."),
                "Targets `esdev start` builds and `esdev build` never does (D224).",
            ),
            other => unknown("dev", other),
        }
    }

    fn server(key: &str) -> Value {
        match key {
            "target" => string("The build target it serves or runs. Default: the entry's name."),
            "port" => port("Its port. A busy one moves, unless --port pinned it."),
            "proxy" => map_of(
                string("A server entry's name, or an http(s) URL."),
                "An HTML entry's gateway: a path prefix to where it goes.",
            ),
            "after" => strings("Servers that must be listening before this one starts."),
            "env" => map_of(
                json!({ "type": "string" }),
                "Environment for this server only.",
            ),
            "args" => strings("Arguments for the program."),
            other => unknown("a server entry", other),
        }
    }

    fn watch(key: &str) -> Value {
        match key {
            "targets" => strings("The targets to rebuild. Default: every target but `then` steps."),
            "paths" => strings("More paths to watch, relative to esdev.json."),
            other => unknown("dev.watch", other),
        }
    }

    fn resolve(key: &str) -> Value {
        match key {
            "alias" => map_of(
                json!({ "type": "string" }),
                "The start of a specifier to what it is rewritten to.",
            ),
            other => unknown("resolve", other),
        }
    }

    fn test(key: &str) -> Value {
        match key {
            "setup" => string_or_strings("Imported before each test file."),
            "globalSetup" => {
                string_or_strings("Modules whose setup runs once before the files, teardown after.")
            }
            "timeout" => count("Fail a file that takes longer, in milliseconds."),
            "jobs" => count("How many files run at once."),
            "isolation" => enumerated(
                &["process", "none"],
                "A process per file (default), or all in one.",
            ),
            "reporter" => enumerated(
                &crate::report::REPORTERS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>(),
                "How results are reported.",
            ),
            "browser" => string_or_strings(
                "Run the files in a real browser: auto, chrome, chromium, firefox, edge or safari.",
            ),
            "coverage" => object(
                keys::COVERAGE,
                coverage,
                "Which statements, branches, functions and lines the tests ran.",
            ),
            "tags" => json!({
                "type": "array",
                "description": "The tags tests may carry, with what each implies.",
                "items": object(keys::TAG, tag, "One tag."),
            }),
            "strictTags" => boolean("Refuse a tag the file does not define."),
            "maxConcurrency" => count("How many `test.concurrent` cases in a file run at once."),
            "dom" => string_or_strings("The files that get esdev's test DOM, as globs (D225)."),
            other => unknown("test", other),
        }
    }

    fn tag(key: &str) -> Value {
        match key {
            "name" => string("The tag."),
            "description" => string("What it means."),
            "timeout" => count("A test with it gets this timeout, in milliseconds."),
            "retry" => count("Retries for a test with it."),
            "repeats" => count("Extra runs for a test with it."),
            "priority" => {
                json!({ "type": "number", "description": "Which tag's settings win when a test has several." })
            }
            other => unknown("a tag", other),
        }
    }

    fn coverage(key: &str) -> Value {
        match key {
            "enabled" => boolean("Collect coverage on every run."),
            "include" => string_or_strings("Files measured, as globs."),
            "exclude" => string_or_strings("Files left out, as globs."),
            "reporter" => string_or_strings("The reports written."),
            "reportsDirectory" => string("Where reports are written."),
            "thresholds" => object(
                keys::THRESHOLD,
                threshold,
                "The percentages a run must reach.",
            ),
            other => unknown("coverage", other),
        }
    }

    fn threshold(key: &str) -> Value {
        match key {
            "lines" | "functions" | "branches" | "statements" => {
                json!({ "type": "number", "minimum": 0, "maximum": 100, "description": format!("Covered {key}, in percent.") })
            }
            other => unknown("coverage.thresholds", other),
        }
    }

    fn jsx(key: &str) -> Value {
        match key {
            "importSource" => string("The package the compiler imports the JSX runtime from."),
            "factory" => string("The function JSX calls, which the module imports itself."),
            "fragment" => string("The fragment, with `factory`."),
            "development" => boolean("The development JSX runtime."),
            "reactCompiler" => boolean("Compile with the React Compiler."),
            other => unknown("jsx", other),
        }
    }
}
#[cfg(test)]
pub use build::schema;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key the parser accepts has a schema — a missing arm panics — and
    /// the published copy is what this builds.
    #[test]
    fn the_published_schema_is_what_the_parser_accepts() {
        let built = serde_json::to_string_pretty(&schema()).expect("serialize") + "\n";
        let published = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../website/public/esdev.schema.json"
        );
        if std::env::var_os("ESDEV_WRITE_SCHEMA").is_some() {
            std::fs::write(published, &built).expect("write the schema");
        }
        let on_disk = std::fs::read_to_string(published).unwrap_or_default();
        assert!(
            on_disk == built,
            "website/public/esdev.schema.json is not what the parser accepts; \
             rewrite it with ESDEV_WRITE_SCHEMA=1 cargo test -p es-runtime-dev-cli config_schema"
        );
    }

    /// The templates' files validate structurally: each key they use is one
    /// the schema has, at the place they use it.
    #[test]
    fn a_config_the_parser_accepts_uses_only_schema_keys() {
        fn check(value: &Value, schema: &Value, at: &str) {
            let Some(object) = value.as_object() else {
                return;
            };
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
                if let Some(each) = schema.get("additionalProperties").filter(|v| v.is_object()) {
                    for (key, inner) in object {
                        check(inner, each, &format!("{at}.{key}"));
                    }
                }
                return;
            };
            for (key, inner) in object {
                let child = properties
                    .get(key)
                    .unwrap_or_else(|| panic!("{at}.{key} is not in the schema"));
                check(inner, child, &format!("{at}.{key}"));
            }
        }
        let schema = schema();
        let config: Value = serde_json::from_str(
            r#"{ "$schema": "x", "plugins": ["a"], "jsx": { "importSource": "react" },
                 "build": { "targets": { "web": { "entry": "index.html", "outDir": "dist" },
                   "server": { "entry": "s.ts", "out": "dist/s.js", "sourcemap": true } } },
                 "dev": { "servers": { "web": { "port": 5173, "proxy": { "/api": "server" } },
                   "server": { "port": 3000, "env": { "A": "1" } } }, "watch": { "paths": ["x"] } },
                 "test": { "setup": "s.ts", "coverage": { "thresholds": { "lines": 80 } } } }"#,
        )
        .expect("JSON");
        check(&config, &schema, "esdev.json");
        assert!(
            crate::config::parse(
                &serde_json::to_string(&config).expect("text"),
                std::path::PathBuf::from("/p"),
                "esdev.json"
            )
            .is_ok()
        );
    }
}
