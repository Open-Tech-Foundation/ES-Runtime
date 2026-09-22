//! A small, strict parser for the modern HTML accepted by the esdev test DOM.
//!
//! This is not an HTML5 error-recovery parser. Input is either conforming,
//! modern markup or an error with a byte offset.
//!
//! Conforming markup includes the omitted tags the HTML specification calls
//! optional: `</li>`, `</p>`, `</td>`, an implied `<tbody>` before a `<tr>`,
//! and the rest of the table, list, select and ruby rules. Those are not
//! errors being repaired — they are the language, and a browser, jsdom and
//! happy-dom all build the same tree from them, so a test fixture written that
//! way must parse the same here.
//!
//! What stays refused is genuinely broken markup: an unclosed `<i>`, misnested
//! formatting, a `<td>` outside a row, table content that would have to be
//! foster-parented. Those browser recovery behaviours are valuable to a
//! browser; silently changing a test fixture is not.

use std::fmt;

use es_runtime_cli_common::Value;

/// A parsed HTML document or fragment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Document {
    /// The optional HTML5 doctype. Only `<!doctype html>` is accepted.
    pub doctype: bool,
    pub children: Vec<Node>,
}

/// A node produced by the strict parser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    Text(String),
    Comment(String),
}

/// An element and its descendants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub attributes: Vec<Attribute>,
    pub children: Vec<Node>,
}

/// One attribute, kept in source order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub value: String,
}

/// A parse error whose position is a byte offset into the supplied UTF-8 text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub offset: usize,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            out,
            "HTML parse error at byte {}: {}",
            self.offset, self.message
        )
    }
}

impl std::error::Error for Error {}

/// Parses a complete document.
///
/// A document may contain one `<!doctype html>` before its root element. The
/// root itself is not synthesized: callers that want the test runner's empty
/// document construct it explicitly.
pub fn parse_document(source: &str) -> Result<Document, Error> {
    let mut parser = Parser::new(source);
    let document = parser.nodes(None, None, true, false)?;
    if !parser.eof() {
        return Err(parser.error("unexpected content after the document"));
    }
    Ok(document)
}

/// Parses nodes for an element's `innerHTML`.
///
/// Fragment parsing shares exactly the same strict grammar as documents but
/// never accepts a doctype.
#[cfg(test)]
pub fn parse_fragment(source: &str) -> Result<Vec<Node>, Error> {
    parse_fragment_in(source, None)
}

/// Fragment parsing with the context element the markup is being parsed into,
/// which is what decides whether a `<tr>` opens an implied `<tbody>`.
pub fn parse_fragment_in(source: &str, context: Option<&str>) -> Result<Vec<Node>, Error> {
    let mut parser = Parser::new(source);
    let document = parser.nodes(None, context, false, false)?;
    if !parser.eof() {
        return Err(parser.error("unexpected content after the fragment"));
    }
    Ok(document.children)
}

/// Encodes a strict fragment parse in the compact records the JS DOM decoder
/// consumes: `[kind, parentIndex, name, attributes, text]`.
///
/// Parent indices point into the same pre-order record array; `-1` denotes a
/// fragment root. Keeping this structural boundary free of V8 handles makes
/// the parser independently testable and the eventual synchronous op one
/// crossing regardless of fragment size.
pub fn fragment_records(source: &str, context: Option<&str>) -> Result<Value, Error> {
    let nodes = parse_fragment_in(source, context)?;
    let mut records = Vec::new();
    for node in &nodes {
        encode_node(node, -1, &mut records);
    }
    Ok(Value::Array(records))
}

/// Encodes a strict document parse as `[doctype, records]`, where `doctype` is
/// whether `<!doctype html>` was present. `DOMParser` needs both, and the
/// records are the same shape a fragment produces.
pub fn document_records(source: &str) -> Result<Value, Error> {
    let document = parse_document(source)?;
    let mut records = Vec::new();
    for node in &document.children {
        encode_node(node, -1, &mut records);
    }
    Ok(Value::Array(vec![
        Value::Bool(document.doctype),
        Value::Array(records),
    ]))
}

fn encode_node(node: &Node, parent: isize, records: &mut Vec<Value>) {
    let index = records.len() as isize;
    match node {
        Node::Element(element) => {
            records.push(Value::Array(vec![
                Value::Number(1.0),
                Value::Number(parent as f64),
                Value::String(element.name.clone()),
                Value::Array(
                    element
                        .attributes
                        .iter()
                        .map(|attribute| {
                            Value::Array(vec![
                                Value::String(attribute.name.clone()),
                                Value::String(attribute.value.clone()),
                            ])
                        })
                        .collect(),
                ),
                Value::String(String::new()),
            ]));
            for child in &element.children {
                encode_node(child, index, records);
            }
        }
        Node::Text(text) => records.push(Value::Array(vec![
            Value::Number(3.0),
            Value::Number(parent as f64),
            Value::String(String::new()),
            Value::Array(Vec::new()),
            Value::String(text.clone()),
        ])),
        Node::Comment(text) => records.push(Value::Array(vec![
            Value::Number(8.0),
            Value::Number(parent as f64),
            Value::String(String::new()),
            Value::Array(Vec::new()),
            Value::String(text.clone()),
        ])),
    }
}

struct Parser<'a> {
    source: &'a str,
    at: usize,
}

impl<'a> Parser<'a> {
    fn new(source: &'a str) -> Self {
        Self { source, at: 0 }
    }

    fn eof(&self) -> bool {
        self.at == self.source.len()
    }

    fn rest(&self) -> &'a str {
        &self.source[self.at..]
    }

    fn error(&self, message: impl Into<String>) -> Error {
        Error {
            offset: self.at,
            message: message.into(),
        }
    }

    /// `closing` is the end tag this level is waiting for, `parent` the element
    /// whose content model it is parsing. They are the same for an element's
    /// children and differ for a fragment, which has a context element but no
    /// end tag of its own.
    fn nodes(
        &mut self,
        closing: Option<&str>,
        parent: Option<&str>,
        document: bool,
        in_svg: bool,
    ) -> Result<Document, Error> {
        let mut out = Document::default();
        loop {
            if self.eof() {
                if let Some(name) = closing {
                    // An optional end tag may be omitted at the end of its
                    // parent's content, and a fragment ends the same way.
                    if has_optional_end_tag(name) {
                        return Ok(out);
                    }
                    return Err(self.error(format!("unclosed <{name}> element")));
                }
                return Ok(out);
            }
            if self.rest().starts_with("</") {
                let end_at = self.at;
                let name = self.end_tag(in_svg)?;
                let Some(expected) = closing else {
                    return Err(Error {
                        offset: end_at,
                        message: format!("unexpected closing </{name}> tag"),
                    });
                };
                if name != expected {
                    // The parent's end tag ends this element too, so hand the
                    // tag back and let the level above match it.
                    if has_optional_end_tag(expected) {
                        self.at = end_at;
                        return Ok(out);
                    }
                    return Err(Error {
                        offset: end_at,
                        message: format!("closing </{name}> tag does not match <{expected}>"),
                    });
                }
                return Ok(out);
            }
            if self.rest().starts_with("<!--") {
                out.children.push(Node::Comment(self.comment()?));
            } else if self.rest().starts_with("<?") {
                out.children.push(Node::Comment(self.bogus_comment()?));
            } else if self.rest().starts_with("<!") {
                if !document || out.doctype || !out.children.is_empty() {
                    return Err(self.error("doctype must appear once, before document content"));
                }
                self.doctype()?;
                out.doctype = true;
            } else if self.rest().starts_with('<') {
                let next = self.peek_start_tag_name(in_svg);
                if let (Some(open), Some(next)) = (closing, next.as_deref())
                    && has_optional_end_tag(open)
                    && implicitly_closed_by(open, next)
                {
                    // Unconsumed: the start tag belongs to the level above.
                    return Ok(out);
                }
                // A cell needs a row. `<tr>`'s start tag is not optional, so
                // this is not abbreviation — it is the foster-parenting case,
                // and naming it beats quietly building a different tree than
                // every browser builds.
                if matches!(parent, Some("table" | "thead" | "tbody" | "tfoot"))
                    && matches!(next.as_deref(), Some("td" | "th"))
                {
                    let cell = next.as_deref().unwrap_or("td");
                    return Err(self.error(format!("<{cell}> must be inside a <tr>")));
                }
                // `<tbody>`'s start tag is optional, so a row directly in a
                // table opens one, and it runs until something ends it.
                if parent == Some("table") && next.as_deref() == Some("tr") {
                    let children = self
                        .nodes(Some("tbody"), Some("tbody"), false, in_svg)?
                        .children;
                    out.children.push(Node::Element(Element {
                        name: "tbody".to_string(),
                        attributes: Vec::new(),
                        children,
                    }));
                    continue;
                }
                out.children.push(Node::Element(self.element(in_svg)?));
            } else {
                out.children.push(Node::Text(self.text()?));
            }
        }
    }

    fn element(&mut self, in_svg: bool) -> Result<Element, Error> {
        self.expect("<")?;
        let name = self.name("element", in_svg)?;
        let in_svg = in_svg || is_svg_root(&name);
        let mut attributes = Vec::new();
        loop {
            self.whitespace();
            if self.take("/>") {
                if !in_svg && !is_void(&name) {
                    return Err(self.error(format!("<{name}/> is not a void element")));
                }
                return Ok(Element {
                    name,
                    attributes,
                    children: Vec::new(),
                });
            }
            if self.take(">") {
                break;
            }
            if self.eof() {
                return Err(self.error("unterminated start tag"));
            }
            let attribute_at = self.at;
            let attribute_name = self.name("attribute", in_svg)?;
            let attribute_name = if in_svg {
                svg_attribute_name(&attribute_name).to_string()
            } else {
                attribute_name
            };
            if attributes
                .iter()
                .any(|attribute: &Attribute| attribute.name == attribute_name)
            {
                return Err(Error {
                    offset: attribute_at,
                    message: format!("duplicate {attribute_name} attribute"),
                });
            }
            self.whitespace();
            let value = if self.take("=") {
                self.whitespace();
                self.attribute_value()?
            } else {
                String::new()
            };
            attributes.push(Attribute {
                name: attribute_name,
                value,
            });
        }
        if !in_svg && is_void(&name) {
            return Ok(Element {
                name,
                attributes,
                children: Vec::new(),
            });
        }
        let children = if is_raw_text(&name) {
            vec![Node::Text(self.raw_text(&name)?)]
        } else {
            self.nodes(Some(&name), Some(&name), false, in_svg)?
                .children
        };
        Ok(Element {
            name,
            attributes,
            children,
        })
    }

    /// The name of the start tag at the cursor, without consuming anything. A
    /// comment, doctype or malformed tag answers `None` and is parsed as usual.
    fn peek_start_tag_name(&mut self, in_svg: bool) -> Option<String> {
        if self.rest().starts_with("<!") || self.rest().starts_with("<?") {
            return None;
        }
        let at = self.at;
        self.at += 1;
        let name = self.name("element", in_svg).ok();
        self.at = at;
        name
    }

    fn end_tag(&mut self, in_svg: bool) -> Result<String, Error> {
        self.expect("</")?;
        let name = self.name("end-tag", in_svg)?;
        self.whitespace();
        self.expect(">")?;
        if !in_svg && is_void(&name) {
            return Err(self.error(format!("void element <{name}> must not have an end tag")));
        }
        Ok(name)
    }

    fn comment(&mut self) -> Result<String, Error> {
        self.expect("<!--")?;
        let start = self.at;
        let Some(end) = self.rest().find("-->") else {
            return Err(self.error("unterminated comment"));
        };
        let value = &self.source[start..start + end];
        if value.contains("--") {
            return Err(Error {
                offset: start,
                message: "comments must not contain --".to_string(),
            });
        }
        self.at += end + 3;
        Ok(value.to_string())
    }

    /// HTML's bogus-comment token, used by template compilers for inert
    /// placeholders such as `<?lit$…>`. This is a comment, never an element.
    fn bogus_comment(&mut self) -> Result<String, Error> {
        self.expect("<")?;
        let start = self.at;
        let Some(end) = self.rest().find('>') else {
            return Err(self.error("unterminated bogus comment"));
        };
        let value = self.source[start..start + end].to_string();
        self.at += end + 1;
        Ok(value)
    }

    fn doctype(&mut self) -> Result<(), Error> {
        let at = self.at;
        let Some(end) = self.rest().find('>') else {
            return Err(self.error("unterminated doctype"));
        };
        let value = &self.source[self.at..self.at + end + 1];
        self.at += end + 1;
        if !value.eq_ignore_ascii_case("<!doctype html>") {
            return Err(Error {
                offset: at,
                message: "only <!doctype html> is supported".to_string(),
            });
        }
        Ok(())
    }

    fn raw_text(&mut self, name: &str) -> Result<String, Error> {
        let start = self.at;
        let needle = format!("</{name}");
        let mut at = self.at;
        while let Some(relative) = self.source[at..].find("</") {
            let candidate = at + relative;
            if self.source[candidate..]
                .get(..needle.len())
                .is_some_and(|text| text.eq_ignore_ascii_case(&needle))
            {
                let after = candidate + needle.len();
                if self.source[after..].starts_with('>')
                    || self.source[after..].starts_with(char::is_whitespace)
                {
                    let text = self.source[start..candidate].to_string();
                    self.at = candidate;
                    let end = self.end_tag(false)?;
                    if end != name {
                        return Err(
                            self.error(format!("closing </{end}> tag does not match <{name}>"))
                        );
                    }
                    return Ok(text);
                }
            }
            at = candidate + 2;
        }
        Err(Error {
            offset: start,
            message: format!("unclosed <{name}> element"),
        })
    }

    fn text(&mut self) -> Result<String, Error> {
        let start = self.at;
        let end = self
            .rest()
            .find('<')
            .map_or(self.source.len(), |offset| self.at + offset);
        self.at = end;
        decode_entities(&self.source[start..end], start)
    }

    fn attribute_value(&mut self) -> Result<String, Error> {
        let start = self.at;
        let quote = self
            .rest()
            .chars()
            .next()
            .filter(|character| matches!(character, '\'' | '"'));
        if let Some(quote) = quote {
            self.at += quote.len_utf8();
            let content = self.at;
            let Some(end) = self.rest().find(quote) else {
                return Err(Error {
                    offset: start,
                    message: "unterminated quoted attribute value".to_string(),
                });
            };
            self.at += end;
            let value = decode_entities(&self.source[content..self.at], content)?;
            self.at += quote.len_utf8();
            return Ok(value);
        }
        let end = self
            .rest()
            .find(|character: char| {
                character.is_ascii_whitespace() || matches!(character, '>' | '/')
            })
            .map_or(self.source.len(), |offset| self.at + offset);
        if end == self.at {
            return Err(self.error("attribute value is missing"));
        }
        self.at = end;
        decode_entities(&self.source[start..end], start)
    }

    fn name(&mut self, kind: &str, allow_svg_case: bool) -> Result<String, Error> {
        let start = self.at;
        let mut characters = self.rest().char_indices();
        let Some((_, first)) = characters.next() else {
            return Err(self.error(format!("{kind} name is missing")));
        };
        let template_marker = kind == "attribute" && matches!(first, '@' | '?' | '.' | '$');
        if !(first.is_ascii_lowercase()
            || allow_svg_case && first.is_ascii_uppercase()
            || template_marker)
        {
            return Err(self.error(format!(
                "{kind} names must start with a lowercase ASCII letter"
            )));
        }
        let mut end = first.len_utf8();
        for (index, character) in characters {
            if character.is_ascii_lowercase()
                || allow_svg_case && character.is_ascii_uppercase()
                || character.is_ascii_digit()
                // Template compilers use `$` in inert marker attributes. It
                // is an ordinary HTML attribute-name character, not recovery
                // syntax, so accepting it keeps strict nesting intact.
                || matches!(character, '-' | '_' | ':' | '$' | '@' | '?' | '.')
            {
                end = index + character.len_utf8();
            } else {
                break;
            }
        }
        self.at = start + end;
        Ok(self.source[start..self.at].to_string())
    }

    fn whitespace(&mut self) {
        self.at += self
            .rest()
            .chars()
            .take_while(|character| character.is_ascii_whitespace())
            .map(char::len_utf8)
            .sum::<usize>();
    }

    fn take(&mut self, expected: &str) -> bool {
        if self.rest().starts_with(expected) {
            self.at += expected.len();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, expected: &str) -> Result<(), Error> {
        self.take(expected)
            .then_some(())
            .ok_or_else(|| self.error(format!("expected {expected:?}")))
    }
}

/// Elements whose end tag the HTML specification allows to be omitted.
fn has_optional_end_tag(name: &str) -> bool {
    matches!(
        name,
        "li" | "dt"
            | "dd"
            | "p"
            | "option"
            | "optgroup"
            | "thead"
            | "tbody"
            | "tfoot"
            | "tr"
            | "td"
            | "th"
            | "caption"
            | "colgroup"
            | "rt"
            | "rp"
    )
}

/// The block-level start tags that end an open `<p>`.
fn closes_paragraph(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "details"
            | "div"
            | "dl"
            | "fieldset"
            | "figcaption"
            | "figure"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "search"
            | "section"
            | "table"
            | "ul"
    )
}

/// Whether an open element with an optional end tag is ended by the start tag
/// that comes next. The other half of the rule — a parent's end tag ending it —
/// is handled where end tags are read.
fn implicitly_closed_by(open: &str, next: &str) -> bool {
    match open {
        "li" => next == "li",
        "dt" | "dd" => matches!(next, "dt" | "dd"),
        "p" => closes_paragraph(next),
        "option" => matches!(next, "option" | "optgroup" | "hr"),
        "optgroup" => matches!(next, "optgroup" | "hr"),
        // A section cannot contain a section, so any of them ends an open one.
        "thead" | "tbody" | "tfoot" => matches!(next, "thead" | "tbody" | "tfoot"),
        "caption" | "colgroup" => matches!(next, "colgroup" | "thead" | "tbody" | "tfoot" | "tr"),
        "tr" => next == "tr",
        "td" | "th" => matches!(next, "td" | "th" | "tr"),
        "rt" | "rp" => matches!(next, "rt" | "rp"),
        _ => false,
    }
}

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_raw_text(name: &str) -> bool {
    matches!(name, "script" | "style")
}

fn is_svg_root(name: &str) -> bool {
    matches!(name, "svg" | "svg:svg")
}

fn svg_attribute_name(name: &str) -> &str {
    match name {
        "attributename" => "attributeName",
        "basefrequency" => "baseFrequency",
        "clippathunits" => "clipPathUnits",
        "gradienttransform" => "gradientTransform",
        "gradientunits" => "gradientUnits",
        "kernelmatrix" => "kernelMatrix",
        "kernelunitlength" => "kernelUnitLength",
        "lengthadjust" => "lengthAdjust",
        "markerheight" => "markerHeight",
        "markerunits" => "markerUnits",
        "markerwidth" => "markerWidth",
        "maskcontentunits" => "maskContentUnits",
        "maskunits" => "maskUnits",
        "numoctaves" => "numOctaves",
        "pathlength" => "pathLength",
        "patterncontentunits" => "patternContentUnits",
        "patterntransform" => "patternTransform",
        "patternunits" => "patternUnits",
        "preserveaspectratio" => "preserveAspectRatio",
        "primitiveunits" => "primitiveUnits",
        "refx" => "refX",
        "refy" => "refY",
        "specularconstant" => "specularConstant",
        "specularexponent" => "specularExponent",
        "spreadmethod" => "spreadMethod",
        "startoffset" => "startOffset",
        "stddeviation" => "stdDeviation",
        "surfacescale" => "surfaceScale",
        "systemlanguage" => "systemLanguage",
        "tablevalues" => "tableValues",
        "viewbox" => "viewBox",
        "viewtarget" => "viewTarget",
        "xchannelselector" => "xChannelSelector",
        "ychannelselector" => "yChannelSelector",
        _ => name,
    }
}

fn decode_entities(value: &str, offset: usize) -> Result<String, Error> {
    let mut output = String::with_capacity(value.len());
    let mut at = 0;
    while let Some(relative) = value[at..].find('&') {
        let entity_at = at + relative;
        output.push_str(&value[at..entity_at]);
        let tail = &value[entity_at + 1..];
        let Some(end) = tail.find(';') else {
            return Err(Error {
                offset: offset + entity_at,
                message: "unterminated character reference".to_string(),
            });
        };
        let name = &tail[..end];
        let character = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ if name.starts_with("#x") || name.starts_with("#X") => {
                u32::from_str_radix(&name[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
            }
            _ if name.starts_with('#') => name[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        }
        .ok_or_else(|| Error {
            offset: offset + entity_at,
            message: format!("unknown character reference &{name};"),
        })?;
        output.push(character);
        at = entity_at + end + 2;
    }
    output.push_str(&value[at..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_modern_document() {
        let document = parse_document("<!doctype html><main id=app><img alt=\"&lt;logo&gt;\"><p>Hello &amp; goodbye</p></main>").expect("parse");
        assert!(document.doctype);
        assert_eq!(
            document.children,
            vec![Node::Element(Element {
                name: "main".to_string(),
                attributes: vec![Attribute {
                    name: "id".to_string(),
                    value: "app".to_string()
                }],
                children: vec![
                    Node::Element(Element {
                        name: "img".to_string(),
                        attributes: vec![Attribute {
                            name: "alt".to_string(),
                            value: "<logo>".to_string()
                        }],
                        children: vec![]
                    }),
                    Node::Element(Element {
                        name: "p".to_string(),
                        attributes: vec![],
                        children: vec![Node::Text("Hello & goodbye".to_string())]
                    }),
                ],
            })]
        );
    }

    #[test]
    fn parses_template_and_raw_text_without_interpreting_markup() {
        let nodes = parse_fragment(
            "<template><button>Save</button></template><script>if (a < b) c()</script>",
        )
        .expect("parse");
        assert_eq!(nodes.len(), 2);
        let Node::Element(template) = &nodes[0] else {
            panic!("template")
        };
        assert_eq!(template.children.len(), 1);
        let Node::Element(script) = &nodes[1] else {
            panic!("script")
        };
        assert_eq!(
            script.children,
            vec![Node::Text("if (a < b) c()".to_string())]
        );
    }

    /// A compact rendering of a parsed tree, for cases that are about shape.
    fn render(nodes: &[Node]) -> String {
        nodes
            .iter()
            .map(|node| match node {
                Node::Text(text) => text.clone(),
                Node::Comment(text) => format!("<!--{text}-->"),
                Node::Element(element) => {
                    let attributes: String = element
                        .attributes
                        .iter()
                        .map(|attribute| format!(" {}=\"{}\"", attribute.name, attribute.value))
                        .collect();
                    format!(
                        "<{0}{1}>{2}</{0}>",
                        element.name,
                        attributes,
                        render(&element.children)
                    )
                }
            })
            .collect()
    }

    #[test]
    fn rejects_legacy_recovery_cases() {
        for source in [
            // Misnested formatting, which the adoption agency algorithm exists
            // to repair and this parser refuses to guess at.
            "<p><strong>x</p></strong>",
            "<b><i>x</b></i>",
            // Truncated rather than omitted: `<i>` has no optional end tag.
            "<p><i>unclosed",
            // A cell outside a row. `<tr>`'s start tag is not optional, so this
            // is not abbreviation, it is foster parenting.
            "<table><td>x</td></table>",
            "<table><tr><td>x</td></tr>",
        ] {
            assert!(parse_fragment(source).is_err(), "{source}");
        }
    }

    #[test]
    fn accepts_the_end_tags_html_allows_to_be_omitted() {
        for (source, expected) in [
            ("<p>one<p>two", "<p>one</p><p>two</p>"),
            ("<div><p>a</div>", "<div><p>a</p></div>"),
            ("<ul><li>a<li>b</ul>", "<ul><li>a</li><li>b</li></ul>"),
            (
                "<ol><li>a<ul><li>b</ul><li>c</ol>",
                "<ol><li>a<ul><li>b</li></ul></li><li>c</li></ol>",
            ),
            ("<dl><dt>t<dd>d</dl>", "<dl><dt>t</dt><dd>d</dd></dl>"),
            (
                "<select><option>a<option>b</select>",
                "<select><option>a</option><option>b</option></select>",
            ),
            (
                "<select><optgroup label=g><option>a<optgroup label=h><option>b</select>",
                "<select><optgroup label=\"g\"><option>a</option></optgroup><optgroup label=\"h\"><option>b</option></optgroup></select>",
            ),
            (
                "<ruby>a<rt>b<rp>)</ruby>",
                "<ruby>a<rt>b</rt><rp>)</rp></ruby>",
            ),
        ] {
            let nodes = parse_fragment(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            assert_eq!(render(&nodes), expected, "{source}");
        }
    }

    #[test]
    fn opens_the_implied_table_section_a_row_belongs_to() {
        for (source, expected) in [
            (
                "<table><tr><td>x</td></tr></table>",
                "<table><tbody><tr><td>x</td></tr></tbody></table>",
            ),
            (
                "<table><tr><td>a</td></tr><tr><td>b</td></tr></table>",
                "<table><tbody><tr><td>a</td></tr><tr><td>b</td></tr></tbody></table>",
            ),
            (
                "<table><tbody><tr><td>a</table>",
                "<table><tbody><tr><td>a</td></tr></tbody></table>",
            ),
            (
                "<table><tr><td>a<td>b</table>",
                "<table><tbody><tr><td>a</td><td>b</td></tr></tbody></table>",
            ),
            (
                "<table><thead><tr><td>h</td></tr></thead><tr><td>x</td></tr></table>",
                "<table><thead><tr><td>h</td></tr></thead><tbody><tr><td>x</td></tr></tbody></table>",
            ),
            (
                "<table><caption>c</caption><tr><td>x</td></tr></table>",
                "<table><caption>c</caption><tbody><tr><td>x</td></tr></tbody></table>",
            ),
            (
                "<table><tfoot><tr><td>f</td></tr><tbody><tr><td>b</td></tr></table>",
                "<table><tfoot><tr><td>f</td></tr></tfoot><tbody><tr><td>b</td></tr></tbody></table>",
            ),
            // Whitespace before the first row stays in the table, and what
            // follows a row belongs to the section the row opened.
            (
                "<table>\n  <tr><td>x</td></tr>\n</table>",
                "<table>\n  <tbody><tr><td>x</td></tr>\n</tbody></table>",
            ),
        ] {
            let nodes = parse_fragment(source).unwrap_or_else(|error| panic!("{source}: {error}"));
            assert_eq!(render(&nodes), expected, "{source}");
        }
    }

    #[test]
    fn a_fragment_opens_an_implied_section_only_inside_a_table() {
        let inside = parse_fragment_in("<tr><td>x</td></tr>", Some("table")).expect("in a table");
        assert_eq!(render(&inside), "<tbody><tr><td>x</td></tr></tbody>");
        let outside = parse_fragment_in("<tr><td>x</td></tr>", Some("tbody")).expect("in a body");
        assert_eq!(render(&outside), "<tr><td>x</td></tr>");
        let bare = parse_fragment_in("<tr><td>x</td></tr>", None).expect("with no context");
        assert_eq!(render(&bare), "<tr><td>x</td></tr>");
    }

    #[test]
    fn rejects_unsafe_or_ambiguous_syntax() {
        for source in [
            "<DIV></DIV>",
            "<input></input>",
            "<x a=1 a=2></x>",
            "text &copy",
            "<!-- x -- y -->",
        ] {
            assert!(parse_fragment(source).is_err(), "{source}");
        }
    }

    #[test]
    fn accepts_template_marker_attributes() {
        let nodes = parse_fragment("<template lit$123$><i @click$part ?hidden$part .value$part data$part=one></i></template>")
            .expect("template marker attributes parse");
        let Node::Element(template) = &nodes[0] else {
            panic!("template")
        };
        assert_eq!(template.attributes[0].name, "lit$123$");
    }

    #[test]
    fn parses_processing_instruction_markers_as_bogus_comments() {
        let nodes = parse_fragment("<p>before<?lit$123$>after</p>").expect("marker parses");
        let Node::Element(paragraph) = &nodes[0] else {
            panic!("paragraph")
        };
        assert_eq!(
            paragraph.children,
            vec![
                Node::Text("before".to_string()),
                Node::Comment("?lit$123$".to_string()),
                Node::Text("after".to_string()),
            ]
        );
    }

    #[test]
    fn parses_well_formed_svg_foreign_content() {
        let nodes = parse_fragment("<svg viewbox='0 0 10 10'><circle cx='5' cy='5' r='4'/></svg>")
            .expect("parse SVG");
        assert_eq!(
            nodes,
            vec![Node::Element(Element {
                name: "svg".to_string(),
                attributes: vec![Attribute {
                    name: "viewBox".to_string(),
                    value: "0 0 10 10".to_string(),
                }],
                children: vec![Node::Element(Element {
                    name: "circle".to_string(),
                    attributes: vec![
                        Attribute {
                            name: "cx".to_string(),
                            value: "5".to_string(),
                        },
                        Attribute {
                            name: "cy".to_string(),
                            value: "5".to_string(),
                        },
                        Attribute {
                            name: "r".to_string(),
                            value: "4".to_string(),
                        },
                    ],
                    children: vec![],
                })],
            })]
        );
    }

    #[test]
    fn encodes_a_fragment_as_flat_preorder_records() {
        let records = fragment_records("<p id=x>one<!--two--></p>", None).expect("parse");
        assert_eq!(
            records,
            Value::Array(vec![
                Value::Array(vec![
                    Value::Number(1.0),
                    Value::Number(-1.0),
                    Value::String("p".to_string()),
                    Value::Array(vec![Value::Array(vec![
                        Value::String("id".to_string()),
                        Value::String("x".to_string()),
                    ])]),
                    Value::String(String::new()),
                ]),
                Value::Array(vec![
                    Value::Number(3.0),
                    Value::Number(0.0),
                    Value::String(String::new()),
                    Value::Array(vec![]),
                    Value::String("one".to_string()),
                ]),
                Value::Array(vec![
                    Value::Number(8.0),
                    Value::Number(0.0),
                    Value::String(String::new()),
                    Value::Array(vec![]),
                    Value::String("two".to_string()),
                ]),
            ])
        );
    }
}
