# Changelog for `@opentf/esdev-templates`

All notable changes to **`@opentf/esdev-templates`**, the project templates
`esdev create` writes, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

The templates are data, released on their own (DECISIONS D220): esdev fetches
this package, checks it against the registry's published sha512, and reads the
newest release whose `engines.esdev` range includes it. A template fix ships
here, without an esdev release.

## [Unreleased]

### Added

- The first release on its own: the templates that were built into esdev
  0.27.0 — `api`, `hono`, `elysia`, `react`, `lib`, `vanilla`, `micro-ui`,
  `preact`, `lit`, and the AFW starters `spa`, `fullstack`, `docs` and
  `library` — each with a `template.json` naming its options, and its
  TypeScript, Tailwind and blog variants as files under
  `_option/<name>/<value>/`. Server templates develop on port 3000.
