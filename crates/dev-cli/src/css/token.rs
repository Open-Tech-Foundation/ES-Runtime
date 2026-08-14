//! [CSS Syntax Level 3 §4][spec] — turning text into tokens.
//!
//! The token set is the spec's, in full, because this is the bottom of a
//! pipeline and every layer above inherits whatever it cannot express. A
//! tokenizer that lumps numbers in with punctuation cannot be built on by a
//! value minifier later without being rewritten first.
//!
//! # Every token carries its own text
//!
//! [`Token::text`] is the **verbatim source** of the token, not a normalised
//! form. That is what makes the tree above this lossless: printing is
//! concatenation, so a stylesheet survives a parse-and-print byte for byte, and
//! a pass that changes nothing changes nothing. `1.50em`, `\30 a` and
//! `URL(x.png)` all come back exactly as written.
//!
//! Semantic values are derived on demand instead ([`Token::unescape`],
//! [`Token::url`]), so nothing is lost by deriving them wrongly.
//!
//! # The three places a naive scan goes wrong
//!
//! * A `\` escapes the next character **anywhere** — inside a string, inside an
//!   identifier, inside an unquoted `url()`. `content: "\""` is one string.
//! * `url(` is a token rather than a function **only** when what follows is
//!   unquoted, and it ends at the first unescaped `)`. A `(` inside makes it a
//!   [`Kind::BadUrl`] per §4.3.6, so a filename with one must be quoted —
//!   `url("a(1).png")`, which is an ordinary function and a string.
//! * A newline inside a quoted string ends it as a [`Kind::BadString`]. The
//!   spec recovers there rather than swallowing the rest of the sheet.
//!
//! [spec]: https://www.w3.org/TR/css-syntax-3/#tokenization

/// A token kind, as [CSS Syntax Level 3 §4][spec] names them.
///
/// `Comment` is the one addition: the spec discards comments during
/// tokenization, and a tool that has to write the file back out cannot.
///
/// [spec]: https://www.w3.org/TR/css-syntax-3/#tokenization
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A run of whitespace, however long.
    Whitespace,
    /// `/* … */`. Unterminated, it runs to the end of the input.
    Comment,

    /// An identifier: `red`, `--custom`, `\31 23`.
    Ident,
    /// An identifier followed directly by `(`: `rgb(`, `var(`.
    Function,
    /// `@` and an identifier: `@media`.
    AtKeyword,
    /// `#` and a name: `#fff`, `#main`.
    Hash,
    /// A quoted string, including its quotes.
    String,
    /// A string ended by a newline (§4.3.4).
    BadString,
    /// An **unquoted** `url(…)`, including `url(` and `)`.
    Url,
    /// A `url(` whose contents cannot be a URL (§4.3.6).
    BadUrl,

    /// A number: `1`, `-2.5`, `+3e10`.
    Number,
    /// A number followed by `%`.
    Percentage,
    /// A number followed by an identifier: `10px`, `2fr`, `1e3ms`.
    Dimension,

    /// `:`
    Colon,
    /// `;`
    Semicolon,
    /// `,`
    Comma,
    /// `(`
    OpenParen,
    /// `)`
    CloseParen,
    /// `[`
    OpenSquare,
    /// `]`
    CloseSquare,
    /// `{`
    OpenCurly,
    /// `}`
    CloseCurly,

    /// `<!--`, which is legal at the top level of a stylesheet.
    Cdo,
    /// `-->`, likewise.
    Cdc,

    /// A single character that is none of the above: `+`, `>`, `*`, `/`, `&`.
    Delim,
}

impl Kind {
    /// Whether this is whitespace or a comment — the tokens that carry no
    /// meaning and are dropped when minifying.
    pub fn is_trivia(self) -> bool {
        matches!(self, Kind::Whitespace | Kind::Comment)
    }
}

/// One token: what it is, and the exact text it was written as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// The verbatim source. Printing a tree is concatenating these.
    pub text: String,
}

impl Token {
    pub fn new(kind: Kind, text: impl Into<String>) -> Self {
        Token {
            kind,
            text: text.into(),
        }
    }

    /// Whether this is whitespace or a comment.
    pub fn is_trivia(&self) -> bool {
        self.kind.is_trivia()
    }

    /// An identifier or at-keyword's name, lowercased and unescaped.
    ///
    /// CSS keywords are case-insensitive, so `@MEDIA` and `@media` are the same
    /// at-rule and anything matching on a name has to agree with that.
    pub fn name(&self) -> String {
        let text = match self.kind {
            Kind::AtKeyword => self.text.strip_prefix('@').unwrap_or(&self.text),
            Kind::Function => self.text.strip_suffix('(').unwrap_or(&self.text),
            _ => &self.text,
        };
        unescape(text).to_lowercase()
    }

    /// A string token's value, without its quotes and with escapes resolved.
    pub fn unescape(&self) -> String {
        let text = match self.kind {
            Kind::String | Kind::BadString => self
                .text
                .strip_prefix(['"', '\''])
                .map(|s| s.strip_suffix(['"', '\'']).unwrap_or(s))
                .unwrap_or(&self.text),
            _ => &self.text,
        };
        unescape(text)
    }

    /// An unquoted `url()` token's URL, trimmed and unescaped.
    pub fn url(&self) -> Option<String> {
        if self.kind != Kind::Url {
            return None;
        }
        let inner = self.text.strip_prefix(['u', 'U'])?;
        let inner = inner.get(3..)?; // past `rl(`
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        Some(unescape(inner.trim()))
    }
}

/// Resolves CSS escapes: `\26` (hex, optionally followed by one space) and
/// `\x` (any other character, taken literally).
fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(&next) = chars.peek() else {
            // A trailing backslash is a literal one (§4.3.7).
            out.push('\\');
            break;
        };
        if !next.is_ascii_hexdigit() {
            out.push(next);
            chars.next();
            continue;
        }
        // Up to six hex digits, then at most one whitespace character which is
        // part of the escape rather than of the text after it.
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
            hex.push(chars.next().expect("peeked"));
        }
        if chars
            .peek()
            .is_some_and(|&c| is_space(c as u32 as u8) && c.is_ascii())
        {
            chars.next();
        }
        match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
            // NUL and surrogates become U+FFFD (§4.3.7).
            Some('\0') | None => out.push('\u{fffd}'),
            Some(c) => out.push(c),
        }
    }
    out
}

/// Splits `source` into tokens.
///
/// Never fails. CSS has no parse error that stops a browser — unknown syntax is
/// skipped and the rest of the sheet still applies — so a tokenizer that
/// refused input would be stricter than the thing it exists to feed.
///
/// The tokens **tile the input**: concatenating every `text` in order
/// reproduces `source` exactly. Everything above depends on that.
pub fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0usize;

    while at < bytes.len() {
        let start = at;
        let kind = scan(bytes, source, &mut at);
        tokens.push(Token::new(kind, &source[start..at]));
    }
    tokens
}

/// Scans one token starting at `*at`, advancing it past the token.
fn scan(bytes: &[u8], source: &str, at: &mut usize) -> Kind {
    match bytes[*at] {
        b if is_space(b) => {
            while *at < bytes.len() && is_space(bytes[*at]) {
                *at += 1;
            }
            Kind::Whitespace
        }

        b'/' if bytes.get(*at + 1) == Some(&b'*') => {
            *at += 2;
            while *at < bytes.len() {
                if bytes[*at] == b'*' && bytes.get(*at + 1) == Some(&b'/') {
                    *at += 2;
                    return Kind::Comment;
                }
                *at += 1;
            }
            Kind::Comment
        }

        quote @ (b'"' | b'\'') => {
            *at += 1;
            while *at < bytes.len() {
                match bytes[*at] {
                    b'\\' => *at = (*at + 2).min(bytes.len()),
                    b if b == quote => {
                        *at += 1;
                        return Kind::String;
                    }
                    b'\n' | b'\r' | b'\x0c' => return Kind::BadString,
                    _ => *at += 1,
                }
            }
            Kind::String
        }

        b'#' => {
            *at += 1;
            let after = scan_name(bytes, *at);
            if after > *at {
                *at = after;
                Kind::Hash
            } else {
                Kind::Delim
            }
        }

        b'@' if starts_ident(bytes, *at + 1) => {
            *at += 1;
            *at = scan_name(bytes, *at);
            Kind::AtKeyword
        }

        b'<' if bytes[*at..].starts_with(b"<!--") => {
            *at += 4;
            Kind::Cdo
        }
        b'-' if bytes[*at..].starts_with(b"-->") => {
            *at += 3;
            Kind::Cdc
        }

        // A number, or a sign/dot that begins one.
        b'0'..=b'9' => scan_numeric(bytes, at),
        b'+' | b'-' | b'.' if starts_number(bytes, *at) => scan_numeric(bytes, at),

        // An identifier, which may turn out to be a function or a url.
        b if starts_ident_byte(b) || (b == b'-' && starts_ident(bytes, *at)) => {
            let after = scan_name(bytes, *at);
            if bytes.get(after) != Some(&b'(') {
                *at = after;
                return Kind::Ident;
            }
            let name = &source[*at..after];
            *at = after + 1;
            if !name.eq_ignore_ascii_case("url") || starts_quoted(bytes, *at) {
                return Kind::Function;
            }
            // An unquoted url(…): everything to the first unescaped `)`.
            let mut bad = false;
            while *at < bytes.len() {
                match bytes[*at] {
                    b'\\' => *at = (*at + 2).min(bytes.len()),
                    b')' => {
                        *at += 1;
                        return if bad { Kind::BadUrl } else { Kind::Url };
                    }
                    // §4.3.6 makes these a bad-url-token.
                    b'"' | b'\'' | b'(' => {
                        bad = true;
                        *at += 1;
                    }
                    _ => *at += 1,
                }
            }
            if bad { Kind::BadUrl } else { Kind::Url }
        }

        b'\\' if starts_ident(bytes, *at) => {
            *at = scan_name(bytes, *at);
            Kind::Ident
        }

        b':' => single(at, Kind::Colon),
        b';' => single(at, Kind::Semicolon),
        b',' => single(at, Kind::Comma),
        b'(' => single(at, Kind::OpenParen),
        b')' => single(at, Kind::CloseParen),
        b'[' => single(at, Kind::OpenSquare),
        b']' => single(at, Kind::CloseSquare),
        b'{' => single(at, Kind::OpenCurly),
        b'}' => single(at, Kind::CloseCurly),

        b => {
            // One *character*, not one byte: a span boundary inside a multi-byte
            // character would make every `&source[..]` above panic.
            *at += char_width(b);
            Kind::Delim
        }
    }
}

fn single(at: &mut usize, kind: Kind) -> Kind {
    *at += 1;
    kind
}

/// Scans a number and whatever follows it: `%` makes a percentage, an
/// identifier makes a dimension.
fn scan_numeric(bytes: &[u8], at: &mut usize) -> Kind {
    if matches!(bytes.get(*at), Some(b'+' | b'-')) {
        *at += 1;
    }
    while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
        *at += 1;
    }
    if bytes.get(*at) == Some(&b'.') && bytes.get(*at + 1).is_some_and(u8::is_ascii_digit) {
        *at += 1;
        while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
    }
    // An exponent, but only if it really is one: `1e3` is a number and `1em` is
    // a dimension, and they differ by a single character.
    if matches!(bytes.get(*at), Some(b'e' | b'E')) {
        let mut ahead = *at + 1;
        if matches!(bytes.get(ahead), Some(b'+' | b'-')) {
            ahead += 1;
        }
        if bytes.get(ahead).is_some_and(u8::is_ascii_digit) {
            *at = ahead;
            while bytes.get(*at).is_some_and(u8::is_ascii_digit) {
                *at += 1;
            }
        }
    }

    if bytes.get(*at) == Some(&b'%') {
        *at += 1;
        return Kind::Percentage;
    }
    if starts_ident(bytes, *at) {
        *at = scan_name(bytes, *at);
        return Kind::Dimension;
    }
    Kind::Number
}

/// Whether a number starts at `at` — the `+`, `-` and `.` cases, which
/// otherwise tokenize as delimiters.
fn starts_number(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at) {
        Some(b'+' | b'-') => match bytes.get(at + 1) {
            Some(b) if b.is_ascii_digit() => true,
            Some(b'.') => bytes.get(at + 2).is_some_and(u8::is_ascii_digit),
            _ => false,
        },
        Some(b'.') => bytes.get(at + 1).is_some_and(u8::is_ascii_digit),
        Some(b) => b.is_ascii_digit(),
        None => false,
    }
}

/// Whether an identifier starts at `at`, including the `-`, `--` and escape
/// forms.
fn starts_ident(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at) {
        Some(&b'-') => match bytes.get(at + 1) {
            Some(&b'-') => true,
            Some(&b'\\') => true,
            Some(&b) => starts_ident_byte(b),
            None => false,
        },
        Some(&b'\\') => !matches!(bytes.get(at + 1), Some(b'\n' | b'\r' | b'\x0c') | None),
        Some(&b) => starts_ident_byte(b),
        None => false,
    }
}

/// Every byte of a multi-byte character is >= 0x80 and every one of those is an
/// identifier character in CSS, so a leading byte needs no decoding.
fn starts_ident_byte(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80..)
}

/// Scans a name (identifier body), returning where it ends.
fn scan_name(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at = scan_escape(bytes, at),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | 0x80.. => at += 1,
            _ => break,
        }
    }
    at
}

/// Scans one escape sequence starting at the `\`, returning where it ends.
///
/// A hex escape swallows **one** following whitespace character, and that is
/// not a detail: `\49 mport` is the six-character identifier `Import`, and a
/// scanner that stopped at the space would read it as `\49` followed by a
/// separate `mport` — which is how an obfuscated `@\49 mport` slips past a
/// check that only matches the plain spelling.
fn scan_escape(bytes: &[u8], at: usize) -> usize {
    let mut end = at + 1;
    if !bytes.get(end).is_some_and(u8::is_ascii_hexdigit) {
        // `\x` — the next character, whatever it is.
        return (at + 2).min(bytes.len());
    }
    let limit = end + 6;
    while end < limit && bytes.get(end).is_some_and(u8::is_ascii_hexdigit) {
        end += 1;
    }
    // `\r\n` counts as the one whitespace character.
    if bytes.get(end) == Some(&b'\r') && bytes.get(end + 1) == Some(&b'\n') {
        end += 2;
    } else if bytes.get(end).is_some_and(|&b| is_space(b)) {
        end += 1;
    }
    end
}

/// Whether the bytes at `at` begin a quoted string, ignoring whitespace.
///
/// `url( "a.png" )` is a function and a string; `url( a.png )` is a URL token.
fn starts_quoted(bytes: &[u8], mut at: usize) -> bool {
    while at < bytes.len() && is_space(bytes[at]) {
        at += 1;
    }
    matches!(bytes.get(at), Some(b'"' | b'\''))
}

/// CSS whitespace, which is not Rust's: a form feed counts, a vertical tab does
/// not.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// How many bytes the UTF-8 character starting with `b` occupies.
fn char_width(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        // A continuation byte cannot start a character; only reachable if the
        // input was not the `&str` its type claims. Advancing terminates.
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Kind> {
        tokenize(source).into_iter().map(|t| t.kind).collect()
    }

    /// The guarantee everything above depends on.
    #[test]
    fn the_tokens_tile_the_input() {
        for source in [
            "body{color:red}",
            "@import \"a.css\" screen and (min-width: 40em);",
            "a{background:url( ./b.png )}/* trailing",
            "a::before{content:\"\\\"}\"}",
            "a{margin:-1.5e3px 0 +.5% 10px}",
            "#fff #main .x[data-y='z'] > * ~ a:hover",
            "<!-- a{b:c} -->",
            "a{content:'日本語'}",
            "url(unterminated",
            "\"unterminated",
            "",
            "   \t\n  ",
        ] {
            let rebuilt: String = tokenize(source).iter().map(|t| t.text.as_str()).collect();
            assert_eq!(rebuilt, source, "tokens lost bytes of {source:?}");
        }
    }

    /// Exhaustive over the alphabet that drives the state machine. A real fuzz
    /// target wants `dev-cli` split into lib and bin; this reaches the
    /// unterminated and interleaved cases by construction instead.
    #[test]
    fn the_tokens_tile_every_short_input_over_the_awkward_alphabet() {
        let alphabet: Vec<char> = r#""'\/*(){};:@#.-+1e %a<!>"#.chars().collect();
        for len in 1..=3usize {
            let mut indices = vec![0usize; len];
            loop {
                let buffer: String = indices.iter().map(|&i| alphabet[i]).collect();
                let rebuilt: String = tokenize(&buffer).iter().map(|t| t.text.as_str()).collect();
                assert_eq!(rebuilt, buffer, "tokens lost bytes of {buffer:?}");

                let mut place = len;
                let mut carried = true;
                while carried && place > 0 {
                    place -= 1;
                    indices[place] += 1;
                    carried = indices[place] == alphabet.len();
                    if carried {
                        indices[place] = 0;
                    }
                }
                if carried {
                    break;
                }
            }
        }
    }

    #[test]
    fn numbers_dimensions_and_percentages_are_told_apart() {
        assert_eq!(kinds("1")[0], Kind::Number);
        assert_eq!(kinds("-2.5")[0], Kind::Number);
        assert_eq!(kinds("+.5")[0], Kind::Number);
        assert_eq!(kinds("1e3")[0], Kind::Number);
        assert_eq!(kinds("1E-3")[0], Kind::Number);
        assert_eq!(kinds("50%")[0], Kind::Percentage);
        assert_eq!(kinds("10px")[0], Kind::Dimension);
        // The one that catches a lazy exponent scan: `1em` is a dimension.
        assert_eq!(kinds("1em")[0], Kind::Dimension);
        assert_eq!(kinds("1e3ms")[0], Kind::Dimension);
    }

    /// A `-` or `.` that does not begin a number is a delimiter, which is what
    /// makes `a-b` one identifier and `a - b` three tokens.
    #[test]
    fn a_sign_that_is_not_a_number_is_a_delimiter() {
        assert_eq!(kinds("- 1")[0], Kind::Delim);
        assert_eq!(kinds(".x")[0], Kind::Delim);
        assert_eq!(kinds("--custom")[0], Kind::Ident);
        assert_eq!(kinds("-webkit-box")[0], Kind::Ident);
    }

    #[test]
    fn an_unquoted_url_is_one_token_and_a_quoted_one_is_a_function() {
        let unquoted = tokenize("url(./a-b.png)");
        assert_eq!(unquoted[0].kind, Kind::Url);
        assert_eq!(unquoted[0].url().as_deref(), Some("./a-b.png"));

        let quoted = kinds("url(\"./a.png\")");
        assert_eq!(quoted[0], Kind::Function);
        assert_eq!(quoted[1], Kind::String);
    }

    /// §4.3.6: a `(` inside an unquoted url makes it a bad-url-token, so the
    /// filename has to be quoted.
    #[test]
    fn a_paren_inside_an_unquoted_url_makes_it_bad() {
        assert_eq!(kinds("url(./a(1).png)")[0], Kind::BadUrl);
    }

    #[test]
    fn a_newline_ends_a_bad_string_without_swallowing_the_sheet() {
        let tokens = tokenize("a{content:\"oops\nb{color:red}");
        let string = tokens
            .iter()
            .find(|t| matches!(t.kind, Kind::String | Kind::BadString))
            .expect("a string");
        assert_eq!(string.kind, Kind::BadString);
        assert_eq!(string.text, "\"oops");
        assert!(kinds("a{content:\"oops\nb{color:red}").contains(&Kind::CloseCurly));
    }

    #[test]
    fn comments_and_strings_do_not_see_into_each_other() {
        assert!(!kinds("a{content:\"/* x */\"}").contains(&Kind::Comment));
        assert!(!kinds("/* \" */a{b:c}").contains(&Kind::String));
    }

    /// Names are matched case-insensitively and with escapes resolved, because
    /// `@MEDIA` and `@media` are the same at-rule.
    #[test]
    fn a_name_is_lowercased_and_unescaped() {
        assert_eq!(tokenize("@MEDIA")[0].name(), "media");
        assert_eq!(tokenize("rgb(")[0].name(), "rgb");
        assert_eq!(tokenize(r"\49 mport")[0].name(), "import");
    }

    #[test]
    fn a_string_value_drops_its_quotes_and_resolves_escapes() {
        assert_eq!(tokenize(r#""a\"b""#)[0].unescape(), "a\"b");
        assert_eq!(tokenize(r#""\26 x""#)[0].unescape(), "&x");
        assert_eq!(tokenize(r#""\0""#)[0].unescape(), "\u{fffd}");
    }

    #[test]
    fn a_multibyte_character_is_never_split() {
        for source in ["a{content:'日本語'}", ".日本 { color: red }", "→ ≠ ∅"] {
            let rebuilt: String = tokenize(source).iter().map(|t| t.text.as_str()).collect();
            assert_eq!(rebuilt, source);
        }
    }
}
