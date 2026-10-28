#[path = "../diagnostic_build_support.rs"]
#[allow(dead_code)]
mod diagnostic_build_support;

use diagnostic_build_support::{Fragment, Rule, RuleMatch, sort_rules, validate_rules};

fn rule(id: &str, token: &str, context: &str, priority: i32) -> Rule {
    Rule {
        id: id.into(),
        category: "foreign_syntax".into(),
        matcher: RuleMatch {
            token: token.into(),
            context: context.into(),
        },
        message: "message".into(),
        replacement: "fn".into(),
        severity: "error".into(),
        quick_fix: true,
        priority,
        source_languages: vec!["javascript".into()],
    }
}

#[test]
fn validates_unique_rules() {
    assert!(
        validate_rules(&[
            rule("ORVI1001", "function", "function_declaration", 100),
            rule("ORVI1002", "def", "function_declaration", 100),
        ])
        .is_ok()
    );
}

#[test]
fn rejects_duplicate_ids_and_ambiguous_triggers() {
    let duplicate = [
        rule("ORVI1001", "function", "function_declaration", 100),
        rule("ORVI1001", "def", "function_declaration", 100),
    ];
    assert!(
        validate_rules(&duplicate)
            .unwrap_err()
            .contains("duplicate rule id")
    );

    let conflict = [
        rule("ORVI1001", "function", "function_declaration", 100),
        rule("ORVI1002", "function", "function_declaration", 100),
    ];
    assert!(validate_rules(&conflict).unwrap_err().contains("conflict"));
}

#[test]
fn rejects_unknown_context_and_invalid_id() {
    assert!(
        validate_rules(&[rule("ORVI1001", "function", "unknown", 100)])
            .unwrap_err()
            .contains("unknown context")
    );
    assert!(
        validate_rules(&[rule("BAD", "function", "function_declaration", 100)])
            .unwrap_err()
            .contains("ORVI0000")
    );
}

#[test]
fn rejects_invalid_replacements_priorities_and_language_metadata() {
    let mut invalid_replacement = rule("ORVI1001", "function", "function_declaration", 100);
    invalid_replacement.replacement = "fn greet".into();
    assert!(
        validate_rules(&[invalid_replacement])
            .unwrap_err()
            .contains("replacement")
    );

    let mut invalid_priority = rule("ORVI1001", "function", "function_declaration", 10_001);
    assert!(
        validate_rules(&[invalid_priority.clone()])
            .unwrap_err()
            .contains("priority")
    );
    invalid_priority.priority = 100;
    invalid_priority.source_languages = vec!["javascript".into(), "javascript".into()];
    assert!(
        validate_rules(&[invalid_priority])
            .unwrap_err()
            .contains("source language")
    );
}

#[test]
fn parses_strict_fragments_and_sorts_deterministically() {
    let fragment: Fragment = serde_json::from_str(
        r#"{"rules":[{"id":"ORVI1002","category":"foreign_syntax","match":{"token":"def","context":"function_declaration"},"message":"message","replacement":"fn","severity":"error","quickFix":true,"priority":50}]}"#,
    )
    .unwrap();
    assert_eq!(fragment.rules.len(), 1);
    assert!(
        serde_json::from_str::<Fragment>(r#"{"rules":[],"unknown":true}"#).is_err(),
        "unknown fragment fields must be rejected"
    );

    let mut rules = vec![
        rule("ORVI1003", "function", "function_declaration", 10),
        rule("ORVI1002", "def", "function_declaration", 10),
        rule("ORVI1001", "function", "function_declaration", 20),
    ];
    sort_rules(&mut rules);
    assert_eq!(
        rules
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>(),
        ["ORVI1002", "ORVI1001", "ORVI1003"]
    );
}
