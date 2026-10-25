use std::fmt;

use crate::{TokenKind, lex, parse};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FormatError {
    pub message: String,
}

impl fmt::Display for FormatError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for FormatError {}

pub fn format_source(source: &str) -> Result<String, FormatError> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return Err(FormatError {
            message: "cannot format source with syntax errors".into(),
        });
    }

    let lexed = lex(source);
    let mut formatter = Formatter::default();
    for token in lexed.tokens {
        let text = token.text(source).unwrap_or_default();
        formatter.token(token.kind, text);
    }
    Ok(formatter.finish())
}

struct Formatter {
    output: String,
    indent: usize,
    line_start: bool,
    pending_space: bool,
    pending_newline: bool,
    parentheses: usize,
    brackets: usize,
    previous: Option<TokenKind>,
    in_type: bool,
    type_depth: usize,
    pending_match: bool,
    match_scopes: Vec<(usize, usize, usize)>,
}

impl Default for Formatter {
    fn default() -> Self {
        Self {
            output: String::new(),
            indent: 0,
            line_start: true,
            pending_space: false,
            pending_newline: false,
            parentheses: 0,
            brackets: 0,
            previous: None,
            in_type: false,
            type_depth: 0,
            pending_match: false,
            match_scopes: Vec::new(),
        }
    }
}

impl Formatter {
    fn token(&mut self, kind: TokenKind, text: &str) {
        match kind {
            TokenKind::Whitespace => {
                self.pending_space = true;
                self.pending_newline |= text.contains('\n');
                return;
            }
            TokenKind::EndOfFile => return,
            TokenKind::LineComment => {
                if !self.line_start {
                    self.output.push(' ');
                }
                self.write_indent();
                self.output.push_str(text.trim_end());
                self.newline();
                self.previous = Some(kind);
                return;
            }
            _ => {}
        }

        if kind == TokenKind::RightBrace {
            if self
                .match_scopes
                .last()
                .is_some_and(|(indent, _, _)| *indent == self.indent)
            {
                self.match_scopes.pop();
            }
            self.indent = self.indent.saturating_sub(1);
            if self.previous != Some(TokenKind::LeftBrace) {
                self.newline();
            }
        } else if self.pending_newline
            && self.parentheses == 0
            && self.brackets == 0
            && !matches!(
                kind,
                TokenKind::Comma | TokenKind::Semicolon | TokenKind::Else
            )
            && self.previous != Some(TokenKind::LeftBrace)
        {
            self.newline();
        }

        if self.in_type
            && self.type_depth == 0
            && !matches!(kind, TokenKind::Identifier | TokenKind::Less)
        {
            self.in_type = false;
        }
        let needs_space = self.needs_space(kind);
        self.write_indent();
        if needs_space {
            self.output.push(' ');
        }

        match kind {
            TokenKind::LeftBrace => {
                self.output.push('{');
                self.indent += 1;
                if self.pending_match {
                    self.match_scopes
                        .push((self.indent, self.parentheses, self.brackets));
                    self.pending_match = false;
                }
                self.newline();
            }
            TokenKind::RightBrace => self.output.push('}'),
            TokenKind::Semicolon => {
                self.output.push(';');
                self.newline();
            }
            TokenKind::Comma => {
                self.output.push(',');
                if self.match_scopes.last() == Some(&(self.indent, self.parentheses, self.brackets))
                {
                    self.newline();
                } else {
                    self.pending_space = true;
                }
            }
            TokenKind::LeftParenthesis => {
                self.output.push('(');
                self.parentheses += 1;
            }
            TokenKind::RightParenthesis => {
                self.output.push(')');
                self.parentheses = self.parentheses.saturating_sub(1);
            }
            TokenKind::LeftBracket => {
                self.output.push('[');
                self.brackets += 1;
            }
            TokenKind::RightBracket => {
                self.output.push(']');
                self.brackets = self.brackets.saturating_sub(1);
            }
            TokenKind::Dot => self.output.push('.'),
            TokenKind::Colon => {
                self.output.push(':');
                self.pending_space = true;
                self.in_type = true;
            }
            _ => self.output.push_str(text),
        }

        if kind == TokenKind::Match {
            self.pending_match = true;
        }
        if kind == TokenKind::Arrow {
            self.in_type = true;
        } else if kind == TokenKind::Less && self.in_type {
            self.type_depth += 1;
        } else if kind == TokenKind::Greater && self.type_depth > 0 {
            self.type_depth -= 1;
        }
        if kind == TokenKind::RightBrace {
            self.pending_newline = true;
        } else if kind == TokenKind::Else || self.is_spaced_operator(kind) {
            self.pending_space = true;
        }
        self.previous = Some(kind);
        if !matches!(kind, TokenKind::Comma | TokenKind::Colon) {
            self.pending_newline = false;
        }
        if !self.is_spaced_operator(kind)
            && !matches!(kind, TokenKind::Else | TokenKind::Comma | TokenKind::Colon)
        {
            self.pending_space = false;
        }
    }

    fn needs_space(&self, kind: TokenKind) -> bool {
        let Some(previous) = self.previous else {
            return false;
        };
        if kind == TokenKind::Question
            || (kind == TokenKind::Less && self.in_type)
            || (kind == TokenKind::Greater && self.type_depth > 0)
            || (previous == TokenKind::Less && self.type_depth > 0)
        {
            return false;
        }
        if self.line_start
            || matches!(
                kind,
                TokenKind::RightParenthesis
                    | TokenKind::RightBracket
                    | TokenKind::RightBrace
                    | TokenKind::Comma
                    | TokenKind::Semicolon
                    | TokenKind::Dot
                    | TokenKind::Colon
            )
            || matches!(
                previous,
                TokenKind::LeftParenthesis | TokenKind::LeftBracket | TokenKind::Dot
            )
        {
            return false;
        }
        self.pending_space
            || self.is_spaced_operator(kind)
            || self.is_spaced_operator(previous)
            || kind == TokenKind::LeftBrace
            || (is_word(kind) && is_word(previous))
            || (kind == TokenKind::Else && previous == TokenKind::RightBrace)
    }

    fn is_spaced_operator(&self, kind: TokenKind) -> bool {
        is_operator(kind)
            && kind != TokenKind::Question
            && !(kind == TokenKind::Less && self.in_type)
            && !(kind == TokenKind::Greater && self.type_depth > 0)
    }

    fn write_indent(&mut self) {
        if self.line_start {
            self.output.push_str(&"    ".repeat(self.indent));
            self.line_start = false;
        }
    }

    fn newline(&mut self) {
        while self.output.ends_with(' ') {
            self.output.pop();
        }
        if !self.output.is_empty() && !self.output.ends_with('\n') {
            self.output.push('\n');
        }
        self.line_start = true;
        self.pending_space = false;
        self.pending_newline = false;
    }

    fn finish(mut self) -> String {
        while self.output.ends_with([' ', '\n']) {
            self.output.pop();
        }
        self.output.push('\n');
        self.output
    }
}

fn is_word(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Identifier
            | TokenKind::Integer
            | TokenKind::Float
            | TokenKind::String
            | TokenKind::Let
            | TokenKind::Const
            | TokenKind::Fn
            | TokenKind::Struct
            | TokenKind::Interface
            | TokenKind::If
            | TokenKind::Else
            | TokenKind::While
            | TokenKind::Return
            | TokenKind::Match
            | TokenKind::Using
            | TokenKind::Defer
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null
    )
}

fn is_operator(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Plus
            | TokenKind::PlusEqual
            | TokenKind::Minus
            | TokenKind::MinusEqual
            | TokenKind::Arrow
            | TokenKind::Star
            | TokenKind::StarEqual
            | TokenKind::StarStar
            | TokenKind::Slash
            | TokenKind::SlashEqual
            | TokenKind::Percent
            | TokenKind::Equal
            | TokenKind::EqualEqual
            | TokenKind::FatArrow
            | TokenKind::Bang
            | TokenKind::BangEqual
            | TokenKind::Less
            | TokenKind::LessEqual
            | TokenKind::Greater
            | TokenKind::GreaterEqual
            | TokenKind::AmpersandAmpersand
            | TokenKind::PipePipe
    )
}

#[cfg(test)]
mod tests {
    use super::format_source;

    #[test]
    fn formats_blocks_operators_and_comments() {
        let source = "fn add(a:int,b:int)->int{ // sum\n return a+b}\nlet result=add(1,2)";
        let formatted = format_source(source).expect("source formats");
        assert_eq!(
            formatted,
            "fn add(a: int, b: int) -> int {\n    // sum\n    return a + b\n}\nlet result = add(1, 2)\n"
        );
    }

    #[test]
    fn is_idempotent_and_preserves_comments() {
        let source = "// before\nlet values:list<int> = [1,2] // after\n";
        let once = format_source(source).expect("source formats");
        let twice = format_source(&once).expect("formatted source formats");
        assert_eq!(once, twice);
        assert!(once.contains("// before"));
        assert!(once.contains("// after"));
    }

    #[test]
    fn rejects_invalid_source() {
        assert!(format_source("let value =").is_err());
    }
}
