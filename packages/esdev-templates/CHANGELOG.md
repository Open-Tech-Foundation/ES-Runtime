# Changelog for `@opentf/esdev-templates`

All notable changes to **`@opentf/esdev-templates`**, the project templates
`esdev create` writes, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

The templates are data, released on their own (DECISIONS D220): esdev fetches
this package, checks it against the registry's published sha512, and reads the
newest release whose `engines.esdev` range includes it. A template fix ships
here, without an esdev release.

## [Unreleased]

### Changed

- Every template's `esdev.json` names its schema (`"$schema"`), so an editor
  completes and checks its keys.

- The React templates use the published `@opentf/esdev-plugin-react` for Fast
  Refresh, a dev dependency, instead of a copy of it in `plugins/` and a
  `refresh.ts` bootstrap their entry imported — a fix to the plugin reaches
  every project through an update. `src/esdev-env.d.ts`, which declared types
  only that bootstrap used, is gone with it.

- Every template's `esdev.json` names its services under `dev.servers`
  (DECISIONS D235): `web` on 5173 for a frontend, `server` on 3000 for an API
  or a full-stack app, and drops `watch.targets` lists that only repeated the
  default. Needs esdev 0.30.0 or later: `engines.esdev` is `>=0.30.0`.

## [0.2.0] - 2026-11-18

### Changed

- Every template's `esdev.json` writes `outDir`, the camelCase spelling esdev
  0.29 reads (DECISIONS D232). Needs esdev 0.29.0 or later: `engines.esdev` is
  `>=0.29.0`, so an older esdev goes on taking 0.1.0.

## [0.1.0] - 2026-11-17

### Changed

- The AFW starters (spa, fullstack, docs, library) moved to
  `@opentf/create-afw`, which AFW maintains and releases; `catalog.json`
  lists it, so `esdev create` still offers them, from there. Needs esdev
  0.28.0 or later, the first to read a listed package.
- The server templates' `start` scripts grant an address,
  `--allow-listen=0.0.0.0:3000`, as esrun now requires (ES-Runtime D221).

### Added

- The first release on its own: the templates that were built into esdev
  0.27.0 — `api`, `hono`, `elysia`, `react`, `lib`, `vanilla`, `micro-ui`,
  `preact`, `lit`, and the AFW starters `spa`, `fullstack`, `docs` and
  `library` — each with a `template.json` naming its options, and its
  TypeScript, Tailwind and blog variants as files under
  `_option/<name>/<value>/`. Server templates develop on port 3000.
