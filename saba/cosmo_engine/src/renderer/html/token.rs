//! The HTML tokenizer: every state of the standard's state machine.
//! https://html.spec.whatwg.org/multipage/parsing.html#tokenization

use crate::renderer::html::attribute::Attribute;
use crate::renderer::html::entities::ENTITIES;
use std::collections::VecDeque;
use std::string::String;
use std::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlToken {
    StartTag {
        tag: String,
        self_closing: bool,
        attributes: Vec<Attribute>,
    },
    EndTag {
        tag: String,
    },
    Char(char),
    Comment(String),
    Doctype {
        name: Option<String>,
        public_id: Option<String>,
        system_id: Option<String>,
        force_quirks: bool,
    },
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Data,
    Rcdata,
    Rawtext,
    ScriptData,
    Plaintext,
    TagOpen,
    EndTagOpen,
    TagName,
    RcdataLessThanSign,
    RcdataEndTagOpen,
    RcdataEndTagName,
    RawtextLessThanSign,
    RawtextEndTagOpen,
    RawtextEndTagName,
    ScriptDataLessThanSign,
    ScriptDataEndTagOpen,
    ScriptDataEndTagName,
    ScriptDataEscapeStart,
    ScriptDataEscapeStartDash,
    ScriptDataEscaped,
    ScriptDataEscapedDash,
    ScriptDataEscapedDashDash,
    ScriptDataEscapedLessThanSign,
    ScriptDataEscapedEndTagOpen,
    ScriptDataEscapedEndTagName,
    ScriptDataDoubleEscapeStart,
    ScriptDataDoubleEscaped,
    ScriptDataDoubleEscapedDash,
    ScriptDataDoubleEscapedDashDash,
    ScriptDataDoubleEscapedLessThanSign,
    ScriptDataDoubleEscapeEnd,
    BeforeAttributeName,
    AttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    AttributeValueDoubleQuoted,
    AttributeValueSingleQuoted,
    AttributeValueUnquoted,
    AfterAttributeValueQuoted,
    SelfClosingStartTag,
    BogusComment,
    MarkupDeclarationOpen,
    CommentStart,
    CommentStartDash,
    Comment,
    CommentLessThanSign,
    CommentLessThanSignBang,
    CommentLessThanSignBangDash,
    CommentLessThanSignBangDashDash,
    CommentEndDash,
    CommentEnd,
    CommentEndBang,
    Doctype,
    BeforeDoctypeName,
    DoctypeName,
    AfterDoctypeName,
    AfterDoctypePublicKeyword,
    BeforeDoctypePublicIdentifier,
    DoctypePublicIdentifierDoubleQuoted,
    DoctypePublicIdentifierSingleQuoted,
    AfterDoctypePublicIdentifier,
    BetweenDoctypePublicAndSystemIdentifiers,
    AfterDoctypeSystemKeyword,
    BeforeDoctypeSystemIdentifier,
    DoctypeSystemIdentifierDoubleQuoted,
    DoctypeSystemIdentifierSingleQuoted,
    AfterDoctypeSystemIdentifier,
    BogusDoctype,
    CdataSection,
    CdataSectionBracket,
    CdataSectionEnd,
}

/// The tag being built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TagInProgress {
    is_start: bool,
    name: String,
    self_closing: bool,
    attributes: Vec<(String, String)>,
}

/// The DOCTYPE being built.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DoctypeInProgress {
    name: Option<String>,
    public_id: Option<String>,
    system_id: Option<String>,
    force_quirks: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlTokenizer {
    state: State,
    input: Vec<char>,
    pos: usize,
    tag: TagInProgress,
    doctype: DoctypeInProgress,
    comment: String,
    temporary_buffer: String,
    last_start_tag: String,
    /// Tokens ready to hand out.
    queue: VecDeque<HtmlToken>,
    eof_emitted: bool,
    /// The tree builder's say on `<![CDATA[`: only foreign content has it.
    cdata_allowed: bool,
}

fn is_html_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{c}' | ' ')
}

/// What a numeric reference to a C1 control means: those code points are read as
/// Windows-1252, as documents that use them intend.
/// https://html.spec.whatwg.org/multipage/parsing.html#numeric-character-reference-end-state
fn windows_1252_override(code: u32) -> Option<u32> {
    Some(match code {
        0x80 => 0x20AC,
        0x82 => 0x201A,
        0x83 => 0x0192,
        0x84 => 0x201E,
        0x85 => 0x2026,
        0x86 => 0x2020,
        0x87 => 0x2021,
        0x88 => 0x02C6,
        0x89 => 0x2030,
        0x8A => 0x0160,
        0x8B => 0x2039,
        0x8C => 0x0152,
        0x8E => 0x017D,
        0x91 => 0x2018,
        0x92 => 0x2019,
        0x93 => 0x201C,
        0x94 => 0x201D,
        0x95 => 0x2022,
        0x96 => 0x2013,
        0x97 => 0x2014,
        0x98 => 0x02DC,
        0x99 => 0x2122,
        0x9A => 0x0161,
        0x9B => 0x203A,
        0x9C => 0x0153,
        0x9E => 0x017E,
        0x9F => 0x0178,
        _ => return None,
    })
}

impl HtmlTokenizer {
    pub fn new(html: String) -> Self {
        // Newlines are normalized before tokenizing.
        // https://html.spec.whatwg.org/multipage/parsing.html#preprocessing-the-input-stream
        let mut input = Vec::with_capacity(html.len());
        let mut chars = html.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                input.push('\n');
            } else {
                input.push(c);
            }
        }
        Self {
            state: State::Data,
            input,
            pos: 0,
            tag: TagInProgress::default(),
            doctype: DoctypeInProgress::default(),
            comment: String::new(),
            temporary_buffer: String::new(),
            last_start_tag: String::new(),
            queue: VecDeque::new(),
            eof_emitted: false,
            cdata_allowed: false,
        }
    }

    /// The tree builder switches states for elements whose content isn't markup.
    pub fn set_state(&mut self, state: State) {
        self.state = state;
    }

    pub fn set_cdata_allowed(&mut self, allowed: bool) {
        self.cdata_allowed = allowed;
    }

    fn consume(&mut self) -> Option<char> {
        let c = self.input.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn reconsume(&mut self, c: Option<char>) {
        if c.is_some() {
            self.pos -= 1;
        }
    }

    fn emit_char(&mut self, c: char) {
        self.queue.push_back(HtmlToken::Char(c));
    }

    fn emit_str(&mut self, s: &str) {
        for c in s.chars() {
            self.emit_char(c);
        }
    }

    fn start_tag(&mut self, is_start: bool) {
        self.tag = TagInProgress {
            is_start,
            ..TagInProgress::default()
        };
    }

    fn emit_tag(&mut self) {
        let tag = std::mem::take(&mut self.tag);
        if tag.is_start {
            self.last_start_tag = tag.name.clone();
            // A repeated attribute is dropped: the first one wins.
            let mut attributes: Vec<Attribute> = Vec::with_capacity(tag.attributes.len());
            for (name, value) in tag.attributes {
                if !attributes.iter().any(|a| a.name() == name) {
                    attributes.push(Attribute::from_name_value(&name, &value));
                }
            }
            self.queue.push_back(HtmlToken::StartTag {
                tag: tag.name,
                self_closing: tag.self_closing,
                attributes,
            });
        } else {
            self.queue.push_back(HtmlToken::EndTag { tag: tag.name });
        }
    }

    fn emit_comment(&mut self) {
        let comment = std::mem::take(&mut self.comment);
        self.queue.push_back(HtmlToken::Comment(comment));
    }

    fn emit_doctype(&mut self) {
        let doctype = std::mem::take(&mut self.doctype);
        self.queue.push_back(HtmlToken::Doctype {
            name: doctype.name,
            public_id: doctype.public_id,
            system_id: doctype.system_id,
            force_quirks: doctype.force_quirks,
        });
    }

    fn emit_eof(&mut self) {
        self.queue.push_back(HtmlToken::Eof);
        self.eof_emitted = true;
    }

    fn attr_name(&mut self) -> Option<&mut String> {
        self.tag.attributes.last_mut().map(|a| &mut a.0)
    }

    fn attr_value(&mut self) -> Option<&mut String> {
        self.tag.attributes.last_mut().map(|a| &mut a.1)
    }

    fn is_appropriate_end_tag(&self) -> bool {
        !self.tag.is_start && self.tag.name == self.last_start_tag
    }

    /// Whether the input continues with `word`, comparing case-insensitively if asked.
    fn lookahead(&self, word: &str, ignore_case: bool) -> bool {
        word.chars().enumerate().all(|(i, w)| {
            self.input.get(self.pos + i).is_some_and(|c| {
                if ignore_case {
                    c.eq_ignore_ascii_case(&w)
                } else {
                    *c == w
                }
            })
        })
    }

    /// Consume a character reference whose `&` has just been read. Returns what
    /// it stands for, or `None` to treat the `&` as text.
    /// https://html.spec.whatwg.org/multipage/parsing.html#character-reference-state
    fn character_reference(&mut self, in_attribute: bool) -> Option<String> {
        match self.input.get(self.pos).copied() {
            Some('#') => self.numeric_reference(),
            Some(c) if c.is_ascii_alphanumeric() => self.named_reference(in_attribute),
            _ => None,
        }
    }

    fn numeric_reference(&mut self) -> Option<String> {
        let start = self.pos;
        self.pos += 1;
        let hex = matches!(self.input.get(self.pos), Some('x') | Some('X'));
        if hex {
            self.pos += 1;
        }
        let radix = if hex { 16 } else { 10 };
        let mut code: u32 = 0;
        let mut digits = 0;
        while let Some(digit) = self.input.get(self.pos).and_then(|c| c.to_digit(radix)) {
            code = code.saturating_mul(radix).saturating_add(digit);
            self.pos += 1;
            digits += 1;
        }
        if digits == 0 {
            self.pos = start;
            return None;
        }
        if self.input.get(self.pos) == Some(&';') {
            self.pos += 1;
        }
        let code = windows_1252_override(code).unwrap_or(code);
        let c = match code {
            0 => '\u{fffd}',
            c if c > 0x10FFFF => '\u{fffd}',
            c => char::from_u32(c).unwrap_or('\u{fffd}'),
        };
        Some(c.to_string())
    }

    fn named_reference(&mut self, in_attribute: bool) -> Option<String> {
        // Names run at most this long; the longest match wins.
        const LONGEST: usize = 40;
        let mut name = String::new();
        let mut best: Option<(usize, &'static str)> = None;
        for (i, c) in self.input[self.pos..].iter().take(LONGEST).enumerate() {
            if !c.is_ascii_alphanumeric() && *c != ';' {
                break;
            }
            name.push(*c);
            if let Ok(found) = ENTITIES.binary_search_by(|(k, _)| (*k).cmp(name.as_str())) {
                best = Some((i + 1, ENTITIES[found].1));
            }
            if *c == ';' {
                break;
            }
        }
        let (len, value) = best?;
        let ends_with_semicolon = self.input[self.pos + len - 1] == ';';
        if in_attribute && !ends_with_semicolon {
            // In an attribute, `&copy=1` is a URL parameter, not a reference.
            let next = self.input.get(self.pos + len);
            if next.is_some_and(|c| *c == '=' || c.is_ascii_alphanumeric()) {
                return None;
            }
        }
        self.pos += len;
        Some(value.to_string())
    }

    /// Run one step of the state machine, which may or may not emit anything.
    fn step(&mut self) {
        match self.state {
            State::Data => match self.consume() {
                Some('&') => match self.character_reference(false) {
                    Some(text) => self.emit_str(&text),
                    None => self.emit_char('&'),
                },
                Some('<') => self.state = State::TagOpen,
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::Rcdata => match self.consume() {
                Some('&') => match self.character_reference(false) {
                    Some(text) => self.emit_str(&text),
                    None => self.emit_char('&'),
                },
                Some('<') => self.state = State::RcdataLessThanSign,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::Rawtext => match self.consume() {
                Some('<') => self.state = State::RawtextLessThanSign,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::ScriptData => match self.consume() {
                Some('<') => self.state = State::ScriptDataLessThanSign,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::Plaintext => match self.consume() {
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::TagOpen => match self.consume() {
                Some('!') => self.state = State::MarkupDeclarationOpen,
                Some('/') => self.state = State::EndTagOpen,
                Some(c) if c.is_ascii_alphabetic() => {
                    self.start_tag(true);
                    self.pos -= 1;
                    self.state = State::TagName;
                }
                Some('?') => {
                    self.comment.clear();
                    self.pos -= 1;
                    self.state = State::BogusComment;
                }
                other => {
                    self.emit_char('<');
                    self.reconsume(other);
                    self.state = State::Data;
                }
            },
            State::EndTagOpen => match self.consume() {
                Some(c) if c.is_ascii_alphabetic() => {
                    self.start_tag(false);
                    self.pos -= 1;
                    self.state = State::TagName;
                }
                Some('>') => self.state = State::Data,
                None => {
                    self.emit_str("</");
                    self.emit_eof();
                }
                Some(_) => {
                    self.comment.clear();
                    self.pos -= 1;
                    self.state = State::BogusComment;
                }
            },
            State::TagName => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::BeforeAttributeName,
                Some('/') => self.state = State::SelfClosingStartTag,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                Some('\0') => self.tag.name.push('\u{fffd}'),
                Some(c) => self.tag.name.push(c.to_ascii_lowercase()),
                None => self.emit_eof(),
            },
            State::RcdataLessThanSign => self.text_less_than(State::Rcdata, State::RcdataEndTagOpen),
            State::RawtextLessThanSign => {
                self.text_less_than(State::Rawtext, State::RawtextEndTagOpen)
            }
            State::RcdataEndTagOpen => {
                self.text_end_tag_open(State::Rcdata, State::RcdataEndTagName)
            }
            State::RawtextEndTagOpen => {
                self.text_end_tag_open(State::Rawtext, State::RawtextEndTagName)
            }
            State::ScriptDataEndTagOpen => {
                self.text_end_tag_open(State::ScriptData, State::ScriptDataEndTagName)
            }
            State::ScriptDataEscapedEndTagOpen => {
                self.text_end_tag_open(State::ScriptDataEscaped, State::ScriptDataEscapedEndTagName)
            }
            State::RcdataEndTagName => self.text_end_tag_name(State::Rcdata),
            State::RawtextEndTagName => self.text_end_tag_name(State::Rawtext),
            State::ScriptDataEndTagName => self.text_end_tag_name(State::ScriptData),
            State::ScriptDataEscapedEndTagName => self.text_end_tag_name(State::ScriptDataEscaped),
            State::ScriptDataLessThanSign => match self.consume() {
                Some('/') => {
                    self.temporary_buffer.clear();
                    self.state = State::ScriptDataEndTagOpen;
                }
                Some('!') => {
                    self.state = State::ScriptDataEscapeStart;
                    self.emit_str("<!");
                }
                other => {
                    self.emit_char('<');
                    self.reconsume(other);
                    self.state = State::ScriptData;
                }
            },
            State::ScriptDataEscapeStart => match self.consume() {
                Some('-') => {
                    self.state = State::ScriptDataEscapeStartDash;
                    self.emit_char('-');
                }
                other => {
                    self.reconsume(other);
                    self.state = State::ScriptData;
                }
            },
            State::ScriptDataEscapeStartDash => match self.consume() {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                other => {
                    self.reconsume(other);
                    self.state = State::ScriptData;
                }
            },
            State::ScriptDataEscaped => match self.consume() {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = State::ScriptDataEscapedLessThanSign,
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::ScriptDataEscapedDash => match self.consume() {
                Some('-') => {
                    self.state = State::ScriptDataEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => self.state = State::ScriptDataEscapedLessThanSign,
                Some('\0') => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(c) => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            State::ScriptDataEscapedDashDash => match self.consume() {
                Some('-') => self.emit_char('-'),
                Some('<') => self.state = State::ScriptDataEscapedLessThanSign,
                Some('>') => {
                    self.state = State::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(c) => {
                    self.state = State::ScriptDataEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            State::ScriptDataEscapedLessThanSign => match self.consume() {
                Some('/') => {
                    self.temporary_buffer.clear();
                    self.state = State::ScriptDataEscapedEndTagOpen;
                }
                Some(c) if c.is_ascii_alphabetic() => {
                    self.temporary_buffer.clear();
                    self.emit_char('<');
                    self.pos -= 1;
                    self.state = State::ScriptDataDoubleEscapeStart;
                }
                other => {
                    self.emit_char('<');
                    self.reconsume(other);
                    self.state = State::ScriptDataEscaped;
                }
            },
            State::ScriptDataDoubleEscapeStart => {
                self.double_escape_boundary(State::ScriptDataDoubleEscaped, State::ScriptDataEscaped)
            }
            State::ScriptDataDoubleEscaped => match self.consume() {
                Some('-') => {
                    self.state = State::ScriptDataDoubleEscapedDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('\0') => self.emit_char('\u{fffd}'),
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::ScriptDataDoubleEscapedDash => match self.consume() {
                Some('-') => {
                    self.state = State::ScriptDataDoubleEscapedDashDash;
                    self.emit_char('-');
                }
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('\0') => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(c) => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            State::ScriptDataDoubleEscapedDashDash => match self.consume() {
                Some('-') => self.emit_char('-'),
                Some('<') => {
                    self.state = State::ScriptDataDoubleEscapedLessThanSign;
                    self.emit_char('<');
                }
                Some('>') => {
                    self.state = State::ScriptData;
                    self.emit_char('>');
                }
                Some('\0') => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char('\u{fffd}');
                }
                Some(c) => {
                    self.state = State::ScriptDataDoubleEscaped;
                    self.emit_char(c);
                }
                None => self.emit_eof(),
            },
            State::ScriptDataDoubleEscapedLessThanSign => match self.consume() {
                Some('/') => {
                    self.temporary_buffer.clear();
                    self.state = State::ScriptDataDoubleEscapeEnd;
                    self.emit_char('/');
                }
                other => {
                    self.reconsume(other);
                    self.state = State::ScriptDataDoubleEscaped;
                }
            },
            State::ScriptDataDoubleEscapeEnd => {
                self.double_escape_boundary(State::ScriptDataEscaped, State::ScriptDataDoubleEscaped)
            }
            State::BeforeAttributeName => match self.consume() {
                Some(c) if is_html_space(c) => {}
                other @ (Some('/') | Some('>') | None) => {
                    self.reconsume(other);
                    self.state = State::AfterAttributeName;
                }
                Some('=') => {
                    self.tag.attributes.push(("=".to_string(), String::new()));
                    self.state = State::AttributeName;
                }
                other => {
                    self.tag.attributes.push((String::new(), String::new()));
                    self.reconsume(other);
                    self.state = State::AttributeName;
                }
            },
            State::AttributeName => match self.consume() {
                other @ (Some('/') | Some('>') | None) => {
                    self.reconsume(other);
                    self.state = State::AfterAttributeName;
                }
                Some(c) if is_html_space(c) => self.state = State::AfterAttributeName,
                Some('=') => self.state = State::BeforeAttributeValue,
                Some('\0') => {
                    if let Some(name) = self.attr_name() {
                        name.push('\u{fffd}');
                    }
                }
                Some(c) => {
                    if let Some(name) = self.attr_name() {
                        name.push(c.to_ascii_lowercase());
                    }
                }
            },
            State::AfterAttributeName => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('/') => self.state = State::SelfClosingStartTag,
                Some('=') => self.state = State::BeforeAttributeValue,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                None => self.emit_eof(),
                other => {
                    self.tag.attributes.push((String::new(), String::new()));
                    self.reconsume(other);
                    self.state = State::AttributeName;
                }
            },
            State::BeforeAttributeValue => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('"') => self.state = State::AttributeValueDoubleQuoted,
                Some('\'') => self.state = State::AttributeValueSingleQuoted,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                other => {
                    self.reconsume(other);
                    self.state = State::AttributeValueUnquoted;
                }
            },
            State::AttributeValueDoubleQuoted => self.quoted_attribute_value('"'),
            State::AttributeValueSingleQuoted => self.quoted_attribute_value('\''),
            State::AttributeValueUnquoted => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::BeforeAttributeName,
                Some('&') => self.attribute_character_reference(),
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                Some('\0') => {
                    if let Some(value) = self.attr_value() {
                        value.push('\u{fffd}');
                    }
                }
                Some(c) => {
                    if let Some(value) = self.attr_value() {
                        value.push(c);
                    }
                }
                None => self.emit_eof(),
            },
            State::AfterAttributeValueQuoted => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::BeforeAttributeName,
                Some('/') => self.state = State::SelfClosingStartTag,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_tag();
                }
                None => self.emit_eof(),
                other => {
                    self.reconsume(other);
                    self.state = State::BeforeAttributeName;
                }
            },
            State::SelfClosingStartTag => match self.consume() {
                Some('>') => {
                    self.tag.self_closing = true;
                    self.state = State::Data;
                    self.emit_tag();
                }
                None => self.emit_eof(),
                other => {
                    self.reconsume(other);
                    self.state = State::BeforeAttributeName;
                }
            },
            State::BogusComment => match self.consume() {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                Some('\0') => self.comment.push('\u{fffd}'),
                Some(c) => self.comment.push(c),
            },
            State::MarkupDeclarationOpen => {
                if self.lookahead("--", false) {
                    self.pos += 2;
                    self.comment.clear();
                    self.state = State::CommentStart;
                } else if self.lookahead("DOCTYPE", true) {
                    self.pos += 7;
                    self.state = State::Doctype;
                } else if self.lookahead("[CDATA[", false) {
                    self.pos += 7;
                    if self.cdata_allowed {
                        self.state = State::CdataSection;
                    } else {
                        self.comment = "[CDATA[".to_string();
                        self.state = State::BogusComment;
                    }
                } else {
                    self.comment.clear();
                    self.state = State::BogusComment;
                }
            }
            State::CommentStart => match self.consume() {
                Some('-') => self.state = State::CommentStartDash,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                other => {
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::CommentStartDash => match self.consume() {
                Some('-') => self.state = State::CommentEnd,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                other => {
                    self.comment.push('-');
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::Comment => match self.consume() {
                Some('<') => {
                    self.comment.push('<');
                    self.state = State::CommentLessThanSign;
                }
                Some('-') => self.state = State::CommentEndDash,
                Some('\0') => self.comment.push('\u{fffd}'),
                Some(c) => self.comment.push(c),
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
            },
            State::CommentLessThanSign => match self.consume() {
                Some('!') => {
                    self.comment.push('!');
                    self.state = State::CommentLessThanSignBang;
                }
                Some('<') => self.comment.push('<'),
                other => {
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::CommentLessThanSignBang => match self.consume() {
                Some('-') => self.state = State::CommentLessThanSignBangDash,
                other => {
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::CommentLessThanSignBangDash => match self.consume() {
                Some('-') => self.state = State::CommentLessThanSignBangDashDash,
                other => {
                    self.reconsume(other);
                    self.state = State::CommentEndDash;
                }
            },
            State::CommentLessThanSignBangDashDash => {
                let c = self.consume();
                self.reconsume(c);
                self.state = State::CommentEnd;
            }
            State::CommentEndDash => match self.consume() {
                Some('-') => self.state = State::CommentEnd,
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                other => {
                    self.comment.push('-');
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::CommentEnd => match self.consume() {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                Some('!') => self.state = State::CommentEndBang,
                Some('-') => self.comment.push('-'),
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                other => {
                    self.comment.push_str("--");
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::CommentEndBang => match self.consume() {
                Some('-') => {
                    self.comment.push_str("--!");
                    self.state = State::CommentEndDash;
                }
                Some('>') => {
                    self.state = State::Data;
                    self.emit_comment();
                }
                None => {
                    self.emit_comment();
                    self.emit_eof();
                }
                other => {
                    self.comment.push_str("--!");
                    self.reconsume(other);
                    self.state = State::Comment;
                }
            },
            State::Doctype => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::BeforeDoctypeName,
                Some('>') => {
                    self.reconsume(Some('>'));
                    self.state = State::BeforeDoctypeName;
                }
                None => {
                    self.doctype = DoctypeInProgress {
                        force_quirks: true,
                        ..DoctypeInProgress::default()
                    };
                    self.emit_doctype();
                    self.emit_eof();
                }
                other => {
                    self.reconsume(other);
                    self.state = State::BeforeDoctypeName;
                }
            },
            State::BeforeDoctypeName => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('>') => {
                    self.doctype = DoctypeInProgress {
                        force_quirks: true,
                        ..DoctypeInProgress::default()
                    };
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.doctype = DoctypeInProgress {
                        force_quirks: true,
                        ..DoctypeInProgress::default()
                    };
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(c) => {
                    self.doctype = DoctypeInProgress::default();
                    let c = if c == '\0' { '\u{fffd}' } else { c.to_ascii_lowercase() };
                    self.doctype.name = Some(c.to_string());
                    self.state = State::DoctypeName;
                }
            },
            State::DoctypeName => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::AfterDoctypeName,
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => self.doctype_eof(),
                Some(c) => {
                    let c = if c == '\0' { '\u{fffd}' } else { c.to_ascii_lowercase() };
                    self.doctype.name.get_or_insert_with(String::new).push(c);
                }
            },
            State::AfterDoctypeName => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => self.doctype_eof(),
                other => {
                    self.reconsume(other);
                    if self.lookahead("PUBLIC", true) {
                        self.pos += 6;
                        self.state = State::AfterDoctypePublicKeyword;
                    } else if self.lookahead("SYSTEM", true) {
                        self.pos += 6;
                        self.state = State::AfterDoctypeSystemKeyword;
                    } else {
                        self.doctype.force_quirks = true;
                        self.state = State::BogusDoctype;
                    }
                }
            },
            State::AfterDoctypePublicKeyword => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::BeforeDoctypePublicIdentifier,
                Some('"') => {
                    self.doctype.public_id = Some(String::new());
                    self.state = State::DoctypePublicIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.public_id = Some(String::new());
                    self.state = State::DoctypePublicIdentifierSingleQuoted;
                }
                other => self.doctype_missing_identifier(other),
            },
            State::BeforeDoctypePublicIdentifier => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('"') => {
                    self.doctype.public_id = Some(String::new());
                    self.state = State::DoctypePublicIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.public_id = Some(String::new());
                    self.state = State::DoctypePublicIdentifierSingleQuoted;
                }
                other => self.doctype_missing_identifier(other),
            },
            State::DoctypePublicIdentifierDoubleQuoted => self.doctype_identifier('"', true),
            State::DoctypePublicIdentifierSingleQuoted => self.doctype_identifier('\'', true),
            State::AfterDoctypePublicIdentifier => match self.consume() {
                Some(c) if is_html_space(c) => {
                    self.state = State::BetweenDoctypePublicAndSystemIdentifiers
                }
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                Some('"') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierSingleQuoted;
                }
                None => self.doctype_eof(),
                other => {
                    self.doctype.force_quirks = true;
                    self.reconsume(other);
                    self.state = State::BogusDoctype;
                }
            },
            State::BetweenDoctypePublicAndSystemIdentifiers => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                Some('"') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierSingleQuoted;
                }
                None => self.doctype_eof(),
                other => {
                    self.doctype.force_quirks = true;
                    self.reconsume(other);
                    self.state = State::BogusDoctype;
                }
            },
            State::AfterDoctypeSystemKeyword => match self.consume() {
                Some(c) if is_html_space(c) => self.state = State::BeforeDoctypeSystemIdentifier,
                Some('"') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierSingleQuoted;
                }
                other => self.doctype_missing_identifier(other),
            },
            State::BeforeDoctypeSystemIdentifier => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('"') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierDoubleQuoted;
                }
                Some('\'') => {
                    self.doctype.system_id = Some(String::new());
                    self.state = State::DoctypeSystemIdentifierSingleQuoted;
                }
                other => self.doctype_missing_identifier(other),
            },
            State::DoctypeSystemIdentifierDoubleQuoted => self.doctype_identifier('"', false),
            State::DoctypeSystemIdentifierSingleQuoted => self.doctype_identifier('\'', false),
            State::AfterDoctypeSystemIdentifier => match self.consume() {
                Some(c) if is_html_space(c) => {}
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => self.doctype_eof(),
                other => {
                    self.reconsume(other);
                    self.state = State::BogusDoctype;
                }
            },
            State::BogusDoctype => match self.consume() {
                Some('>') => {
                    self.state = State::Data;
                    self.emit_doctype();
                }
                None => {
                    self.emit_doctype();
                    self.emit_eof();
                }
                Some(_) => {}
            },
            State::CdataSection => match self.consume() {
                Some(']') => self.state = State::CdataSectionBracket,
                Some(c) => self.emit_char(c),
                None => self.emit_eof(),
            },
            State::CdataSectionBracket => match self.consume() {
                Some(']') => self.state = State::CdataSectionEnd,
                other => {
                    self.emit_char(']');
                    self.reconsume(other);
                    self.state = State::CdataSection;
                }
            },
            State::CdataSectionEnd => match self.consume() {
                Some(']') => self.emit_char(']'),
                Some('>') => self.state = State::Data,
                other => {
                    self.emit_str("]]");
                    self.reconsume(other);
                    self.state = State::CdataSection;
                }
            },
        }
    }

    fn doctype_eof(&mut self) {
        self.doctype.force_quirks = true;
        self.emit_doctype();
        self.emit_eof();
    }

    fn doctype_missing_identifier(&mut self, c: Option<char>) {
        self.doctype.force_quirks = true;
        match c {
            Some('>') => {
                self.state = State::Data;
                self.emit_doctype();
            }
            None => {
                self.emit_doctype();
                self.emit_eof();
            }
            other => {
                self.reconsume(other);
                self.state = State::BogusDoctype;
            }
        }
    }

    fn doctype_identifier(&mut self, quote: char, public: bool) {
        match self.consume() {
            Some(c) if c == quote => {
                self.state = if public {
                    State::AfterDoctypePublicIdentifier
                } else {
                    State::AfterDoctypeSystemIdentifier
                };
            }
            Some('>') => {
                self.doctype.force_quirks = true;
                self.state = State::Data;
                self.emit_doctype();
            }
            None => self.doctype_eof(),
            Some(c) => {
                let c = if c == '\0' { '\u{fffd}' } else { c };
                let target = if public {
                    &mut self.doctype.public_id
                } else {
                    &mut self.doctype.system_id
                };
                target.get_or_insert_with(String::new).push(c);
            }
        }
    }

    fn quoted_attribute_value(&mut self, quote: char) {
        match self.consume() {
            Some(c) if c == quote => self.state = State::AfterAttributeValueQuoted,
            Some('&') => self.attribute_character_reference(),
            Some('\0') => {
                if let Some(value) = self.attr_value() {
                    value.push('\u{fffd}');
                }
            }
            Some(c) => {
                if let Some(value) = self.attr_value() {
                    value.push(c);
                }
            }
            None => self.emit_eof(),
        }
    }

    fn attribute_character_reference(&mut self) {
        let text = self
            .character_reference(true)
            .unwrap_or_else(|| "&".to_string());
        if let Some(value) = self.attr_value() {
            value.push_str(&text);
        }
    }

    /// `<` inside RCDATA or RAWTEXT.
    fn text_less_than(&mut self, text_state: State, end_tag_open: State) {
        match self.consume() {
            Some('/') => {
                self.temporary_buffer.clear();
                self.state = end_tag_open;
            }
            other => {
                self.emit_char('<');
                self.reconsume(other);
                self.state = text_state;
            }
        }
    }

    fn text_end_tag_open(&mut self, text_state: State, end_tag_name: State) {
        match self.consume() {
            Some(c) if c.is_ascii_alphabetic() => {
                self.start_tag(false);
                self.pos -= 1;
                self.state = end_tag_name;
            }
            other => {
                self.emit_str("</");
                self.reconsume(other);
                self.state = text_state;
            }
        }
    }

    /// Only the element's own end tag ends a text element: `</b>` in a `<title>`
    /// is just text.
    fn text_end_tag_name(&mut self, text_state: State) {
        let c = self.consume();
        match c {
            Some(c) if is_html_space(c) && self.is_appropriate_end_tag() => {
                self.state = State::BeforeAttributeName;
            }
            Some('/') if self.is_appropriate_end_tag() => {
                self.state = State::SelfClosingStartTag;
            }
            Some('>') if self.is_appropriate_end_tag() => {
                self.state = State::Data;
                self.emit_tag();
            }
            Some(c) if c.is_ascii_alphabetic() => {
                self.tag.name.push(c.to_ascii_lowercase());
                self.temporary_buffer.push(c);
            }
            other => {
                self.emit_str("</");
                let buffer = std::mem::take(&mut self.temporary_buffer);
                self.emit_str(&buffer);
                self.reconsume(other);
                self.state = text_state;
            }
        }
    }

    fn double_escape_boundary(&mut self, if_script: State, otherwise: State) {
        let c = self.consume();
        match c {
            Some(c) if is_html_space(c) || c == '/' || c == '>' => {
                self.state = if self.temporary_buffer == "script" {
                    if_script
                } else {
                    otherwise
                };
                self.emit_char(c);
            }
            Some(c) if c.is_ascii_alphabetic() => {
                self.temporary_buffer.push(c.to_ascii_lowercase());
                self.emit_char(c);
            }
            other => {
                self.reconsume(other);
                self.state = otherwise;
            }
        }
    }
}

impl Iterator for HtmlTokenizer {
    type Item = HtmlToken;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(token) = self.queue.pop_front() {
                return Some(token);
            }
            if self.eof_emitted {
                return None;
            }
            self.step();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;
    use std::vec;

    fn tokens(html: &str) -> Vec<HtmlToken> {
        HtmlTokenizer::new(html.to_string()).collect()
    }

    fn text_of(html: &str) -> String {
        tokens(html)
            .into_iter()
            .filter_map(|t| match t {
                HtmlToken::Char(c) => Some(c),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_empty() {
        assert_eq!(tokens(""), vec![HtmlToken::Eof]);
    }

    #[test]
    fn test_start_and_end_tag() {
        assert_eq!(
            tokens("<body></body>"),
            vec![
                HtmlToken::StartTag {
                    tag: "body".to_string(),
                    self_closing: false,
                    attributes: Vec::new(),
                },
                HtmlToken::EndTag {
                    tag: "body".to_string(),
                },
                HtmlToken::Eof,
            ]
        );
    }

    #[test]
    fn test_attributes() {
        let attr = Attribute::from_name_value;
        assert_eq!(
            tokens("<p class=\"A\" id='B' foo=bar></p>")[0],
            HtmlToken::StartTag {
                tag: "p".to_string(),
                self_closing: false,
                attributes: vec![attr("class", "A"), attr("id", "B"), attr("foo", "bar")],
            }
        );
    }

    #[test]
    fn a_repeated_attribute_keeps_the_first_value() {
        let attr = Attribute::from_name_value;
        assert_eq!(
            tokens("<p a=1 a=2>")[0],
            HtmlToken::StartTag {
                tag: "p".to_string(),
                self_closing: false,
                attributes: vec![attr("a", "1")],
            }
        );
    }

    #[test]
    fn test_self_closing_tag() {
        assert_eq!(
            tokens("<img />")[0],
            HtmlToken::StartTag {
                tag: "img".to_string(),
                self_closing: true,
                attributes: Vec::new(),
            }
        );
    }

    #[test]
    fn script_content_is_text_once_the_parser_says_so() {
        let mut tokenizer = HtmlTokenizer::new("<script>a<b></script>".to_string());
        assert!(matches!(tokenizer.next(), Some(HtmlToken::StartTag { .. })));
        tokenizer.set_state(State::ScriptData);
        let rest: Vec<HtmlToken> = tokenizer.collect();
        let mut expected: Vec<HtmlToken> = "a<b>".chars().map(HtmlToken::Char).collect();
        expected.push(HtmlToken::EndTag {
            tag: "script".to_string(),
        });
        expected.push(HtmlToken::Eof);
        assert_eq!(rest, expected);
    }

    #[test]
    fn test_bogus_comment_question_mark_is_a_comment() {
        assert_eq!(
            tokens("<?xml version=1?>a"),
            vec![
                HtmlToken::Comment("?xml version=1?".to_string()),
                HtmlToken::Char('a'),
                HtmlToken::Eof,
            ]
        );
    }

    #[test]
    fn comments_and_doctypes_become_tokens() {
        assert_eq!(
            tokens("<!-- hi --><!DOCTYPE html>"),
            vec![
                HtmlToken::Comment(" hi ".to_string()),
                HtmlToken::Doctype {
                    name: Some("html".to_string()),
                    public_id: None,
                    system_id: None,
                    force_quirks: false,
                },
                HtmlToken::Eof,
            ]
        );
    }

    #[test]
    fn named_references_decode_with_and_without_a_semicolon() {
        assert_eq!(text_of("&eacute;&amp;&copy &notin;"), "é&© ∉");
        // No semicolon: the longest legacy name that matches, and the rest is text.
        assert_eq!(text_of("&notit;"), "¬it;");
        assert_eq!(text_of("&bogus;"), "&bogus;");
    }

    #[test]
    fn numeric_references_decode_and_map_windows_1252() {
        assert_eq!(text_of("&#65;&#x41;&#X41"), "AAA");
        assert_eq!(text_of("&#128;"), "€");
        assert_eq!(text_of("&#0;"), "\u{fffd}");
        assert_eq!(text_of("&#x110000;"), "\u{fffd}");
    }

    #[test]
    fn an_attribute_keeps_what_looks_like_a_url_parameter() {
        let attr = Attribute::from_name_value;
        assert_eq!(
            tokens("<a href='?a=1&copy=2&amp;b=3&not'>")[0],
            HtmlToken::StartTag {
                tag: "a".to_string(),
                self_closing: false,
                attributes: vec![attr("href", "?a=1&copy=2&b=3¬")],
            }
        );
    }

    #[test]
    fn newlines_normalize() {
        assert_eq!(text_of("a\r\nb\rc"), "a\nb\nc");
    }
}
