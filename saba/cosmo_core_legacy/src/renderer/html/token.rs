use crate::renderer::html::attribute::Attribute;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlToken {
    // Start tag token
    StartTag {
        tag: String,
        self_closing: bool,
        attributes: Vec<Attribute>,
    },
    // End tag token
    EndTag {
        tag: String,
    },
    // String token
    Char(char),
    // End of file
    Eof,
}

// 13.2.5 Tokenization
// There are 80 states in the HTML specification.
// But I will implement only the states that are necessary for the rendering engine.
// TemporaryBuffer is not a state, but it is used in the specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// https://html.spec.whatwg.org/multipage/parsing.html#data-state
    Data,
    /// https://html.spec.whatwg.org/multipage/parsing.html#tag-open-state
    TagOpen,
    /// https://html.spec.whatwg.org/multipage/parsing.html#end-tag-open-state
    EndTagOpen,
    /// https://html.spec.whatwg.org/multipage/parsing.html#tag-name-state
    TagName,
    /// https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-name-state
    BeforeAttributeName,
    /// https://html.spec.whatwg.org/multipage/parsing.html#attribute-name-state
    AttributeName,
    /// https://html.spec.whatwg.org/multipage/parsing.html#after-attribute-name-state
    AfterAttributeName,
    /// https://html.spec.whatwg.org/multipage/parsing.html#before-attribute-value-state
    BeforeAttributeValue,
    /// https://html.spec.whatwg.org/multipage/parsing.html#attribute-value-(double-quoted)-state
    AttributeValueDoubleQuoted,
    /// https://html.spec.whatwg.org/multipage/parsing.html#attribute-value-(single-quoted)-state
    AttributeValueSingleQuoted,
    /// https://html.spec.whatwg.org/multipage/parsing.html#attribute-value-(unquoted)-state
    AttributeValueUnquoted,
    /// https://html.spec.whatwg.org/multipage/parsing.html#after-attribute-value-(quoted)-state
    AfterAttributeValueQuoted,
    /// https://html.spec.whatwg.org/multipage/parsing.html#self-closing-start-tag-state
    SelfClosingStartTag,
    /// https://html.spec.whatwg.org/multipage/parsing.html#script-data-state
    ScriptData,
    /// https://html.spec.whatwg.org/multipage/parsing.html#script-data-less-than-sign-state
    ScriptDataLessThanSign,
    /// https://html.spec.whatwg.org/multipage/parsing.html#script-data-end-tag-open-state
    ScriptDataEndTagOpen,
    /// https://html.spec.whatwg.org/multipage/parsing.html#script-data-end-tag-name-state
    ScriptDataEndTagName,
    /// https://html.spec.whatwg.org/multipage/parsing.html#temporary-buffer
    TemporaryBuffer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtmlTokenizer {
    state: State,
    pos: usize,
    reconsume: bool,
    latest_token: Option<HtmlToken>,
    input: Vec<char>,
    buf: String,
}

impl HtmlTokenizer {
    pub fn new(html: String) -> Self {
        Self {
            state: State::Data,
            pos: 0,
            reconsume: false,
            latest_token: None,
            input: html.chars().collect(),
            buf: String::new(),
        }
    }

    // Check if the current position is the end of the file
    fn is_eof(&self) -> bool {
        self.pos > self.input.len()
    }

    // Current position moved to the next character
    fn consume_next_input(&mut self) -> char {
        let c = self.input[self.pos];
        self.pos += 1;
        c
    }

    // Create a tag
    // If start_tag_token is true, create a StartTag token
    // Otherwise, create an EndTag token
    fn create_tag(&mut self, start_tag_token: bool) {
        if start_tag_token {
            self.latest_token = Some(HtmlToken::StartTag {
                tag: String::new(),
                self_closing: false,
                attributes: Vec::new(),
            });
        } else {
            self.latest_token = Some(HtmlToken::EndTag { tag: String::new() })
        }
    }

    // Set reconsume flag to false
    // Return the character before the current position
    fn reconsume_input(&mut self) -> char {
        self.reconsume = false;
        self.input[self.pos - 1]
    }

    // Append the character to the tag name in the latest_token created by create_tag
    fn append_tag_name(&mut self, c: char) {
        assert!(self.latest_token.is_some());

        if let Some(t) = self.latest_token.as_mut() {
            match t {
                HtmlToken::StartTag {
                    ref mut tag,
                    self_closing: _, // Ignore self_closing
                    attributes: _,   // Ignore attributes
                }
                | HtmlToken::EndTag { ref mut tag } => tag.push(c),
                _ => panic!("`latest_token` should be either StartTag or EndTag"),
            }
        }
    }

    // Take the latest_token and return it
    fn take_latest_token(&mut self) -> Option<HtmlToken> {
        assert!(self.latest_token.is_some());

        let t = self.latest_token.as_ref().cloned();
        self.latest_token = None;
        assert!(self.latest_token.is_none());

        t
    }

    // Start a new attribute
    // Create a new Attribute and append it to the latest_token
    fn start_new_attribute(&mut self) {
        assert!(self.latest_token.is_some());

        if let Some(t) = self.latest_token.as_mut() {
            match t {
                HtmlToken::StartTag {
                    tag: _,
                    self_closing: _, // Ignore self_closing
                    ref mut attributes,
                } => {
                    attributes.push(Attribute::new());
                }
                _ => panic!("`latest_token` should be either StartTag"),
            }
        }
    }

    // Append the attribute to the latest_token created by create_tag
    fn append_attribute(&mut self, c: char, is_name: bool) {
        assert!(self.latest_token.is_some());

        if let Some(t) = self.latest_token.as_mut() {
            match t {
                HtmlToken::StartTag {
                    tag: _,
                    self_closing: _, // Ignore self_closing
                    ref mut attributes,
                } => {
                    let len = attributes.len();
                    assert!(len > 0);

                    attributes[len - 1].add_char(c, is_name);
                }
                _ => panic!("`latest_token` should be either StartTag"),
            }
        }
    }

    // Set the self_closing flag to true when the latest_token is a StartTag
    fn set_self_closing_flag(&mut self) {
        assert!(self.latest_token.is_some());

        if let Some(t) = self.latest_token.as_mut() {
            match t {
                HtmlToken::StartTag {
                    tag: _,
                    ref mut self_closing,
                    attributes: _,
                } => *self_closing = true,
                _ => panic!("`latest_token` should be a StartTag"),
            }
        }
    }
}

// Implement the Iterator trait for HtmlTokenizer
// https://doc.rust-lang.org/std/iter/trait.Iterator.html
impl Iterator for HtmlTokenizer {
    type Item = HtmlToken;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.input.len() {
            return None;
        }

        loop {
            let c = match self.reconsume {
                true => self.reconsume_input(),
                false => self.consume_next_input(),
            };

            // Implement the states here
            match self.state {
                State::Data => {
                    // If character is '<', switch to TagOpen state
                    // Also, if character is EOF, return Eof token
                    // Otherwise, return Char token
                    if c == '<' {
                        self.state = State::TagOpen;
                        continue;
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    return Some(HtmlToken::Char(c));
                }

                State::TagOpen => {
                    // If character is '/', switch to EndTagOpen state
                    // If character is an ASCII letter, switch to TagName state
                    // Otherwise, return Char token
                    if c == '/' {
                        self.state = State::EndTagOpen;
                        continue;
                    }

                    if c.is_ascii_alphabetic() {
                        self.reconsume = true;
                        self.state = State::TagName;
                        self.create_tag(true);
                        continue;
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.reconsume = true;
                    self.state = State::Data;
                }

                // EndTagOpen state handles end tags.
                // If character achieves the end of file, return Eof token.
                // If character is an ASCII letter, switch to TagName state
                State::EndTagOpen => {
                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    if c.is_ascii_alphabetic() {
                        self.reconsume = true;
                        self.state = State::TagName;
                        self.create_tag(false);
                        continue;
                    }
                }

                // TagName state handles tag names.
                // 1. If character is a whitespace character, switch to BeforeAttributeName state
                // 2. If character is '/', switch to SelfClosingStartTag state
                // 3. If character is '>', switch to Data state and return the latest_token created by create_tag
                // 4. If character is an ASCII alpha, append the character to the tag name
                // 5. IF character achieves the end of file, return Eof token
                // 6. Otherwise, append the character to the tag name
                State::TagName => {
                    if c == ' ' {
                        self.state = State::BeforeAttributeName;
                        continue;
                    }

                    if c == '/' {
                        self.state = State::SelfClosingStartTag;
                        continue;
                    }

                    if c == '>' {
                        self.state = State::Data;
                        return self.take_latest_token();
                    }

                    if c.is_ascii_uppercase() {
                        self.append_tag_name(c.to_ascii_lowercase());
                        continue;
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.append_tag_name(c);
                }

                // BeforeAttributeName state handles attributes.
                // 1. If character is '/' or '>' or EOF, set the reconsume flag to true and switch to AfterAttributeName state.
                // 2. Otherwise, switch to AttributeName state and call start_new_attribute method.
                State::BeforeAttributeName => {
                    if c == '/' || c == '>' || self.is_eof() {
                        self.reconsume = true;
                        self.state = State::AfterAttributeName;
                        continue;
                    }

                    self.reconsume = true;
                    self.state = State::AttributeName;
                    self.start_new_attribute();
                }

                // AttributeName state handles attribute names.
                // 1. If character is a whitespace, '/', '>', or EOF, switch to AfterAttributeName state.
                //    Then, set the reconsume flag to true.
                // 2. If character is '=', switch to BeforeAttributeValue state.
                // 3. Otherwise, call append_attribute method.
                State::AttributeName => {
                    if c == ' ' || c == '/' || c == '>' || self.is_eof() {
                        self.reconsume = true;
                        self.state = State::AfterAttributeName;
                        continue;
                    }

                    if c == '=' {
                        self.state = State::BeforeAttributeValue;
                        continue;
                    }

                    if c.is_ascii_uppercase() {
                        self.append_attribute(c.to_ascii_lowercase(), /* is_name = */ true);
                        continue;
                    }

                    self.append_attribute(c, /* is_name = */ true);
                }

                // AfterAttributeName state handles attributes.
                // 1. If character is '/', switch to SelfClosingStartTag state.
                // 2. If character is '=', switch to BeforeAttributeValue state.
                // 3. If character is '>', switch to Data state and return the latest_token.
                // 4. If character is EOF, return Eof token.
                // 5. Otherwise, set the reconsume flag to true and switch to AttributeName state.
                //    Then, call start_new_attribute method.
                State::AfterAttributeName => {
                    if c == ' ' {
                        // Ignore whitespace
                        continue;
                    }

                    if c == '/' {
                        self.state = State::SelfClosingStartTag;
                        continue;
                    }

                    if c == '=' {
                        self.state = State::BeforeAttributeValue;
                        continue;
                    }

                    if c == '>' {
                        self.state = State::Data;
                        return self.take_latest_token();
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.reconsume = true;
                    self.state = State::AttributeName;
                    self.start_new_attribute();
                }

                // BeforeAttributeValue state handles attribute values.
                // 1. If character is '"', switch to AttributeValueDoubleQuoted state.
                // 2. If character is "'", switch to AttributeValueSingleQuoted state.
                // 3. Otherwise, set the reconsume flag to true and switch to AttributeValueUnquoted state.
                State::BeforeAttributeValue => {
                    if c == ' ' {
                        // Ignore whitespace
                        continue;
                    }

                    if c == '"' {
                        self.state = State::AttributeValueDoubleQuoted;
                        continue;
                    }

                    if c == '\'' {
                        self.state = State::AttributeValueSingleQuoted;
                        continue;
                    }

                    self.reconsume = true;
                    self.state = State::AttributeValueUnquoted;
                }

                // AttributeValueDoubleQuoted state handles attribute values.
                // 1. If character is '"', switch to AfterAttributeValueQuoted state.
                // 2. If character is EOF, return Eof token.
                // 3. Otherwise, call append_attribute method.
                State::AttributeValueDoubleQuoted => {
                    if c == '"' {
                        self.state = State::AfterAttributeValueQuoted;
                        continue;
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.append_attribute(c, /* is_name = */ false);
                }

                // AttributeValueSingleQuoted state handles attribute values.
                // 1. If character is "'", switch to AfterAttributeValueQuoted state.
                // 2. If character is EOF, return Eof token.
                // 3. Otherwise, call append_attribute method.
                State::AttributeValueSingleQuoted => {
                    if c == '\'' {
                        self.state = State::AfterAttributeValueQuoted;
                        continue;
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.append_attribute(c, /* is_name = */ false);
                }

                // AttributeValueUnquoted state handles attribute values.
                // 1. If character is a whitespace, switch to BeforeAttributeName state.
                // 2. If character is '>', switch to Data state and return the latest_token.
                // 3. If character is EOF, return Eof token.
                // 4. Otherwise, call append_attribute method.
                State::AttributeValueUnquoted => {
                    if c == ' ' {
                        self.state = State::BeforeAttributeName;
                        continue;
                    }

                    if c == '>' {
                        self.state = State::Data;
                        return self.take_latest_token();
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.append_attribute(c, /* is_name = */ false);
                }

                // AfterAttributeValueQuoted state handles attribute values.
                // 1. If character is a whitespace, switch to BeforeAttributeName state.
                // 2. If character is '/', switch to SelfClosingStartTag state.
                // 3. If character is '>', switch to Data state and return the latest_token.
                // 4. If character is EOF, return Eof token.
                // 5. Otherwise, set the reconsume flag to true and switch to BeforeAttributeName state.
                State::AfterAttributeValueQuoted => {
                    if c == ' ' {
                        self.state = State::BeforeAttributeName;
                        continue;
                    }

                    if c == '/' {
                        self.state = State::SelfClosingStartTag;
                        continue;
                    }

                    if c == '>' {
                        self.state = State::Data;
                        return self.take_latest_token();
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    self.reconsume = true;
                    self.state = State::BeforeAttributeName;
                }

                // SelfClosingStartTag state handles self-closing tags.
                // 1. If character is '>', call set_self_closing_flag method and switch to Data state.
                //    Then, return the latest_token.
                // 2. If character is EOF, return Eof token.
                State::SelfClosingStartTag => {
                    if c == '>' {
                        self.set_self_closing_flag();
                        self.state = State::Data;
                        return self.take_latest_token();
                    }

                    if self.is_eof() {
                        // invalid parse error
                        return Some(HtmlToken::Eof);
                    }
                }

                // ScriptData state handles script data.
                // 1. If character is '<', switch to ScriptDataLessThanSign state.
                // 2. If character is EOF, return Eof token.
                // 3. Otherwise, return Char token.
                State::ScriptData => {
                    if c == '<' {
                        self.state = State::ScriptDataLessThanSign;
                        continue;
                    }

                    if self.is_eof() {
                        return Some(HtmlToken::Eof);
                    }

                    return Some(HtmlToken::Char(c));
                }

                // ScriptDataLessThanSign state handles script data.
                // This state decides if the character is '</script' or simply literal characters.
                // 1. If character is '/', reset the temporary buffer and switch to ScriptDataEndTagOpen state.
                // 2. Otherwise, set reconsume flag to true and switch to ScriptData state. Then, return Char token.
                State::ScriptDataLessThanSign => {
                    if c == '/' {
                        // Reset the temporary buffer with an empty string
                        self.buf = String::new();
                        self.state = State::ScriptDataEndTagOpen;
                        continue;
                    }

                    self.reconsume = true;
                    self.state = State::ScriptData;
                    return Some(HtmlToken::Char('<'));
                }

                // ScriptDataEndTagOpen state handles script data.
                // 1. If character is an alpabetic character, set reconsume flag to true and switch to ScriptDataEndTagName state.
                //    Then, call create_tag method to create an EndTag.
                // 2. Otherwise, set reconsume flag to true and switch to ScriptData state. Then, return Char token.
                State::ScriptDataEndTagOpen => {
                    if c.is_ascii_alphabetic() {
                        self.reconsume = true;
                        self.state = State::ScriptDataEndTagName;
                        self.create_tag(false);
                        continue;
                    }

                    self.reconsume = true;
                    self.state = State::ScriptData;
                    // The specification says to return '<' and '/' characters.
                    // But I will return '<' character only.
                    return Some(HtmlToken::Char('<'));
                }

                // ScriptDataEndTagName state handles script data.
                // 1. If character is '>', switch to ScriptData state. Then, return the latest_token.
                // 2. If character is an alpabetic character, set reconsume flag to true and append the character to the temporary buffer.
                //    Then, call append_tag_name method.
                // 3. Otherwise, switch to TemporaryBuffer state and append '</' and current character to the temporary buffer.
                State::ScriptDataEndTagName => {
                    if c == '>' {
                        self.state = State::Data;
                        return self.take_latest_token();
                    }

                    if c.is_ascii_alphabetic() {
                        self.buf.push(c);
                        self.append_tag_name(c.to_ascii_lowercase());
                        continue;
                    }

                    self.state = State::TemporaryBuffer;
                    self.buf = String::from("</") + &self.buf;
                    self.buf.push(c);
                    continue;
                }

                // TemporaryBuffer state handles script data.
                // This state is not in the specification.
                State::TemporaryBuffer => {
                    self.reconsume = true;

                    if self.buf.chars().count() == 0 {
                        self.state = State::ScriptData;
                        continue;
                    }

                    // Delete the first character
                    let c = self
                        .buf
                        .chars()
                        .nth(0)
                        .expect("self.buf should have at least 1 character");
                    self.buf.remove(0);
                    return Some(HtmlToken::Char(c));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc::string::ToString;
    use alloc::vec;

    #[test]
    fn test_empty() {
        let html = "".to_string();
        let mut tokenizer = HtmlTokenizer::new(html);
        assert!(tokenizer.next().is_none());
    }

    #[test]
    fn test_start_and_end_tag() {
        let html = "<body></body>".to_string();
        let mut tokenizer = HtmlTokenizer::new(html);
        let expected = [
            HtmlToken::StartTag {
                tag: "body".to_string(),
                self_closing: false,
                attributes: Vec::new(),
            },
            HtmlToken::EndTag {
                tag: "body".to_string(),
            },
        ];
        for e in expected {
            assert_eq!(Some(e), tokenizer.next());
        }
    }

    #[test]
    fn test_attributes() {
        let html = "<p class=\"A\" id='B' foo=bar></p>".to_string();
        let mut tokenizer = HtmlTokenizer::new(html);
        let mut attr1 = Attribute::new();
        attr1.add_char('c', true);
        attr1.add_char('l', true);
        attr1.add_char('a', true);
        attr1.add_char('s', true);
        attr1.add_char('s', true);
        attr1.add_char('A', false);

        let mut attr2 = Attribute::new();
        attr2.add_char('i', true);
        attr2.add_char('d', true);
        attr2.add_char('B', false);

        let mut attr3 = Attribute::new();
        attr3.add_char('f', true);
        attr3.add_char('o', true);
        attr3.add_char('o', true);
        attr3.add_char('b', false);
        attr3.add_char('a', false);
        attr3.add_char('r', false);

        let expected = [
            HtmlToken::StartTag {
                tag: "p".to_string(),
                self_closing: false,
                attributes: vec![attr1, attr2, attr3],
            },
            HtmlToken::EndTag {
                tag: "p".to_string(),
            },
        ];
        for e in expected {
            assert_eq!(Some(e), tokenizer.next());
        }
    }

    #[test]
    fn test_self_closing_tag() {
        let html = "<img />".to_string();
        let mut tokenizer = HtmlTokenizer::new(html);
        let expected = [HtmlToken::StartTag {
            tag: "img".to_string(),
            self_closing: true,
            attributes: Vec::new(),
        }];
        for e in expected {
            assert_eq!(Some(e), tokenizer.next());
        }
    }

    #[test]
    fn test_script_tag() {
        let html = "<script>js code;</script>".to_string();
        let mut tokenizer = HtmlTokenizer::new(html);
        let expected = [
            HtmlToken::StartTag {
                tag: "script".to_string(),
                self_closing: false,
                attributes: Vec::new(),
            },
            HtmlToken::Char('j'),
            HtmlToken::Char('s'),
            HtmlToken::Char(' '),
            HtmlToken::Char('c'),
            HtmlToken::Char('o'),
            HtmlToken::Char('d'),
            HtmlToken::Char('e'),
            HtmlToken::Char(';'),
            HtmlToken::EndTag {
                tag: "script".to_string(),
            },
        ];
        for e in expected {
            assert_eq!(Some(e), tokenizer.next());
        }
    }
}
