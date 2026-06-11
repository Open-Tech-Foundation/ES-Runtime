//! Host ops backing the URL family (SPEC §2.4), powered by the WHATWG-ish
//! `url` crate (DECISIONS.md D17).
//!
//! The ops are pure computation (no capability): parse a URL and return its
//! components, or apply one component setter and return the new components. The
//! JS `URL`/`URLSearchParams` wrappers (prelude) drive these. Components are
//! passed back as a JSON object string and `JSON.parse`d in the prelude; a
//! `Value::Null` result signals a parse/setter failure the wrapper turns into a
//! `TypeError`.

use es_runtime_engine::{Engine, OpDecl, Value};
use url::Url;

use crate::Result;

/// Registers `url_parse` and `url_set`.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    engine.register_op(OpDecl::sync("url_parse", |args| {
        let input = args.first().and_then(Value::as_str).unwrap_or("");
        let base = args.get(1).and_then(Value::as_str);
        Ok(match parse(input, base) {
            Some(url) => Value::String(components_json(&url)),
            None => Value::Null,
        })
    }))?;

    engine.register_op(OpDecl::sync("url_set", |args| {
        let href = args.first().and_then(Value::as_str).unwrap_or("");
        let component = args.get(1).and_then(Value::as_str).unwrap_or("");
        let value = args.get(2).and_then(Value::as_str).unwrap_or("");
        Ok(match set_component(href, component, value) {
            Some(json) => Value::String(json),
            None => Value::Null,
        })
    }))?;
    Ok(())
}

/// Parses `input`, optionally against `base` (the `new URL(input, base)` form).
fn parse(input: &str, base: Option<&str>) -> Option<Url> {
    match base {
        Some(base) => {
            let base = Url::parse(base).ok()?;
            Url::options().base_url(Some(&base)).parse(input).ok()
        }
        None => Url::parse(input).ok(),
    }
}

/// Applies one component setter to `href` and returns the resulting components.
///
/// Per WHATWG, an invalid component setter is a silent no-op (the URL is
/// returned unchanged); only an invalid `href` assignment fails (→ `None`, which
/// the wrapper turns into a `TypeError`).
fn set_component(href: &str, component: &str, value: &str) -> Option<String> {
    if component == "href" {
        return Url::parse(value).ok().as_ref().map(components_json);
    }

    let mut url = Url::parse(href).ok()?;
    match component {
        "protocol" => {
            let _ = url.set_scheme(value.trim_end_matches(':'));
        }
        "username" => {
            let _ = url.set_username(value);
        }
        "password" => {
            let _ = url.set_password((!value.is_empty()).then_some(value));
        }
        "host" | "hostname" => {
            let _ = url.set_host((!value.is_empty()).then_some(value));
        }
        "port" => {
            let port = if value.is_empty() {
                None
            } else {
                match value.parse::<u16>() {
                    Ok(p) => Some(p),
                    Err(_) => return Some(components_json(&url)),
                }
            };
            let _ = url.set_port(port);
        }
        "pathname" => url.set_path(value),
        "search" => {
            let query = value.strip_prefix('?').unwrap_or(value);
            url.set_query((!query.is_empty()).then_some(query));
        }
        "hash" => {
            let fragment = value.strip_prefix('#').unwrap_or(value);
            url.set_fragment((!fragment.is_empty()).then_some(fragment));
        }
        _ => {}
    }
    Some(components_json(&url))
}

/// Serializes a URL's WHATWG components as a JSON object string.
fn components_json(url: &Url) -> String {
    let host = match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        (None, _) => String::new(),
    };
    let port = url.port().map(|p| p.to_string()).unwrap_or_default();
    let search = url
        .query()
        .filter(|q| !q.is_empty())
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let hash = url
        .fragment()
        .filter(|f| !f.is_empty())
        .map(|f| format!("#{f}"))
        .unwrap_or_default();
    let origin = url.origin().ascii_serialization();
    let protocol = format!("{}:", url.scheme());

    let mut out = String::from("{");
    let mut first = true;
    let mut field = |key: &str, value: &str| {
        if !first {
            out.push(',');
        }
        first = false;
        push_json_string(&mut out, key);
        out.push(':');
        push_json_string(&mut out, value);
    };
    field("href", url.as_str());
    field("protocol", &protocol);
    field("username", url.username());
    field("password", url.password().unwrap_or(""));
    field("host", &host);
    field("hostname", url.host_str().unwrap_or(""));
    field("port", &port);
    field("pathname", url.path());
    field("search", &search);
    field("hash", &hash);
    field("origin", &origin);
    out.push('}');
    out
}

/// Appends `s` as a JSON string literal (quoted, with the required escapes).
fn push_json_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}
