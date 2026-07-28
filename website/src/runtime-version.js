// The released ES-Runtime version shown in the nav. The single source of truth
// is the workspace Cargo.toml; bump the string below to match on each release.
// Kept as a committed module (rather than read from Cargo.toml in the build) so
// the website build needs no build-time file read and stays portable to any
// static host, including Cloudflare.
export const RUNTIME_VERSION = "0.12.0";
