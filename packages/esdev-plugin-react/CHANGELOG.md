# Changelog for `@opentf/esdev-plugin-react`

## [Unreleased]

### Fixed

- Include the package and its React Refresh peer in the workspace lockfile, so
  frozen installs used by CI and the site deployment include this workspace.

## [0.1.0] - 2026-10-01

### Added

- React Fast Refresh plugin for esdev's top-level `plugins` list, including
  browser runtime initialization and source maps for transformed modules.
