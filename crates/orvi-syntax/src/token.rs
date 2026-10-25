use crate::Span;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TokenKind {
    Whitespace,
    LineComment,
    Identifier,
    Integer,
    Float,
    String,
    Let,
    Const,
    Fn,
    Struct,
    Interface,
    If,
    Else,
    While,
    Return,
    Match,
    Async,
    Await,
    Parallel,
    Task,
    Using,
    Defer,
    True,
    False,
    Null,
    LeftParenthesis,
    RightParenthesis,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Comma,
    Colon,
    Semicolon,
    Dot,
    DotDot,
    DotDotEqual,
    Question,
    Plus,
    PlusEqual,
    Minus,
    MinusEqual,
    Arrow,
    Star,
    StarEqual,
    StarStar,
    Slash,
    SlashEqual,
    Percent,
    Equal,
    EqualEqual,
    FatArrow,
    Bang,
    BangEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    AmpersandAmpersand,
    PipePipe,
    Unknown,
    EndOfFile,
}

impl TokenKind {
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::LineComment)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn text(self, source: &str) -> Option<&str> {
        self.span.text(source)
    }
}
