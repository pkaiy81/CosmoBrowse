use std::string::String;
use std::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub enum CssToken {
    /// https://www.w3.org/TR/css-syntax-3/#typedef-hash-token
    HashToken(String),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-delim-token
    Delim(char),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-number-token
    Number(f64),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-dimension-token
    Dimension(f64, String),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-colon-token
    Colon,
    /// https://www.w3.org/TR/css-syntax-3/#typedef-semicolon-token
    SemiColon,
    /// https://www.w3.org/TR/css-syntax-3/#tokendef-open-paren
    OpenParenthesis,
    /// https://www.w3.org/TR/css-syntax-3/#tokendef-close-paren
    CloseParenthesis,
    /// https://www.w3.org/TR/css-syntax-3/#tokendef-open-curly
    OpenCurly,
    /// https://www.w3.org/TR/css-syntax-3/#tokendef-close-curly
    CloseCurly,
    /// https://www.w3.org/TR/css-syntax-3/#typedef-ident-token
    Ident(String),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-string-token
    StringToken(String),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-at-keyword-token
    AtKeyword(String),
    /// https://www.w3.org/TR/css-syntax-3/#typedef-whitespace-token
    /// A run of whitespace (or a comment). Significant only in selectors,
    /// where it is the descendant combinator; skipped everywhere else.
    Whitespace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CssTokenizer {
    pos: usize,
    input: Vec<char>,
}

impl CssTokenizer {
    pub fn new(css: String) -> Self {
        Self {
            pos: 0,
            input: css.chars().collect(),
        }
    }

    // https://www.w3.org/TR/css-syntax-3/#consume-string-token
    fn consume_string_token(&mut self) -> String {
        let mut s = String::new();

        loop {
            if self.pos >= self.input.len() {
                return s;
            }

            self.pos += 1;
            let c = self.input[self.pos];
            match c {
                '"' | '\'' => break,
                _ => s.push(c),
            }
        }

        s
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-number
    /// https://www.w3.org/TR/css-syntax-3/#consume-a-numeric-token
    fn consume_numeric_token(&mut self) -> CssToken {
        let mut num = 0f64;
        let mut floating = false;
        let mut floating_digit = 1f64;

        loop {
            if self.pos >= self.input.len() {
                return CssToken::Number(num);
            }

            let c = self.input[self.pos];

            match c {
                '0'..='9' => {
                    if floating {
                        floating_digit *= 1f64 / 10f64;
                        num += (c.to_digit(10).unwrap() as f64) * floating_digit
                    } else {
                        num = num * 10.0 + (c.to_digit(10).unwrap() as f64);
                    }
                    self.pos += 1;
                }
                '.' => {
                    floating = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }

        if self.pos < self.input.len() {
            let c = self.input[self.pos];
            if c.is_ascii_alphabetic() {
                let unit = self.consume_ident_token();
                return CssToken::Dimension(num, unit);
            }
            // <percentage-token>: represent as a Dimension with unit "%" so
            // consumers can resolve it against the relevant base value.
            // https://www.w3.org/TR/css-syntax-3/#percentage-token-diagram
            if c == '%' {
                self.pos += 1;
                return CssToken::Dimension(num, String::from("%"));
            }
        }

        CssToken::Number(num)
    }

    // https://www.w3.org/TR/css-syntax-3/#consume-ident-like-token
    // https://www.w3.org/TR/css-syntax-3/#consume-name
    fn consume_ident_token(&mut self) -> String {
        let mut s = String::new();
        s.push(self.input[self.pos]);

        loop {
            self.pos += 1;
            if self.pos >= self.input.len() {
                break;
            }
            let c = self.input[self.pos];
            match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => {
                    s.push(c);
                }
                _ => break,
            }
        }

        s
    }
}

impl Iterator for CssTokenizer {
    type Item = CssToken;

    /// https://www.w3.org/TR/css-syntax-3/#consume-token
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.pos >= self.input.len() {
                return None;
            }

            let c = self.input[self.pos];

            let token = match c {
                // Determine next token
                '(' => CssToken::OpenParenthesis,
                ')' => CssToken::CloseParenthesis,
                ',' => CssToken::Delim(','),
                // `.8em` — a leading-dot decimal is a number, not a class
                // dot. (Real CSS uses this constantly; treating it as
                // Delim('.') turned .8em into 8em.)
                '.' if self
                    .input
                    .get(self.pos + 1)
                    .is_some_and(|c| c.is_ascii_digit()) =>
                {
                    let t = self.consume_numeric_token();
                    self.pos -= 1;
                    t
                }
                '.' => CssToken::Delim('.'),
                ':' => CssToken::Colon,
                ';' => CssToken::SemiColon,
                '{' => CssToken::OpenCurly,
                '}' => CssToken::CloseCurly,
                // Whitespace runs collapse into a single <whitespace-token>.
                // Selector parsing needs it to distinguish the descendant
                // combinator `.a .b` from the compound selector `.a.b`; all
                // other consumers skip it. Spec: CSS Syntax §4.3.1.
                ' ' | '\n' | '\t' | '\r' => {
                    while self.pos < self.input.len()
                        && matches!(self.input[self.pos], ' ' | '\n' | '\t' | '\r')
                    {
                        self.pos += 1;
                    }
                    return Some(CssToken::Whitespace);
                }
                // CSS comment `/* ... */` acts as whitespace. Emitting a token
                // (not silently skipping) is essential twice over: a comment
                // before an at-rule (e.g. `/* mobile */ @media { ... }`) must
                // not hide the `@media` from the parser, and `.a/* */.b` must
                // still separate selector tokens like whitespace does.
                '/' if self.input.get(self.pos + 1) == Some(&'*') => {
                    self.pos += 2;
                    while self.pos + 1 < self.input.len()
                        && !(self.input[self.pos] == '*' && self.input[self.pos + 1] == '/')
                    {
                        self.pos += 1;
                    }
                    // Skip past the closing `*/` (or to EOF if unterminated).
                    self.pos = (self.pos + 2).min(self.input.len());
                    return Some(CssToken::Whitespace);
                }
                '"' | '\'' => {
                    let value = self.consume_string_token();
                    CssToken::StringToken(value)
                }
                '0'..='9' => {
                    let t = self.consume_numeric_token();
                    self.pos -= 1;
                    t
                }
                '#' => {
                    // If the character is #ID, we always handle it as ID selector.
                    let value = self.consume_ident_token();
                    self.pos -= 1;
                    CssToken::HashToken(value)
                }
                '-' => {
                    // `-` starting a number (`-16px`, `-.5em`) is a negative
                    // numeric token; otherwise it begins an ident (`-webkit-x`,
                    // custom properties `--x`).
                    if self
                        .input
                        .get(self.pos + 1)
                        .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                    {
                        self.pos += 1; // consume the sign
                        let t = match self.consume_numeric_token() {
                            CssToken::Number(v) => CssToken::Number(-v),
                            CssToken::Dimension(v, unit) => CssToken::Dimension(-v, unit),
                            other => other,
                        };
                        self.pos -= 1;
                        t
                    } else {
                        let t = CssToken::Ident(self.consume_ident_token());
                        self.pos -= 1;
                        t
                    }
                }
                '@' => {
                    // `@` starts an at-keyword when what follows would start an
                    // identifier. Spec: CSS Syntax §4.3.1 — an identifier may
                    // begin with `-` (or `--`), which is how every vendor-
                    // prefixed at-rule is spelled: requiring a letter here made
                    // `@-webkit-keyframes` a delim, so the parser never saw an
                    // at-rule and the block's contents leaked into the sheet.
                    // https://www.w3.org/TR/css-syntax-3/#would-start-an-identifier
                    let ident_start = |c: char| c.is_ascii_alphabetic() || c == '_';
                    let (first, second) = (self.input[self.pos + 1], self.input[self.pos + 2]);
                    let would_start_ident = if first == '-' {
                        ident_start(second) || second == '-'
                    } else {
                        ident_start(first)
                    };
                    if would_start_ident {
                        // skip @
                        self.pos += 1;
                        let t = CssToken::AtKeyword(self.consume_ident_token());
                        self.pos -= 1;
                        t
                    } else {
                        CssToken::Delim('@')
                    }
                }
                'a'..='z' | 'A'..='Z' | '_' => {
                    let t = CssToken::Ident(self.consume_ident_token());
                    self.pos -= 1;
                    t
                }
                _ => {
                    // Unsupported character — emit a delim-token so the parser
                    // can decide whether to skip it.  Crashing the renderer for
                    // a stray CSS char (any non-ASCII punctuation, escapes,
                    // exotic Unicode used by CMS-generated stylesheets, etc.)
                    // is worse than producing a best-effort stream.
                    CssToken::Delim(c)
                }
            };

            self.pos += 1;
            return Some(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;
    use std::vec::Vec;

    /// Tokens excluding whitespace — these tests verify the semantic stream;
    /// whitespace tokens only matter to the selector parser.
    fn semantic_tokens(style: &str) -> Vec<CssToken> {
        CssTokenizer::new(style.to_string())
            .filter(|t| *t != CssToken::Whitespace)
            .collect()
    }

    #[test]
    fn test_empty() {
        let style = "".to_string();
        let mut t = CssTokenizer::new(style);
        assert!(t.next().is_none());
    }

    #[test]
    fn test_one_rule() {
        let expected = [
            CssToken::Ident("p".to_string()),
            CssToken::OpenCurly,
            CssToken::Ident("color".to_string()),
            CssToken::Colon,
            CssToken::Ident("red".to_string()),
            CssToken::SemiColon,
            CssToken::CloseCurly,
        ];
        assert_eq!(semantic_tokens("p { color: red; }"), expected.to_vec());
    }

    #[test]
    fn test_descendant_whitespace_token() {
        // `.a .b` and `.a.b` must tokenize differently: the descendant
        // combinator is represented by a Whitespace token.
        let with_ws: Vec<CssToken> = CssTokenizer::new(".a .b".to_string()).collect();
        let without_ws: Vec<CssToken> = CssTokenizer::new(".a.b".to_string()).collect();
        assert!(with_ws.contains(&CssToken::Whitespace));
        assert!(!without_ws.contains(&CssToken::Whitespace));
    }

    #[test]
    fn test_id_selector() {
        let expected = [
            CssToken::HashToken("#id".to_string()),
            CssToken::OpenCurly,
            CssToken::Ident("color".to_string()),
            CssToken::Colon,
            CssToken::Ident("red".to_string()),
            CssToken::SemiColon,
            CssToken::CloseCurly,
        ];
        assert_eq!(semantic_tokens("#id { color: red; }"), expected.to_vec());
    }

    #[test]
    fn test_class_selector() {
        let expected = [
            CssToken::Delim('.'),
            CssToken::Ident("class".to_string()),
            CssToken::OpenCurly,
            CssToken::Ident("color".to_string()),
            CssToken::Colon,
            CssToken::Ident("red".to_string()),
            CssToken::SemiColon,
            CssToken::CloseCurly,
        ];
        assert_eq!(semantic_tokens(".class { color: red; }"), expected.to_vec());
    }

    #[test]
    fn test_multiple_rules() {
        let expected = [
            CssToken::Ident("p".to_string()),
            CssToken::OpenCurly,
            CssToken::Ident("content".to_string()),
            CssToken::Colon,
            CssToken::StringToken("Hey".to_string()),
            CssToken::SemiColon,
            CssToken::CloseCurly,
            CssToken::Ident("h1".to_string()),
            CssToken::OpenCurly,
            CssToken::Ident("font-size".to_string()),
            CssToken::Colon,
            CssToken::Number(40.0),
            CssToken::SemiColon,
            CssToken::Ident("color".to_string()),
            CssToken::Colon,
            CssToken::Ident("blue".to_string()),
            CssToken::SemiColon,
            CssToken::CloseCurly,
        ];
        assert_eq!(
            semantic_tokens("p { content: \"Hey\"; } h1 { font-size: 40; color: blue; }"),
            expected.to_vec()
        );
    }

    #[test]
    fn test_dimension_tokens() {
        let expected = [
            CssToken::Ident("body".to_string()),
            CssToken::OpenCurly,
            CssToken::Ident("width".to_string()),
            CssToken::Colon,
            CssToken::Dimension(60.0, "vw".to_string()),
            CssToken::SemiColon,
            CssToken::Ident("font-size".to_string()),
            CssToken::Colon,
            CssToken::Dimension(1.5, "em".to_string()),
            CssToken::SemiColon,
            CssToken::CloseCurly,
        ];
        assert_eq!(
            semantic_tokens("body { width: 60vw; font-size: 1.5em; }"),
            expected.to_vec()
        );
    }

    #[test]
    fn test_leading_dot_decimal_is_a_number() {
        let mut t = CssTokenizer::new("padding:.8em".to_string());
        let mut toks = Vec::new();
        while let Some(tok) = t.next() {
            toks.push(tok);
        }
        assert!(
            toks.contains(&CssToken::Dimension(0.8, "em".to_string())),
            "{:?}",
            toks
        );
    }
}
