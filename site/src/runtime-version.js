// The released ES-Runtime version shown in the nav. The single source of truth
// is the workspace Cargo.toml; scripts/release.sh rewrites the string below on
// each bump. Kept as a committed module (rather than read from Cargo.toml in
// vite.config.js) so the site build needs no build-time file read and stays
// portable to any static host, including Cloudflare.
export const RUNTIME_VERSION = "0.7.0";
