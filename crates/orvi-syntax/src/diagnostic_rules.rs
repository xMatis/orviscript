use crate::{Span, Token, TokenKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticContext {
    FunctionDeclaration,
    VariableDeclaration,
    Condition,
    Loop,
    TypePosition,
    Import,
    Return,
    Expression,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuleSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleDiagnostic {
    pub id: &'static str,
    pub category: &'static str,
    pub message: &'static str,
    pub replacement: Option<&'static str>,
    pub severity: RuleSeverity,
    pub span: Span,
    pub priority: i32,
    pub source_languages: &'static [&'static str],
}

struct GeneratedRule {
    id: &'static str,
    category: &'static str,
    token: &'static str,
    context: DiagnosticContext,
    message: &'static str,
    replacement: &'static str,
    severity: RuleSeverity,
    quick_fix: bool,
    priority: i32,
    source_languages: &'static [&'static str],
}

struct GeneratedBucket {
    token: &'static str,
    start: usize,
    end: usize,
}

fn diagnostic_token_hash(token: &str) -> usize {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in token.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as usize
}

include!(concat!(env!("OUT_DIR"), "/diagnostic_rules.rs"));

const MAX_RULE_DIAGNOSTICS: usize = 100;

pub fn diagnostic_rules_hash() -> &'static str {
    DIAGNOSTIC_RULES_HASH
}

pub fn compiled_diagnostic_rules() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/diagnostic-rules.compiled.json"))
}

pub fn rule_diagnostics_for_tokens(source: &str, tokens: &[Token]) -> Vec<RuleDiagnostic> {
    let mut diagnostics = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        if diagnostics.len() == MAX_RULE_DIAGNOSTICS {
            break;
        }
        if matches!(token.kind, TokenKind::String | TokenKind::EndOfFile)
            || (!GENERATED_USES_NON_IDENTIFIERS && token.kind != TokenKind::Identifier)
        {
            continue;
        }
        let Some(text) = token.text(source) else {
            continue;
        };
        if let Some(rule) = generated_candidates(text)
            .iter()
            .filter(|rule| rule.token == text && context_matches(rule.context, tokens, index))
            .max_by_key(|rule| rule.priority)
        {
            diagnostics.push(RuleDiagnostic {
                id: rule.id,
                category: rule.category,
                message: rule.message,
                replacement: rule.quick_fix.then_some(rule.replacement),
                severity: rule.severity,
                span: token.span,
                priority: rule.priority,
                source_languages: rule.source_languages,
            });
        }
    }
    diagnostics
}

fn context_matches(context: DiagnosticContext, tokens: &[Token], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    let previous = index.checked_sub(1).and_then(|index| tokens.get(index));
    let previous_previous = index.checked_sub(2).and_then(|index| tokens.get(index));
    let next = tokens.get(index + 1);
    let following = tokens.get(index + 2);
    let at_declaration_boundary = previous.is_none_or(|token| {
        matches!(
            token.kind,
            TokenKind::LeftBrace
                | TokenKind::RightBrace
                | TokenKind::Semicolon
                | TokenKind::Export
                | TokenKind::Async
        )
    });
    match context {
        DiagnosticContext::FunctionDeclaration => {
            token.kind == TokenKind::Identifier
                && at_declaration_boundary
                && next.is_some_and(|token| token.kind == TokenKind::Identifier)
                && following.is_some_and(|token| token.kind == TokenKind::LeftParenthesis)
        }
        DiagnosticContext::VariableDeclaration => {
            token.kind == TokenKind::Identifier
                && at_declaration_boundary
                && next.is_some_and(|token| token.kind == TokenKind::Identifier)
                && following.is_none_or(|token| token.kind != TokenKind::LeftParenthesis)
        }
        DiagnosticContext::Condition => {
            previous.is_some_and(|token| matches!(token.kind, TokenKind::If | TokenKind::While))
                || (previous.is_some_and(|token| token.kind == TokenKind::LeftParenthesis)
                    && previous_previous.is_some_and(|token| {
                        matches!(token.kind, TokenKind::If | TokenKind::While)
                    }))
        }
        DiagnosticContext::Loop => token.kind == TokenKind::Identifier && at_declaration_boundary,
        DiagnosticContext::TypePosition => previous.is_some_and(|token| {
            matches!(
                token.kind,
                TokenKind::Colon | TokenKind::Arrow | TokenKind::Less
            )
        }),
        DiagnosticContext::Import => {
            at_declaration_boundary
                || previous
                    .is_some_and(|token| matches!(token.kind, TokenKind::Import | TokenKind::From))
        }
        DiagnosticContext::Return => {
            at_declaration_boundary || previous.is_some_and(|token| token.kind == TokenKind::Return)
        }
        DiagnosticContext::Expression => {
            !at_declaration_boundary
                || previous.is_some_and(|token| {
                    matches!(
                        token.kind,
                        TokenKind::Equal
                            | TokenKind::Comma
                            | TokenKind::LeftParenthesis
                            | TokenKind::LeftBracket
                            | TokenKind::Return
                            | TokenKind::FatArrow
                    )
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticContext, diagnostic_rules_hash, generated_candidates, rule_diagnostics_for_tokens,
    };
    use crate::lex;

    fn diagnose(source: &str) -> Vec<super::RuleDiagnostic> {
        let lexed = lex(source);
        let tokens = lexed
            .tokens
            .into_iter()
            .filter(|token| !token.kind.is_trivia())
            .collect::<Vec<_>>();
        rule_diagnostics_for_tokens(source, &tokens)
    }

    #[test]
    fn generated_catalog_is_indexed_by_token() {
        assert_eq!(diagnostic_rules_hash().len(), 16);
        let candidates = generated_candidates("function");
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].context,
            DiagnosticContext::FunctionDeclaration
        );
        assert!(generated_candidates("missing").is_empty());
    }

    #[test]
    fn matches_only_function_declaration_context() {
        let source = "function greet() {}";
        let diagnostics = diagnose(source);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].id, "ORVI1001");
        assert_eq!(diagnostics[0].replacement, Some("fn"));

        for source in [
            "let function = 1;",
            "print(function);",
            "\"function greet() {}\";",
            "// function greet() {}\nlet value = 1;",
        ] {
            assert!(diagnose(source).is_empty());
        }
    }

    #[test]
    fn limits_rule_diagnostics_per_file() {
        let source = (0..150)
            .map(|number| format!("function example_{number}() {{}}\n"))
            .collect::<String>();
        assert_eq!(diagnose(&source).len(), super::MAX_RULE_DIAGNOSTICS);
    }
}
