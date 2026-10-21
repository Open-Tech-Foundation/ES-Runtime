//! The one glob implementation (DECISIONS D180): `runtime:fs`'s `Glob` and
//! `glob_scan`, and `runtime:archive`'s filters, compile patterns here, so a
//! pattern means the same thing wherever a program writes it.

use globset::{GlobBuilder, GlobMatcher};

/// Compiles a glob pattern into a matcher plus a "negated" flag, covering the
/// full conventional set: `?`, `*` (not crossing `/`), `**` (crossing), `[ab]`,
/// `[a-z]`, `[!abc]` **and** `[^abc]`, `{a,b}`, `\` escaping, and a leading `!`
/// that negates the whole pattern.
///
/// `\` escaping is the one part that is platform-dependent: globset disables it
/// on Windows, where `\` is the path separator, so `\!x.ts` is a path there
/// rather than a literal `!x.ts`. Node's minimatch makes the same call
/// (`windowsPathsNoEscape`). Forcing it on would make pattern semantics uniform
/// but stop `\` separating components in the real Windows paths this matches
/// against, which is the trade globset's default is choosing.
pub fn compile(pattern: &str) -> Result<(GlobMatcher, bool), String> {
    // A leading `!` negates; `\!…` is a literal `!` (globset unescapes it).
    let (negated, body) = match pattern.strip_prefix('!') {
        Some(rest) => (true, rest.to_string()),
        None => (false, pattern.to_string()),
    };
    // Accept the `[^…]` negated-class form (globset spells it `[!…]`).
    let body = body.replace("[^", "[!");
    let matcher = GlobBuilder::new(&body)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| format!("invalid glob pattern {pattern:?}: {e}"))?;
    Ok((matcher, negated))
}

/// A list of patterns with Bun's reading of `!`: a path is taken when it
/// matches at least one positive pattern and no negative one, and a list of
/// only negative patterns takes everything they do not exclude.
#[derive(Debug, Clone)]
pub struct GlobList {
    include: Vec<GlobMatcher>,
    exclude: Vec<GlobMatcher>,
}

impl GlobList {
    /// Compiles every pattern, or names the first that does not compile.
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Result<GlobList, String> {
        let mut list = GlobList {
            include: Vec::new(),
            exclude: Vec::new(),
        };
        for pattern in patterns {
            let (matcher, negated) = compile(pattern.as_ref())?;
            if negated {
                list.exclude.push(matcher);
            } else {
                list.include.push(matcher);
            }
        }
        Ok(list)
    }

    /// Whether `path` is taken.
    pub fn matches(&self, path: &str) -> bool {
        (self.include.is_empty() || self.include.iter().any(|glob| glob.is_match(path)))
            && !self.exclude.iter().any(|glob| glob.is_match(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_reads_bang_as_exclusion() {
        let list = GlobList::new(&["src/**", "!**/*.test.ts"]).unwrap();
        assert!(list.matches("src/a.ts"));
        assert!(!list.matches("src/a.test.ts"));
        assert!(!list.matches("docs/a.md"));

        let only_excludes = GlobList::new(&["!**/*.map"]).unwrap();
        assert!(only_excludes.matches("a.js"));
        assert!(!only_excludes.matches("dist/a.js.map"));

        assert!(GlobList::new(&["**/*.ts"]).unwrap().matches("a/b/c.ts"));
        assert!(
            !GlobList::new(&["*.ts"]).unwrap().matches("a/c.ts"),
            "* does not cross /"
        );
        assert!(GlobList::new(&["[^a]*"]).unwrap().matches("b.txt"));
        assert!(GlobList::new(&["a["]).is_err());
    }
}
