# oxc_resolver, as esdev builds it

A copy of [oxc_resolver](https://github.com/oxc-project/oxc-resolver)
11.24.2 from crates.io (MIT, see `LICENSE`), with esdev's tsconfig discovery
rule ([D196](../../../docs/DECISIONS.md)). It is wired in through the
workspace's `[patch.crates-io]`, so rolldown's resolver and the one esdev runs
unbundled modules through are this copy. Every changed line is marked
`esdev (D196)`.

## What is changed

1. **`src/tsconfig_resolver.rs`, `find_tsconfig_auto`**: the nearest
   `tsconfig.json` is returned. Upstream returns it only when its
   `files`/`include`/`exclude` (or a reference's) would compile the file, and
   otherwise walks on to an ancestor's. `TsConfig::claims_ownership_of`, used
   only there, is removed from `src/tsconfig.rs`.
2. **`src/tsconfig.rs`, `is_file_extension_allowed_in_tsconfig`**: any file
   with an extension passes. Upstream allows `.ts`/`.tsx`/`.mts`/`.cts`, and
   the `.js` family only with `allowJs`. This is what a wildcard `include`
   (the default `**/*`, or a bare `src`) and a solution config's reference
   selection match against.
3. **`src/tsconfig.rs`, `is_extensionless_or_uncompiled_js`**: only an
   extensionless path is refused; `allowJs` is not consulted.

## Not carried over

- `src/tests` and the `mod tests;` line in `src/lib.rs`: the tests read
  fixtures the crates.io package does not ship. The rule is tested where it is
  used, in `crates/dev-cli`.
- `examples/`, `benches/`, the dev-dependencies, the lint table and the
  profiles in the published `Cargo.toml`.

## Moving to a new release

Copy the new release's `src/` over this one (keeping the `mod tests;` removal),
re-apply the three changes above, set the version in `Cargo.toml` and in the
workspace's `oxc_resolver` pin together, and run `cargo test -p
es-runtime-dev-cli`. If upstream's discovery has changed shape, the dev-cli
tests named in D196 are the specification.
