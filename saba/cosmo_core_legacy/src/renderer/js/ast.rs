use crate::renderer::js::token::JsLexer;
use crate::renderer::js::token::Token;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::iter::Peekable;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    ExpressionStatement(Option<Rc<Node>>),
    AdditiveExpression {
        operator: char,
        left: Option<Rc<Node>>,
        right: Option<Rc<Node>>,
    },
    AssignmentExpression {
        operator: char,
        left: Option<Rc<Node>>,
        right: Option<Rc<Node>>,
    },
    MemberExpression {
        object: Option<Rc<Node>>,
        property: Option<Rc<Node>>,
    },
    NumericLiteral(u64),
    /// https://262.ecma-international.org/#prod-VariableStatement
    VariableDeclaration {
        declarations: Vec<Option<Rc<Node>>>,
    },
    VariableDeclarator {
        id: Option<Rc<Node>>,
        init: Option<Rc<Node>>,
    },
    Identifier(String),
    StringLiteral(String),
    BlockStatement {
        body: Vec<Option<Rc<Node>>>,
    },
    ReturnStatement {
        argument: Option<Rc<Node>>,
    },
    /// https://262.ecma-international.org/#prod-FunctionDeclaration
    FunctionDeclaration {
        id: Option<Rc<Node>>,
        params: Vec<Option<Rc<Node>>>,
        body: Option<Rc<Node>>,
    },
    /// https://262.ecma-international.org/#prod-CallExpression
    CallExpression {
        callee: Option<Rc<Node>>,
        arguments: Vec<Option<Rc<Node>>>,
    },
}

impl Node {
    pub fn new_expression_statement(expression: Option<Rc<Self>>) -> Option<Rc<Self>> {
        Some(Rc::new(Node::ExpressionStatement(expression)))
    }

    pub fn new_additive_expression(
        operator: char,
        left: Option<Rc<Node>>,
        right: Option<Rc<Node>>,
    ) -> Option<Rc<Self>> {
        Some(Rc::new(Node::AdditiveExpression {
            operator,
            left,
            right,
        }))
    }

    pub fn new_assignment_expression(
        operator: char,
        left: Option<Rc<Node>>,
        right: Option<Rc<Node>>,
    ) -> Option<Rc<Self>> {
        Some(Rc::new(Node::AssignmentExpression {
            operator,
            left,
            right,
        }))
    }

    pub fn new_member_expression(
        object: Option<Rc<Self>>,
        property: Option<Rc<Self>>,
    ) -> Option<Rc<Self>> {
        Some(Rc::new(Node::MemberExpression { object, property }))
    }

    pub fn new_numeric_literal(value: u64) -> Option<Rc<Self>> {
        Some(Rc::new(Node::NumericLiteral(value)))
    }

    pub fn new_variable_declarator(
        id: Option<Rc<Self>>,
        init: Option<Rc<Self>>,
    ) -> Option<Rc<Self>> {
        Some(Rc::new(Node::VariableDeclarator { id, init }))
    }

    pub fn new_variable_declaration(declarations: Vec<Option<Rc<Self>>>) -> Option<Rc<Self>> {
        Some(Rc::new(Node::VariableDeclaration { declarations }))
    }

    pub fn new_identifier(name: String) -> Option<Rc<Self>> {
        Some(Rc::new(Node::Identifier(name)))
    }

    pub fn new_string_literal(value: String) -> Option<Rc<Self>> {
        Some(Rc::new(Node::StringLiteral(value)))
    }

    pub fn new_block_statement(body: Vec<Option<Rc<Self>>>) -> Option<Rc<Self>> {
        Some(Rc::new(Node::BlockStatement { body }))
    }

    pub fn new_return_statement(argument: Option<Rc<Self>>) -> Option<Rc<Self>> {
        Some(Rc::new(Node::ReturnStatement { argument }))
    }

    pub fn new_function_declaration(
        id: Option<Rc<Self>>,
        params: Vec<Option<Rc<Self>>>,
        body: Option<Rc<Self>>,
    ) -> Option<Rc<Self>> {
        Some(Rc::new(Node::FunctionDeclaration { id, params, body }))
    }

    pub fn new_call_expression(
        callee: Option<Rc<Self>>,
        arguments: Vec<Option<Rc<Self>>>,
    ) -> Option<Rc<Self>> {
        Some(Rc::new(Node::CallExpression { callee, arguments }))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    body: Vec<Rc<Node>>,
}

impl Program {
    pub fn new() -> Self {
        Self { body: Vec::new() }
    }

    pub fn set_body(&mut self, body: Vec<Rc<Node>>) {
        self.body = body;
    }

    pub fn body(&self) -> &Vec<Rc<Node>> {
        &self.body
    }
}

// Construct an AST from the token stream.
pub struct JsParser {
    t: Peekable<JsLexer>,
}

impl JsParser {
    pub fn new(t: JsLexer) -> Self {
        Self { t: t.peekable() }
    }

    // p.363
    // Program ::= ( SourceElements )? <EOF>
    pub fn parse_ast(&mut self) -> Program {
        let mut program = Program::new();

        let mut body = Vec::new();

        loop {
            let node = self.source_element(); // 1

            match node {
                Some(n) => body.push(n),
                None => {
                    // 2
                    program.set_body(body);
                    return program;
                }
            }
        }
    }

    // SourceElement ::= Statement | FunctionDeclaration
    // p.407
    fn source_element(&mut self) -> Option<Rc<Node>> {
        let t = match self.t.peek() {
            Some(t) => t,
            None => return None,
        };

        match t {
            Token::Keyword(keyword) => {
                if keyword == "function" {
                    // Consumes a "function" keyword.
                    assert!(self.t.next().is_some());
                    self.function_declaration() // 1
                } else {
                    self.statement()
                }
            }
            _ => self.statement(), // 2
        }
    }

    // FunctionDeclaration ::= "function" Identifier ( "(" FormalParameterList? ")" ) FunctionBody
    fn function_declaration(&mut self) -> Option<Rc<Node>> {
        let id = self.identifier();
        let params = self.parameter_list();
        Node::new_function_declaration(id, params, self.function_body())
    }

    // FormalParameterList ::= Identifier ( "," Identifier )*
    // p.409
    fn parameter_list(&mut self) -> Vec<Option<Rc<Node>>> {
        let mut params = Vec::new();

        // Consume a '('. If the next token is not '(', occurs error.
        match self.t.next() {
            // 1
            Some(t) => match t {
                Token::Punctuator(c) => assert!(c == '('),
                _ => panic!("function should have '(' but got {:?}", t),
            },
            None => panic!("function should have '(' but got None"),
        }

        loop {
            // Until the next token is ')', add an dummy argument to the params.
            match self.t.peek() {
                Some(t) => match t {
                    Token::Punctuator(c) => {
                        if c == &')' {
                            // 2
                            // Consume a ')'.
                            assert!(self.t.next().is_some());
                            return params;
                        }
                        if c == &',' {
                            // Consume a ','.
                            assert!(self.t.next().is_some());
                        }
                    }
                    _ => {
                        params.push(self.identifier()); // 3
                    }
                },
                None => return params,
            }
        }
    }

    // FunctionBody ::= "{" SourceElements? "}"
    fn function_body(&mut self) -> Option<Rc<Node>> {
        // Consume a '{'.
        match self.t.next() {
            // 1
            Some(t) => match t {
                Token::Punctuator(c) => assert!(c == '{'),
                _ => unimplemented!(
                    "function body should have open curly blacket but got {:?}",
                    t
                ),
            },
            None => unimplemented!("function body should have open curly blacket but got None"),
        }

        let mut body = Vec::new();
        loop {
            // Until the next token is '}', resolve as a code within the function.
            match self.t.peek() {
                Some(t) => match t {
                    Token::Punctuator(c) => {
                        if c == &'}' {
                            // 2
                            // Consume a '}'. Return a BlockStatement.
                            assert!(self.t.next().is_some());
                            return Node::new_block_statement(body);
                        }
                    }
                    _ => {}
                },
                None => {}
            }

            body.push(self.source_element()); // 3
        }
    }

    // Statement ::= ExpressionStatement | VariableStatement | ReturnStatement
    // ExpressionStatement ::= AssignmentExpression ( ";" )?
    // p.411 X1
    fn statement(&mut self) -> Option<Rc<Node>> {
        let t = match self.t.peek() {
            Some(t) => t,
            None => return None,
        };

        let node = match t {
            Token::Keyword(keyword) => {
                if keyword == "var" {
                    // Consumes a "var" keyword.
                    assert!(self.t.next().is_some());

                    self.variable_declaration() // 1
                } else if keyword == "return" {
                    // X1
                    // Consumes a "return" keyword.
                    assert!(self.t.next().is_some());

                    Node::new_return_statement(self.assignment_expression())
                } else {
                    None
                }
            }
            _ => Node::new_expression_statement(self.assignment_expression()), // 2
        };

        if let Some(Token::Punctuator(c)) = self.t.peek() {
            // Consume a semicolon.
            if c == &';' {
                assert!(self.t.next().is_some());
            }
        }

        node
    }

    // VariableDeclaration ::= Identifier ( Initialiser )?
    fn variable_declaration(&mut self) -> Option<Rc<Node>> {
        let ident = self.identifier(); // 1

        let declarator = Node::new_variable_declarator(ident, self.initialiser()); // 2

        let mut declarations = Vec::new();
        declarations.push(declarator); // 3

        Node::new_variable_declaration(declarations) // 4
    }

    // Identifier ::= <identifier name>
    // <identifier name> ::= (& | _ | a-z | A-Z) (& | a-z | A-Z)*
    fn identifier(&mut self) -> Option<Rc<Node>> {
        let t = match self.t.next() {
            Some(token) => token,
            None => return None,
        };

        match t {
            Token::Identifier(name) => Node::new_identifier(name),
            _ => None,
        }
    }

    // Initialiser ::= "=" AssignmentExpression
    fn initialiser(&mut self) -> Option<Rc<Node>> {
        let t = match self.t.next() {
            Some(token) => token,
            None => return None,
        };

        match t {
            Token::Punctuator(c) => match c {
                '=' => self.assignment_expression(),
                _ => None,
            },
            _ => None,
        }
    }

    // AssignmentExpression ::= AdditiveExpression ( "=" AdditiveExpression )?
    // p.388
    fn assignment_expression(&mut self) -> Option<Rc<Node>> {
        let expr = self.additive_expression();

        let t = match self.t.peek() {
            Some(token) => token,
            None => return expr,
        };

        match t {
            Token::Punctuator('=') => {
                // Consumes an assignment operator.(=)
                assert!(self.t.next().is_some());
                Node::new_assignment_expression('=', expr, self.assignment_expression())
                // 1
            }
            _ => expr, // 2
        }
    }

    // AdditiveExpression ::= LeftHandSideExpression ( AdditiveOperator AssignmentExpression )*
    fn additive_expression(&mut self) -> Option<Rc<Node>> {
        let left = self.left_hand_side_expression(); // 1

        let t = match self.t.peek() {
            Some(token) => token.clone(),
            None => return left, // 2
        };

        match t {
            Token::Punctuator(c) => match c {
                // 3
                '+' | '-' => {
                    // Consume an additive or subtractive operator.
                    assert!(self.t.next().is_some());
                    Node::new_additive_expression(c, left, self.assignment_expression())
                }
                _ => left,
            },
            _ => left,
        }
    }

    // LeftHandSideExpression ::= CallExpression | MemberExpression
    // p.412
    fn left_hand_side_expression(&mut self) -> Option<Rc<Node>> {
        let expr = self.member_expression();

        let t = match self.t.peek() {
            Some(token) => token,
            None => return expr,
        };

        match t {
            Token::Punctuator(c) => {
                // 1
                if c == &'(' {
                    // Consume a '('.
                    assert!(self.t.next().is_some());
                    // Return a CallExpression node because of calling a function.
                    return Node::new_call_expression(expr, self.arguments());
                }

                expr
            }
            _ => expr,
        }
    }

    // MemberExpression ::= PrimaryExpression ( "." Identifier )*
    // p.414
    fn member_expression(&mut self) -> Option<Rc<Node>> {
        let expr = self.primary_expression();

        let t = match self.t.peek() {
            Some(token) => token,
            None => return expr,
        };

        match t {
            Token::Punctuator(c) => {
                if c == &'.' {
                    // Consume a '.'.
                    assert!(self.t.next().is_some());
                    return Node::new_member_expression(expr, self.identifier());
                    // 1
                }

                expr
            }
            _ => expr, // 2
        }
    }

    // Arguments ::= "(" ( ArgumentList )? ")"
    // ArgumentList ::= AssignmentExpression ( "," AssignmentExpression )*
    // p.413
    fn arguments(&mut self) -> Vec<Option<Rc<Node>>> {
        let mut arguments = Vec::new();

        loop {
            // Until the next token is ')', add an argument to the arguments vector.
            match self.t.peek() {
                Some(t) => match t {
                    Token::Punctuator(c) => {
                        if c == &')' {
                            // Consume a ')'.
                            assert!(self.t.next().is_some());
                            return arguments;
                        }
                        if c == &',' {
                            // Consume a ','.
                            assert!(self.t.next().is_some());
                        }
                    }
                    _ => arguments.push(self.assignment_expression()),
                },
                None => return arguments,
            }
        }
    }

    // PrimaryExpression ::= Identifier | Literal
    // Literal ::= <digit>+
    // <digit> ::= 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9
    // p.389
    fn primary_expression(&mut self) -> Option<Rc<Node>> {
        let t = match self.t.next() {
            Some(token) => token,
            None => return None,
        };

        match t {
            Token::Identifier(value) => Node::new_identifier(value), // 1
            Token::StringLiteral(value) => Node::new_string_literal(value), // 2
            Token::Number(value) => Node::new_numeric_literal(value),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn test_empty() {
        let input = "".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let expected = Program::new();
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_num() {
        let input = "42".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::ExpressionStatement(Some(Rc::new(
            Node::NumericLiteral(42),
        )))));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_add_nums() {
        let input = "1 + 2".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::ExpressionStatement(Some(Rc::new(
            Node::AdditiveExpression {
                operator: '+',
                left: Some(Rc::new(Node::NumericLiteral(1))),
                right: Some(Rc::new(Node::NumericLiteral(2))),
            },
        )))));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_assign_variable() {
        let input = "var foo=\"bar\";".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::VariableDeclaration {
            declarations: [Some(Rc::new(Node::VariableDeclarator {
                id: Some(Rc::new(Node::Identifier("foo".to_string()))),
                init: Some(Rc::new(Node::StringLiteral("bar".to_string()))),
            }))]
            .to_vec(),
        }));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_add_variable_and_num() {
        let input = "var foo=42; var result=foo+1;".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::VariableDeclaration {
            declarations: [Some(Rc::new(Node::VariableDeclarator {
                id: Some(Rc::new(Node::Identifier("foo".to_string()))),
                init: Some(Rc::new(Node::NumericLiteral(42))),
            }))]
            .to_vec(),
        }));
        body.push(Rc::new(Node::VariableDeclaration {
            declarations: [Some(Rc::new(Node::VariableDeclarator {
                id: Some(Rc::new(Node::Identifier("result".to_string()))),
                init: Some(Rc::new(Node::AdditiveExpression {
                    operator: '+',
                    left: Some(Rc::new(Node::Identifier("foo".to_string()))),
                    right: Some(Rc::new(Node::NumericLiteral(1))),
                })),
            }))]
            .to_vec(),
        }));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_define_function() {
        let input = "function foo() { return 42; }".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::FunctionDeclaration {
            id: Some(Rc::new(Node::Identifier("foo".to_string()))),
            params: [].to_vec(),
            body: Some(Rc::new(Node::BlockStatement {
                body: [Some(Rc::new(Node::ReturnStatement {
                    argument: Some(Rc::new(Node::NumericLiteral(42))),
                }))]
                .to_vec(),
            })),
        }));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_add_function_add_num() {
        let input = "function foo() { return 42; } var result = foo() + 1;".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::FunctionDeclaration {
            id: Some(Rc::new(Node::Identifier("foo".to_string()))),
            params: [].to_vec(),
            body: Some(Rc::new(Node::BlockStatement {
                body: [Some(Rc::new(Node::ReturnStatement {
                    argument: Some(Rc::new(Node::NumericLiteral(42))),
                }))]
                .to_vec(),
            })),
        }));
        body.push(Rc::new(Node::VariableDeclaration {
            declarations: [Some(Rc::new(Node::VariableDeclarator {
                id: Some(Rc::new(Node::Identifier("result".to_string()))),
                init: Some(Rc::new(Node::AdditiveExpression {
                    operator: '+',
                    left: Some(Rc::new(Node::CallExpression {
                        callee: Some(Rc::new(Node::Identifier("foo".to_string()))),
                        arguments: [].to_vec(),
                    })),
                    right: Some(Rc::new(Node::NumericLiteral(1))),
                })),
            }))]
            .to_vec(),
        }));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }

    #[test]
    fn test_define_function_with_args() {
        let input = "function foo(a, b) { return a+b; }".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let mut expected = Program::new();
        let mut body = Vec::new();
        body.push(Rc::new(Node::FunctionDeclaration {
            id: Some(Rc::new(Node::Identifier("foo".to_string()))),
            params: [
                Some(Rc::new(Node::Identifier("a".to_string()))),
                Some(Rc::new(Node::Identifier("b".to_string()))),
            ]
            .to_vec(),
            body: Some(Rc::new(Node::BlockStatement {
                body: [Some(Rc::new(Node::ReturnStatement {
                    argument: Some(Rc::new(Node::AdditiveExpression {
                        operator: '+',
                        left: Some(Rc::new(Node::Identifier("a".to_string()))),
                        right: Some(Rc::new(Node::Identifier("b".to_string()))),
                    })),
                }))]
                .to_vec(),
            })),
        }));
        expected.set_body(body);
        assert_eq!(expected, parser.parse_ast());
    }
}
