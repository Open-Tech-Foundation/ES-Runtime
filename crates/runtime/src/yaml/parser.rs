//! Tokens to events.
//!
//! An explicit-stack state machine over the YAML 1.2 grammar — the same states
//! libyaml and PyYAML use — so nesting depth costs heap, not native stack. The
//! events are what the yaml-test-suite specifies, which is why this layer
//! exists separately from building values: it can be checked against the
//! suite line for line.

use super::scanner::{ScalarStyle, Scanner, Token, TokenKind};
use super::{Error, Mark};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EventKind {
    StreamStart,
    StreamEnd,
    DocumentStart {
        explicit: bool,
    },
    DocumentEnd {
        explicit: bool,
    },
    Alias(String),
    Scalar {
        anchor: Option<String>,
        tag: Option<String>,
        value: String,
        style: ScalarStyle,
    },
    SequenceStart {
        anchor: Option<String>,
        tag: Option<String>,
        flow: bool,
    },
    SequenceEnd,
    MappingStart {
        anchor: Option<String>,
        tag: Option<String>,
        flow: bool,
    },
    MappingEnd,
}

#[derive(Debug, Clone)]
pub(crate) struct Event {
    pub kind: EventKind,
    pub start: Mark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    StreamStart,
    ImplicitDocumentStart,
    DocumentStart,
    DocumentContent,
    DocumentEnd,
    BlockNode,
    BlockSequenceFirstEntry,
    BlockSequenceEntry,
    IndentlessSequenceEntry,
    BlockMappingFirstKey,
    BlockMappingKey,
    BlockMappingValue,
    FlowSequenceFirstEntry,
    FlowSequenceEntry,
    FlowSequenceEntryMappingKey,
    FlowSequenceEntryMappingValue,
    FlowSequenceEntryMappingEnd,
    FlowMappingFirstKey,
    FlowMappingKey,
    FlowMappingValue,
    FlowMappingEmptyValue,
    End,
}

const DEFAULT_TAG_HANDLES: [(&str, &str); 2] = [("!", "!"), ("!!", "tag:yaml.org,2002:")];

pub(crate) struct Parser {
    scanner: Scanner,
    state: State,
    states: Vec<State>,
    tag_handles: Vec<(String, String)>,
    /// Whether the document just ended was closed by `...`. Directives may
    /// only follow such a document (or start the stream).
    open_ended: bool,
}

fn describe(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::StreamStart => "the stream start",
        TokenKind::StreamEnd => "the end of the stream",
        TokenKind::VersionDirective(..)
        | TokenKind::TagDirective(..)
        | TokenKind::ReservedDirective => "a directive",
        TokenKind::DocumentStart => "'---'",
        TokenKind::DocumentEnd => "'...'",
        TokenKind::BlockSequenceStart => "a block sequence",
        TokenKind::BlockMappingStart => "a block mapping",
        TokenKind::BlockEnd => "the end of a block collection",
        TokenKind::FlowSequenceStart => "'['",
        TokenKind::FlowSequenceEnd => "']'",
        TokenKind::FlowMappingStart => "'{'",
        TokenKind::FlowMappingEnd => "'}'",
        TokenKind::BlockEntry => "'-'",
        TokenKind::FlowEntry => "','",
        TokenKind::Key => "a mapping key",
        TokenKind::Value => "':'",
        TokenKind::Alias(_) => "an alias",
        TokenKind::Anchor(_) => "an anchor",
        TokenKind::Tag(..) => "a tag",
        TokenKind::Scalar(..) => "a scalar",
    }
}

impl Parser {
    pub fn new(input: &str) -> Result<Parser, Error> {
        Ok(Parser {
            scanner: Scanner::new(input)?,
            state: State::StreamStart,
            states: Vec::new(),
            tag_handles: Vec::new(),
            open_ended: false,
        })
    }

    /// The next event, or `None` once the stream has ended.
    pub fn next_event(&mut self) -> Result<Option<Event>, Error> {
        if self.state == State::End {
            return Ok(None);
        }
        self.step().map(Some)
    }

    fn peek(&mut self) -> Result<&Token, Error> {
        self.scanner.peek()
    }

    fn peek_kind(&mut self) -> Result<&TokenKind, Error> {
        Ok(&self.scanner.peek()?.kind)
    }

    fn take(&mut self) -> Result<Token, Error> {
        self.scanner.next_token()
    }

    fn unexpected<T>(&mut self, context: &str, expected: &str) -> Result<T, Error> {
        let token = self.peek()?.clone();
        Err(Error::syntax(
            format!(
                "{context}: expected {expected}, but found {}",
                describe(&token.kind)
            ),
            token.start,
        ))
    }

    fn pop_state(&mut self) {
        self.state = self.states.pop().unwrap_or(State::End);
    }

    fn event(kind: EventKind, start: Mark) -> Event {
        Event { kind, start }
    }

    fn empty_scalar(start: Mark) -> Event {
        Self::event(
            EventKind::Scalar {
                anchor: None,
                tag: None,
                value: String::new(),
                style: ScalarStyle::Plain,
            },
            start,
        )
    }

    fn step(&mut self) -> Result<Event, Error> {
        match self.state {
            State::StreamStart => {
                let token = self.take()?;
                debug_assert_eq!(token.kind, TokenKind::StreamStart);
                self.state = State::ImplicitDocumentStart;
                Ok(Self::event(EventKind::StreamStart, token.start))
            }
            State::ImplicitDocumentStart => self.document_start(true),
            State::DocumentStart => self.document_start(false),
            State::DocumentEnd => self.document_end(),
            State::DocumentContent => {
                let token = self.peek()?;
                let start = token.start;
                if matches!(
                    token.kind,
                    TokenKind::VersionDirective(..)
                        | TokenKind::TagDirective(..)
                        | TokenKind::ReservedDirective
                        | TokenKind::DocumentStart
                        | TokenKind::DocumentEnd
                        | TokenKind::StreamEnd
                ) {
                    self.pop_state();
                    Ok(Self::empty_scalar(start))
                } else {
                    self.node(true, false)
                }
            }
            State::BlockNode => self.node(true, false),
            State::BlockSequenceFirstEntry => {
                self.take()?;
                self.block_sequence_entry()
            }
            State::BlockSequenceEntry => self.block_sequence_entry(),
            State::IndentlessSequenceEntry => self.indentless_sequence_entry(),
            State::BlockMappingFirstKey => {
                self.take()?;
                self.block_mapping_key()
            }
            State::BlockMappingKey => self.block_mapping_key(),
            State::BlockMappingValue => self.block_mapping_value(),
            State::FlowSequenceFirstEntry => {
                self.take()?;
                self.flow_sequence_entry(true)
            }
            State::FlowSequenceEntry => self.flow_sequence_entry(false),
            State::FlowSequenceEntryMappingKey => self.flow_sequence_entry_mapping_key(),
            State::FlowSequenceEntryMappingValue => self.flow_sequence_entry_mapping_value(),
            State::FlowSequenceEntryMappingEnd => {
                self.state = State::FlowSequenceEntry;
                let start = self.peek()?.start;
                Ok(Self::event(EventKind::MappingEnd, start))
            }
            State::FlowMappingFirstKey => {
                self.take()?;
                self.flow_mapping_key(true)
            }
            State::FlowMappingKey => self.flow_mapping_key(false),
            State::FlowMappingValue => self.flow_mapping_value(false),
            State::FlowMappingEmptyValue => self.flow_mapping_value(true),
            State::End => unreachable!("next_event returns None at the end"),
        }
    }

    fn document_start(&mut self, first: bool) -> Result<Event, Error> {
        // A bare (marker-less) document may start the stream or follow `...`;
        // after a document that was not closed, only `---` can begin another.
        let mut bare_allowed = first || !self.open_ended;
        while *self.peek_kind()? == TokenKind::DocumentEnd {
            self.take()?;
            self.open_ended = false;
            bare_allowed = true;
        }
        let token = self.peek()?.clone();
        match token.kind {
            TokenKind::StreamEnd => {
                self.take()?;
                self.state = State::End;
                Ok(Self::event(EventKind::StreamEnd, token.start))
            }
            TokenKind::VersionDirective(..)
            | TokenKind::TagDirective(..)
            | TokenKind::ReservedDirective
                if self.open_ended =>
            {
                Err(Error::syntax(
                    "a directive must follow a document closed with '...'",
                    token.start,
                ))
            }
            TokenKind::DocumentStart
            | TokenKind::VersionDirective(..)
            | TokenKind::TagDirective(..)
            | TokenKind::ReservedDirective => {
                self.directives()?;
                if *self.peek_kind()? != TokenKind::DocumentStart {
                    return self.unexpected("after directives", "'---'");
                }
                self.take()?;
                self.states.push(State::DocumentEnd);
                self.state = State::DocumentContent;
                Ok(Self::event(
                    EventKind::DocumentStart { explicit: true },
                    token.start,
                ))
            }
            _ if bare_allowed => {
                self.tag_handles = Vec::new();
                self.states.push(State::DocumentEnd);
                self.state = State::BlockNode;
                Ok(Self::event(
                    EventKind::DocumentStart { explicit: false },
                    token.start,
                ))
            }
            _ => self.unexpected("after a document", "'---' or '...'"),
        }
    }

    fn directives(&mut self) -> Result<(), Error> {
        let mut version = false;
        self.tag_handles = Vec::new();
        loop {
            let token = self.peek()?.clone();
            match token.kind {
                TokenKind::VersionDirective(major, _) => {
                    if version {
                        return Err(Error::syntax("duplicate %YAML directive", token.start));
                    }
                    if major != 1 {
                        return Err(Error::syntax(
                            format!("unsupported YAML version {major}"),
                            token.start,
                        ));
                    }
                    version = true;
                }
                TokenKind::TagDirective(handle, prefix) => {
                    if self.tag_handles.iter().any(|(h, _)| *h == handle) {
                        return Err(Error::syntax(
                            format!("duplicate %TAG directive for {handle}"),
                            token.start,
                        ));
                    }
                    self.tag_handles.push((handle, prefix));
                }
                TokenKind::ReservedDirective => {}
                _ => return Ok(()),
            }
            self.take()?;
        }
    }

    fn document_end(&mut self) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        let explicit = token.kind == TokenKind::DocumentEnd;
        if explicit {
            self.take()?;
        } else if !matches!(
            token.kind,
            TokenKind::DocumentStart
                | TokenKind::StreamEnd
                | TokenKind::VersionDirective(..)
                | TokenKind::TagDirective(..)
                | TokenKind::ReservedDirective
        ) {
            return self.unexpected("after the document's content", "the end of the document");
        }
        self.open_ended = !explicit;
        self.state = State::DocumentStart;
        Ok(Self::event(
            EventKind::DocumentEnd { explicit },
            token.start,
        ))
    }

    fn resolve_tag(&self, handle: &str, suffix: &str, mark: Mark) -> Result<String, Error> {
        if handle.is_empty() {
            return Ok(suffix.to_string());
        }
        let prefix = self
            .tag_handles
            .iter()
            .map(|(h, p)| (h.as_str(), p.as_str()))
            .chain(DEFAULT_TAG_HANDLES)
            .find(|(h, _)| *h == handle)
            .map(|(_, p)| p);
        match prefix {
            Some(p) => Ok(format!("{p}{suffix}")),
            None => Err(Error::syntax(
                format!("undefined tag handle {handle}"),
                mark,
            )),
        }
    }

    fn node(&mut self, block: bool, indentless_sequence: bool) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        if let TokenKind::Alias(name) = token.kind {
            self.take()?;
            self.pop_state();
            return Ok(Self::event(EventKind::Alias(name), token.start));
        }
        let start = token.start;
        let mut anchor = None;
        let mut tag = None;
        for _ in 0..2 {
            let token = self.peek()?.clone();
            match token.kind {
                TokenKind::Anchor(name) if anchor.is_none() => {
                    self.take()?;
                    anchor = Some(name);
                }
                TokenKind::Tag(handle, suffix) if tag.is_none() => {
                    self.take()?;
                    tag = Some(self.resolve_tag(&handle, &suffix, token.start)?);
                }
                _ => break,
            }
        }
        let token = self.peek()?.clone();
        let has_properties = anchor.is_some() || tag.is_some();
        match token.kind {
            TokenKind::BlockEntry if indentless_sequence => {
                self.state = State::IndentlessSequenceEntry;
                Ok(Self::event(
                    EventKind::SequenceStart {
                        anchor,
                        tag,
                        flow: false,
                    },
                    start,
                ))
            }
            TokenKind::Scalar(value, style) => {
                self.take()?;
                self.pop_state();
                Ok(Self::event(
                    EventKind::Scalar {
                        anchor,
                        tag,
                        value,
                        style,
                    },
                    start,
                ))
            }
            TokenKind::FlowSequenceStart => {
                self.state = State::FlowSequenceFirstEntry;
                Ok(Self::event(
                    EventKind::SequenceStart {
                        anchor,
                        tag,
                        flow: true,
                    },
                    start,
                ))
            }
            TokenKind::FlowMappingStart => {
                self.state = State::FlowMappingFirstKey;
                Ok(Self::event(
                    EventKind::MappingStart {
                        anchor,
                        tag,
                        flow: true,
                    },
                    start,
                ))
            }
            TokenKind::BlockSequenceStart if block => {
                self.state = State::BlockSequenceFirstEntry;
                Ok(Self::event(
                    EventKind::SequenceStart {
                        anchor,
                        tag,
                        flow: false,
                    },
                    start,
                ))
            }
            TokenKind::BlockMappingStart if block => {
                self.state = State::BlockMappingFirstKey;
                Ok(Self::event(
                    EventKind::MappingStart {
                        anchor,
                        tag,
                        flow: false,
                    },
                    start,
                ))
            }
            TokenKind::Alias(_) if has_properties => Err(Error::syntax(
                "an alias cannot have an anchor or a tag",
                token.start,
            )),
            _ if has_properties => {
                self.pop_state();
                Ok(Self::event(
                    EventKind::Scalar {
                        anchor,
                        tag,
                        value: String::new(),
                        style: ScalarStyle::Plain,
                    },
                    start,
                ))
            }
            _ => self.unexpected(
                if block {
                    "while parsing a block node"
                } else {
                    "while parsing a flow node"
                },
                "a node",
            ),
        }
    }

    fn block_sequence_entry(&mut self) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        match token.kind {
            TokenKind::BlockEntry => {
                self.take()?;
                let next = self.peek()?.clone();
                if matches!(next.kind, TokenKind::BlockEntry | TokenKind::BlockEnd) {
                    self.state = State::BlockSequenceEntry;
                    Ok(Self::empty_scalar(next.start))
                } else {
                    self.states.push(State::BlockSequenceEntry);
                    self.node(true, false)
                }
            }
            TokenKind::BlockEnd => {
                self.take()?;
                self.pop_state();
                Ok(Self::event(EventKind::SequenceEnd, token.start))
            }
            _ => self.unexpected("in a block sequence", "'-' or the end of the sequence"),
        }
    }

    fn indentless_sequence_entry(&mut self) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        if token.kind == TokenKind::BlockEntry {
            self.take()?;
            let next = self.peek()?.clone();
            if matches!(
                next.kind,
                TokenKind::BlockEntry | TokenKind::Key | TokenKind::Value | TokenKind::BlockEnd
            ) {
                self.state = State::IndentlessSequenceEntry;
                Ok(Self::empty_scalar(next.start))
            } else {
                self.states.push(State::IndentlessSequenceEntry);
                self.node(true, false)
            }
        } else {
            self.pop_state();
            Ok(Self::event(EventKind::SequenceEnd, token.start))
        }
    }

    fn block_mapping_key(&mut self) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        match token.kind {
            TokenKind::Key => {
                self.take()?;
                let next = self.peek()?.clone();
                if matches!(
                    next.kind,
                    TokenKind::Key | TokenKind::Value | TokenKind::BlockEnd
                ) {
                    self.state = State::BlockMappingValue;
                    Ok(Self::empty_scalar(next.start))
                } else {
                    self.states.push(State::BlockMappingValue);
                    self.node(true, true)
                }
            }
            // `: value` with nothing before it: an empty key.
            TokenKind::Value => {
                self.state = State::BlockMappingValue;
                Ok(Self::empty_scalar(token.start))
            }
            TokenKind::BlockEnd => {
                self.take()?;
                self.pop_state();
                Ok(Self::event(EventKind::MappingEnd, token.start))
            }
            _ => self.unexpected("in a block mapping", "a key or the end of the mapping"),
        }
    }

    fn block_mapping_value(&mut self) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        if token.kind == TokenKind::Value {
            self.take()?;
            let next = self.peek()?.clone();
            if matches!(
                next.kind,
                TokenKind::Key | TokenKind::Value | TokenKind::BlockEnd
            ) {
                self.state = State::BlockMappingKey;
                Ok(Self::empty_scalar(next.start))
            } else {
                self.states.push(State::BlockMappingKey);
                self.node(true, true)
            }
        } else {
            self.state = State::BlockMappingKey;
            Ok(Self::empty_scalar(token.start))
        }
    }

    fn flow_sequence_entry(&mut self, first: bool) -> Result<Event, Error> {
        let mut token = self.peek()?.clone();
        if token.kind != TokenKind::FlowSequenceEnd {
            if !first {
                if token.kind != TokenKind::FlowEntry {
                    return self.unexpected("in a flow sequence", "',' or ']'");
                }
                self.take()?;
                token = self.peek()?.clone();
            }
            match token.kind {
                TokenKind::Key | TokenKind::Value => {
                    self.state = State::FlowSequenceEntryMappingKey;
                    return Ok(Self::event(
                        EventKind::MappingStart {
                            anchor: None,
                            tag: None,
                            flow: true,
                        },
                        token.start,
                    ));
                }
                TokenKind::FlowSequenceEnd => {}
                TokenKind::FlowEntry => {
                    return self.unexpected("in a flow sequence", "an entry");
                }
                _ => {
                    self.states.push(State::FlowSequenceEntry);
                    return self.node(false, false);
                }
            }
        }
        self.take()?;
        self.pop_state();
        Ok(Self::event(EventKind::SequenceEnd, token.start))
    }

    fn flow_sequence_entry_mapping_key(&mut self) -> Result<Event, Error> {
        let mut token = self.peek()?.clone();
        if token.kind == TokenKind::Key {
            self.take()?;
            token = self.peek()?.clone();
        }
        if matches!(
            token.kind,
            TokenKind::Value | TokenKind::FlowEntry | TokenKind::FlowSequenceEnd
        ) {
            self.state = State::FlowSequenceEntryMappingValue;
            Ok(Self::empty_scalar(token.start))
        } else {
            self.states.push(State::FlowSequenceEntryMappingValue);
            self.node(false, false)
        }
    }

    fn flow_sequence_entry_mapping_value(&mut self) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        if token.kind == TokenKind::Value {
            self.take()?;
            let next = self.peek()?.clone();
            if matches!(next.kind, TokenKind::FlowEntry | TokenKind::FlowSequenceEnd) {
                self.state = State::FlowSequenceEntryMappingEnd;
                Ok(Self::empty_scalar(next.start))
            } else {
                self.states.push(State::FlowSequenceEntryMappingEnd);
                self.node(false, false)
            }
        } else {
            self.state = State::FlowSequenceEntryMappingEnd;
            Ok(Self::empty_scalar(token.start))
        }
    }

    fn flow_mapping_key(&mut self, first: bool) -> Result<Event, Error> {
        let mut token = self.peek()?.clone();
        if token.kind != TokenKind::FlowMappingEnd {
            if !first {
                if token.kind != TokenKind::FlowEntry {
                    return self.unexpected("in a flow mapping", "',' or '}'");
                }
                self.take()?;
                token = self.peek()?.clone();
            }
            match token.kind {
                TokenKind::Key => {
                    self.take()?;
                    let next = self.peek()?.clone();
                    if matches!(
                        next.kind,
                        TokenKind::Value | TokenKind::FlowEntry | TokenKind::FlowMappingEnd
                    ) {
                        self.state = State::FlowMappingValue;
                        return Ok(Self::empty_scalar(next.start));
                    }
                    self.states.push(State::FlowMappingValue);
                    return self.node(false, false);
                }
                TokenKind::Value => {
                    self.state = State::FlowMappingValue;
                    return Ok(Self::empty_scalar(token.start));
                }
                TokenKind::FlowMappingEnd => {}
                TokenKind::FlowEntry => {
                    return self.unexpected("in a flow mapping", "an entry");
                }
                _ => {
                    self.states.push(State::FlowMappingEmptyValue);
                    return self.node(false, false);
                }
            }
        }
        self.take()?;
        self.pop_state();
        Ok(Self::event(EventKind::MappingEnd, token.start))
    }

    fn flow_mapping_value(&mut self, empty: bool) -> Result<Event, Error> {
        let token = self.peek()?.clone();
        if empty {
            self.state = State::FlowMappingKey;
            return Ok(Self::empty_scalar(token.start));
        }
        if token.kind == TokenKind::Value {
            self.take()?;
            let next = self.peek()?.clone();
            if matches!(next.kind, TokenKind::FlowEntry | TokenKind::FlowMappingEnd) {
                self.state = State::FlowMappingKey;
                return Ok(Self::empty_scalar(next.start));
            }
            self.states.push(State::FlowMappingKey);
            self.node(false, false)
        } else {
            self.state = State::FlowMappingKey;
            Ok(Self::empty_scalar(token.start))
        }
    }
}
