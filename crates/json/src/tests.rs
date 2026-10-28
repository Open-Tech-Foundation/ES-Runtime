use super::*;

/// A value as the sink saw it. Objects keep every key in order, duplicates
/// included, so a test sees exactly what the parser reported.
#[derive(Debug, PartialEq)]
enum V {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Wide(Vec<u16>),
    Arr(Vec<V>),
    Obj(Vec<(V, V)>),
}

#[derive(Default)]
struct Collect {
    stack: Vec<V>,
    keys: Vec<V>,
    root: Option<V>,
}

impl Collect {
    fn put(&mut self, v: V) -> Result<(), ()> {
        match self.stack.last_mut() {
            Some(V::Arr(items)) => items.push(v),
            Some(V::Obj(members)) => members.push((self.keys.pop().unwrap(), v)),
            _ => self.root = Some(v),
        }
        Ok(())
    }
}

fn text(t: Text<'_>) -> V {
    match t {
        Text::Str(s) => V::Str(s.into_owned()),
        Text::Utf16(units) => V::Wide(units),
    }
}

impl Sink for Collect {
    type Abort = ();
    fn null(&mut self) -> Result<(), ()> {
        self.put(V::Null)
    }
    fn bool(&mut self, b: bool) -> Result<(), ()> {
        self.put(V::Bool(b))
    }
    fn number(&mut self, n: f64) -> Result<(), ()> {
        self.put(V::Num(n))
    }
    fn string(&mut self, t: Text<'_>) -> Result<(), ()> {
        self.put(text(t))
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
        self.keys.push(text(k));
        Ok(())
    }
    fn end_object(&mut self) -> Result<(), ()> {
        let v = self.stack.pop().unwrap();
        self.put(v)
    }
}

fn read(text: &str, dialect: Dialect) -> Result<V, Error> {
    let mut sink = Collect::default();
    match parse(text, dialect, &mut sink) {
        Ok(()) => Ok(sink.root.unwrap()),
        Err(Failure::Error(e)) => Err(*e),
        Err(Failure::Abort(())) => unreachable!(),
    }
}

fn jsonc(text: &str) -> V {
    read(text, Dialect::Jsonc).unwrap_or_else(|e| panic!("{text:?}: {e}"))
}

fn json5(text: &str) -> V {
    read(text, Dialect::Json5).unwrap_or_else(|e| panic!("{text:?}: {e}"))
}

fn jsonc_err(text: &str) -> String {
    read(text, Dialect::Jsonc).expect_err(text).to_string()
}

fn json5_err(text: &str) -> String {
    read(text, Dialect::Json5).expect_err(text).to_string()
}

fn obj(members: &[(&str, V)]) -> V {
    V::Obj(members.iter().map(|(k, v)| (s(k), clone(v))).collect())
}

fn clone(v: &V) -> V {
    match v {
        V::Null => V::Null,
        V::Bool(b) => V::Bool(*b),
        V::Num(n) => V::Num(*n),
        V::Str(s) => V::Str(s.clone()),
        V::Wide(w) => V::Wide(w.clone()),
        V::Arr(items) => V::Arr(items.iter().map(clone).collect()),
        V::Obj(m) => V::Obj(m.iter().map(|(k, v)| (clone(k), clone(v))).collect()),
    }
}

fn s(text: &str) -> V {
    V::Str(text.to_owned())
}

// ---- JSONC --------------------------------------------------------------------

#[test]
fn jsonc_reads_comments_anywhere_a_space_may_be() {
    let text = "// leading\n{ /* a */ \"a\" /* b */ : /* c */ 1 /* d */, // e\n \"b\": [ /* f */ ] } /* end */ // tail";
    assert_eq!(
        jsonc(text),
        obj(&[("a", V::Num(1.0)), ("b", V::Arr(vec![]))])
    );
    assert_eq!(jsonc("1//"), V::Num(1.0));
    assert_eq!(jsonc("/**/1/***/"), V::Num(1.0));
}

#[test]
fn jsonc_takes_one_trailing_comma() {
    assert_eq!(jsonc("[1,2,]"), V::Arr(vec![V::Num(1.0), V::Num(2.0)]));
    assert_eq!(jsonc("{\"a\":1,}"), obj(&[("a", V::Num(1.0))]));
    assert!(jsonc_err("[1,,]").starts_with("expected a value, found ','"));
    assert!(jsonc_err("[,]").starts_with("expected a value, found ','"));
    assert!(jsonc_err("{,}").starts_with("expected a property name, found ','"));
    assert!(jsonc_err("{\"a\":1,,}").starts_with("expected a property name"));
}

#[test]
fn jsonc_is_json_otherwise() {
    for bad in [
        "{a:1}",
        "['a']",
        "0x10",
        "+1",
        ".5",
        "5.",
        "Infinity",
        "NaN",
        "01",
        "\"\\v\"",
        "\"\\x41\"",
        "\"a\nb\"",
        "\"\t\"",
        "# comment\n1",
        "1 2",
        "",
        "  ",
        "tru",
        "nulls",
        "-",
        "1e",
        "1.e3",
        "\"\\'\"",
    ] {
        assert!(
            read(bad, Dialect::Jsonc).is_err(),
            "{bad:?} must be refused"
        );
    }
    assert_eq!(
        jsonc_err("{a:1}"),
        "a property name must be a double-quoted string at line 1 column 2"
    );
}

#[test]
fn jsonc_ignores_a_leading_byte_order_mark_only() {
    assert_eq!(jsonc("\u{feff}{}"), obj(&[]));
    assert!(read("{}\u{feff}", Dialect::Jsonc).is_err());
    assert!(read(" \u{feff}{}", Dialect::Jsonc).is_err());
}

#[test]
fn an_unclosed_comment_or_string_is_refused_where_it_starts() {
    assert_eq!(
        jsonc_err("[1, /* open\n]"),
        "a comment is not closed at line 1 column 5"
    );
    assert_eq!(
        jsonc_err("\n  \"abc"),
        "a string is not closed at line 2 column 3"
    );
    assert_eq!(jsonc_err("1 / 2"), "unexpected '/' at line 1 column 3");
}

#[test]
fn numbers_read_as_json_parse_reads_them() {
    assert_eq!(jsonc("-0"), V::Num(-0.0));
    assert!(matches!(jsonc("-0"), V::Num(n) if n.is_sign_negative()));
    assert_eq!(jsonc("1e400"), V::Num(f64::INFINITY));
    assert_eq!(jsonc("-1e400"), V::Num(f64::NEG_INFINITY));
    assert_eq!(jsonc("1E-400"), V::Num(0.0));
    assert_eq!(jsonc("0.1"), V::Num(0.1));
    assert_eq!(
        jsonc("123456789012345678901234567890"),
        V::Num(1.2345678901234568e29)
    );
}

#[test]
fn escapes_and_surrogates() {
    assert_eq!(
        jsonc(r#""a\"\\\/\b\f\n\r\tz""#),
        s("a\"\\/\u{8}\u{c}\n\r\tz")
    );
    assert_eq!(jsonc(r#""\ud83d\ude00""#), s("😀"));
    assert_eq!(jsonc(r#""\u00e9""#), s("é"));
    // A lone surrogate survives, as a JavaScript string keeps it.
    assert_eq!(jsonc(r#""a\ud800b""#), V::Wide(vec![0x61, 0xd800, 0x62]));
    assert_eq!(jsonc(r#""\udc00\ud800""#), V::Wide(vec![0xdc00, 0xd800]));
    assert_eq!(jsonc(r#""\ud800""#), V::Wide(vec![0xd800]));
    assert!(jsonc_err(r#""\u12""#).starts_with("an escape needs 4 hex digits"));
    // U+2028 needs no escape in JSON.
    assert_eq!(jsonc("\"\u{2028}\""), s("\u{2028}"));
}

#[test]
fn duplicate_keys_are_all_reported_in_order() {
    // The engine's sink makes the last one win, as JSON.parse does.
    assert_eq!(
        jsonc(r#"{"a":1,"b":2,"a":3}"#),
        obj(&[("a", V::Num(1.0)), ("b", V::Num(2.0)), ("a", V::Num(3.0))])
    );
}

#[test]
fn error_positions_count_lines_and_characters() {
    assert_eq!(
        jsonc_err("{\r\n  \"é\": tru\n}"),
        "expected a value, found 'tru' at line 2 column 8"
    );
    assert_eq!(
        jsonc_err("[1]\r[2]"),
        "unexpected text after the value at line 2 column 1"
    );
    assert_eq!(
        jsonc_err("[1 2]"),
        "expected ',' or ']', found '2' at line 1 column 4"
    );
    assert_eq!(
        jsonc_err("[1"),
        "expected ',' or ']', found the end of the text at line 1 column 3"
    );
}

#[test]
fn nesting_is_bounded() {
    let ok = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
    assert!(read(&ok, Dialect::Jsonc).is_ok());
    let deep = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
    let e = read(&deep, Dialect::Jsonc).unwrap_err();
    assert_eq!(e.kind, ErrorKind::Depth);
    assert_eq!(e.column, MAX_DEPTH + 1);
    // Far deeper than the stack would allow without the bound.
    let deeper = "{\"a\":".repeat(100_000);
    assert_eq!(
        read(&deeper, Dialect::Json5).unwrap_err().kind,
        ErrorKind::Depth
    );
}

#[test]
fn validate_agrees_with_parse() {
    assert!(validate("{\"a\": [1, 2,], // c\n}", Dialect::Jsonc).is_ok());
    assert_eq!(
        validate("{a: 1}", Dialect::Jsonc).unwrap_err().kind,
        ErrorKind::Syntax
    );
    assert!(validate("{a: 1}", Dialect::Json5).is_ok());
}

// ---- JSON5 --------------------------------------------------------------------

#[test]
fn json5_example_from_its_specification() {
    let text = r#"// This file is written in JSON5 syntax, naturally, but npm needs a regular
// JSON file, so compile via `npm run build`. Be sure to keep both in sync!
{
  unquoted: 'and you can quote me on that',
  singleQuotes: 'I can use "double quotes" here',
  lineBreaks: "Look, Mom! \
No \\n's!",
  hexadecimal: 0xdecaf,
  leadingDecimalPoint: .8675309, andTrailing: 8675309.,
  positiveSign: +1,
  trailingComma: 'in objects', andIn: ['arrays',],
  "backwardsCompatible": "with JSON",
}"#;
    assert_eq!(
        json5(text),
        obj(&[
            ("unquoted", s("and you can quote me on that")),
            ("singleQuotes", s("I can use \"double quotes\" here")),
            ("lineBreaks", s("Look, Mom! No \\n's!")),
            ("hexadecimal", V::Num(912_559.0)),
            ("leadingDecimalPoint", V::Num(0.8675309)),
            ("andTrailing", V::Num(8_675_309.0)),
            ("positiveSign", V::Num(1.0)),
            ("trailingComma", s("in objects")),
            ("andIn", V::Arr(vec![s("arrays")])),
            ("backwardsCompatible", s("with JSON")),
        ])
    );
}

#[test]
fn json5_numbers() {
    assert_eq!(json5("Infinity"), V::Num(f64::INFINITY));
    assert_eq!(json5("-Infinity"), V::Num(f64::NEG_INFINITY));
    assert_eq!(json5("+Infinity"), V::Num(f64::INFINITY));
    assert!(matches!(json5("NaN"), V::Num(n) if n.is_nan()));
    assert!(matches!(json5("-NaN"), V::Num(n) if n.is_nan()));
    assert_eq!(json5("-0x10"), V::Num(-16.0));
    assert_eq!(json5("0XFF"), V::Num(255.0));
    assert_eq!(json5("+.5e1"), V::Num(5.0));
    assert_eq!(json5("5.e-1"), V::Num(0.5));
    assert_eq!(json5("-.0"), V::Num(-0.0));
    for bad in [
        "01",
        ".",
        "+",
        "-.",
        "0x",
        "0xg",
        "1.2.3",
        "Infinityx",
        "NaNa",
        "+ 1",
        "- Infinity",
        "1e",
        "+-1",
        "0b1",
        "0o7",
        "08",
        "1_000",
    ] {
        assert!(
            read(bad, Dialect::Json5).is_err(),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn json5_strings() {
    assert_eq!(json5(r#"'a"b\'c'"#), s("a\"b'c"));
    assert_eq!(json5(r#""\v\0\x41\q\/""#), s("\u{b}\0Aq/"));
    assert_eq!(json5("'a\\\r\nb\\\rc\\\u{2028}d'"), s("abcd"));
    assert_eq!(json5("'\t\u{2028}\u{2029}'"), s("\t\u{2028}\u{2029}"));
    assert_eq!(json5(r#"'\xff'"#), s("ÿ"));
    for bad in [
        r#"'\1'"#, r#"'\01'"#, r#"'\x4'"#, "'a\nb'", "'a\rb'", "'open",
    ] {
        assert!(
            read(bad, Dialect::Json5).is_err(),
            "{bad:?} must be refused"
        );
    }
}

#[test]
fn json5_property_names() {
    assert_eq!(
        json5("{$a:1, _b:2, c3:3, ünï:4, \\u0061b:5, true:6, NaN:7, Infinity:8, 'q':9, \"r\":10,}"),
        obj(&[
            ("$a", V::Num(1.0)),
            ("_b", V::Num(2.0)),
            ("c3", V::Num(3.0)),
            ("ünï", V::Num(4.0)),
            ("ab", V::Num(5.0)),
            ("true", V::Num(6.0)),
            ("NaN", V::Num(7.0)),
            ("Infinity", V::Num(8.0)),
            ("q", V::Num(9.0)),
            ("r", V::Num(10.0)),
        ])
    );
    for bad in [
        "{1a:1}",
        "{a-b:1}",
        "{\\u0031:1}",
        "{a\\x62:1}",
        "{:1}",
        "{a b:1}",
        "{1:1}",
    ] {
        assert!(
            read(bad, Dialect::Json5).is_err(),
            "{bad:?} must be refused"
        );
    }
    // An escaped keyword is a name, never the value.
    assert!(read("\\u0074rue", Dialect::Json5).is_err());
    assert!(read("[undefined]", Dialect::Json5).is_err());
}

#[test]
fn json5_whitespace_and_comments() {
    let spaces = "\u{b}\u{c}\u{a0}\u{feff}\u{2028}\u{2029}\u{1680}\u{2000}\u{3000}";
    assert_eq!(
        json5(&format!("{spaces}[{spaces}1{spaces}]{spaces}")),
        V::Arr(vec![V::Num(1.0)])
    );
    assert_eq!(
        json5("[1 // to the separator\u{2028}, 2]"),
        V::Arr(vec![V::Num(1.0), V::Num(2.0)])
    );
    assert!(read("\u{85}1", Dialect::Json5).is_err());
    assert!(read("\u{200b}1", Dialect::Json5).is_err());
}

#[test]
fn json5_lines_end_at_the_separators_too() {
    let e = json5_err("1\u{2028}2");
    assert!(e.ends_with("at line 2 column 1"), "{e}");
    let e = jsonc_err("1\u{2028}2");
    assert!(e.ends_with("at line 1 column 2"), "{e}");
}

// ---- JSONTestSuite and json5-tests ----------------------------------------------

/// Cases whose outcome differs from the suite's expectation, and why. Keyed
/// `dialect suite: name`. A listed case that stops differing fails the run, so
/// the list cannot go stale.
const DIFFERENCES: &[(&str, &str)] = &[
    // Both dialects: what JSONC adds to JSON, and D191's limits.
    (
        "jsonc JSONTestSuite: i_structure_500_nested_arrays.json",
        "nesting is bounded at 256 levels (D189)",
    ),
    (
        "jsonc JSONTestSuite: i_structure_UTF-8_BOM_empty_object.json",
        "a leading byte order mark is ignored, as RFC 8259 §8.1 permits",
    ),
    (
        "jsonc JSONTestSuite: n_array_extra_comma.json",
        "one trailing comma is allowed",
    ),
    (
        "jsonc JSONTestSuite: n_array_number_and_comma.json",
        "one trailing comma is allowed",
    ),
    (
        "jsonc JSONTestSuite: n_object_trailing_comma.json",
        "one trailing comma is allowed",
    ),
    (
        "jsonc JSONTestSuite: n_object_trailing_comment.json",
        "comments are allowed",
    ),
    (
        "jsonc JSONTestSuite: n_object_trailing_comment_slash_open.json",
        "comments are allowed",
    ),
    (
        "jsonc JSONTestSuite: n_structure_object_with_comment.json",
        "comments are allowed",
    ),
    (
        "json5 JSONTestSuite: i_structure_500_nested_arrays.json",
        "nesting is bounded at 256 levels (D189)",
    ),
    (
        "json5 JSONTestSuite: i_structure_UTF-8_BOM_empty_object.json",
        "a leading byte order mark is ignored, as RFC 8259 §8.1 permits",
    ),
    (
        "json5 JSONTestSuite: n_array_extra_comma.json",
        "one trailing comma is allowed",
    ),
    (
        "json5 JSONTestSuite: n_array_number_and_comma.json",
        "one trailing comma is allowed",
    ),
    (
        "json5 JSONTestSuite: n_object_trailing_comma.json",
        "one trailing comma is allowed",
    ),
    (
        "json5 JSONTestSuite: n_object_trailing_comment.json",
        "comments are allowed",
    ),
    (
        "json5 JSONTestSuite: n_object_trailing_comment_slash_open.json",
        "comments are allowed",
    ),
    (
        "json5 JSONTestSuite: n_structure_object_with_comment.json",
        "comments are allowed",
    ),
    // JSON5 on JSON's refusals: what JSON5 adds.
    (
        "json5 JSONTestSuite: n_number_+1.json",
        "a leading + is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_-2..json",
        "a trailing decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_-NaN.json",
        "a signed NaN is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_.2e-3.json",
        "a leading decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_0.e1.json",
        "a trailing decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_2.e+3.json",
        "a trailing decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_2.e-3.json",
        "a trailing decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_2.e3.json",
        "a trailing decimal point is JSON5",
    ),
    ("json5 JSONTestSuite: n_number_NaN.json", "NaN is JSON5"),
    (
        "json5 JSONTestSuite: n_number_hex_1_digit.json",
        "hex is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_hex_2_digits.json",
        "hex is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_infinity.json",
        "Infinity is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_minus_infinity.json",
        "-Infinity is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_neg_real_without_int_part.json",
        "a leading decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_real_without_fractional_part.json",
        "a trailing decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_number_starting_with_dot.json",
        "a leading decimal point is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_object_key_with_single_quotes.json",
        "single-quoted strings are JSON5",
    ),
    (
        "json5 JSONTestSuite: n_object_repeated_null_null.json",
        "an identifier key, here `null`, is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_object_single_quote.json",
        "single-quoted strings are JSON5",
    ),
    (
        "json5 JSONTestSuite: n_object_unquoted_key.json",
        "an identifier key is JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_backslash_00.json",
        "any other character escapes to itself in JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_escape_x.json",
        "\\x escapes are JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_escaped_ctrl_char_tab.json",
        "any other character escapes to itself in JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_escaped_emoji.json",
        "any other character escapes to itself in JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_invalid_backslash_esc.json",
        "any other character escapes to itself in JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_single_quote.json",
        "single-quoted strings are JSON5",
    ),
    (
        "json5 JSONTestSuite: n_string_unescaped_ctrl_char.json",
        "JSON5 strings need only quotes, backslashes and line breaks escaped",
    ),
    (
        "json5 JSONTestSuite: n_string_unescaped_tab.json",
        "JSON5 strings need only quotes, backslashes and line breaks escaped",
    ),
    (
        "json5 JSONTestSuite: n_string_unicode_CapitalU.json",
        "any other character escapes to itself in JSON5",
    ),
    (
        "json5 JSONTestSuite: n_structure_whitespace_formfeed.json",
        "form feed is JSON5 whitespace",
    ),
    // JSONC on JSON5 cases that use only what JSONC also has.
    (
        "jsonc json5-tests: arrays/trailing-comma-array.json5",
        "only a trailing comma",
    ),
    (
        "jsonc json5-tests: comments/block-comment-following-array-element.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: comments/block-comment-following-top-level-value.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: comments/block-comment-preceding-top-level-value.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: comments/block-comment-with-asterisks.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: comments/inline-comment-following-array-element.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: comments/inline-comment-following-top-level-value.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: comments/inline-comment-preceding-top-level-value.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: new-lines/comment-cr.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: new-lines/comment-crlf.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: new-lines/comment-lf.json5",
        "only a comment",
    ),
    (
        "jsonc json5-tests: objects/trailing-comma-object.json5",
        "only a trailing comma",
    ),
];

/// json5-tests' errorSpec positions that are the reference parser's, not ours.
const POSITIONS: &[(&str, &str)] = &[
    (
        "json5 json5-tests: comments/top-level-inline-comment.txt",
        "the reference counts the end of the text one past the last character",
    ),
    (
        "json5 json5-tests: strings/unescaped-multi-line-string.txt",
        "we point at the line break; the reference just after it, at column 0",
    ),
];

/// The value as the generator encodes the engine's: a number as its float64
/// bits, a string with a lone surrogate as UTF-16 units, and an object with
/// JavaScript's property semantics (a duplicate key keeps its first place and
/// its last value; integer keys come first, ascending).
fn encode(v: &V) -> serde_json::Value {
    use serde_json::{Value, json};
    match v {
        V::Null => Value::Null,
        V::Bool(b) => Value::Bool(*b),
        V::Num(n) => json!({ "n": format!("{:016x}", n.to_bits()) }),
        V::Str(s) => Value::String(s.clone()),
        V::Wide(units) => json!({ "u16": units }),
        V::Arr(items) => Value::Array(items.iter().map(encode).collect()),
        V::Obj(members) => {
            let mut props: Vec<(serde_json::Value, serde_json::Value)> = Vec::new();
            for (k, v) in members {
                let (k, v) = (encode(k), encode(v));
                match props.iter_mut().find(|(seen, _)| *seen == k) {
                    Some(slot) => slot.1 = v,
                    None => props.push((k, v)),
                }
            }
            let index = |k: &serde_json::Value| {
                k.as_str()
                    .filter(|s| *s == "0" || (!s.starts_with('0') && !s.is_empty()))
                    .and_then(|s| s.parse::<u32>().ok())
                    .filter(|i| *i != u32::MAX)
            };
            props.sort_by_key(|(k, _)| index(k).map_or((1, 0), |i| (0, i)));
            json!({ "o": props.into_iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>() })
        }
    }
}

#[test]
fn json_test_suite_and_json5_tests() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../test-suites/cases.json")).unwrap();
    let differences: std::collections::HashMap<&str, &str> = DIFFERENCES.iter().copied().collect();
    let mut used = std::collections::HashSet::new();
    let mut failures = Vec::new();
    let mut passed = 0;
    for case in &cases {
        let suite = case["suite"].as_str().unwrap();
        let name = case["name"].as_str().unwrap();
        let kind = case["kind"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let json = &case["json"];
        for dialect in [Dialect::Jsonc, Dialect::Json5] {
            let label = match dialect {
                Dialect::Jsonc => "jsonc",
                Dialect::Json5 => "json5",
            };
            let id = format!("{label} {suite}: {name}");
            // Whether the dialect must accept the text, by the suite's rules.
            let mut accept = match (suite, kind, dialect) {
                (_, "either", _) => json["accepts"].as_bool().unwrap(),
                ("JSONTestSuite", k, _) => k == "valid",
                (_, k, Dialect::Json5) => k == "valid",
                (_, _, Dialect::Jsonc) => name.ends_with(".json"),
            };
            let listed = differences.get(id.as_str());
            if listed.is_some() {
                used.insert(id.clone());
                accept = !accept;
            }
            let got = read(text, dialect);
            match (&got, accept) {
                (Ok(value), true) => {
                    // JSON.parse's value, wherever it read the text and the
                    // case is not a listed difference.
                    if json["accepts"] == true
                        && listed.is_none()
                        && serde_json::to_string(&encode(value)).unwrap()
                            != json["value"].as_str().unwrap()
                    {
                        failures.push(format!("{id}: value differs from JSON.parse"));
                        continue;
                    }
                }
                (Err(e), false) => {
                    let position_listed = POSITIONS.iter().any(|(p, _)| *p == id);
                    if position_listed {
                        used.insert(id.clone());
                    }
                    if dialect == Dialect::Json5
                        && !position_listed
                        && let Some(at) = case.get("error").filter(|e| !e.is_null())
                        && (e.line, e.column)
                            != (
                                at["line"].as_u64().unwrap() as usize,
                                at["column"].as_u64().unwrap() as usize,
                            )
                    {
                        failures.push(format!(
                            "{id}: error at {}:{}, the suite says {}:{}",
                            e.line, e.column, at["line"], at["column"]
                        ));
                        continue;
                    }
                }
                (Ok(_), false) => {
                    failures.push(format!("{id}: accepted, must be refused"));
                    continue;
                }
                (Err(e), true) => {
                    failures.push(format!("{id}: refused: {e}"));
                    continue;
                }
            }
            passed += 1;
        }
    }
    for (id, _) in DIFFERENCES.iter().chain(POSITIONS) {
        if !used.contains(*id) {
            failures.push(format!("{id}: listed in DIFFERENCES but not in the suites"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} passed, {} failed:\n{}",
        passed,
        failures.len(),
        failures.join("\n")
    );
}

// ---- edit -----------------------------------------------------------------------

fn path<'a>(steps: &[&'a str]) -> Vec<Segment<'a>> {
    steps
        .iter()
        .map(|s| match s.parse::<usize>() {
            Ok(i) => Segment::Index(i),
            Err(_) => Segment::Key(std::borrow::Cow::Borrowed(*s)),
        })
        .collect()
}

/// Sets the value at `steps` to `value`, written as the plan asks: `value` is
/// given compact, and laid out here with the plan's indent the way
/// `JSON.stringify(v, null, indent)` would for these simple test values.
fn set(text: &str, dialect: Dialect, steps: &[&str], value: &str) -> String {
    let p = path(steps);
    let plan = plan(text, dialect, &p).unwrap();
    let value = layout(value, &plan.indent);
    let out = edit(text, dialect, &p, Some(&value)).unwrap();
    assert!(
        validate(&out, dialect).is_ok(),
        "the result must parse:\n{out}"
    );
    out
}

/// Lays out a compact JSON array or object of scalars over lines with
/// `indent`, as `JSON.stringify` does; anything else as it is.
fn layout(compact: &str, indent: &str) -> String {
    if indent.is_empty()
        || compact.len() < 3
        || !(compact.starts_with('[') || compact.starts_with('{'))
    {
        return compact.to_owned();
    }
    let (open, close) = compact.split_at(compact.len() - 1);
    let (open, body) = open.split_at(1);
    let items: Vec<&str> = body.split(',').collect();
    format!(
        "{open}\n{indent}{}\n{close}",
        items.join(&format!(",\n{indent}"))
    )
}

fn remove(text: &str, dialect: Dialect, steps: &[&str]) -> String {
    let out = edit(text, dialect, &path(steps), None).unwrap();
    assert!(
        validate(&out, dialect).is_ok(),
        "the result must parse:\n{out}"
    );
    out
}

const TSCONFIG: &str = r#"{
  // Visit https://aka.ms/tsconfig to read more about this file
  "compilerOptions": {
    /* Language and Environment */
    "target": "ES2020", // the emitted language
    "strict": true
  },
  "include": ["src"]
}
"#;

#[test]
fn edit_replaces_a_value_and_nothing_else() {
    assert_eq!(
        set(
            TSCONFIG,
            Dialect::Jsonc,
            &["compilerOptions", "target"],
            r#""ES2022""#
        ),
        TSCONFIG.replace(r#""ES2020""#, r#""ES2022""#)
    );
    assert_eq!(
        set(TSCONFIG, Dialect::Jsonc, &["include", "0"], r#""lib""#),
        TSCONFIG.replace(r#"["src"]"#, r#"["lib"]"#)
    );
}

#[test]
fn edit_appends_a_key_in_the_style_of_its_siblings() {
    let out = set(
        TSCONFIG,
        Dialect::Jsonc,
        &["compilerOptions", "types"],
        r#"["@opentf/esrun-types"]"#,
    );
    assert_eq!(
        out,
        r#"{
  // Visit https://aka.ms/tsconfig to read more about this file
  "compilerOptions": {
    /* Language and Environment */
    "target": "ES2020", // the emitted language
    "strict": true,
    "types": [
      "@opentf/esrun-types"
    ]
  },
  "include": ["src"]
}
"#
    );
    // A comment after the last member stays on its line; the comma goes before it.
    let text = "{\n  \"a\": 1 // one\n}";
    assert_eq!(
        set(text, Dialect::Jsonc, &["b"], "2"),
        "{\n  \"a\": 1, // one\n  \"b\": 2\n}"
    );
}

#[test]
fn edit_keeps_trailing_commas_tabs_and_crlf() {
    let text = "{\r\n\t\"a\": 1,\r\n}\r\n";
    assert_eq!(
        set(text, Dialect::Jsonc, &["b"], "[1,2]"),
        "{\r\n\t\"a\": 1,\r\n\t\"b\": [\r\n\t\t1,\r\n\t\t2\r\n\t],\r\n}\r\n"
    );
    let p = plan(text, Dialect::Jsonc, &path(&["b"])).unwrap();
    assert_eq!((p.indent.as_str(), p.trailing_commas), ("\t", true));
}

#[test]
fn edit_creates_missing_objects_on_the_path() {
    let text = "{\n    \"name\": \"app\"\n}\n";
    assert_eq!(
        set(
            text,
            Dialect::Jsonc,
            &["compilerOptions", "types"],
            r#"["x"]"#
        ),
        "{\n    \"name\": \"app\",\n    \"compilerOptions\": {\n        \"types\": [\n            \"x\"\n        ]\n    }\n}\n"
    );
    assert_eq!(
        set("{}", Dialect::Jsonc, &["a", "b"], "1"),
        r#"{"a": {"b": 1}}"#
    );
    let e = edit(text, Dialect::Jsonc, &path(&["list", "0"]), Some("1")).unwrap_err();
    assert!(matches!(e, EditError::Path(_)), "{e}");
}

#[test]
fn edit_fills_an_empty_container_in_the_file_s_layout() {
    assert_eq!(
        set(
            "{\n  \"paths\": {}\n}",
            Dialect::Jsonc,
            &["paths", "a"],
            "1"
        ),
        "{\n  \"paths\": {\n    \"a\": 1\n  }\n}"
    );
    assert_eq!(
        set("{\"paths\": {}}", Dialect::Jsonc, &["paths", "a"], "1"),
        "{\"paths\": {\"a\": 1}}"
    );
    assert_eq!(
        set(
            "{\n  \"list\": [ /* none yet */ ]\n}",
            Dialect::Jsonc,
            &["list", "0"],
            "1"
        ),
        "{\n  \"list\": [ /* none yet */\n    1\n  ]\n}"
    );
}

#[test]
fn edit_writes_inline_containers_inline() {
    assert_eq!(
        set(r#"{"a":1,"b":2}"#, Dialect::Jsonc, &["c"], "3"),
        r#"{"a":1,"b":2,"c":3}"#
    );
    assert_eq!(
        set(r#"{"a": 1}"#, Dialect::Jsonc, &["b"], "[1,2]"),
        r#"{"a": 1, "b": [1,2]}"#
    );
    assert_eq!(set("[1, 2,]", Dialect::Jsonc, &["2"], "3"), "[1, 2, 3,]");
    assert_eq!(set("[1, 2]", Dialect::Jsonc, &["2"], "3"), "[1, 2, 3]");
}

#[test]
fn edit_refuses_what_it_cannot_do() {
    let e = edit("[1]", Dialect::Jsonc, &path(&["5"]), Some("1")).unwrap_err();
    assert!(matches!(e, EditError::Range(_)), "{e}");
    let e = edit(
        r#"{"a": "x"}"#,
        Dialect::Jsonc,
        &path(&["a", "b"]),
        Some("1"),
    )
    .unwrap_err();
    assert_eq!(
        e.to_string(),
        "the value at .a is a string, which has no property \"b\""
    );
    let e = edit("[1]", Dialect::Jsonc, &path(&["a"]), Some("1")).unwrap_err();
    assert_eq!(
        e.to_string(),
        "the value at the top is an array, which has no property \"a\""
    );
    let e = edit("{a: 1}", Dialect::Jsonc, &path(&["a"]), Some("1")).unwrap_err();
    assert!(matches!(e, EditError::Syntax(_)), "{e}");
    let e = edit("{}", Dialect::Jsonc, &path(&["a"]), Some("'x'")).unwrap_err();
    assert!(matches!(e, EditError::Path(_)), "{e}");
    let e = edit("{}", Dialect::Jsonc, &[], None).unwrap_err();
    assert!(matches!(e, EditError::Path(_)), "{e}");
}

#[test]
fn edit_removes_a_member_with_its_line_and_comma() {
    let out = remove(TSCONFIG, Dialect::Jsonc, &["compilerOptions", "target"]);
    assert_eq!(
        out,
        TSCONFIG.replace("    \"target\": \"ES2020\", // the emitted language\n", "")
    );
    // The last member takes the comma before it.
    let out = remove(TSCONFIG, Dialect::Jsonc, &["compilerOptions", "strict"]);
    assert_eq!(
        out,
        TSCONFIG.replace(
            "    \"target\": \"ES2020\", // the emitted language\n    \"strict\": true\n",
            "    \"target\": \"ES2020\" // the emitted language\n"
        )
    );
    assert_eq!(
        remove(r#"{"a": 1, "b": 2, "c": 3}"#, Dialect::Jsonc, &["b"]),
        r#"{"a": 1, "c": 3}"#
    );
    assert_eq!(
        remove(r#"{"a": 1, "b": 2}"#, Dialect::Jsonc, &["b"]),
        r#"{"a": 1}"#
    );
    assert_eq!(remove(r#"{"a": 1}"#, Dialect::Jsonc, &["a"]), r#"{}"#);
    assert_eq!(remove("[1, 2, 3]", Dialect::Jsonc, &["0"]), "[2, 3]");
    // Every occurrence of a repeated key goes; a missing one changes nothing.
    assert_eq!(
        remove(r#"{"a": 1, "b": 2, "a": 3}"#, Dialect::Jsonc, &["a"]),
        r#"{"b": 2}"#
    );
    assert_eq!(
        remove(TSCONFIG, Dialect::Jsonc, &["missing", "x"]),
        TSCONFIG
    );
    assert_eq!(remove("[1]", Dialect::Jsonc, &["4"]), "[1]");
}

#[test]
fn edit_sets_the_last_of_a_repeated_key_and_the_root() {
    assert_eq!(
        set(r#"{"a": 1, "a": 2}"#, Dialect::Jsonc, &["a"], "3"),
        r#"{"a": 1, "a": 3}"#
    );
    assert_eq!(
        set("// c\n[1]\n", Dialect::Jsonc, &[], "true"),
        "// c\ntrue\n"
    );
}

#[test]
fn edit_writes_json5_keys_as_the_file_does() {
    let text = "{\n  // JSON5\n  name: 'app',\n  list: ['a',],\n}\n";
    let p = plan(text, Dialect::Json5, &path(&["b"])).unwrap();
    assert_eq!((p.quote, p.trailing_commas), (Some('\''), true));
    assert_eq!(
        set(text, Dialect::Json5, &["version"], "'1.0'"),
        "{\n  // JSON5\n  name: 'app',\n  list: ['a',],\n  version: '1.0',\n}\n"
    );
    assert_eq!(
        set(text, Dialect::Json5, &["kebab-key"], "1"),
        "{\n  // JSON5\n  name: 'app',\n  list: ['a',],\n  'kebab-key': 1,\n}\n"
    );
    let quoted = "{\"a\": 1}";
    assert_eq!(
        set(quoted, Dialect::Json5, &["b"], "2"),
        "{\"a\": 1, \"b\": 2}"
    );
    // A JSONC plan names no quote: its strings are always double-quoted.
    assert_eq!(
        plan(
            text.replace('\'', "\"")
                .replace("name", "\"name\"")
                .replace("list", "\"list\"")
                .as_str(),
            Dialect::Jsonc,
            &[]
        )
        .unwrap()
        .quote,
        None
    );
}
