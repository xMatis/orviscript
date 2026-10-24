use orvi_syntax::{BinaryOperator, ExpressionKind, StatementKind, parse};

#[test]
fn applies_standard_arithmetic_precedence() {
    let parsed = parse("let value = 2 + 3 * 4");
    assert!(!parsed.has_errors());
    let StatementKind::Variable { initializer, .. } = &parsed.program.statements[0].kind else {
        panic!("expected variable declaration")
    };
    let ExpressionKind::Binary {
        operator, right, ..
    } = &initializer.kind
    else {
        panic!("expected addition")
    };
    assert_eq!(*operator, BinaryOperator::Add);
    assert!(matches!(
        right.kind,
        ExpressionKind::Binary {
            operator: BinaryOperator::Multiply,
            ..
        }
    ));
}

#[test]
fn parses_control_flow_and_functions() {
    let parsed = parse("fn choose(value) { if value { return 1 } else { return 2 } }");
    assert!(!parsed.has_errors());
    assert!(matches!(
        parsed.program.statements[0].kind,
        StatementKind::Function { .. }
    ));
}

#[test]
fn reports_missing_initializer() {
    let parsed = parse("let value =");
    assert!(parsed.has_errors());
    assert!(
        parsed
            .parse_errors
            .iter()
            .any(|error| error.message == "expected expression")
    );
}

#[test]
fn reports_invalid_assignment_target() {
    let parsed = parse("(1 + 2) = 3");
    assert!(
        parsed
            .parse_errors
            .iter()
            .any(|error| error.message == "invalid assignment target")
    );
}
