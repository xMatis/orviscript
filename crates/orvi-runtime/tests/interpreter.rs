use orvi_runtime::execute;
use orvi_syntax::parse;

fn run(source: &str) -> Result<Vec<String>, String> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return Err(format!(
            "syntax errors: {:?} {:?}",
            parsed.lexical_errors, parsed.parse_errors
        ));
    }
    execute(&parsed.program)
        .map(|execution| execution.output)
        .map_err(|error| error.message)
}

#[test]
fn evaluates_variables_and_arithmetic() {
    let output =
        run("let value = 2 + 3 * 4; value = value - 1; print(value)").expect("program runs");
    assert_eq!(output, ["13"]);
}

#[test]
fn executes_if_else_and_while() {
    let output = run("let n = 0; while n < 3 { n = n + 1 } if n == 3 { print(\"done\") } else { print(\"bad\") }").expect("program runs");
    assert_eq!(output, ["done"]);
}

#[test]
fn calls_recursive_functions() {
    let source = "fn factorial(n) { if n <= 1 { return 1 } return n * factorial(n - 1) } print(factorial(6))";
    assert_eq!(run(source).expect("program runs"), ["720"]);
}

#[test]
fn closures_keep_their_defining_environment() {
    let source = "fn counter() { let value = 0; fn next() { value = value + 1; return value } return next } let next = counter(); print(next()); print(next())";
    assert_eq!(run(source).expect("program runs"), ["1", "2"]);
}

#[test]
fn constants_cannot_be_reassigned() {
    let error = run("const answer = 42; answer = 0").expect_err("assignment fails");
    assert_eq!(error, "cannot assign to constant 'answer'");
}

#[test]
fn conditions_require_boolean_values() {
    let error = run("if 1 { print(\"no\") }").expect_err("condition fails");
    assert_eq!(error, "condition must be bool");
}

#[test]
fn reports_division_by_zero() {
    let error = run("print(10 / 0)").expect_err("division fails");
    assert_eq!(error, "division by zero");
}

#[test]
fn print_accepts_multiple_values() {
    assert_eq!(
        run("print(\"value\", 4, true, null)").expect("program runs"),
        ["value 4 true null"]
    );
}
