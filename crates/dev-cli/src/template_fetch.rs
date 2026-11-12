//! Where `esdev create`'s templates come from: the `@opentf/esdev-templates`
//! package, chosen, checked and read here (DECISIONS D220).
//!
//! Every step is a refusal unless it is exactly what was expected:
//!
//! - **Which release.** The newest stable, non-deprecated version whose
//!   `engines.esdev` range includes this esdev — or the one `--templates`
//!   pins, refused if its range excludes this esdev. A newer release this
//!   esdev cannot read is named, so the way forward (`esdev upgrade`) is said
//!   rather than discovered.
//! - **From where.** The tarball's URL must be on the registry's own origin.
//!   The registry is the party trusted to say what a release is; a URL it
//!   hands out pointing elsewhere is a second party nobody chose.
//! - **What it is.** The bytes must match the registry's `dist.integrity`
//!   (an SRI `sha512-…`), checked before a single byte is decompressed.
//! - **What it holds.** Read in memory, never extracted: regular files only
//!   (no links, devices or anything else), paths of plain relative names
//!   under `package/` (no `..`, no root, no drive), with a cap on the count
//!   and on the unpacked size, so a hostile archive is refused rather than
//!   unpacked a little at a time.
//!
//! The network half lives in [`crate::create`], which already owns the
//! registry client, its proxy, its timeout and its retries.

use std::io::Read as _;

/// The package `esdev create` reads its templates from.
pub const PACKAGE: &str = "@opentf/esdev-templates";

/// The most a templates tarball may be, compressed. Today's is under 100 KiB.
pub const TARBALL_LIMIT: usize = 16 * 1024 * 1024;
/// The most it may unpack to, and how many files it may hold.
const UNPACKED_LIMIT: u64 = 64 * 1024 * 1024;
const FILE_LIMIT: usize = 10_000;

/// The release `choose` settled on.
#[derive(Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub tarball: String,
    pub integrity: String,
    /// A newer stable release this esdev cannot read, and the range it needs.
    pub newer: Option<(String, String)>,
}

/// The release to read, from the package's registry metadata.
pub fn choose(
    metadata: &serde_json::Value,
    esdev: &str,
    pinned: Option<&str>,
) -> Result<Release, String> {
    let esdev_version =
        stable(esdev).ok_or_else(|| format!("esdev's own version {esdev} is not a release"))?;
    let versions = metadata
        .get("versions")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| format!("the registry lists no versions of {PACKAGE}"))?;
    let range_of = |manifest: &serde_json::Value| {
        manifest
            .pointer("/engines/esdev")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let release = |version: &str, manifest: &serde_json::Value| -> Result<Release, String> {
        let field = |pointer: &str| {
            manifest
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("{PACKAGE}@{version} has no {pointer} in the registry"))
        };
        Ok(Release {
            version: version.to_string(),
            tarball: field("/dist/tarball")?,
            integrity: field("/dist/integrity")?,
            newer: None,
        })
    };

    if let Some(pinned) = pinned {
        let manifest = versions
            .get(pinned)
            .ok_or_else(|| format!("there is no {PACKAGE}@{pinned} in the registry"))?;
        let range = range_of(manifest)
            .ok_or_else(|| format!("{PACKAGE}@{pinned} names no engines.esdev range"))?;
        if !satisfies(&range, esdev_version)? {
            return Err(format!(
                "{PACKAGE}@{pinned} needs esdev {range}, and this is esdev {esdev}.\n\n\
                 Run `esdev upgrade`, or pin a release this esdev reads."
            ));
        }
        return release(pinned, manifest);
    }

    // Stable and not withdrawn, newest first.
    let mut candidates: Vec<((u64, u64, u64), &String, &serde_json::Value)> = versions
        .iter()
        .filter(|(_, manifest)| manifest.get("deprecated").is_none())
        .filter_map(|(version, manifest)| stable(version).map(|parsed| (parsed, version, manifest)))
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    let mut newer = None;
    for (_, version, manifest) in candidates {
        let Some(range) = range_of(manifest) else {
            continue;
        };
        if satisfies(&range, esdev_version)? {
            let mut chosen = release(version, manifest)?;
            chosen.newer = newer;
            return Ok(chosen);
        }
        newer.get_or_insert((version.clone(), range));
    }
    Err(match newer {
        Some((version, range)) => format!(
            "no release of {PACKAGE} reads with esdev {esdev}: the newest, {version}, needs esdev {range}.\n\n\
             Run `esdev upgrade`."
        ),
        None => format!("the registry has no stable release of {PACKAGE}"),
    })
}

/// A stable `major.minor.patch`, or `None` for a prerelease or anything else.
pub fn stable(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split('+').next()?;
    if core.contains('-') {
        return None;
    }
    let mut parts = core.split('.');
    let numbers = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(numbers)
}

/// Whether `version` is in `range`: comparators (`>=`, `>`, `<=`, `<`, `=`)
/// on full versions, separated by spaces, all of which must hold. That is the
/// whole grammar — what a templates package needs to say which esdev reads it
/// — and anything else (`^`, `~`, `||`, `x`) is refused rather than guessed.
pub fn satisfies(range: &str, version: (u64, u64, u64)) -> Result<bool, String> {
    let mut any = false;
    for comparator in range.split_whitespace() {
        any = true;
        let (op, rest) = ["<=", ">=", "<", ">", "="]
            .iter()
            .find_map(|op| comparator.strip_prefix(op).map(|rest| (*op, rest)))
            .unwrap_or(("=", comparator));
        let bound = stable(rest).ok_or_else(|| {
            format!("engines.esdev {range:?}: {comparator:?} is not a comparator on a version")
        })?;
        let holds = match op {
            "<=" => version <= bound,
            ">=" => version >= bound,
            "<" => version < bound,
            ">" => version > bound,
            _ => version == bound,
        };
        if !holds {
            return Ok(false);
        }
    }
    if !any {
        return Err("engines.esdev is empty".to_string());
    }
    Ok(true)
}

/// Whether the tarball's URL is on the registry's origin.
pub fn same_origin(registry: &url::Url, tarball: &url::Url) -> bool {
    registry.scheme() == tarball.scheme()
        && registry.host_str() == tarball.host_str()
        && registry.port_or_known_default() == tarball.port_or_known_default()
}

/// Checks `bytes` against an SRI `integrity` value. Only `sha512` counts:
/// the registry has published it for every package since 2017, and a weaker
/// one alone (`sha1-…`) is refused rather than trusted.
pub fn verify(bytes: &[u8], integrity: &str) -> Result<(), String> {
    use base64::Engine as _;
    use sha2::Digest as _;

    let expected: Vec<&str> = integrity
        .split_whitespace()
        .filter_map(|entry| entry.strip_prefix("sha512-"))
        .map(|hash| hash.split('?').next().unwrap_or(hash))
        .collect();
    if expected.is_empty() {
        return Err(format!(
            "the registry gives no sha512 integrity for {PACKAGE} ({integrity:?})"
        ));
    }
    let actual = base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(bytes));
    if expected.contains(&actual.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "{PACKAGE} does not match the registry's integrity: what was downloaded is not what \
             was published, so nothing was read from it"
        ))
    }
}

/// The files in an npm tarball, paths relative to the package root.
pub fn unpack(tarball: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let gunzipped = flate2::read::GzDecoder::new(tarball).take(UNPACKED_LIMIT + 1);
    let mut archive = tar::Archive::new(gunzipped);
    let mut files = Vec::new();
    let mut total: u64 = 0;
    let entries = archive
        .entries()
        .map_err(|e| format!("{PACKAGE} is not a tarball: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("{PACKAGE}: a broken entry: {e}"))?;
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|e| format!("{PACKAGE}: an unreadable path: {e}"))?
            .into_owned();
        let shown = path.display().to_string();
        if !kind.is_file() {
            return Err(format!("{PACKAGE}: {shown} is not a regular file"));
        }
        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                std::path::Component::Normal(part) => parts.push(
                    part.to_str()
                        .ok_or_else(|| format!("{PACKAGE}: {shown} is not UTF-8"))?
                        .to_string(),
                ),
                _ => return Err(format!("{PACKAGE}: {shown} leaves the package")),
            }
        }
        // npm packs everything under `package/`.
        if parts.first().map(String::as_str) != Some("package") || parts.len() < 2 {
            return Err(format!("{PACKAGE}: {shown} is outside package/"));
        }
        let relative = parts[1..].join("/");
        let size = entry
            .header()
            .size()
            .map_err(|e| format!("{PACKAGE}: {shown}: {e}"))?;
        total += size;
        if total > UNPACKED_LIMIT || files.len() >= FILE_LIMIT {
            return Err(format!(
                "{PACKAGE} unpacks to more than a templates package can be"
            ));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        entry
            .read_to_end(&mut bytes)
            .map_err(|e| format!("{PACKAGE}: {shown}: {e}"))?;
        files.push((relative, bytes));
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(versions: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "versions": versions })
    }

    fn version(range: &str) -> serde_json::Value {
        serde_json::json!({
            "engines": { "esdev": range },
            "dist": { "tarball": "https://registry.example/t.tgz", "integrity": "sha512-x" }
        })
    }

    #[test]
    fn a_range_is_comparators_that_all_hold() {
        assert!(satisfies(">=0.27.0", (0, 27, 0)).unwrap());
        assert!(satisfies(">=0.27.0 <1.0.0", (0, 99, 3)).unwrap());
        assert!(!satisfies(">=0.27.0 <1.0.0", (1, 0, 0)).unwrap());
        assert!(!satisfies(">0.27.0", (0, 27, 0)).unwrap());
        assert!(satisfies("0.27.0", (0, 27, 0)).unwrap());
        for unread in ["^0.27.0", "~0.27.0", ">=0.27 || 1.0.0", ""] {
            assert!(
                satisfies(unread, (0, 27, 0)).is_err(),
                "{unread:?} was read"
            );
        }
    }

    /// The newest stable release this esdev reads; a newer one it cannot is
    /// named; prereleases and withdrawn releases are passed over.
    #[test]
    fn the_newest_readable_stable_release_is_chosen() {
        let mut deprecated = version(">=0.27.0");
        deprecated["deprecated"] = serde_json::json!("broken");
        let chosen = choose(
            &metadata(serde_json::json!({
                "0.1.0": version(">=0.27.0"),
                "0.2.0": version(">=0.27.0"),
                "0.2.1": deprecated,
                "0.3.0-rc.1": version(">=0.27.0"),
                "0.4.0": version(">=0.30.0"),
            })),
            "0.27.0",
            None,
        )
        .unwrap();
        assert_eq!(chosen.version, "0.2.0");
        assert_eq!(
            chosen.newer,
            Some(("0.4.0".to_string(), ">=0.30.0".to_string()))
        );
    }

    #[test]
    fn nothing_readable_says_how_to_read_it() {
        let err = choose(
            &metadata(serde_json::json!({ "0.4.0": version(">=0.30.0") })),
            "0.27.0",
            None,
        )
        .unwrap_err();
        assert!(
            err.contains("0.4.0") && err.contains(">=0.30.0") && err.contains("esdev upgrade"),
            "{err}"
        );
    }

    /// A pin is taken as named — and refused when this esdev cannot read it.
    #[test]
    fn a_pin_is_taken_or_refused() {
        let versions = metadata(serde_json::json!({
            "0.1.0": version(">=0.27.0"),
            "0.4.0": version(">=0.30.0"),
        }));
        assert_eq!(
            choose(&versions, "0.27.0", Some("0.1.0")).unwrap().version,
            "0.1.0"
        );
        let refused = choose(&versions, "0.27.0", Some("0.4.0")).unwrap_err();
        assert!(refused.contains("needs esdev >=0.30.0"), "{refused}");
        assert!(
            choose(&versions, "0.27.0", Some("9.9.9"))
                .unwrap_err()
                .contains("no ")
        );
    }

    #[test]
    fn the_tarball_is_on_the_registry_s_origin() {
        let registry = url::Url::parse("https://registry.npmjs.org").unwrap();
        let ok =
            url::Url::parse("https://registry.npmjs.org/@opentf/esdev-templates/-/x.tgz").unwrap();
        assert!(same_origin(&registry, &ok));
        for elsewhere in [
            "http://registry.npmjs.org/x.tgz",
            "https://evil.example/x.tgz",
            "https://registry.npmjs.org:8443/x.tgz",
        ] {
            assert!(
                !same_origin(&registry, &url::Url::parse(elsewhere).unwrap()),
                "{elsewhere}"
            );
        }
    }

    #[test]
    fn integrity_is_sha512_and_must_match() {
        use base64::Engine as _;
        use sha2::Digest as _;
        let bytes = b"the package";
        let good = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(bytes))
        );
        assert!(verify(bytes, &good).is_ok());
        assert!(verify(bytes, &format!("sha1-abc {good}")).is_ok());
        assert!(
            verify(b"something else", &good)
                .unwrap_err()
                .contains("not what was published")
        );
        assert!(verify(bytes, "sha1-abc").unwrap_err().contains("no sha512"));
    }

    fn tarball(entries: &[(&str, tar::EntryType, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, kind, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(*kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            if *kind == tar::EntryType::Symlink {
                header.set_link_name("/etc/passwd").unwrap();
            }
            // Written raw, so a hostile path is not normalised by the builder.
            let name = path.as_bytes();
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_package_unpacks_its_regular_files_relative_to_its_root() {
        let files = unpack(&tarball(&[
            ("package/package.json", tar::EntryType::Regular, b"{}"),
            (
                "package/templates/api/template.json",
                tar::EntryType::Regular,
                b"{}",
            ),
        ]))
        .unwrap();
        let paths: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["package.json", "templates/api/template.json"]);
    }

    #[test]
    fn a_hostile_package_is_refused() {
        for (path, kind, why) in [
            (
                "package/../escape",
                tar::EntryType::Regular,
                "leaves the package",
            ),
            ("/etc/passwd", tar::EntryType::Regular, "leaves the package"),
            (
                "package/link",
                tar::EntryType::Symlink,
                "not a regular file",
            ),
            ("other/file", tar::EntryType::Regular, "outside package/"),
        ] {
            let err = unpack(&tarball(&[(path, kind, b"x")])).unwrap_err();
            assert!(err.contains(why), "{path}: {err}");
        }
    }
}
