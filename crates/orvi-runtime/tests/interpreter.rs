use orvi_runtime::{
    ExecutionContext, TaskStatus, execute, execute_file, execute_tasks, execute_with_context,
};
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

fn run_in(source: &str, directory: &std::path::Path) -> Result<Vec<String>, String> {
    let parsed = parse(source);
    if parsed.has_errors() {
        return Err(format!(
            "syntax errors: {:?} {:?}",
            parsed.lexical_errors, parsed.parse_errors
        ));
    }
    execute_with_context(
        &parsed.program,
        ExecutionContext {
            cwd: directory.into(),
            script_dir: directory.into(),
            project_dir: directory.into(),
        },
    )
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
fn runs_lazy_async_functions_with_top_level_await() {
    let source = r#"
        async fn compute(value: int) -> int {
            print("started");
            return value * 2;
        }
        async fn succeed() -> Result<int, string> { return Ok(7); }
        async fn use_success() -> Result<int, string> {
            let value = (await succeed())?;
            return Ok(value);
        }

        let abandoned: Future<int> = compute(1);
        let future: Future<int> = compute(21);
        print("before");
        print(await future);
        print(await future);
        print(await use_success());
    "#;
    let output = run(source).expect("async program runs");
    assert_eq!(output, ["before", "started", "42", "42", "Ok(7)"]);
}

#[test]
fn runs_spawned_sleeps_concurrently_on_one_thread() {
    let source = r#"
        async fn delayed(value: string) -> Result<string, string> {
            (await time.sleep_async(120))?;
            return Ok(value);
        }

        async fn both() -> Result<void, string> {
            let first: Task<Result<string, string>> = task.spawn(delayed("first"));
            let second: Task<Result<string, string>> = task.spawn(delayed("second"));
            print((await task.join(first))?);
            print((await task.join(second))?);
            return Ok();
        }

        print(await both());
    "#;
    let started = std::time::Instant::now();
    let output = run(source).expect("concurrent async program runs");
    let elapsed = started.elapsed();
    assert_eq!(output, ["Ok(first)", "Ok(second)", "Ok()"]);
    assert!(
        elapsed < std::time::Duration::from_millis(210),
        "two 120 ms sleeps took {elapsed:?}"
    );
}

#[test]
fn waits_for_unjoined_tasks_before_leaving_their_scope() {
    let source = r#"
        async fn child() -> Result<void, string> {
            (await time.sleep_async(5))?;
            print("child finished");
            return Ok();
        }

        async fn parent() -> Result<void, string> {
            task.spawn(child());
            print("parent body finished");
            return Ok();
        }

        print(await parent());
    "#;
    let output = run(source).expect("scoped task program runs");
    assert_eq!(output, ["parent body finished", "child finished", "Ok()"]);
}

#[test]
fn runs_ten_thousand_scoped_tasks_and_enforces_the_limit() {
    let source = r#"
        async fn tiny(value: int) -> int { return value }
        let index = 0;
        while index < 10000 {
            task.spawn(tiny(index));
            index = index + 1;
        }
    "#;
    let started = std::time::Instant::now();
    run(source).expect("10,000 scoped tasks run");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "10,000 short tasks exceeded the regression budget"
    );

    let over_limit = source.replace("index < 10000", "index < 10001");
    let error = run(&over_limit).expect_err("10,001st task must be rejected");
    assert!(error.contains("scope limit of 10000 tasks"), "{error}");
}

#[test]
fn cancellation_and_timeout_run_deferred_cleanup() {
    let source = r#"
        async fn cancellable(label: string) -> Result<void, string> {
            defer print("cleanup", label);
            (await time.sleep_async(5000))?;
            return Ok();
        }

        async fn exercise() -> Result<void, string> {
            let handle = task.spawn(cancellable("cancel"));
            (await time.sleep_async(5))?;
            print(task.cancel(handle));
            print(await task.join(handle));
            print(await task.timeout(cancellable("timeout"), 5));
            return Ok();
        }

        print(await exercise());
    "#;
    let started = std::time::Instant::now();
    let output = run(source).expect("cancellation program runs");
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert_eq!(
        output,
        [
            "true",
            "cleanup cancel",
            "Err(task was cancelled)",
            "cleanup timeout",
            "Err(operation timed out)",
            "Ok()",
        ]
    );
}

#[test]
fn performs_async_file_io() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = r#"
        async fn round_trip() -> Result<string, string> {
            (await fs.write_text_async("async.txt", "hello async"))?;
            return await fs.read_text_async("async.txt");
        }

        print(await round_trip());
    "#;
    let output = run_in(source, directory.path()).expect("async file I/O runs");
    assert_eq!(output, ["Ok(hello async)"]);
}

#[test]
fn runs_processes_without_blocking_the_executor() {
    let source = r#"
        async fn version() -> Result<map<any>, string> {
            return await process.run_async("rustc", ["--version"]);
        }

        print(await version());
    "#;
    let output = run(source).expect("async process runs");
    assert!(output[0].starts_with("Ok({ exit_code: 0, success: true"));
    assert!(output[0].contains("rustc"));
}

#[test]
fn performs_http_requests_concurrently() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
    let address = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let mut workers = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("request");
            workers.push(std::thread::spawn(move || {
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).expect("read request");
                std::thread::sleep(std::time::Duration::from_millis(150));
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                    )
                    .expect("write response");
            }));
        }
        for worker in workers {
            worker.join().expect("request worker");
        }
    });
    let source = format!(
        r#"
            async fn fetch(url: string) -> Result<string, string> {{
                let response = (await http.get_async(url))?;
                return Ok(response.body);
            }}

            async fn both() -> Result<void, string> {{
                let first = task.spawn(fetch("http://{address}"));
                let second = task.spawn(fetch("http://{address}"));
                print((await task.join(first))?);
                print((await task.join(second))?);
                return Ok();
            }}

            print(await both());
        "#
    );
    let started = std::time::Instant::now();
    let output = run(&source).expect("async HTTP runs");
    let elapsed = started.elapsed();
    server.join().expect("server exits");
    assert_eq!(output, ["Ok(hello)", "Ok(hello)", "Ok()"]);
    assert!(
        elapsed < std::time::Duration::from_millis(280),
        "two 150 ms requests took {elapsed:?}"
    );
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

#[test]
fn reads_writes_compares_and_collects_maps() {
    let source = "let data: map<int> = { count: 1, other: 2 }; data[\"count\"] = 3; data.extra = 4; print(data.count); print(data == { other: 2, count: 3, extra: 4 }); print(data)";
    assert_eq!(
        run(source).expect("program runs"),
        ["3", "true", "{ count: 3, other: 2, extra: 4 }"]
    );

    let source = "{ let cycle = {}; cycle.self = cycle; print(cycle); } print(\"alive\")";
    let parsed = parse(source);
    assert!(!parsed.has_errors());
    let execution = execute(&parsed.program).expect("program runs");
    assert_eq!(execution.output, ["{ self: <cycle> }", "alive"]);
    assert!(execution.collected_objects >= 1);
}

#[test]
fn parses_and_stringifies_json_without_panics() {
    let source = "fn count() -> Result<int, string> { let data = json.parse(\"{\\\"count\\\":4}\")?; return Ok(data.count) } print(count()); print(json.stringify({ name: \"orvi\", values: [1, true, null] })); print(json.parse(\"{\"))";
    assert_eq!(
        run(source).expect("program runs"),
        [
            "Ok(4)",
            "Ok({\"name\":\"orvi\",\"values\":[1,true,null]})",
            "Err(EOF while parsing an object at line 1 column 1)",
        ]
    );

    let source = "let cycle = {}; cycle.self = cycle; print(json.stringify(cycle))";
    assert_eq!(
        run(source).expect("cycle is returned as Err"),
        ["Err(JSON cannot represent cyclic values)"]
    );
}

#[test]
fn provides_files_paths_environment_processes_and_context() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = "print(fs.write_text(\"note.txt\", \"hello\")); print(fs.exists(\"note.txt\")); print(fs.read_text(\"note.txt\")); print(path.file_name(path.join([\"folder\", \"note.txt\"]))); print(path.parent(\"folder/note.txt\")); print(path.is_absolute(context.cwd)); print(env.current_dir()); print(fs.remove_file(\"note.txt\")); print(fs.exists(\"note.txt\"))";
    let output = run_in(source, directory.path()).expect("automation APIs run");
    assert_eq!(output[0], "Ok()");
    assert_eq!(output[1], "true");
    assert_eq!(output[2], "Ok(hello)");
    assert_eq!(output[3], "Some(note.txt)");
    assert_eq!(output[4], "Some(folder)");
    assert_eq!(output[5], directory.path().is_absolute().to_string());
    assert_eq!(
        output[6],
        format!("Ok({})", directory.path().to_string_lossy())
    );
    assert_eq!(output[7], "Ok()");
    assert_eq!(output[8], "false");

    let source = if cfg!(windows) {
        "print(process.run(\"cmd\", [\"/C\", \"echo orvi\"]))"
    } else {
        "print(process.run(\"sh\", [\"-c\", \"printf orvi\"]))"
    };
    let output = run_in(source, directory.path()).expect("process runs");
    assert!(output[0].starts_with("Ok({ exit_code: 0, success: true, stdout: orvi"));
}

#[test]
fn imports_local_modules_once_and_standard_module_members() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let module_path = directory.path().join("math.orvi");
    std::fs::write(
        &module_path,
        "print(\"loaded\"); export const base: int = 40; export fn add(value: int) -> int { return base + value }",
    )
    .expect("module source");
    let entry_path = directory.path().join("main.orvi");
    let source = "import { base } from \"./math.orvi\"; import { add } from \"./math.orvi\"; import { exists } from \"std:fs\"; print(add(2)); print(base); print(exists(\"math.orvi\"))";
    std::fs::write(&entry_path, source).expect("entry source");
    let parsed = parse(source);
    assert!(!parsed.has_errors());
    let execution = execute_file(
        &parsed.program,
        ExecutionContext {
            cwd: directory.path().into(),
            script_dir: directory.path().into(),
            project_dir: directory.path().into(),
        },
        entry_path,
    )
    .expect("imports run");
    assert_eq!(execution.output, ["loaded", "42", "40", "true"]);
}

#[test]
fn keeps_unexported_module_bindings_private() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let module_path = directory.path().join("module.orvi");
    std::fs::write(&module_path, "const hidden = 1; export const visible = 2;")
        .expect("module source");
    let entry_path = directory.path().join("main.orvi");
    let source = "import { hidden } from \"./module.orvi\"; print(hidden)";
    let parsed = parse(source);
    let error = execute_file(
        &parsed.program,
        ExecutionContext {
            cwd: directory.path().into(),
            script_dir: directory.path().into(),
            project_dir: directory.path().into(),
        },
        entry_path,
    )
    .expect_err("private binding must not be importable");
    assert!(error.message.contains("has no export 'hidden'"));
}

#[test]
fn distinguishes_void_from_null_and_supports_empty_ok() {
    let source = r#"
        fn finish() -> void { return }
        fn empty() -> Result<void, string> { return Ok() }
        print(finish());
        print(empty());
    "#;
    let output = run(source).expect("void program runs");
    assert_eq!(output, ["void", "Ok()"]);
}

#[test]
fn runs_task_dag_once_and_exposes_dependency_results() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let entry = directory.path().join("tasks.orvi");
    let source = r#"
        task build -> Result<string, string> {
            print("build");
            return Ok("dist");
        }
        task deploy -> Result<string, string> {
            needs: [build];
            let output = task.result("build")?;
            print("deploy", output);
            return Ok(output);
        }
    "#;
    std::fs::write(&entry, source).expect("task source");
    let parsed = parse(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
    let execution = execute_tasks(
        &parsed.program,
        ExecutionContext {
            cwd: directory.path().into(),
            script_dir: directory.path().into(),
            project_dir: directory.path().into(),
        },
        entry,
        &["deploy".into()],
    )
    .expect("task execution");
    assert_eq!(execution.output, ["build", "deploy dist"]);
    assert_eq!(execution.tasks[0].status, TaskStatus::Succeeded);
    assert_eq!(execution.tasks[1].status, TaskStatus::Succeeded);
}

#[test]
fn skips_dependents_after_task_failure_and_rejects_cycles() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let entry = directory.path().join("tasks.orvi");
    let source = r#"
        task build -> Result<null, string> { return Err("failed") }
        task deploy -> Result<null, string> { needs: [build]; print("must not run"); return Ok(null) }
    "#;
    let parsed = parse(source);
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
    let execution = execute_tasks(
        &parsed.program,
        ExecutionContext::default(),
        entry.clone(),
        &["deploy".into()],
    )
    .expect("failed tasks are reports");
    assert!(execution.output.is_empty());
    assert_eq!(execution.tasks[0].status, TaskStatus::Failed);
    assert_eq!(execution.tasks[1].status, TaskStatus::Skipped);

    let parsed = parse(
        "task first -> Result<null, string> { needs: [second]; return Ok(null) } task second -> Result<null, string> { needs: [first]; return Ok(null) }",
    );
    assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
    let error = execute_tasks(
        &parsed.program,
        ExecutionContext::default(),
        entry,
        &["first".into()],
    )
    .expect_err("task cycle must fail before execution");
    assert!(error.message.contains("cyclic task dependency"));
}

#[test]
fn keeps_task_scope_alive_while_calling_a_function() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let entry = directory.path().join("tasks.orvi");
    let source = r#"
        fn helper() -> int { return 1 }
        task test_scope -> Result<string, string> {
            let retained = "alive";
            helper();
            print(retained);
            return Ok(retained);
        }
    "#;
    let parsed = parse(source);
    let execution = execute_tasks(
        &parsed.program,
        ExecutionContext::default(),
        entry,
        &["test_scope".into()],
    )
    .expect("task execution");
    assert_eq!(execution.output, ["alive"]);
    assert_eq!(execution.tasks[0].status, TaskStatus::Succeeded);
}

#[test]
fn provides_http_and_time_modules() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("local listener");
    let address = listener.local_addr().expect("listener address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let mut request = [0_u8; 1024];
        let _ = stream.read(&mut request).expect("read request");
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
            )
            .expect("write response");
    });
    let source = format!(
        "print(http.get(\"http://{address}\")); print(time.unix_ms()); print(time.monotonic_ms()); print(time.sleep_ms(0))"
    );
    let output = run(&source).expect("standard modules run");
    server.join().expect("server exits");
    assert!(output[0].starts_with("Ok({ status: 200"), "{}", output[0]);
    assert!(output[0].contains("body: hello"), "{}", output[0]);
    assert!(output[1].starts_with("Ok("));
    assert!(output[2].parse::<i64>().is_ok());
    assert_eq!(output[3], "Ok()");
    assert_eq!(
        run("print(http.get(\"file:///secret\"))").unwrap(),
        ["Err(http URL must use http:// or https://)"]
    );
}

#[test]
fn imports_exports_from_path_packages() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let app = directory.path().join("app");
    let package = directory.path().join("math");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(package.join("src")).unwrap();
    std::fs::write(
        app.join("orvi.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n\n[dependencies]\nmath = { path = \"../math\" }\n",
    )
    .unwrap();
    std::fs::write(
        package.join("orvi.toml"),
        "[project]\nname = \"math\"\nversion = \"1.0.0\"\nentry = \"src/main.orvi\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("src/main.orvi"),
        "export fn double(value: int) -> int { return value * 2 }",
    )
    .unwrap();
    let source = "import { double } from \"pkg:math\"; print(double(21))";
    let entry = app.join("src/main.orvi");
    std::fs::write(&entry, source).unwrap();
    let parsed = parse(source);
    let execution = execute_file(
        &parsed.program,
        ExecutionContext {
            cwd: app.clone(),
            script_dir: app.join("src"),
            project_dir: app,
        },
        entry,
    )
    .expect("package import runs");
    assert_eq!(execution.output, ["42"]);
}
