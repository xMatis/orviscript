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

#[test]
fn enforces_binding_types_on_initialization_and_assignment() {
    assert_eq!(
        run("let value: int = \"wrong\"").expect_err("initialization fails"),
        "expected int, received string"
    );
    assert_eq!(
        run("let value: int = 1; value = false").expect_err("assignment fails"),
        "expected int, received bool"
    );
}

#[test]
fn enforces_parameter_and_return_types() {
    assert_eq!(
        run("fn twice(value: int) -> int { return value * 2 } twice(\"wrong\")")
            .expect_err("argument fails"),
        "expected int, received string"
    );
    assert_eq!(
        run("fn wrong() -> int { return \"wrong\" } wrong()").expect_err("return value fails"),
        "expected int, received string"
    );
}

#[test]
fn reads_writes_and_compares_lists() {
    let source = "let values: list<int> = [1, 2, 3]; values[1] = 7; print(values[1]); print(values == [1, 7, 3]); print(values)";
    assert_eq!(
        run(source).expect("program runs"),
        ["7", "true", "[1, 7, 3]"]
    );
}

#[test]
fn reports_invalid_list_indices_and_element_types() {
    assert_eq!(
        run("let values = [1]; print(values[-1])").expect_err("negative index fails"),
        "list index must be non-negative"
    );
    assert_eq!(
        run("let values = [1]; print(values[1])").expect_err("large index fails"),
        "list index is out of bounds"
    );
    assert_eq!(
        run("let values: list<int> = [1, \"wrong\"]").expect_err("element type fails"),
        "expected list<int>, received list"
    );
}

#[test]
fn formats_and_compares_cyclic_lists_safely() {
    let source = "let left = [null]; left[0] = left; let right = [null]; right[0] = right; print(left); print(left == right)";
    assert_eq!(run(source).expect("program runs"), ["[<cycle>]", "true"]);
}

#[test]
fn collects_unreachable_list_and_closure_cycles() {
    let source = "{ let cycle = [null]; cycle[0] = cycle; } { fn recursive() { return recursive } } print(\"alive\")";
    let parsed = parse(source);
    assert!(!parsed.has_errors());
    let execution = execute(&parsed.program).expect("program runs");

    assert_eq!(execution.output, ["alive"]);
    assert!(execution.collected_objects >= 2);
}

#[test]
fn constructs_reads_and_updates_structs() {
    let source = "struct Server { host: string; port: int = 22 } let server: Server = Server(\"localhost\"); print(server.port); server.port = 8080; print(server)";
    assert_eq!(
        run(source).expect("program runs"),
        ["22", "Server { host: localhost, port: 8080 }"]
    );
}

#[test]
fn enforces_struct_field_types_and_collects_cycles() {
    assert_eq!(
        run("struct Server { port: int } let server = Server(80); server.port = \"wrong\"")
            .expect_err("field assignment fails"),
        "expected int, received string"
    );

    let source = "{ struct Node { next: any = null } let node = Node(); node.next = node; print(node) } print(\"alive\")";
    let parsed = parse(source);
    assert!(!parsed.has_errors());
    let execution = execute(&parsed.program).expect("program runs");
    assert_eq!(execution.output, ["Node { next: <cycle> }", "alive"]);
    assert!(execution.collected_objects >= 1);
}

#[test]
fn matches_option_and_result_variants() {
    let source = "print(match Some(5) { Some(value) => value * 2, None => 0 }); print(match Err(\"failed\") { Ok(value) => value, Err(message) => message })";
    assert_eq!(run(source).expect("program runs"), ["10", "failed"]);
}

#[test]
fn propagates_none_and_err_with_try() {
    let source = "fn increment(value: Option<int>) -> Option<int> { let inner = value?; return Some(inner + 1) } fn work(value: Result<int, string>) -> Result<int, string> { return Ok(value? + 1) } print(increment(Some(4))); print(increment(None)); print(work(Ok(9))); print(work(Err(\"failed\")))";
    assert_eq!(
        run(source).expect("program runs"),
        ["Some(5)", "None", "Ok(10)", "Err(failed)"]
    );
}

#[test]
fn runs_defer_and_using_cleanup_in_lifo_order() {
    let source = "fn work() { defer print(\"first\"); using handle = \"db\" => print(\"close\", handle); defer print(\"last\"); return 7 } print(work())";
    assert_eq!(
        run(source).expect("program runs"),
        ["last", "close db", "first", "7"]
    );
}

#[test]
fn runs_cleanup_when_try_propagates() {
    let source = "fn work(value: Option<int>) -> Option<int> { defer print(\"cleanup\"); let inner = value?; return Some(inner) } print(work(None))";
    assert_eq!(run(source).expect("program runs"), ["cleanup", "None"]);
}

#[test]
fn uses_structs_through_structural_interfaces() {
    let source = "interface Named { name: string } struct Job { name: string; retries: int = 3 } fn label(value: Named) -> string { return value.name } print(label(Job(\"release\")))";
    assert_eq!(run(source).expect("program runs"), ["release"]);

    let error = run(
        "interface Named { name: string } struct Job { title: string } let value: Named = Job(\"release\")",
    )
    .expect_err("incompatible struct fails");
    assert_eq!(error, "expected Named, received Job");

    let source = "struct Job { name: string } interface Named { name: string } let value: Named = Job(\"release\"); print(value.name)";
    assert_eq!(
        run(source).expect("declaration order does not change compatibility"),
        ["release"]
    );
}
