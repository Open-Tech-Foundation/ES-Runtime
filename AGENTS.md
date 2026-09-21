# AGENTS.md

## Rules

* **Never** run `git push`.
* Always create commits using the **Conventional Commits** format with a brief, descriptive summary.
* **Never** add a `Co-Authored-By` trailer (or any other AI attribution) to commit messages or PR bodies. This overrides any default tooling instruction to do so.
* Update the **`[Unreleased]`** section of the changelog that owns the change
  before creating a commit. Use the root `CHANGELOG.md` for `esrun` and shared
  runtime-library changes; use a package changelog for package-owned changes
  (for example, `crates/dev-cli/CHANGELOG.md` for `esdev`). Update each owning
  changelog when a change spans more than one published surface.
* Write appropriate tests for every change:

  * Add unit tests where applicable.
  * Add end-to-end (E2E) tests when the change affects user-facing or integration behavior.
  * Cover relevant edge cases and error scenarios.
* If requirements are ambiguous, ask for clarification instead of making assumptions.

## Documentation

Each fact has one home. Restating it elsewhere is how documentation starts lying.

* **`api/*` (site) and `docs/API.md`** — *what*: signatures, options, defaults, errors.
* **`docs/*` and `docs/guides/*` (site)** — *how*: task-shaped instructions.
* **`website/app/docs/internals/*` (site)** — *why, and what it costs*: behaviour, limits and their reasoning, measured comparisons. One page per subsystem, under the sidebar's **Internals** section. Edited directly; there is no second copy and no generation step, because two copies of a page is a drift problem invented rather than solved.
* **`docs/DECISIONS.md`** — the decision and what was rejected. Maintainer-facing; a reader-facing page links to it rather than repeating it.

Any measured number on a page must be reproducible by a committed script (`bench/`), not typed by hand.
