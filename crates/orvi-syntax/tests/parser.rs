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

#[test]
fn parses_binding_parameter_and_return_types() {
    let parsed = parse("let port: int = 22; fn label(value: int) -> string { return \"ok\" }");
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);

    let StatementKind::Variable {
        type_annotation: Some(annotation),
        ..
    } = &parsed.program.statements[0].kind
    else {
        panic!("expected typed variable")
    };
    assert_eq!(annotation.name.name, "int");

    let StatementKind::Function {
        parameters,
        return_type: Some(return_type),
        ..
    } = &parsed.program.statements[1].kind
    else {
        panic!("expected typed function")
    };
    assert_eq!(
        parameters[0].type_annotation.as_ref().unwrap().name.name,
        "int"
    );
    assert_eq!(return_type.name.name, "string");
}

#[test]
fn parses_lists_indexing_and_index_assignment() {
    let parsed = parse("let values: list<int> = [1, 2]; values[0] = values[1]");
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
    let StatementKind::Variable {
        type_annotation: Some(annotation),
        initializer,
        ..
    } = &parsed.program.statements[0].kind
    else {
        panic!("expected typed list declaration")
    };
    assert_eq!(annotation.name.name, "list");
    assert_eq!(annotation.arguments[0].name.name, "int");
    assert!(matches!(initializer.kind, ExpressionKind::List(_)));
    let StatementKind::Expression(expression) = &parsed.program.statements[1].kind else {
        panic!("expected assignment")
    };
    assert!(matches!(
        expression.kind,
        ExpressionKind::IndexAssign { .. }
    ));
}

#[test]
fn parses_struct_fields_defaults_and_member_assignment() {
    let parsed = parse(
        "struct Server { host: string; port: int = 22 } let server = Server(\"localhost\"); server.port = 8080",
    );
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
    let StatementKind::Struct { name, fields } = &parsed.program.statements[0].kind else {
        panic!("expected struct declaration")
    };
    assert_eq!(name.name, "Server");
    assert_eq!(fields.len(), 2);
    assert!(fields[0].default.is_none());
    assert!(fields[1].default.is_some());
    let StatementKind::Expression(expression) = &parsed.program.statements[2].kind else {
        panic!("expected field assignment")
    };
    assert!(matches!(
        expression.kind,
        ExpressionKind::MemberAssign { .. }
    ));
}

#[test]
fn parses_match_patterns_and_try_operator() {
    let parsed = parse(
        "fn unwrap(value: Option<int>) -> Option<int> { let inner = value?; return Some(match inner { 0 => 1, _ => inner }) }",
    );
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
    let StatementKind::Function { body, .. } = &parsed.program.statements[0].kind else {
        panic!("expected function")
    };
    let StatementKind::Variable { initializer, .. } = &body[0].kind else {
        panic!("expected binding")
    };
    assert!(matches!(initializer.kind, ExpressionKind::Try(_)));
}
