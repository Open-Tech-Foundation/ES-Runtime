//! Host ops backing `URLPattern` (SPEC §2.4), delegating to the `urlpattern`
//! crate — the same reasoning as `url_ops`: the standard's tokenizer, parser and
//! canonicalization rules are large and exacting, so they are delegated to a
//! maintained implementation of the spec rather than reimplemented in the JS
//! prelude.
//!
//! **The split.** Rust does the parsing and canonicalization and hands back each
//! component's regular expression as *source*; V8 compiles it and does the
//! matching. That is the division the crate's `quirks` module exists for (and
//! what Deno itself uses), and it matters for two measured reasons:
//!
//! * **Construction cost.** Compiling the components through the `regex` crate
//!   costs ~600 µs per pattern — a 50-route table would spend ~35 ms before
//!   serving anything. Emitting the source instead costs ~6 µs, and V8's RegExp
//!   compiler handles the rest.
//! * **`ignoreCase`.** `urlpattern` 0.6.0's `impl RegExp for regex::Regex`
//!   discards its `flags` argument, so under `RegexSyntax::Rust` the option is
//!   silently dropped for any component containing a group. Carrying the source
//!   to V8 lets the flags be applied where the regex is actually compiled.
//!
//! Because the compiled pattern lives in JS, there is no host-side registry and
//! nothing to free: a `URLPattern` is an ordinary JS object.
//!
//! Wire shape, chosen to avoid marshaling a JS object per call:
//!
//! * `urlpattern_parse(kind, input, base, ignoreCase)` → a flat array of
//!   `[patternString, regexpSource, [groupName, …]]` per component, followed by
//!   `hasRegExpGroups`. Throws `TypeError` for a malformed pattern.
//! * `urlpattern_canonicalize(init)` → the eight canonicalized component values
//!   for a `URLPatternInit` *input*, or `null` if it cannot be processed. A URL
//!   string input needs no op: the prelude reads the components off `URL`.
//!
//! `kind` is 0 for a pattern string and 1 for a `URLPatternInit`, whose eight
//! components plus `baseURL` arrive as an array of nine nullable strings.

use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use url::Url;
use urlpattern::quirks::{self, EcmaRegexp, StringOrInit};
use urlpattern::{RegexSyntax, UrlPatternInit, UrlPatternOptions};

use crate::Result;

/// The eight components, in the order both ops use.
const COMPONENT_COUNT: usize = 8;
/// Index of `baseURL` in the nine-element init array.
const BASE_URL_SLOT: usize = 8;

fn type_error(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::TypeError, message.into())
}

/// Reads a nullable string argument.
fn opt_str(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// The options the crate compiles a pattern under. `RegexSyntax::EcmaScript` is
/// what keeps the regex as source rather than compiling it here.
fn options(ignore_case: bool) -> UrlPatternOptions {
    UrlPatternOptions {
        regex_syntax: RegexSyntax::EcmaScript,
        ignore_case,
    }
}

/// Builds a [`UrlPatternInit`] from the nine-element array the prelude sends.
fn init_from_array(
    items: &[Value],
    base: Option<&str>,
) -> std::result::Result<UrlPatternInit, OpError> {
    let get = |i: usize| opt_str(items.get(i));
    // A `baseURL` inside the dictionary rides along as the ninth element. A
    // dictionary paired with a separate base argument is rejected before we get
    // here, so at most one of the two is ever set.
    let own_base = opt_str(items.get(BASE_URL_SLOT));
    let base = base.or(own_base.as_deref());
    let base_url = match base {
        Some(base) => {
            Some(Url::parse(base).map_err(|e| type_error(format!("Invalid base URL: {e}")))?)
        }
        None => None,
    };
    Ok(UrlPatternInit {
        protocol: get(0),
        username: get(1),
        password: get(2),
        hostname: get(3),
        port: get(4),
        pathname: get(5),
        search: get(6),
        hash: get(7),
        base_url,
    })
}

/// Resolves the `(kind, input, base)` argument triple into a [`UrlPatternInit`].
fn init_from_args(
    kind: f64,
    input: Option<&Value>,
    base: Option<&str>,
) -> std::result::Result<UrlPatternInit, OpError> {
    if kind == 0.0 {
        let pattern = match input {
            Some(Value::String(s)) => s.as_str(),
            _ => return Err(type_error("URLPattern input must be a string")),
        };
        let base_url = match base {
            Some(base) => {
                Some(Url::parse(base).map_err(|e| type_error(format!("Invalid base URL: {e}")))?)
            }
            None => None,
        };
        UrlPatternInit::parse_constructor_string::<EcmaRegexp>(pattern, base_url)
            .map_err(|e| type_error(e.to_string()))
    } else {
        match input {
            Some(Value::Array(items)) => init_from_array(items, base),
            _ => Err(type_error("URLPattern init must be an array of components")),
        }
    }
}

/// Flattens one component into `[patternString, regexpSource, [groupName, …]]`.
fn push_component(out: &mut Vec<Value>, component: &quirks::UrlPatternComponent) {
    out.push(Value::String(component.pattern_string.clone()));
    out.push(Value::String(component.regexp_string.clone()));
    out.push(Value::Array(
        component
            .group_name_list
            .iter()
            .map(|name| Value::String(name.clone()))
            .collect(),
    ));
}

/// Registers `urlpattern_parse` and `urlpattern_canonicalize`.
///
/// No capability is required: these ops are pure computation over their
/// arguments and reach no host resource.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    engine.register_op(OpDecl::sync("urlpattern_parse", |args| {
        let kind = args.first().and_then(Value::as_number).unwrap_or(0.0);
        let base = args.get(2).and_then(Value::as_str);
        let ignore_case = matches!(args.get(3), Some(Value::Bool(true)));

        let init = init_from_args(kind, args.get(1), base)?;
        let pattern = quirks::parse_pattern::<EcmaRegexp>(init, options(ignore_case))
            .map_err(|e| type_error(e.to_string()))?;

        let mut out = Vec::with_capacity(COMPONENT_COUNT * 3 + 1);
        for component in [
            &pattern.protocol,
            &pattern.username,
            &pattern.password,
            &pattern.hostname,
            &pattern.port,
            &pattern.pathname,
            &pattern.search,
            &pattern.hash,
        ] {
            push_component(&mut out, component);
        }
        out.push(Value::Bool(pattern.has_regexp_groups));
        Ok(Value::Array(out))
    }))?;

    engine.register_op(OpDecl::sync("urlpattern_canonicalize", |args| {
        let Some(Value::Array(items)) = args.first() else {
            return Err(type_error("URLPattern init must be an array of components"));
        };
        let get = |i: usize| opt_str(items.get(i));
        let init = quirks::UrlPatternInit {
            protocol: get(0),
            username: get(1),
            password: get(2),
            hostname: get(3),
            port: get(4),
            pathname: get(5),
            search: get(6),
            hash: get(7),
            base_url: get(BASE_URL_SLOT),
        };
        // An init that cannot be processed is a non-match, not an error — the
        // same answer a URL string that fails to parse gets.
        let Ok(Some((match_input, _))) = quirks::process_match_input(StringOrInit::Init(init), None)
        else {
            return Ok(Value::Null);
        };
        let Some(values) = quirks::parse_match_input(match_input) else {
            return Ok(Value::Null);
        };
        Ok(Value::Array(vec![
            Value::String(values.protocol),
            Value::String(values.username),
            Value::String(values.password),
            Value::String(values.hostname),
            Value::String(values.port),
            Value::String(values.pathname),
            Value::String(values.search),
            Value::String(values.hash),
        ]))
    }))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(pathname: &str, ignore_case: bool) -> quirks::UrlPattern {
        let init = UrlPatternInit {
            pathname: Some(pathname.to_owned()),
            ..Default::default()
        };
        quirks::parse_pattern::<EcmaRegexp>(init, options(ignore_case)).unwrap()
    }

    #[test]
    fn a_component_yields_anchored_regex_source_and_group_names() {
        let pattern = parse("/u/:id", false);
        assert_eq!(pattern.pathname.pattern_string, "/u/:id");
        assert!(pattern.pathname.regexp_string.starts_with('^'));
        assert!(pattern.pathname.regexp_string.ends_with('$'));
        assert_eq!(pattern.pathname.group_name_list, ["id"]);
    }

    #[test]
    fn has_regexp_groups_reports_a_custom_regex() {
        assert!(parse("/u/:id(\\d+)", false).has_regexp_groups);
        assert!(!parse("/u/:id", false).has_regexp_groups);
    }

    /// Why the flags are applied in the prelude and not here: under
    /// `RegexSyntax::Rust` the crate's `impl RegExp for regex::Regex` discards
    /// its `flags` argument, so `ignoreCase` never reaches a component that
    /// compiles to a regex. Emitting source leaves the flags to V8, so the only
    /// thing this side must get right is producing the same source either way.
    #[test]
    fn ignore_case_does_not_change_the_emitted_source() {
        assert_eq!(
            parse("/API/:id", true).pathname.regexp_string,
            parse("/API/:id", false).pathname.regexp_string,
        );
    }

    #[test]
    fn a_malformed_pattern_is_an_error() {
        let init = UrlPatternInit {
            pathname: Some("/u/{".to_owned()),
            ..Default::default()
        };
        assert!(quirks::parse_pattern::<EcmaRegexp>(init, options(false)).is_err());
    }

    /// Construction cost, which is the whole reason for this split. Reported,
    /// not asserted — a measurement, not a gate.
    #[test]
    #[ignore = "measurement: cargo test -p es-runtime --lib -- --ignored --nocapture urlpattern_construction_cost"]
    #[allow(clippy::print_stdout)]
    fn urlpattern_construction_cost() {
        let n = 200;
        let start = std::time::Instant::now();
        for i in 0..n {
            let _ = parse(&format!("/api/v{i}/users/:id"), false);
        }
        let per = start.elapsed().as_secs_f64() * 1e6 / f64::from(n);
        println!("quirks parse (regex source only): {per:.1} us/pattern");
    }
}
