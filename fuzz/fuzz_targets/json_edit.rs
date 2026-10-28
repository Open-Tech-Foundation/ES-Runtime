//! Edits arbitrary JSONC and JSON5 (DECISIONS D191). For text that parses, a
//! path is picked from its value by the input's first bytes, and a value set
//! there or removed. The result must parse, and its value must be the
//! original value with that one change: an edit that keeps the layout must
//! still mean what it was asked to.
#![no_main]

use std::borrow::Cow;

use es_runtime_json::{Dialect, Failure, Segment, Sink, Text, edit, parse, plan};
use libfuzzer_sys::fuzz_target;

/// A value with JavaScript's object semantics: a repeated key keeps its first
/// place and its last value. Numbers compare by bits, so NaN equals NaN.
#[derive(Debug, Clone, PartialEq)]
enum V {
    Null,
    Bool(bool),
    Num(u64),
    Str(Vec<u16>),
    Arr(Vec<V>),
    Obj(Vec<(Vec<u16>, V)>),
}

#[derive(Default)]
struct Build {
    stack: Vec<V>,
    keys: Vec<Vec<u16>>,
    root: Option<V>,
}

fn units(text: Text<'_>) -> Vec<u16> {
    match text {
        Text::Str(s) => s.encode_utf16().collect(),
        Text::Utf16(u) => u,
    }
}

impl Build {
    fn put(&mut self, v: V) -> Result<(), ()> {
        match self.stack.last_mut() {
            Some(V::Arr(items)) => items.push(v),
            Some(V::Obj(members)) => {
                let key = self.keys.pop().expect("a key precedes its value");
                match members.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = v,
                    None => members.push((key, v)),
                }
            }
            _ => self.root = Some(v),
        }
        Ok(())
    }
}

impl Sink for Build {
    type Abort = ();
    fn null(&mut self) -> Result<(), ()> {
        self.put(V::Null)
    }
    fn bool(&mut self, b: bool) -> Result<(), ()> {
        self.put(V::Bool(b))
    }
    fn number(&mut self, n: f64) -> Result<(), ()> {
        self.put(V::Num(if n.is_nan() { f64::NAN.to_bits() } else { n.to_bits() }))
    }
    fn string(&mut self, t: Text<'_>) -> Result<(), ()> {
        self.put(V::Str(units(t)))
    }
    fn begin_array(&mut self) -> Result<(), ()> {
        self.stack.push(V::Arr(Vec::new()));
        Ok(())
    }
    fn end_array(&mut self) -> Result<(), ()> {
        let v = self.stack.pop().unwrap();
        self.put(v)
    }
    fn begin_object(&mut self) -> Result<(), ()> {
        self.stack.push(V::Obj(Vec::new()));
        Ok(())
    }
    fn key(&mut self, k: Text<'_>) -> Result<(), ()> {
        self.keys.push(units(k));
        Ok(())
    }
    fn end_object(&mut self) -> Result<(), ()> {
        let v = self.stack.pop().unwrap();
        self.put(v)
    }
}

fn read(text: &str, dialect: Dialect) -> Option<V> {
    let mut build = Build::default();
    match parse(text, dialect, &mut build) {
        Ok(()) => build.root,
        Err(Failure::Error(_)) => None,
        Err(Failure::Abort(())) => unreachable!(),
    }
}

/// The values the fuzzer sets, compact, with the value each means.
fn values() -> Vec<(&'static str, V)> {
    let s = |t: &str| V::Str(t.encode_utf16().collect());
    vec![
        ("1", V::Num(1f64.to_bits())),
        ("\"x\"", s("x")),
        ("[1,2]", V::Arr(vec![V::Num(1f64.to_bits()), V::Num(2f64.to_bits())])),
        ("{\"k\":null}", V::Obj(vec![(units(Text::Str("k".into())), V::Null)])),
        ("{}", V::Obj(vec![])),
    ]
}

/// A compact array or object laid out over lines with `indent`.
fn layout(compact: &str, indent: &str) -> String {
    if indent.is_empty() || compact.len() < 3 || !compact.starts_with(['[', '{']) {
        return compact.to_owned();
    }
    let (open, rest) = compact.split_at(1);
    let (body, close) = rest.split_at(rest.len() - 1);
    let items: Vec<&str> = body.split(',').collect();
    format!("{open}\n{indent}{}\n{close}", items.join(&format!(",\n{indent}")))
}

/// Walks `choices` down `value` to pick a path: an existing step, or a new key
/// or the next index at the end.
fn pick(value: &V, choices: &[u8]) -> Vec<Segment<'static>> {
    let mut path = Vec::new();
    let mut at = value;
    for &choice in choices {
        match at {
            V::Obj(members) if !members.is_empty() && usize::from(choice) % 4 != 0 => {
                let (key, v) = &members[usize::from(choice) % members.len()];
                let Ok(key) = String::from_utf16(key) else {
                    break;
                };
                path.push(Segment::Key(Cow::Owned(key)));
                at = v;
            }
            V::Obj(_) => {
                path.push(Segment::Key(Cow::Owned(format!("new{choice}"))));
                return path;
            }
            V::Arr(items) => {
                let i = usize::from(choice) % (items.len() + 1);
                path.push(Segment::Index(i));
                match items.get(i) {
                    Some(v) => at = v,
                    None => return path,
                }
            }
            _ => break,
        }
    }
    path
}

/// `value` with `path` set to `new`, or removed.
fn apply(value: &mut V, path: &[Segment<'_>], new: Option<&V>) {
    let Some((last, parents)) = path.split_last() else {
        *value = new.cloned().expect("the root is only replaced");
        return;
    };
    let mut at = value;
    for segment in parents {
        at = match (at, segment) {
            (V::Obj(members), Segment::Key(k)) => {
                let k: Vec<u16> = k.encode_utf16().collect();
                &mut members.iter_mut().find(|(key, _)| *key == k).unwrap().1
            }
            (V::Arr(items), Segment::Index(i)) => &mut items[*i],
            _ => unreachable!("pick follows the value"),
        };
    }
    match (at, last) {
        (V::Obj(members), Segment::Key(k)) => {
            let k: Vec<u16> = k.encode_utf16().collect();
            match (members.iter().position(|(key, _)| *key == k), new) {
                (Some(i), Some(v)) => members[i].1 = v.clone(),
                (Some(i), None) => {
                    members.remove(i);
                }
                (None, Some(v)) => members.push((k, v.clone())),
                (None, None) => {}
            }
        }
        (V::Arr(items), Segment::Index(i)) => match new {
            Some(v) if *i < items.len() => items[*i] = v.clone(),
            Some(v) => items.push(v.clone()),
            None if *i < items.len() => {
                items.remove(*i);
            }
            None => {}
        },
        _ => unreachable!("pick follows the value"),
    }
}

fuzz_target!(|input: &[u8]| {
    let Some((&op, rest)) = input.split_first() else {
        return;
    };
    let choices_len = usize::from(op >> 4).min(rest.len());
    let (choices, text) = rest.split_at(choices_len);
    let Ok(text) = std::str::from_utf8(text) else {
        return;
    };
    let dialect = if op & 1 == 0 { Dialect::Jsonc } else { Dialect::Json5 };
    let Some(before) = read(text, dialect) else {
        return;
    };
    let path = pick(&before, choices);
    let values = values();
    let remove = op & 2 != 0 && !path.is_empty();
    let (compact, meaning) = &values[usize::from(op >> 2) % values.len()];
    let result = if remove {
        edit(text, dialect, &path, None)
    } else {
        let plan = plan(text, dialect, &path).expect("a picked path can be planned");
        edit(text, dialect, &path, Some(&layout(compact, &plan.indent)))
    };
    let after = result.expect("a picked path can be edited");
    let mut want = before;
    apply(&mut want, &path, if remove { None } else { Some(meaning) });
    assert_eq!(read(&after, dialect), Some(want), "edit of {path:?} gave:\n{after}");
});
