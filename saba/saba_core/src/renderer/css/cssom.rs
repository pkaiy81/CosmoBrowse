use crate::alloc::string::ToString;
use crate::renderer::css::token::CssToken;
use crate::renderer::css::token::CssTokenizer;
use alloc::string::String;
use alloc::vec::Vec;
use core::iter::Peekable;

#[derive(Debug, Clone)]
pub struct CssParser {
    t: Peekable<CssTokenizer>,
}

impl CssParser {
    pub fn new(t: CssTokenizer) -> Self {
        Self { t: t.peekable() }
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-component-value
    fn consume_component_value(&mut self) -> ComponentValue {
        self.t
            .next()
            .expect("should have a token in consume_component_value")
    }

    fn consume_ident(&mut self) -> String {
        let token = match self.t.next() {
            Some(t) => t,
            None => panic!("should have a token but got None"),
        };

        match token {
            CssToken::Ident(ref ident) => ident.to_string(),
            _ => {
                panic!("Parse error: {:?} is an unexpected token.", token);
            }
        }
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-a-declaration
    fn consume_declaration(&mut self) -> Option<Declaration> {
        if self.t.peek().is_none() {
            return None;
        }

        // Initialize the declaration structure
        let mut declaration = Declaration::new();
        // Set the identifier to the property of Declaration structure
        declaration.set_property(self.consume_ident());
        // If the next token is not a colon, it is a parse error.
        // Return None
        match self.t.next() {
            Some(token) => match token {
                CssToken::Colon => {}
                _ => return None,
            },
            None => return None,
        }

        // Set the value to the value of the Declaration structure
        declaration.set_value(self.consume_component_value());

        Some(declaration)
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-a-list-of-declarations
    fn consume_list_of_declarations(&mut self) -> Vec<Declaration> {
        let mut declarations = Vec::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t,
                None => return declarations,
            };

            match token {
                CssToken::CloseCurly => {
                    assert_eq!(self.t.next(), Some(CssToken::CloseCurly));
                    return declarations;
                }
                CssToken::SemiColon => {
                    assert_eq!(self.t.next(), Some(CssToken::SemiColon));
                    // One declaration ends. Action is not required.
                }
                CssToken::Ident(ref _ident) => {
                    if let Some(declaration) = self.consume_declaration() {
                        declarations.push(declaration);
                    }
                }
                _ => {
                    self.t.next();
                }
            }
        }
    }

    fn consume_selector(&mut self) -> Selector {
        let token = match self.t.next() {
            Some(t) => t,
            None => panic!("should have a token but got None"),
        };

        match token {
            CssToken::HashToken(value) => Selector::IdSelector(value[1..].to_string()),
            CssToken::Delim(delim) => {
                if delim == '.' {
                    return Selector::ClassSelector(self.consume_ident());
                }
                panic!("Parse error: {:?} is an unexpected token.", token);
            }
            CssToken::Ident(ident) => {
                // Selectors like a:hover are treated as tag name selectors,
                // so if a colon (:) appears, advance the token until just
                // before the start of the declaration block
                if self.t.peek() == Some(&CssToken::Colon) {
                    while self.t.peek() != Some(&CssToken::OpenCurly) {
                        self.t.next();
                    }
                }
                Selector::TypeSelector(ident.to_string())
            }
            CssToken::AtKeyword(_keyword) => {
                // Advance the token to ignore the rules starting with @ just before the declarative block.
                while self.t.peek() != Some(&CssToken::OpenCurly) {
                    self.t.next();
                }
                Selector::UnknownSelector
            }
            _ => {
                self.t.next();
                Selector::UnknownSelector
            }
        }
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-qualified-rule
    /// https://www.w3.org/TR/css-syntax-3/#qualified-rule
    /// https://www.w3.org/TR/css-syntax-3/#style-rules
    fn consume_qualified_rule(&mut self) -> Option<QualifiedRule> {
        let mut rule = QualifiedRule::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t,
                None => return None,
            };

            match token {
                CssToken::OpenCurly => {
                    assert_eq!(self.t.next(), Some(CssToken::OpenCurly));
                    rule.set_declarations(self.consume_list_of_declarations());
                    return Some(rule);
                }
                _ => {
                    rule.set_selector(self.consume_selector());
                }
            }
        }
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-a-list-of-rules
    fn consume_list_of_rules(&mut self) -> Vec<QualifiedRule> {
        // create a empty vector
        let mut rules = Vec::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t,
                None => return rules,
            };
            match token {
                // If an AtKeyword token appears, it indicates the start of a rule, such as @import for other CSS imports, @media for media queries, etc.
                CssToken::AtKeyword(_keyword) => {
                    let _rule = self.consume_qualified_rule();
                    // Ignore the rule for now because we are not supporting @import, @media, etc.
                }
                _ => {
                    // resolve one rule and add vector
                    let rule = self.consume_qualified_rule();
                    match rule {
                        Some(r) => rules.push(r),
                        None => return rules,
                    }
                }
            }
        }
    }

    /// https://www.w3.org/TR/css-syntax-3/#parse-stylesheet
    pub fn parse_stylesheet(&mut self) -> StyleSheet {
        // Create a new CSSStyleSheet instance
        let mut sheet = StyleSheet::new();

        // Create list of rules from the token stream and assign it to the CSSStyleSheet field
        sheet.set_rules(self.consume_list_of_rules());
        sheet
    }
}

// 6.1.2 The CSSStyleSheet Interface
// https://www.w3.org/TR/cssom-1/#cssstylesheet
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSheet {
    /// https://drafts.csswg.org/cssom/#dom-cssstylesheet-cssrules
    pub rules: Vec<QualifiedRule>,
}

impl StyleSheet {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn set_rules(&mut self, rules: Vec<QualifiedRule>) {
        self.rules = rules;
    }
}

// qualified rule
/// https://www.w3.org/TR/css-syntax-3/#qualified-rule
#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedRule {
    /// https://www.w3.org/TR/selectors-4/#typedef-selector-list
    /// The prelude of the qualified rule is parsed as a <selector-list>.
    pub selector: Selector,
    /// https://www.w3.org/TR/css-syntax-3/#parse-a-list-of-declarations
    /// The content of the qualified rule’s block is parsed as a list of declarations.
    pub declarations: Vec<Declaration>,
}

impl QualifiedRule {
    pub fn new() -> Self {
        Self {
            selector: Selector::TypeSelector("".to_string()),
            declarations: Vec::new(),
        }
    }

    pub fn set_selector(&mut self, selector: Selector) {
        self.selector = selector;
    }

    pub fn set_declarations(&mut self, declarations: Vec<Declaration>) {
        self.declarations = declarations;
    }
}

/// https://www.w3.org/TR/selectors-4/
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// https://www.w3.org/TR/selectors-4/#type-selectors
    TypeSelector(String),
    /// https://www.w3.org/TR/selectors-4/#class-html
    ClassSelector(String),
    /// https://www.w3.org/TR/selectors-4/#id-selectors
    IdSelector(String),
    /// パース中にエラーが起こったときに使用されるセレクタ
    UnknownSelector,
}

/// https://www.w3.org/TR/css-syntax-3/#declaration
#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: ComponentValue,
}

impl Declaration {
    pub fn new() -> Self {
        Self {
            property: String::new(),
            value: ComponentValue::Ident(String::new()),
        }
    }

    pub fn set_property(&mut self, property: String) {
        self.property = property;
    }

    pub fn set_value(&mut self, value: ComponentValue) {
        self.value = value;
    }
}

pub type ComponentValue = CssToken;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_empty() {
        let style = "".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        assert_eq!(cssom.rules.len(), 0);
    }

    #[test]
    fn test_one_rule() {
        let style = "p { color: red; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule = QualifiedRule::new();
        rule.set_selector(Selector::TypeSelector("p".to_string()));
        let mut declaration = Declaration::new();
        declaration.set_property("color".to_string());
        declaration.set_value(ComponentValue::Ident("red".to_string()));
        rule.set_declarations(vec![declaration]);

        let expected = [rule];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_id_selector() {
        let style = "#id { color: red; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule = QualifiedRule::new();
        rule.set_selector(Selector::IdSelector("id".to_string()));
        let mut declaration = Declaration::new();
        declaration.set_property("color".to_string());
        declaration.set_value(ComponentValue::Ident("red".to_string()));
        rule.set_declarations(vec![declaration]);

        let expected = [rule];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_class_selector() {
        let style = ".class { color: red; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule = QualifiedRule::new();
        rule.set_selector(Selector::ClassSelector("class".to_string()));
        let mut declaration = Declaration::new();
        declaration.set_property("color".to_string());
        declaration.set_value(ComponentValue::Ident("red".to_string()));
        rule.set_declarations(vec![declaration]);

        let expected = [rule];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_multiple_rules() {
        let style = "p { content: \"Hey\"; } h1 { font-size: 40; color: blue; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule1 = QualifiedRule::new();
        rule1.set_selector(Selector::TypeSelector("p".to_string()));
        let mut declaration1 = Declaration::new();
        declaration1.set_property("content".to_string());
        declaration1.set_value(ComponentValue::StringToken("Hey".to_string()));
        rule1.set_declarations(vec![declaration1]);

        let mut rule2 = QualifiedRule::new();
        rule2.set_selector(Selector::TypeSelector("h1".to_string()));
        let mut declaration2 = Declaration::new();
        declaration2.set_property("font-size".to_string());
        declaration2.set_value(ComponentValue::Number(40.0));
        let mut declaration3 = Declaration::new();
        declaration3.set_property("color".to_string());
        declaration3.set_value(ComponentValue::Ident("blue".to_string()));
        rule2.set_declarations(vec![declaration2, declaration3]);

        let expected = [rule1, rule2];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }
}
