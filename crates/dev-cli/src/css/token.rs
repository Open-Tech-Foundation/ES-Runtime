//! Tokenizing CSS, to the depth the rest of the pipeline needs and no further.
//!
//! This is the layer everything above is built on, so what it guarantees is
//! worth stating exactly: **every byte of the input belongs to exactly one
//! token, and the tokens are in order.** Concatenating the spans reproduces the
//! input. That is what lets the passes above splice — they replace the spans
//! they understand and copy the rest through untouched, so a construct nothing
//! here has a name for still survives the build byte for byte.
//!
//! # Why the token set is small
//!
//! A CSS *parser* would build rules, selectors, declarations and values.
//! Nothing above needs them: resolving `@import` means finding a URL and a
//! statement's end, rewriting `url()` means finding a URL, and minifying means
//! knowing which whitespace is inside a string. All three are answered by
//! [`Kind`] as it stands.
//!
//! The cost of a bigger token set is not the code, it is the *risk*: every
//! construct given a name is a construct that can be misread and re-emitted
//! wrongly. Names are added here when a pass above needs one.
//!
//! # What it follows, and where it stops
//!
//! [CSS Syntax Level 3 §4][spec] — the tokenizer's escapes, string rules and
//! the `url(` special case, which are the three places a naive scan goes wrong:
//!
//! * A `\` escapes the next character **anywhere**, including inside a string
//!   and inside an unquoted `url()`. `content: "\""` is one string.
//! * `url(` is a token, not a function call, when what follows is unquoted, and
//!   it ends at the first `)`. A `(` inside an unquoted url is a parse error in
//!   the spec (it makes a bad-url-token), so a filename containing one has to
//!   be quoted — `url("a(1).png")`, which is an ordinary function and a string.
//! * A newline inside a quoted string ends it as a **bad string**. The spec
//!   recovers there; so does this, by ending the token at the newline.
//!
//! Deliberately *not* implemented: numeric, dimension, percentage, hash,
//! at-keyword-vs-ident distinctions beyond the leading `@`, and unicode-range.
//! Every one of them would land in [`Kind::Other`] today, which is exactly
//! where they belong until something needs to tell them apart.
//!
//! [spec]: https://www.w3.org/TR/css-syntax-3/#tokenization

/// What a token is, to the depth described above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A run of whitespace. One token however long, because the only question
    /// asked of it is whether it can be shortened.
    Whitespace,
    /// `/* … */`, including the delimiters. An unterminated one runs to the end
    /// of the input, which is what browsers do.
    Comment,
    /// A quoted string, including the quotes.
    String,
    /// An **unquoted** `url(…)`, including `url(` and `)`. See
    /// [`Token::url_body`] for the part inside.
    Url,
    /// An identifier followed directly by `(` — `rgb(`, `var(`, `supports(`.
    /// The `url(` of a *quoted* url is one of these.
    Function,
    /// `@` followed by an identifier: `@import`, `@media`, `@layer`.
    AtKeyword,
    /// A bare identifier.
    Ident,
    /// `{`
    OpenBrace,
    /// `}`
    CloseBrace,
    /// `(`
    OpenParen,
    /// `)`
    CloseParen,
    /// `;`
    Semicolon,
    /// Anything else, one character at a time: `:`, `,`, numbers, `#`, `>`.
    Other,
}

/// One token, as a kind and the span of input it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    /// Byte offset of the first character.
    pub start: usize,
    /// Byte offset one past the last character.
    pub end: usize,
}

impl Token {
    /// The token's text.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    /// The bytes inside a [`Kind::Url`] token's parentheses, trimmed.
    ///
    /// Returns the span as well as the text, because a caller rewriting the URL
    /// has to replace *that* range and leave `url(` and `)` where they are.
    pub fn url_body(&self, source: &str) -> Option<(usize, usize)> {
        if self.kind != Kind::Url {
            return None;
        }
        let open = source[self.start..self.end].find('(')? + self.start + 1;
        // The closing paren, unless the token was unterminated at end of input.
        let close = if source[..self.end].ends_with(')') {
            self.end - 1
        } else {
            self.end
        };
        let inner = &source[open..close];
        let lead = inner.len() - inner.trim_start().len();
        let trail = inner.len() - inner.trim_end().len();
        Some((open + lead, close - trail))
    }
}

/// Splits `source` into tokens.
///
/// Never fails. CSS has no parse error that stops a browser — unknown syntax is
/// skipped and the rest of the sheet still applies — so a tokenizer that
/// refused input would be stricter than the thing it exists to feed.
pub fn tokenize(source: &str) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut at = 0usize;

    while at < bytes.len() {
        let start = at;
        let kind = match bytes[at] {
            b if is_space(b) => {
                while at < bytes.len() && is_space(bytes[at]) {
                    at += 1;
                }
                Kind::Whitespace
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at += 2;
                while at < bytes.len() {
                    if bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/') {
                        at += 2;
                        break;
                    }
                    at += 1;
                }
                Kind::Comment
            }
            quote @ (b'"' | b'\'') => {
                at += 1;
                while at < bytes.len() {
                    match bytes[at] {
                        b'\\' => at = (at + 2).min(bytes.len()),
                        b if b == quote => {
                            at += 1;
                            break;
                        }
                        // A bad string. The spec stops here and so does this;
                        // re-emitting the span unchanged keeps the damage to
                        // whatever the author already had.
                        b'\n' | b'\r' | b'\x0c' => break,
                        _ => at += 1,
                    }
                }
                Kind::String
            }
            b'@' if is_ident_start(bytes.get(at + 1).copied()) => {
                at += 1;
                at = scan_ident(bytes, at);
                Kind::AtKeyword
            }
            b if is_ident_start(Some(b)) => {
                let after = scan_ident(bytes, at);
                if bytes.get(after) == Some(&b'(') {
                    let name = &source[at..after];
                    at = after + 1;
                    // `url(` is a token rather than a function *only* when what
                    // follows is unquoted — the one place where a `(` inside is
                    // data and not nesting.
                    if name.eq_ignore_ascii_case("url") && !starts_quoted(bytes, at) {
                        while at < bytes.len() {
                            match bytes[at] {
                                b'\\' => at = (at + 2).min(bytes.len()),
                                b')' => {
                                    at += 1;
                                    break;
                                }
                                _ => at += 1,
                            }
                        }
                        Kind::Url
                    } else {
                        Kind::Function
                    }
                } else {
                    at = after;
                    Kind::Ident
                }
            }
            b'{' => {
                at += 1;
                Kind::OpenBrace
            }
            b'}' => {
                at += 1;
                Kind::CloseBrace
            }
            b'(' => {
                at += 1;
                Kind::OpenParen
            }
            b')' => {
                at += 1;
                Kind::CloseParen
            }
            b';' => {
                at += 1;
                Kind::Semicolon
            }
            b'\\' => {
                // An escape at the start of an identifier — `\31 23` is a valid
                // class name. Scanning it as an identifier keeps the whole of it
                // in one token.
                let after = scan_ident(bytes, at);
                at = if after > at { after } else { at + 1 };
                Kind::Ident
            }
            _ => {
                // One *character*, not one byte: a multi-byte character split
                // across tokens would put a span boundary inside it, and every
                // `&source[..]` above would panic.
                at += char_width(bytes[at]);
                Kind::Other
            }
        };
        tokens.push(Token {
            kind,
            start,
            end: at,
        });
    }

    tokens
}

/// Whether the bytes at `at` begin a quoted string, ignoring whitespace.
///
/// `url( "a.png" )` is a function and a string; `url( a.png )` is a URL token.
/// The whitespace between is why this cannot just look at one byte.
fn starts_quoted(bytes: &[u8], mut at: usize) -> bool {
    while at < bytes.len() && is_space(bytes[at]) {
        at += 1;
    }
    matches!(bytes.get(at), Some(b'"' | b'\''))
}

/// Scans an identifier starting at `at`, returning where it ends.
fn scan_ident(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at = (at + 2).min(bytes.len()),
            b if is_ident_part(b) => at += 1,
            _ => break,
        }
    }
    at
}

/// CSS whitespace, which is not Rust's: a form feed counts and a vertical tab
/// does not.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

fn is_ident_start(b: Option<u8>) -> bool {
    // Every byte of a multi-byte character is >= 0x80 and every one of those is
    // an identifier character in CSS, so a leading byte needs no decoding.
    matches!(b, Some(b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'\\') | Some(0x80..))
}

fn is_ident_part(b: u8) -> bool {
    matches!(b, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | 0x80..)
}

/// How many bytes the UTF-8 character starting with `b` occupies.
fn char_width(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        // A continuation byte on its own cannot start a character. Only
        // reachable if the input was not the `&str` its type claims; advancing
        // one byte at least terminates.
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<Kind> {
        tokenize(source).into_iter().map(|t| t.kind).collect()
    }

    /// The guarantee everything above depends on. Asserted over the awkward
    /// cases rather than a tidy one, because a tidy one cannot fail it.
    #[test]
    fn the_tokens_reproduce_the_input() {
        for source in [
            "body{color:red}",
            "@import \"a.css\" screen and (min-width: 40em);",
            "a{background:url( ./b.png )}/* trailing",
            "a::before{content:\"\\\"}\"}",
            "@media (min-width:0){a{b:c}}",
            "/* only a comment */",
            "",
            "  \t\n  ",
            "a{content:'日本語'}",
            "url(unterminated",
            "\"unterminated",
        ] {
            let rebuilt: String = tokenize(source)
                .iter()
                .map(|t| t.text(source))
                .collect::<Vec<_>>()
                .concat();
            assert_eq!(rebuilt, source, "tokens lost bytes of {source:?}");
        }
    }

    /// The `url(` special case, which is the one a naive scanner gets wrong: a
    /// `)` ends it, but a `"` inside does not make it a string and a `\)` does
    /// not end it.
    #[test]
    fn an_unquoted_url_runs_to_its_closing_paren() {
        let source = "a{background:url(./a-b_c.png)}";
        let url = tokenize(source)
            .into_iter()
            .find(|t| t.kind == Kind::Url)
            .expect("a url token");
        assert_eq!(url.text(source), "url(./a-b_c.png)");
        let (start, end) = url.url_body(source).expect("a body");
        assert_eq!(&source[start..end], "./a-b_c.png");

        // An escaped `)` is part of the URL, which is the only way to write one.
        let escaped = "a{background:url(./a\\).png)}";
        let url = tokenize(escaped)
            .into_iter()
            .find(|t| t.kind == Kind::Url)
            .expect("a url token");
        assert_eq!(url.text(escaped), "url(./a\\).png)");
    }

    /// A quoted one is an ordinary function and a string, so the string's own
    /// rules apply to what is inside it.
    #[test]
    fn a_quoted_url_is_a_function_and_a_string() {
        let source = "a{background:url(\"./a).png\")}";
        let kinds = kinds(source);
        assert!(kinds.contains(&Kind::Function), "{kinds:?}");
        assert!(!kinds.contains(&Kind::Url), "{kinds:?}");
        let string = tokenize(source)
            .into_iter()
            .find(|t| t.kind == Kind::String)
            .expect("a string token");
        assert_eq!(string.text(source), "\"./a).png\"");
    }

    /// Whitespace inside `url()` is not part of the URL, and is not part of the
    /// token boundary either.
    #[test]
    fn a_urls_body_is_trimmed_without_moving_the_token() {
        let source = "a{background:url(  ./b.png  )}";
        let url = tokenize(source)
            .into_iter()
            .find(|t| t.kind == Kind::Url)
            .expect("a url token");
        let (start, end) = url.url_body(source).expect("a body");
        assert_eq!(&source[start..end], "./b.png");
    }

    /// An escaped quote does not end the string, which is what stops the rest
    /// of a stylesheet being read as if it were inside one.
    #[test]
    fn an_escape_does_not_end_a_string() {
        let source = r#"a{content:"a\"b"}"#;
        let string = tokenize(source)
            .into_iter()
            .find(|t| t.kind == Kind::String)
            .expect("a string token");
        assert_eq!(string.text(source), r#""a\"b""#);
    }

    /// The spec's recovery: a newline ends a string as a bad one rather than
    /// running to the end of the file and swallowing the sheet.
    #[test]
    fn a_newline_ends_a_bad_string() {
        let source = "a{content:\"oops\nb{color:red}";
        let string = tokenize(source)
            .into_iter()
            .find(|t| t.kind == Kind::String)
            .expect("a string token");
        assert_eq!(string.text(source), "\"oops");
        // …and the rest is still tokenized as CSS.
        assert!(kinds(source).contains(&Kind::CloseBrace));
    }

    #[test]
    fn an_at_keyword_is_recognised_and_a_bare_at_is_not() {
        assert_eq!(kinds("@import")[0], Kind::AtKeyword);
        assert_eq!(kinds("@")[0], Kind::Other);
    }

    /// A comment inside a string is text, and a string inside a comment is not
    /// a string. Both directions, because getting one right proves nothing.
    #[test]
    fn comments_and_strings_do_not_see_into_each_other() {
        let quoted = "a{content:\"/* not a comment */\"}";
        assert!(!kinds(quoted).contains(&Kind::Comment), "{quoted}");

        let commented = "/* \" not a string */a{b:c}";
        assert!(!kinds(commented).contains(&Kind::String), "{commented}");
    }

    /// The tiling invariant again, over inputs nobody would write.
    ///
    /// A real fuzz target belongs here and cannot go here yet: `dev-cli` is a
    /// binary crate, and `cargo fuzz` needs a library to link. This is the
    /// cheap approximation — every short string over an alphabet of the
    /// characters that drive the tokenizer's state machine, which reaches the
    /// unterminated and interleaved cases exhaustively rather than by luck.
    #[test]
    fn the_tokens_tile_every_short_input_over_the_awkward_alphabet() {
        let alphabet: Vec<char> = r#""'\/*(){};@ ab"#.chars().collect();
        let mut buffer = String::new();

        // Every string of length 1..=3. Exhaustive at this length, and the
        // tokenizer has no state that needs more to reach.
        for len in 1..=3usize {
            let mut indices = vec![0usize; len];
            loop {
                buffer.clear();
                buffer.extend(indices.iter().map(|&i| alphabet[i]));

                let rebuilt: String = tokenize(&buffer)
                    .iter()
                    .map(|t| t.text(&buffer))
                    .collect::<Vec<_>>()
                    .concat();
                assert_eq!(rebuilt, buffer, "tokens lost bytes of {buffer:?}");

                // Odometer over the alphabet.
                let mut place = len;
                loop {
                    if place == 0 {
                        break;
                    }
                    place -= 1;
                    indices[place] += 1;
                    if indices[place] < alphabet.len() {
                        break;
                    }
                    indices[place] = 0;
                    if place == 0 {
                        break;
                    }
                }
                if indices.iter().all(|&i| i == 0) {
                    break;
                }
            }
        }
    }

    /// Multi-byte characters are legal in identifiers, in strings and as stray
    /// text. A span boundary inside one would panic every slice above.
    #[test]
    fn a_multibyte_character_is_never_split() {
        for source in ["a{content:'日本語'}", ".日本 { color: red }", "→ ≠ ∅"] {
            // Slicing every span is the assertion: a boundary inside a
            // character panics here rather than in a caller.
            for token in tokenize(source) {
                let _ = token.text(source);
            }
        }
    }
}
