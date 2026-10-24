use crate::{LexError, LexErrorKind, Span, Token, TokenKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
}

impl Lexed {
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

pub fn lex(source: &str) -> Lexed {
    Lexer::new(source).run()
}

struct Lexer<'source> {
    source: &'source str,
    cursor: usize,
    tokens: Vec<Token>,
    errors: Vec<LexError>,
}

impl<'source> Lexer<'source> {
    fn new(source: &'source str) -> Self {
        Self {
            source,
            cursor: 0,
            tokens: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn run(mut self) -> Lexed {
        while self.cursor < self.source.len() {
            self.scan_token();
        }

        self.tokens.push(Token {
            kind: TokenKind::EndOfFile,
            span: Span::from_valid_bounds(self.cursor, self.cursor),
        });

        Lexed {
            tokens: self.tokens,
            errors: self.errors,
        }
    }

    fn scan_token(&mut self) {
        let start = self.cursor;
        let Some(character) = self.advance() else {
            return;
        };

        match character {
            character if character.is_whitespace() => self.scan_whitespace(start),
            character if is_identifier_start(character) => self.scan_identifier(start),
            character if character.is_ascii_digit() => self.scan_number(start),
            '"' => self.scan_string(start),
            '(' => self.push_token(TokenKind::LeftParenthesis, start),
            ')' => self.push_token(TokenKind::RightParenthesis, start),
            '{' => self.push_token(TokenKind::LeftBrace, start),
            '}' => self.push_token(TokenKind::RightBrace, start),
            '[' => self.push_token(TokenKind::LeftBracket, start),
            ']' => self.push_token(TokenKind::RightBracket, start),
            ',' => self.push_token(TokenKind::Comma, start),
            ':' => self.push_token(TokenKind::Colon, start),
            ';' => self.push_token(TokenKind::Semicolon, start),
            '?' => self.push_token(TokenKind::Question, start),
            '.' => {
                let kind = if self.consume('.') {
                    if self.consume('=') {
                        TokenKind::DotDotEqual
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                };
                self.push_token(kind, start);
            }
            '+' => {
                let kind = if self.consume('=') {
                    TokenKind::PlusEqual
                } else {
                    TokenKind::Plus
                };
                self.push_token(kind, start);
            }
            '-' => {
                let kind = if self.consume('>') {
                    TokenKind::Arrow
                } else if self.consume('=') {
                    TokenKind::MinusEqual
                } else {
                    TokenKind::Minus
                };
                self.push_token(kind, start);
            }
            '*' => {
                let kind = if self.consume('*') {
                    TokenKind::StarStar
                } else if self.consume('=') {
                    TokenKind::StarEqual
                } else {
                    TokenKind::Star
                };
                self.push_token(kind, start);
            }
            '/' => {
                if self.consume('/') {
                    self.scan_line_comment(start);
                } else {
                    let kind = if self.consume('=') {
                        TokenKind::SlashEqual
                    } else {
                        TokenKind::Slash
                    };
                    self.push_token(kind, start);
                }
            }
            '%' => self.push_token(TokenKind::Percent, start),
            '=' => {
                let kind = if self.consume('=') {
                    TokenKind::EqualEqual
                } else if self.consume('>') {
                    TokenKind::FatArrow
                } else {
                    TokenKind::Equal
                };
                self.push_token(kind, start);
            }
            '!' => {
                let kind = if self.consume('=') {
                    TokenKind::BangEqual
                } else {
                    TokenKind::Bang
                };
                self.push_token(kind, start);
            }
            '<' => {
                let kind = if self.consume('=') {
                    TokenKind::LessEqual
                } else {
                    TokenKind::Less
                };
                self.push_token(kind, start);
            }
            '>' => {
                let kind = if self.consume('=') {
                    TokenKind::GreaterEqual
                } else {
                    TokenKind::Greater
                };
                self.push_token(kind, start);
            }
            '&' if self.consume('&') => self.push_token(TokenKind::AmpersandAmpersand, start),
            '|' if self.consume('|') => self.push_token(TokenKind::PipePipe, start),
            character => self.push_unexpected_character(character, start),
        }
    }

    fn scan_whitespace(&mut self, start: usize) {
        while self.current().is_some_and(char::is_whitespace) {
            self.advance();
        }
        self.push_token(TokenKind::Whitespace, start);
    }

    fn scan_identifier(&mut self, start: usize) {
        while self.current().is_some_and(is_identifier_continue) {
            self.advance();
        }

        let text = &self.source[start..self.cursor];
        self.push_token(keyword_kind(text).unwrap_or(TokenKind::Identifier), start);
    }

    fn scan_number(&mut self, start: usize) {
        while self.current().is_some_and(|value| value.is_ascii_digit()) {
            self.advance();
        }

        let kind = if self.current() == Some('.')
            && self.following().is_some_and(|value| value.is_ascii_digit())
        {
            self.advance();
            while self.current().is_some_and(|value| value.is_ascii_digit()) {
                self.advance();
            }
            TokenKind::Float
        } else {
            TokenKind::Integer
        };

        self.push_token(kind, start);
    }

    fn scan_string(&mut self, start: usize) {
        let mut terminated = false;

        while let Some(character) = self.current() {
            match character {
                '"' => {
                    self.advance();
                    terminated = true;
                    break;
                }
                '\n' | '\r' => break,
                '\\' => self.scan_escape(),
                _ => {
                    self.advance();
                }
            }
        }

        if !terminated {
            self.errors.push(LexError {
                kind: LexErrorKind::UnterminatedString,
                span: Span::from_valid_bounds(start, self.cursor),
            });
        }

        self.push_token(TokenKind::String, start);
    }

    fn scan_escape(&mut self) {
        let start = self.cursor;
        self.advance();

        let Some(character) = self.current() else {
            return;
        };

        if matches!(character, '\n' | '\r') {
            return;
        }

        self.advance();

        if !matches!(character, '0' | 'n' | 'r' | 't' | '\\' | '"') {
            self.errors.push(LexError {
                kind: LexErrorKind::InvalidEscape(character),
                span: Span::from_valid_bounds(start, self.cursor),
            });
        }
    }

    fn scan_line_comment(&mut self, start: usize) {
        while self
            .current()
            .is_some_and(|character| !matches!(character, '\n' | '\r'))
        {
            self.advance();
        }
        self.push_token(TokenKind::LineComment, start);
    }

    fn push_unexpected_character(&mut self, character: char, start: usize) {
        let span = Span::from_valid_bounds(start, self.cursor);
        self.tokens.push(Token {
            kind: TokenKind::Unknown,
            span,
        });
        self.errors.push(LexError {
            kind: LexErrorKind::UnexpectedCharacter(character),
            span,
        });
    }

    fn push_token(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::from_valid_bounds(start, self.cursor),
        });
    }

    fn current(&self) -> Option<char> {
        self.source[self.cursor..].chars().next()
    }

    fn following(&self) -> Option<char> {
        self.source[self.cursor..].chars().nth(1)
    }

    fn advance(&mut self) -> Option<char> {
        let character = self.current()?;
        self.cursor += character.len_utf8();
        Some(character)
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.current() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn keyword_kind(identifier: &str) -> Option<TokenKind> {
    Some(match identifier {
        "let" => TokenKind::Let,
        "const" => TokenKind::Const,
        "fn" => TokenKind::Fn,
        "struct" => TokenKind::Struct,
        "if" => TokenKind::If,
        "else" => TokenKind::Else,
        "while" => TokenKind::While,
        "return" => TokenKind::Return,
        "match" => TokenKind::Match,
        "async" => TokenKind::Async,
        "await" => TokenKind::Await,
        "parallel" => TokenKind::Parallel,
        "task" => TokenKind::Task,
        "using" => TokenKind::Using,
        "defer" => TokenKind::Defer,
        "true" => TokenKind::True,
        "false" => TokenKind::False,
        "null" => TokenKind::Null,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::lex;
    use crate::{LexErrorKind, TokenKind};

    fn significant_kinds(source: &str) -> Vec<TokenKind> {
        lex(source)
            .tokens
            .into_iter()
            .filter_map(|token| {
                (!token.kind.is_trivia() && token.kind != TokenKind::EndOfFile)
                    .then_some(token.kind)
            })
            .collect()
    }

    #[test]
    fn recognizes_keywords_and_unicode_identifiers() {
        let source = "let żółw = true const _value = null";

        assert_eq!(
            significant_kinds(source),
            vec![
                TokenKind::Let,
                TokenKind::Identifier,
                TokenKind::Equal,
                TokenKind::True,
                TokenKind::Const,
                TokenKind::Identifier,
                TokenKind::Equal,
                TokenKind::Null,
            ]
        );
        assert!(!lex(source).has_errors());
    }

    #[test]
    fn preserves_every_source_byte_in_tokens() {
        let source = "let value = 42 // comment\r\n@";
        let lexed = lex(source);
        let reconstructed = lexed
            .tokens
            .iter()
            .filter(|token| token.kind != TokenKind::EndOfFile)
            .map(|token| token.text(source).expect("token has a valid source span"))
            .collect::<String>();

        assert_eq!(reconstructed, source);
    }

    #[test]
    fn recognizes_numbers_without_consuming_ranges() {
        assert_eq!(
            significant_kinds("1 2.5 1..2 1..=2"),
            vec![
                TokenKind::Integer,
                TokenKind::Float,
                TokenKind::Integer,
                TokenKind::DotDot,
                TokenKind::Integer,
                TokenKind::Integer,
                TokenKind::DotDotEqual,
                TokenKind::Integer,
            ]
        );
    }

    #[test]
    fn uses_longest_matching_operator() {
        let source = "+ += - -= -> * *= ** / /= == => != <= >= && ||";

        assert_eq!(
            significant_kinds(source),
            vec![
                TokenKind::Plus,
                TokenKind::PlusEqual,
                TokenKind::Minus,
                TokenKind::MinusEqual,
                TokenKind::Arrow,
                TokenKind::Star,
                TokenKind::StarEqual,
                TokenKind::StarStar,
                TokenKind::Slash,
                TokenKind::SlashEqual,
                TokenKind::EqualEqual,
                TokenKind::FatArrow,
                TokenKind::BangEqual,
                TokenKind::LessEqual,
                TokenKind::GreaterEqual,
                TokenKind::AmpersandAmpersand,
                TokenKind::PipePipe,
            ]
        );
    }

    #[test]
    fn accepts_supported_string_escapes() {
        let source = r#""line\n\t\r\0\\\"""#;
        let lexed = lex(source);

        assert_eq!(significant_kinds(source), vec![TokenKind::String]);
        assert!(lexed.errors.is_empty());
    }

    #[test]
    fn reports_invalid_escape_and_keeps_string_token() {
        let source = r#""bad\q""#;
        let lexed = lex(source);

        assert_eq!(significant_kinds(source), vec![TokenKind::String]);
        assert_eq!(lexed.errors.len(), 1);
        assert_eq!(lexed.errors[0].kind, LexErrorKind::InvalidEscape('q'));
        assert_eq!(lexed.errors[0].span.text(source), Some("\\q"));
    }

    #[test]
    fn reports_unterminated_string_without_consuming_newline() {
        let source = "\"open\nlet";
        let lexed = lex(source);

        assert_eq!(lexed.errors.len(), 1);
        assert_eq!(lexed.errors[0].kind, LexErrorKind::UnterminatedString);
        assert_eq!(lexed.errors[0].span.text(source), Some("\"open"));
        assert!(
            lexed
                .tokens
                .iter()
                .any(|token| token.kind == TokenKind::Let)
        );
    }

    #[test]
    fn reports_unexpected_unicode_character_at_its_byte_span() {
        let source = "🙂";
        let lexed = lex(source);

        assert_eq!(lexed.errors.len(), 1);
        assert_eq!(
            lexed.errors[0].kind,
            LexErrorKind::UnexpectedCharacter('🙂')
        );
        assert_eq!(lexed.errors[0].span.start(), 0);
        assert_eq!(lexed.errors[0].span.end(), source.len());
    }

    #[test]
    fn emits_empty_end_of_file_token() {
        let lexed = lex("");

        assert_eq!(lexed.tokens.len(), 1);
        assert_eq!(lexed.tokens[0].kind, TokenKind::EndOfFile);
        assert!(lexed.tokens[0].span.is_empty());
    }
}
