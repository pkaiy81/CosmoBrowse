use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

static RESERVED_WORDS: [&str; 3] = ["var", "function", "return"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// https://262.ecma-international.org/#sec-punctuators
    Punctuator(char),
    /// https://262.ecma-international.org/#sec-literals-numeric-literals
    Number(u64),
    /// https://262.ecma-international.org/#sec-identifier-names
    Identifier(String),
    /// https://262.ecma-international.org/#sec-keywords-and-reserved-words
    Keyword(String),
    /// https://262.ecma-international.org/#sec-literals-string-literals
    StringLiteral(String),
}

pub struct JsLexer {
    pos: usize,
    input: Vec<char>,
}

impl JsLexer {
    pub fn new(js: String) -> Self {
        Self {
            pos: 0,
            input: js.chars().collect(),
        }
    }

    fn cosume_number(&mut self) -> u64 {
        let mut num = 0;

        loop {
            if self.pos >= self.input.len() {
                return num;
            }

            let c = self.input[self.pos];

            match c {
                '0'..='9' => {
                    num = num * 10 + (c.to_digit(10).unwrap() as u64);
                    self.pos += 1;
                }
                _ => break,
            }
        }

        return num;
    }

    fn consume_identifier(&mut self) -> String {
        let mut result = String::new();

        loop {
            if self.pos >= self.input.len() {
                return result;
            }

            if self.input[self.pos].is_ascii_alphanumeric() || self.input[self.pos] == '$' {
                result.push(self.input[self.pos]);
                self.pos += 1;
            } else {
                return result;
            }
        }
    }

    fn consume_string(&mut self) -> String {
        let mut result = String::new();
        self.pos += 1;

        loop {
            if self.pos >= self.input.len() {
                return result;
            }

            if self.input[self.pos] == '"' {
                // Start of string
                self.pos += 1;
                return result;
            }

            result.push(self.input[self.pos]);
            self.pos += 1;
        }
    }

    fn contains(&self, keyword: &str) -> bool {
        for i in 0..keyword.len() {
            if keyword
                .chars()
                .nth(i)
                .expect("failed to access to i-th char")
                != self.input[self.pos + i]
            {
                return false;
            }
        }

        true
    }

    fn check_reserved_word(&self) -> Option<String> {
        for word in RESERVED_WORDS {
            if self.contains(word) {
                return Some(word.to_string());
            }
        }

        None
    }
}

impl Iterator for JsLexer {
    type Item = Token;

    fn next(&mut self) -> Option<Self::Item> {
        // Skip ASCII whitespace and any character this minimal tokenizer
        // does not understand. Real-world pages (especially Wix/CMS-built
        // sites) ship JavaScript that uses dozens of operators (`!`, `<`,
        // `>`, `*`, `/`, `:`, `?`, `[`, `]`, `&`, `|`, `'`, etc.) that the
        // engine cannot produce tokens for. Rather than panicking, skip
        // those bytes so the parser sees a (possibly partial) stream of
        // tokens for the constructs we *do* support, plus EOF when the
        // unknown tail is exhausted. We use a loop (not recursion) to
        // avoid stack overflow on inputs with thousands of unsupported
        // characters in a row.
        loop {
            if self.pos >= self.input.len() {
                return None;
            }
            let c = self.input[self.pos];

            // Whitespace / newline.
            if c == ' ' || c == '\n' || c == '\r' || c == '\t' {
                self.pos += 1;
                continue;
            }

            // Reserved word?
            if let Some(keyword) = self.check_reserved_word() {
                self.pos += keyword.len();
                return Some(Token::Keyword(keyword));
            }

            return Some(match c {
                '+' | '-' | ';' | '=' | '(' | ')' | '{' | '}' | ',' | '.' => {
                    let t = Token::Punctuator(c);
                    self.pos += 1;
                    t
                }
                '0'..='9' => Token::Number(self.cosume_number()),
                // https://262.ecma-international.org/#prod-IdentifierStart
                'a'..='z' | 'A'..='Z' | '_' | '$' => {
                    Token::Identifier(self.consume_identifier())
                }
                '"' => Token::StringLiteral(self.consume_string()),
                _ => {
                    // Unsupported character — skip and continue the loop.
                    self.pos += 1;
                    continue;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let input = "".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn test_num() {
        let input = "42".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        let expected = [Token::Number(42)].to_vec();
        let mut i = 0;
        while lexer.peek().is_some() {
            assert_eq!(Some(expected[i].clone()), lexer.next());
            i += 1;
        }
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn test_add_nums() {
        let input = "1 + 2".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        let expected = [Token::Number(1), Token::Punctuator('+'), Token::Number(2)].to_vec();
        let mut i = 0;
        while lexer.peek().is_some() {
            assert_eq!(Some(expected[i].clone()), lexer.next());
            i += 1;
        }
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn test_assign_variable() {
        let input = "var foo=\"bar\";".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        let expected = [
            Token::Keyword("var".to_string()),
            Token::Identifier("foo".to_string()),
            Token::Punctuator('='),
            Token::StringLiteral("bar".to_string()),
            Token::Punctuator(';'),
        ]
        .to_vec();
        let mut i = 0;
        while lexer.peek().is_some() {
            assert_eq!(Some(expected[i].clone()), lexer.next());
            i += 1;
        }
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn test_add_variable_and_num() {
        let input = "var foo=42; var result=foo+1;".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        let expected = [
            Token::Keyword("var".to_string()),
            Token::Identifier("foo".to_string()),
            Token::Punctuator('='),
            Token::Number(42),
            Token::Punctuator(';'),
            Token::Keyword("var".to_string()),
            Token::Identifier("result".to_string()),
            Token::Punctuator('='),
            Token::Identifier("foo".to_string()),
            Token::Punctuator('+'),
            Token::Number(1),
            Token::Punctuator(';'),
        ]
        .to_vec();
        let mut i = 0;
        while lexer.peek().is_some() {
            assert_eq!(Some(expected[i].clone()), lexer.next());
            i += 1;
        }
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn test_add_local_variable_and_num() {
        let input = "function foo() { var a=42; return a; } var result = foo() + 1;".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        let expected = [
            Token::Keyword("function".to_string()),
            Token::Identifier("foo".to_string()),
            Token::Punctuator('('),
            Token::Punctuator(')'),
            Token::Punctuator('{'),
            Token::Keyword("var".to_string()),
            Token::Identifier("a".to_string()),
            Token::Punctuator('='),
            Token::Number(42),
            Token::Punctuator(';'),
            Token::Keyword("return".to_string()),
            Token::Identifier("a".to_string()),
            Token::Punctuator(';'),
            Token::Punctuator('}'),
            Token::Keyword("var".to_string()),
            Token::Identifier("result".to_string()),
            Token::Punctuator('='),
            Token::Identifier("foo".to_string()),
            Token::Punctuator('('),
            Token::Punctuator(')'),
            Token::Punctuator('+'),
            Token::Number(1),
            Token::Punctuator(';'),
        ]
        .to_vec();
        let mut i = 0;
        while lexer.peek().is_some() {
            assert_eq!(Some(expected[i].clone()), lexer.next());
            i += 1;
        }
        assert!(lexer.peek().is_none());
    }

    #[test]
    fn test_skip_unsupported_chars_does_not_panic() {
        // Real-world JS uses operators this minimal tokenizer cannot produce
        // tokens for.  Rather than panicking, those chars are silently skipped.
        let input = "var x = a != b; if (!c) { return c < 1 ? 'a' : 'b'; }".to_string();
        let mut lexer = JsLexer::new(input).peekable();
        // Iterate to completion to ensure the loop terminates.
        while lexer.next().is_some() {}
    }

    #[test]
    fn test_long_run_of_unsupported_chars_does_not_overflow_stack() {
        // 50k unknown chars in a row.  Used to recurse and stack-overflow;
        // now must terminate quickly via the iterative skip loop.
        let mut input = String::new();
        for _ in 0..50_000 {
            input.push('!');
        }
        input.push_str("var x = 1");
        let mut lexer = JsLexer::new(input).peekable();
        while lexer.next().is_some() {}
    }
}
