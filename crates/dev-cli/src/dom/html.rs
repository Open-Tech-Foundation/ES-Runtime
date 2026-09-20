//! A small, strict parser for the modern HTML accepted by the esdev test DOM.
//!
//! This is not an HTML5 error-recovery parser. Input is either well-nested,
//! modern markup or an error with a byte offset. In particular, the parser
//! does not invent omitted end tags, foster-parent table content, or repair
//! misnested formatting elements. Those browser compatibility behaviours are
//! valuable to a browser; silently changing a test fixture is not.

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
#[allow(
    dead_code,
    reason = "document parsing is wired with starting-HTML test directives"
)]
pub fn parse_document(source: &str) -> Result<Document, Error> {
    let mut parser = Parser::new(source);
    let document = parser.nodes(None, true, false)?;
    if !parser.eof() {
        return Err(parser.error("unexpected content after the document"));
    }
    Ok(document)
}

/// Parses nodes for an element's `innerHTML`.
///
/// Fragment parsing shares exactly the same strict grammar as documents but
/// never accepts a doctype.
pub fn parse_fragment(source: &str) -> Result<Vec<Node>, Error> {
    let mut parser = Parser::new(source);
    let document = parser.nodes(None, false, false)?;
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
pub fn fragment_records(source: &str) -> Result<Value, Error> {
    let nodes = parse_fragment(source)?;
    let mut records = Vec::new();
    for node in &nodes {
        encode_node(node, -1, &mut records);
    }
    Ok(Value::Array(records))
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

    fn nodes(
        &mut self,
        closing: Option<&str>,
        document: bool,
        in_svg: bool,
    ) -> Result<Document, Error> {
        let mut out = Document::default();
        loop {
            if self.eof() {
                if let Some(name) = closing {
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
            self.nodes(Some(&name), false, in_svg)?.children
        };
        Ok(Element {
            name,
            attributes,
            children,
        })
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
        if !(first.is_ascii_lowercase() || allow_svg_case && first.is_ascii_uppercase() || template_marker) {
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

    #[test]
    fn rejects_legacy_recovery_cases() {
        for source in [
            "<p><strong>x</p></strong>",
            "<table><tr><td>x</table>",
            "<p>one<p>two",
        ] {
            assert!(parse_fragment(source).is_err(), "{source}");
        }
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
        let Node::Element(template) = &nodes[0] else { panic!("template") };
        assert_eq!(template.attributes[0].name, "lit$123$");
    }

    #[test]
    fn parses_processing_instruction_markers_as_bogus_comments() {
        let nodes = parse_fragment("<p>before<?lit$123$>after</p>").expect("marker parses");
        let Node::Element(paragraph) = &nodes[0] else { panic!("paragraph") };
        assert_eq!(paragraph.children, vec![
            Node::Text("before".to_string()), Node::Comment("?lit$123$".to_string()), Node::Text("after".to_string()),
        ]);
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
        let records = fragment_records("<p id=x>one<!--two--></p>").expect("parse");
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
