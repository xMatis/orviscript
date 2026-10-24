use orvi_syntax::{LexErrorKind, TokenKind, lex, line_column};

#[test]
fn lexes_representative_orvi_program() {
    let source = r#"
struct Server {
    host: string
    port: int = 22

    fn address(self) -> string {
        return self.host + ":" + string(self.port)
    }
}

task deploy -> Result<void, Error> {
    let outcome = process.run("npm", ["run", "build"])?
    if outcome.exitCode != 0 { return Err(Error("build failed")) }
    return Ok()
}
"#;

    let lexed = lex(source);

    assert!(lexed.errors.is_empty());
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Struct)
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Task)
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Arrow)
    );
    assert!(
        lexed
            .tokens
            .iter()
            .any(|token| token.kind == TokenKind::Question)
    );
}

#[test]
fn every_token_stream_is_contiguous_and_lossless() {
    let cases = [
        "",
        "let value = 42",
        "const message = \"hello\\nworld\" // greeting\r\n",
        "1..=10",
        "żółw + 🙂",
        "\"unterminated\nnext",
    ];

    for source in cases {
        let lexed = lex(source);
        let mut expected_start = 0;

        for token in &lexed.tokens {
            assert_eq!(token.span.start(), expected_start);
            assert!(token.text(source).is_some());
            expected_start = token.span.end();
        }

        assert_eq!(expected_start, source.len());
        assert_eq!(
            lexed.tokens.last().map(|token| token.kind),
            Some(TokenKind::EndOfFile)
        );
    }
}

#[test]
fn diagnostics_map_to_human_readable_positions() {
    let source = "let value = 1\n  🙂";
    let lexed = lex(source);
    let error = lexed.errors.first().expect("unexpected character error");

    assert_eq!(error.kind, LexErrorKind::UnexpectedCharacter('🙂'));
    assert_eq!(
        line_column(source, error.span.start()).map(|position| (position.line, position.column)),
        Some((2, 3))
    );
}

#[test]
fn single_boolean_operator_characters_are_recoverable_errors() {
    let source = "left & right | value";
    let lexed = lex(source);

    assert_eq!(lexed.errors.len(), 2);
    assert_eq!(
        lexed
            .tokens
            .iter()
            .filter(|token| token.kind == TokenKind::Unknown)
            .count(),
        2
    );
    assert!(lexed.tokens.iter().any(|token| {
        token.kind == TokenKind::Identifier && token.text(source) == Some("value")
    }));
}
