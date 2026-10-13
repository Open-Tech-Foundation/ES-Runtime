//! Events to the runtime's [`Value`] tree.
//!
//! Scalars resolve by the YAML 1.2 core schema. The rest is policy the
//! specification leaves to the application, and each choice here is the one
//! that cannot lose data silently: duplicate keys are an error rather than
//! last-wins, an unknown tag is an error rather than ignored, and a key that is
//! itself a collection is an error because a JavaScript object cannot hold it.
//! Merge keys (`<<`), dropped from YAML 1.2 but used throughout CI and compose
//! files, are applied.

use super::parser::{Event, EventKind, Parser};
use super::scanner::ScalarStyle;
use super::{Error, Mark};
use es_runtime_engine::Value;
use std::collections::HashMap;

/// How deep collections may nest. Composition recurses once per level, and
/// the input is untrusted.
pub(crate) const MAX_DEPTH: usize = 256;

/// How many nodes aliases may add to a document, in total. Each alias is a
/// copy of what it names, so without a bound a few hundred bytes of nested
/// aliases (the "billion laughs" document) expand without limit.
pub(crate) const MAX_ALIAS_NODES: usize = 1_000_000;

const TAG_PREFIX: &str = "tag:yaml.org,2002:";

struct Anchor {
    value: Value,
    /// The scalar's text, for when an alias is used as a mapping key.
    key: Option<String>,
    nodes: usize,
}

struct Composer {
    parser: Parser,
    anchors: HashMap<String, Anchor>,
    /// Anchors whose node is still being built; an alias to one is a cycle.
    open: Vec<String>,
    alias_nodes: usize,
}

/// A composed node, with what a mapping needs to use it as a key.
struct Node {
    value: Value,
    /// The text a scalar key contributes, before schema resolution.
    key: Option<String>,
    /// Whether this is the merge key `<<` (plain, untagged).
    merge: bool,
    nodes: usize,
    start: Mark,
}

pub(crate) fn compose(input: &str) -> Result<Value, Error> {
    let mut composer = Composer {
        parser: Parser::new(input)?,
        anchors: HashMap::new(),
        open: Vec::new(),
        alias_nodes: 0,
    };
    composer.expect(|k| matches!(k, EventKind::StreamStart))?;
    let first = composer.next()?;
    let value = match first.kind {
        EventKind::StreamEnd => return Ok(Value::Null),
        EventKind::DocumentStart { .. } => {
            let event = composer.next()?;
            let node = composer.node(event, 0)?;
            composer.expect(|k| matches!(k, EventKind::DocumentEnd { .. }))?;
            node.value
        }
        _ => unreachable!("the parser starts every document with DocumentStart"),
    };
    let next = composer.next()?;
    match next.kind {
        EventKind::StreamEnd => Ok(value),
        _ => Err(Error::syntax(
            "the input holds more than one document; YAML.parse reads exactly one",
            next.start,
        )),
    }
}

/// Checks that `input` would parse. It composes the value, because the
/// failures that come after syntax (a duplicate key, an unknown tag, alias
/// expansion) are only found while building it, and `validate` must agree
/// with `parse`.
pub(crate) fn validate(input: &str) -> Result<(), Error> {
    compose(input).map(|_| ())
}

impl Composer {
    fn next(&mut self) -> Result<Event, Error> {
        match self.parser.next_event()? {
            Some(event) => Ok(event),
            None => unreachable!("the composer stops at StreamEnd"),
        }
    }

    fn expect(&mut self, is: impl Fn(&EventKind) -> bool) -> Result<Event, Error> {
        let event = self.next()?;
        debug_assert!(is(&event.kind), "unexpected event {:?}", event.kind);
        Ok(event)
    }

    fn register(&mut self, anchor: Option<String>, node: &Node) {
        if let Some(name) = anchor {
            self.open.retain(|n| *n != name);
            self.anchors.insert(
                name,
                Anchor {
                    value: node.value.clone(),
                    key: node.key.clone(),
                    nodes: node.nodes,
                },
            );
        }
    }

    fn node(&mut self, event: Event, depth: usize) -> Result<Node, Error> {
        let start = event.start;
        match event.kind {
            EventKind::Alias(name) => {
                if self.open.contains(&name) {
                    return Err(Error::syntax(
                        format!("alias *{name} refers to itself"),
                        start,
                    ));
                }
                let Some(anchor) = self.anchors.get(&name) else {
                    return Err(Error::syntax(format!("unknown anchor *{name}"), start));
                };
                self.alias_nodes += anchor.nodes;
                if self.alias_nodes > MAX_ALIAS_NODES {
                    return Err(Error::limit(
                        format!("aliases expand to more than {MAX_ALIAS_NODES} nodes"),
                        start,
                    ));
                }
                Ok(Node {
                    value: anchor.value.clone(),
                    key: anchor.key.clone(),
                    merge: false,
                    nodes: anchor.nodes,
                    start,
                })
            }
            EventKind::Scalar {
                anchor,
                tag,
                value,
                style,
            } => {
                let merge = tag.is_none() && style == ScalarStyle::Plain && value == "<<";
                let resolved = resolve_scalar(&value, tag.as_deref(), style, start)?;
                let node = Node {
                    value: resolved,
                    key: Some(value),
                    merge,
                    nodes: 1,
                    start,
                };
                self.register(anchor, &node);
                Ok(node)
            }
            EventKind::SequenceStart { anchor, tag, .. } => {
                check_collection_tag(tag.as_deref(), "seq", start)?;
                if depth >= MAX_DEPTH {
                    return Err(too_deep(start));
                }
                if let Some(name) = &anchor {
                    self.open.push(name.clone());
                }
                let mut items = Vec::new();
                let mut nodes = 1;
                loop {
                    let event = self.next()?;
                    if event.kind == EventKind::SequenceEnd {
                        break;
                    }
                    let item = self.node(event, depth + 1)?;
                    nodes += item.nodes;
                    items.push(item.value);
                }
                let node = Node {
                    value: Value::Array(items),
                    key: None,
                    merge: false,
                    nodes,
                    start,
                };
                self.register(anchor, &node);
                Ok(node)
            }
            EventKind::MappingStart { anchor, tag, .. } => {
                check_collection_tag(tag.as_deref(), "map", start)?;
                if depth >= MAX_DEPTH {
                    return Err(too_deep(start));
                }
                if let Some(name) = &anchor {
                    self.open.push(name.clone());
                }
                let node = self.mapping(depth, start)?;
                self.register(anchor, &node);
                Ok(node)
            }
            other => unreachable!("not a node event: {other:?}"),
        }
    }

    fn mapping(&mut self, depth: usize, start: Mark) -> Result<Node, Error> {
        let mut map = Mapping::default();
        let mut nodes = 1;
        loop {
            let event = self.next()?;
            if event.kind == EventKind::MappingEnd {
                break;
            }
            let key = self.node(event, depth + 1)?;
            let value_event = self.next()?;
            let value = self.node(value_event, depth + 1)?;
            nodes += key.nodes + value.nodes;
            if key.merge {
                merge_into(&mut map, value)?;
                continue;
            }
            let name = match (&key.value, key.key) {
                (Value::Array(_), _) => {
                    return Err(Error::syntax(
                        "a sequence cannot be a mapping key in a JavaScript object",
                        key.start,
                    ));
                }
                (Value::Object(_), _) => {
                    return Err(Error::syntax(
                        "a mapping cannot be a mapping key in a JavaScript object",
                        key.start,
                    ));
                }
                (_, Some(text)) => text,
                (_, None) => unreachable!("scalar nodes carry their text"),
            };
            map.insert(name, value.value, key.start)?;
        }
        Ok(Node {
            value: Value::Object(map.entries),
            key: None,
            merge: false,
            nodes,
            start,
        })
    }
}

#[derive(Default)]
struct Mapping {
    entries: Vec<(String, Value)>,
    /// Key → (index in `entries`, whether it came from a merge).
    index: HashMap<String, (usize, bool)>,
}

impl Mapping {
    fn insert(&mut self, key: String, value: Value, mark: Mark) -> Result<(), Error> {
        match self.index.get_mut(&key) {
            Some((_, false)) => Err(Error::syntax(
                format!("duplicate mapping key {key:?}"),
                mark,
            )),
            // A key written in the mapping overrides one merged into it, wherever
            // the merge appeared.
            Some((at, merged)) => {
                *merged = false;
                self.entries[*at].1 = value;
                Ok(())
            }
            None => {
                self.index.insert(key.clone(), (self.entries.len(), false));
                self.entries.push((key, value));
                Ok(())
            }
        }
    }

    /// Adds a merged key unless the mapping already has it: keys written in
    /// the mapping, and keys from earlier merge sources, take precedence.
    fn merge(&mut self, key: String, value: Value) {
        if !self.index.contains_key(&key) {
            self.index.insert(key.clone(), (self.entries.len(), true));
            self.entries.push((key, value));
        }
    }
}

fn merge_into(map: &mut Mapping, source: Node) -> Result<(), Error> {
    let bad = || {
        Error::syntax(
            "a merge key (<<) takes a mapping or a sequence of mappings",
            source.start,
        )
    };
    match source.value {
        Value::Object(entries) => {
            for (k, v) in entries {
                map.merge(k, v);
            }
            Ok(())
        }
        Value::Array(ref items) => {
            if !items.iter().all(|i| matches!(i, Value::Object(_))) {
                return Err(bad());
            }
            let Value::Array(items) = source.value else {
                unreachable!()
            };
            for item in items {
                let Value::Object(entries) = item else {
                    unreachable!()
                };
                for (k, v) in entries {
                    map.merge(k, v);
                }
            }
            Ok(())
        }
        _ => Err(bad()),
    }
}

fn too_deep(start: Mark) -> Error {
    Error::limit(format!("nesting deeper than {MAX_DEPTH} levels"), start)
}

fn check_collection_tag(tag: Option<&str>, kind: &str, start: Mark) -> Result<(), Error> {
    match tag {
        None | Some("!") => Ok(()),
        Some(t) => match t.strip_prefix(TAG_PREFIX) {
            Some(k) if k == kind => Ok(()),
            // Typed collections of the YAML 1.1 type repository, which hold
            // ordinary sequences and mappings.
            Some("omap" | "pairs") if kind == "seq" => Ok(()),
            Some("set") if kind == "map" => Ok(()),
            _ => Err(unsupported_tag(t, start)),
        },
    }
}

fn unsupported_tag(tag: &str, start: Mark) -> Error {
    let shown = match tag.strip_prefix(TAG_PREFIX) {
        Some(short) => format!("!!{short}"),
        None => tag.to_string(),
    };
    Error::syntax(format!("unsupported tag {shown}"), start)
}

/// Resolves a scalar by its tag, or by the core schema when it is plain and
/// untagged.
fn resolve_scalar(
    value: &str,
    tag: Option<&str>,
    style: ScalarStyle,
    start: Mark,
) -> Result<Value, Error> {
    let Some(tag) = tag else {
        return Ok(if style == ScalarStyle::Plain {
            resolve_plain(value)
        } else {
            Value::String(value.to_string())
        });
    };
    if tag == "!" {
        return Ok(Value::String(value.to_string()));
    }
    let invalid = |kind: &str| Error::syntax(format!("{value:?} is not a valid !!{kind}"), start);
    match tag.strip_prefix(TAG_PREFIX) {
        Some("str") => Ok(Value::String(value.to_string())),
        // Kept as written, as before: the runtime has no decoded form for them.
        Some("binary" | "timestamp") => Ok(Value::String(value.to_string())),
        Some("null") => match resolve_plain(value) {
            Value::Null => Ok(Value::Null),
            _ => Err(invalid("null")),
        },
        Some("bool") => match resolve_plain(value) {
            v @ Value::Bool(_) => Ok(v),
            _ => Err(invalid("bool")),
        },
        Some("int") => parse_int(value)
            .map(Value::Number)
            .ok_or_else(|| invalid("int")),
        Some("float") => parse_int(value)
            .or_else(|| parse_float(value))
            .map(Value::Number)
            .ok_or_else(|| invalid("float")),
        _ => Err(unsupported_tag(tag, start)),
    }
}

/// The YAML 1.2 core schema (§10.3.2).
pub(crate) fn resolve_plain(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        _ => match parse_int(s).or_else(|| parse_float(s)) {
            Some(n) => Value::Number(n),
            None => Value::String(s.to_string()),
        },
    }
}

/// `[-+]? [0-9]+`, `0o[0-7]+` or `0x[0-9a-fA-F]+`.
pub(crate) fn parse_int(s: &str) -> Option<f64> {
    let (radix, digits, negative) = if let Some(d) = s.strip_prefix("0o") {
        (8, d, false)
    } else if let Some(d) = s.strip_prefix("0x") {
        (16, d, false)
    } else {
        let (negative, d) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        (10, d, negative)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = match u128::from_str_radix(digits, radix) {
        Ok(n) => n as f64,
        // Past u128 only a decimal can still be a finite double; let the float
        // parser round it correctly.
        Err(_) if radix == 10 => digits.parse::<f64>().ok()?,
        Err(_) => digits.chars().fold(0.0, |acc, c| {
            acc * radix as f64 + c.to_digit(radix).unwrap() as f64
        }),
    };
    // An integer has no negative zero.
    Some(if negative && magnitude != 0.0 {
        -magnitude
    } else {
        magnitude
    })
}

/// `[-+]? ( \. [0-9]+ | [0-9]+ ( \. [0-9]* )? ) ( [eE] [-+]? [0-9]+ )?`, and the
/// infinities and NaN.
pub(crate) fn parse_float(s: &str) -> Option<f64> {
    match s {
        ".nan" | ".NaN" | ".NAN" => return Some(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return Some(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return Some(f64::NEG_INFINITY),
        _ => {}
    }
    let b = s.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'-' | b'+')) {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let int_digits = i - int_start;
    let mut frac_digits = 0;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        frac_digits = i - frac_start;
    }
    if int_digits == 0 && frac_digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        let exp_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
    }
    if i != b.len() {
        return None;
    }
    // Rust's parser takes every form the pattern does except a bare trailing
    // point before an exponent (`1.e5`), so normalise that one.
    let normalised = s.replacen(".e", ".0e", 1).replacen(".E", ".0E", 1);
    let text = if normalised.ends_with('.') {
        &normalised[..normalised.len() - 1]
    } else {
        &normalised
    };
    text.parse::<f64>().ok()
}
