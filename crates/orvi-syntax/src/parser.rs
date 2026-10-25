use std::fmt;

use crate::{
    BinaryOperator, Expression, ExpressionKind, Identifier, LexError, Literal, MatchArm, Parameter,
    Pattern, PatternKind, Program, Span, Statement, StatementKind, StructField, Token, TokenKind,
    TypeAnnotation, UnaryOperator, lex,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub message: String,
    pub span: Span,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    pub program: Program,
    pub lexical_errors: Vec<LexError>,
    pub parse_errors: Vec<ParseError>,
}

impl Parsed {
    pub fn has_errors(&self) -> bool {
        !self.lexical_errors.is_empty() || !self.parse_errors.is_empty()
    }
}

pub fn parse(source: &str) -> Parsed {
    let lexed = lex(source);
    let tokens = lexed
        .tokens
        .into_iter()
        .filter(|token| !token.kind.is_trivia())
        .collect();
    let mut parser = Parser {
        source,
        tokens,
        current: 0,
        errors: Vec::new(),
    };
    let program = parser.program();
    Parsed {
        program,
        lexical_errors: lexed.errors,
        parse_errors: parser.errors,
    }
}

struct Parser<'source> {
    source: &'source str,
    tokens: Vec<Token>,
    current: usize,
    errors: Vec<ParseError>,
}

impl Parser<'_> {
    fn program(&mut self) -> Program {
        let mut statements = Vec::new();
        while !self.at(TokenKind::EndOfFile) {
            match self.declaration() {
                Ok(statement) => statements.push(statement),
                Err(error) => {
                    self.errors.push(error);
                    self.synchronize();
                }
            }
        }
        Program { statements }
    }

    fn declaration(&mut self) -> Result<Statement, ParseError> {
        if self.consume(TokenKind::Let) {
            return self.variable(true);
        }
        if self.consume(TokenKind::Const) {
            return self.variable(false);
        }
        if self.consume(TokenKind::Fn) {
            return self.function();
        }
        if self.consume(TokenKind::Struct) {
            return self.struct_declaration();
        }
        self.statement()
    }

    fn variable(&mut self, mutable: bool) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let name = self.expect_identifier("expected variable name")?;
        let type_annotation = if self.consume(TokenKind::Colon) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        self.expect(TokenKind::Equal, "expected '=' after variable name")?;
        let initializer = self.expression()?;
        self.consume(TokenKind::Semicolon);
        Ok(Statement {
            span: start.cover(initializer.span),
            kind: StatementKind::Variable {
                name,
                mutable,
                type_annotation,
                initializer,
            },
        })
    }

    fn function(&mut self) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let name = self.expect_identifier("expected function name")?;
        self.expect(
            TokenKind::LeftParenthesis,
            "expected '(' after function name",
        )?;
        let mut parameters = Vec::new();
        if !self.at(TokenKind::RightParenthesis) {
            loop {
                let name = self.expect_identifier("expected parameter name")?;
                let type_annotation = if self.consume(TokenKind::Colon) {
                    Some(self.type_annotation()?)
                } else {
                    None
                };
                parameters.push(Parameter {
                    name,
                    type_annotation,
                });
                if !self.consume(TokenKind::Comma) {
                    break;
                }
            }
        }
        self.expect(TokenKind::RightParenthesis, "expected ')' after parameters")?;
        let return_type = if self.consume(TokenKind::Arrow) {
            Some(self.type_annotation()?)
        } else {
            None
        };
        let body = self.block_statements()?;
        let end = self.previous().span;
        Ok(Statement {
            span: start.cover(end),
            kind: StatementKind::Function {
                name,
                parameters,
                return_type,
                body,
            },
        })
    }

    fn struct_declaration(&mut self) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let name = self.expect_identifier("expected struct name")?;
        self.expect(TokenKind::LeftBrace, "expected '{' after struct name")?;
        let mut fields = Vec::new();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::EndOfFile) {
            let field_name = self.expect_identifier("expected field name")?;
            self.expect(TokenKind::Colon, "expected ':' after field name")?;
            let type_annotation = self.type_annotation()?;
            let default = if self.consume(TokenKind::Equal) {
                Some(self.expression()?)
            } else {
                None
            };
            self.consume(TokenKind::Semicolon);
            let end = default
                .as_ref()
                .map_or(type_annotation.span, |value| value.span);
            fields.push(StructField {
                span: field_name.span.cover(end),
                name: field_name,
                type_annotation,
                default,
            });
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected '}' after struct fields")?
            .span;
        Ok(Statement {
            span: start.cover(end),
            kind: StatementKind::Struct { name, fields },
        })
    }

    fn statement(&mut self) -> Result<Statement, ParseError> {
        if self.consume(TokenKind::If) {
            return self.if_statement();
        }
        if self.consume(TokenKind::While) {
            return self.while_statement();
        }
        if self.consume(TokenKind::Return) {
            return self.return_statement();
        }
        if self.consume(TokenKind::Defer) {
            let start = self.previous().span;
            let expression = self.expression()?;
            self.consume(TokenKind::Semicolon);
            return Ok(Statement {
                span: start.cover(expression.span),
                kind: StatementKind::Defer(expression),
            });
        }
        if self.consume(TokenKind::Using) {
            return self.using_statement();
        }
        if self.at(TokenKind::LeftBrace) {
            let start = self.advance().span;
            let statements = self.block_contents()?;
            return Ok(Statement {
                span: start.cover(self.previous().span),
                kind: StatementKind::Block(statements),
            });
        }
        let expression = self.expression()?;
        self.consume(TokenKind::Semicolon);
        Ok(Statement {
            span: expression.span,
            kind: StatementKind::Expression(expression),
        })
    }

    fn if_statement(&mut self) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let condition = self.expression()?;
        let then_branch = Box::new(self.statement()?);
        let else_branch = if self.consume(TokenKind::Else) {
            Some(Box::new(self.statement()?))
        } else {
            None
        };
        let end = else_branch
            .as_ref()
            .map_or(then_branch.span, |branch| branch.span);
        Ok(Statement {
            span: start.cover(end),
            kind: StatementKind::If {
                condition,
                then_branch,
                else_branch,
            },
        })
    }

    fn while_statement(&mut self) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let condition = self.expression()?;
        let body = Box::new(self.statement()?);
        Ok(Statement {
            span: start.cover(body.span),
            kind: StatementKind::While { condition, body },
        })
    }

    fn return_statement(&mut self) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let value = if self.at(TokenKind::Semicolon)
            || self.at(TokenKind::RightBrace)
            || self.at(TokenKind::EndOfFile)
        {
            None
        } else {
            Some(self.expression()?)
        };
        self.consume(TokenKind::Semicolon);
        let end = value.as_ref().map_or(start, |expression| expression.span);
        Ok(Statement {
            span: start.cover(end),
            kind: StatementKind::Return(value),
        })
    }

    fn using_statement(&mut self) -> Result<Statement, ParseError> {
        let start = self.previous().span;
        let name = self.expect_identifier("expected resource name")?;
        self.expect(TokenKind::Equal, "expected '=' after resource name")?;
        let initializer = self.expression()?;
        self.expect(TokenKind::FatArrow, "expected '=>' before resource cleanup")?;
        let cleanup = self.expression()?;
        self.consume(TokenKind::Semicolon);
        Ok(Statement {
            span: start.cover(cleanup.span),
            kind: StatementKind::Using {
                name,
                initializer,
                cleanup,
            },
        })
    }

    fn block_statements(&mut self) -> Result<Vec<Statement>, ParseError> {
        self.expect(TokenKind::LeftBrace, "expected '{' before function body")?;
        self.block_contents()
    }

    fn block_contents(&mut self) -> Result<Vec<Statement>, ParseError> {
        let mut statements = Vec::new();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::EndOfFile) {
            statements.push(self.declaration()?);
        }
        self.expect(TokenKind::RightBrace, "expected '}' after block")?;
        Ok(statements)
    }

    fn expression(&mut self) -> Result<Expression, ParseError> {
        self.assignment()
    }

    fn assignment(&mut self) -> Result<Expression, ParseError> {
        let expression = self.or()?;
        if self.consume(TokenKind::Equal) {
            let value = self.assignment()?;
            let span = expression.span.cover(value.span);
            return match expression.kind {
                ExpressionKind::Variable(name) => Ok(Expression {
                    span,
                    kind: ExpressionKind::Assign {
                        name,
                        value: Box::new(value),
                    },
                }),
                ExpressionKind::Index { object, index } => Ok(Expression {
                    span,
                    kind: ExpressionKind::IndexAssign {
                        object,
                        index,
                        value: Box::new(value),
                    },
                }),
                ExpressionKind::Member { object, field } => Ok(Expression {
                    span,
                    kind: ExpressionKind::MemberAssign {
                        object,
                        field,
                        value: Box::new(value),
                    },
                }),
                _ => Err(ParseError {
                    message: "invalid assignment target".into(),
                    span: expression.span,
                }),
            };
        }
        Ok(expression)
    }

    fn or(&mut self) -> Result<Expression, ParseError> {
        self.binary(Self::and, &[(TokenKind::PipePipe, BinaryOperator::Or)])
    }
    fn and(&mut self) -> Result<Expression, ParseError> {
        self.binary(
            Self::equality,
            &[(TokenKind::AmpersandAmpersand, BinaryOperator::And)],
        )
    }
    fn equality(&mut self) -> Result<Expression, ParseError> {
        self.binary(
            Self::comparison,
            &[
                (TokenKind::EqualEqual, BinaryOperator::Equal),
                (TokenKind::BangEqual, BinaryOperator::NotEqual),
            ],
        )
    }
    fn comparison(&mut self) -> Result<Expression, ParseError> {
        self.binary(
            Self::term,
            &[
                (TokenKind::Less, BinaryOperator::Less),
                (TokenKind::LessEqual, BinaryOperator::LessEqual),
                (TokenKind::Greater, BinaryOperator::Greater),
                (TokenKind::GreaterEqual, BinaryOperator::GreaterEqual),
            ],
        )
    }
    fn term(&mut self) -> Result<Expression, ParseError> {
        self.binary(
            Self::factor,
            &[
                (TokenKind::Plus, BinaryOperator::Add),
                (TokenKind::Minus, BinaryOperator::Subtract),
            ],
        )
    }
    fn factor(&mut self) -> Result<Expression, ParseError> {
        self.binary(
            Self::unary,
            &[
                (TokenKind::Star, BinaryOperator::Multiply),
                (TokenKind::Slash, BinaryOperator::Divide),
                (TokenKind::Percent, BinaryOperator::Remainder),
            ],
        )
    }

    fn binary(
        &mut self,
        operand: fn(&mut Self) -> Result<Expression, ParseError>,
        operators: &[(TokenKind, BinaryOperator)],
    ) -> Result<Expression, ParseError> {
        let mut expression = operand(self)?;
        while let Some((_, operator)) = operators.iter().find(|(kind, _)| self.at(*kind)) {
            self.advance();
            let right = operand(self)?;
            let span = expression.span.cover(right.span);
            expression = Expression {
                span,
                kind: ExpressionKind::Binary {
                    left: Box::new(expression),
                    operator: *operator,
                    right: Box::new(right),
                },
            };
        }
        Ok(expression)
    }

    fn unary(&mut self) -> Result<Expression, ParseError> {
        let operator = if self.consume(TokenKind::Bang) {
            Some(UnaryOperator::Not)
        } else if self.consume(TokenKind::Minus) {
            Some(UnaryOperator::Negate)
        } else {
            None
        };
        if let Some(operator) = operator {
            let start = self.previous().span;
            let operand = self.unary()?;
            return Ok(Expression {
                span: start.cover(operand.span),
                kind: ExpressionKind::Unary {
                    operator,
                    operand: Box::new(operand),
                },
            });
        }
        self.call()
    }

    fn call(&mut self) -> Result<Expression, ParseError> {
        let mut expression = self.primary()?;
        loop {
            if self.consume(TokenKind::LeftParenthesis) {
                let mut arguments = Vec::new();
                if !self.at(TokenKind::RightParenthesis) {
                    loop {
                        arguments.push(self.expression()?);
                        if !self.consume(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                let end = self
                    .expect(TokenKind::RightParenthesis, "expected ')' after arguments")?
                    .span;
                expression = Expression {
                    span: expression.span.cover(end),
                    kind: ExpressionKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                };
            } else if self.consume(TokenKind::LeftBracket) {
                let index = self.expression()?;
                let end = self
                    .expect(TokenKind::RightBracket, "expected ']' after index")?
                    .span;
                expression = Expression {
                    span: expression.span.cover(end),
                    kind: ExpressionKind::Index {
                        object: Box::new(expression),
                        index: Box::new(index),
                    },
                };
            } else if self.consume(TokenKind::Dot) {
                let field = self.expect_identifier("expected field name after '.'")?;
                expression = Expression {
                    span: expression.span.cover(field.span),
                    kind: ExpressionKind::Member {
                        object: Box::new(expression),
                        field,
                    },
                };
            } else if self.consume(TokenKind::Question) {
                let span = expression.span.cover(self.previous().span);
                expression = Expression {
                    span,
                    kind: ExpressionKind::Try(Box::new(expression)),
                };
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn primary(&mut self) -> Result<Expression, ParseError> {
        let token = self.advance();
        let text = token.text(self.source).unwrap_or_default();
        if token.kind == TokenKind::Match {
            return self.match_expression(token.span);
        }
        let kind = match token.kind {
            TokenKind::Null => ExpressionKind::Literal(Literal::Null),
            TokenKind::True => ExpressionKind::Literal(Literal::Bool(true)),
            TokenKind::False => ExpressionKind::Literal(Literal::Bool(false)),
            TokenKind::Integer => {
                ExpressionKind::Literal(Literal::Integer(text.parse().map_err(|_| ParseError {
                    message: "integer literal is out of range".into(),
                    span: token.span,
                })?))
            }
            TokenKind::Float => {
                ExpressionKind::Literal(Literal::Float(text.parse().map_err(|_| ParseError {
                    message: "invalid float literal".into(),
                    span: token.span,
                })?))
            }
            TokenKind::String => ExpressionKind::Literal(Literal::String(unescape_string(text))),
            TokenKind::Identifier => ExpressionKind::Variable(Identifier {
                name: text.to_owned(),
                span: token.span,
            }),
            TokenKind::LeftParenthesis => {
                let expression = self.expression()?;
                self.expect(TokenKind::RightParenthesis, "expected ')' after expression")?;
                return Ok(expression);
            }
            TokenKind::LeftBracket => {
                let mut values = Vec::new();
                if !self.at(TokenKind::RightBracket) {
                    loop {
                        values.push(self.expression()?);
                        if !self.consume(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                let end = self
                    .expect(TokenKind::RightBracket, "expected ']' after list")?
                    .span;
                return Ok(Expression {
                    span: token.span.cover(end),
                    kind: ExpressionKind::List(values),
                });
            }
            _ => {
                return Err(ParseError {
                    message: "expected expression".into(),
                    span: token.span,
                });
            }
        };
        Ok(Expression {
            kind,
            span: token.span,
        })
    }

    fn match_expression(&mut self, start: Span) -> Result<Expression, ParseError> {
        let value = self.expression()?;
        self.expect(TokenKind::LeftBrace, "expected '{' after match value")?;
        let mut arms = Vec::new();
        while !self.at(TokenKind::RightBrace) && !self.at(TokenKind::EndOfFile) {
            let pattern = self.pattern()?;
            self.expect(TokenKind::FatArrow, "expected '=>' after match pattern")?;
            let arm_value = self.expression()?;
            let span = pattern.span.cover(arm_value.span);
            arms.push(MatchArm {
                pattern,
                value: arm_value,
                span,
            });
            if !self.consume(TokenKind::Comma) && !self.at(TokenKind::RightBrace) {
                return Err(ParseError {
                    message: "expected ',' between match arms".into(),
                    span: self.peek().span,
                });
            }
        }
        let end = self
            .expect(TokenKind::RightBrace, "expected '}' after match arms")?
            .span;
        Ok(Expression {
            span: start.cover(end),
            kind: ExpressionKind::Match {
                value: Box::new(value),
                arms,
            },
        })
    }

    fn pattern(&mut self) -> Result<Pattern, ParseError> {
        let token = self.advance();
        let text = token.text(self.source).unwrap_or_default();
        let kind = match token.kind {
            TokenKind::Identifier if text == "_" => PatternKind::Wildcard,
            TokenKind::Identifier => {
                let name = Identifier {
                    name: text.to_owned(),
                    span: token.span,
                };
                let binding = if self.consume(TokenKind::LeftParenthesis) {
                    let binding = self.expect_identifier("expected variant binding")?;
                    self.expect(
                        TokenKind::RightParenthesis,
                        "expected ')' after variant binding",
                    )?;
                    Some(binding)
                } else {
                    None
                };
                let end = binding.as_ref().map_or(name.span, |binding| binding.span);
                return Ok(Pattern {
                    span: token.span.cover(end),
                    kind: PatternKind::Variant { name, binding },
                });
            }
            TokenKind::Null => PatternKind::Literal(Literal::Null),
            TokenKind::True => PatternKind::Literal(Literal::Bool(true)),
            TokenKind::False => PatternKind::Literal(Literal::Bool(false)),
            TokenKind::Integer => {
                PatternKind::Literal(Literal::Integer(text.parse().map_err(|_| ParseError {
                    message: "integer literal is out of range".into(),
                    span: token.span,
                })?))
            }
            TokenKind::Float => {
                PatternKind::Literal(Literal::Float(text.parse().map_err(|_| ParseError {
                    message: "invalid float literal".into(),
                    span: token.span,
                })?))
            }
            TokenKind::String => PatternKind::Literal(Literal::String(unescape_string(text))),
            _ => {
                return Err(ParseError {
                    message: "expected match pattern".into(),
                    span: token.span,
                });
            }
        };
        Ok(Pattern {
            kind,
            span: token.span,
        })
    }

    fn expect_identifier(&mut self, message: &str) -> Result<Identifier, ParseError> {
        let token = self.expect(TokenKind::Identifier, message)?;
        Ok(Identifier {
            name: token.text(self.source).unwrap_or_default().to_owned(),
            span: token.span,
        })
    }

    fn type_annotation(&mut self) -> Result<TypeAnnotation, ParseError> {
        let name = self.expect_identifier("expected type name")?;
        let mut arguments = Vec::new();
        let mut end = name.span;
        if self.consume(TokenKind::Less) {
            loop {
                arguments.push(self.type_annotation()?);
                if !self.consume(TokenKind::Comma) {
                    break;
                }
            }
            end = self
                .expect(TokenKind::Greater, "expected '>' after type arguments")?
                .span;
        }
        Ok(TypeAnnotation {
            span: name.span.cover(end),
            name,
            arguments,
        })
    }

    fn expect(&mut self, kind: TokenKind, message: &str) -> Result<Token, ParseError> {
        if self.at(kind) {
            Ok(self.advance())
        } else {
            Err(ParseError {
                message: message.into(),
                span: self.peek().span,
            })
        }
    }

    fn consume(&mut self, kind: TokenKind) -> bool {
        if self.at(kind) {
            self.advance();
            true
        } else {
            false
        }
    }
    fn at(&self, kind: TokenKind) -> bool {
        self.peek().kind == kind
    }
    fn peek(&self) -> Token {
        self.tokens[self.current]
    }
    fn previous(&self) -> Token {
        self.tokens[self.current.saturating_sub(1)]
    }
    fn advance(&mut self) -> Token {
        let token = self.peek();
        if token.kind != TokenKind::EndOfFile {
            self.current += 1;
        }
        token
    }

    fn synchronize(&mut self) {
        if !self.at(TokenKind::EndOfFile) {
            self.advance();
        }
        while !self.at(TokenKind::EndOfFile) {
            if self.previous().kind == TokenKind::Semicolon {
                return;
            }
            if matches!(
                self.peek().kind,
                TokenKind::Let
                    | TokenKind::Const
                    | TokenKind::Fn
                    | TokenKind::Struct
                    | TokenKind::If
                    | TokenKind::While
                    | TokenKind::Return
            ) {
                return;
            }
            self.advance();
        }
    }
}

fn unescape_string(source: &str) -> String {
    let content = source
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(source);
    let mut result = String::new();
    let mut chars = content.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match chars.next() {
            Some('0') => result.push('\0'),
            Some('n') => result.push('\n'),
            Some('r') => result.push('\r'),
            Some('t') => result.push('\t'),
            Some('\\') => result.push('\\'),
            Some('"') => result.push('"'),
            Some(other) => {
                result.push('\\');
                result.push(other);
            }
            None => result.push('\\'),
        }
    }
    result
}
