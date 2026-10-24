use std::error::Error;
use std::fmt;

use crate::Span;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LexErrorKind {
    UnexpectedCharacter(char),
    InvalidEscape(char),
    UnterminatedString,
}

impl fmt::Display for LexErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedCharacter(character) => {
                write!(formatter, "unexpected character '{character}'")
            }
            Self::InvalidEscape(character) => {
                write!(formatter, "invalid escape sequence '\\{character}'")
            }
            Self::UnterminatedString => formatter.write_str("unterminated string literal"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LexError {
    pub kind: LexErrorKind,
    pub span: Span,
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)
    }
}

impl Error for LexError {}
