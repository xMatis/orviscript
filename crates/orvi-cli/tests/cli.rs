use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn source_file(source: &str) -> PathBuf {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is valid")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("orviscript-{id}-{}.orvi", std::process::id()));
    fs::write(&path, source).expect("temporary source can be written");
    path
}

fn run(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orviscript"))
        .args(arguments)
        .output()
        .expect("CLI starts")
}

fn run_in(arguments: &[&str], directory: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orviscript"))
        .args(arguments)
        .current_dir(directory)
        .output()
        .expect("CLI starts")
}

fn run_with_home(arguments: &[&str], home: &std::path::Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orviscript"))
        .args(arguments)
        .env("ORVISCRIPT_HOME", home)
        .output()
        .expect("CLI starts")
}

#[test]
fn initializes_a_project_without_overwriting_it() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is valid")
        .as_nanos();
    let project = std::env::temp_dir().join(format!("orviscript-init-{id}"));
    let project_text = project.to_str().expect("UTF-8 temporary path");

    let output = run(&["init", project_text]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.join("orvi.toml").is_file());
    assert!(project.join("src/main.orvi").is_file());
    assert!(project.join(".gitignore").is_file());
    assert!(
        fs::read_to_string(project.join("orvi.toml"))
            .unwrap()
            .contains("entry = \"src/main.orvi\"")
    );

    let second = run(&["init", project_text]);
    assert_eq!(second.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&second.stderr).contains("would overwrite"));
    fs::remove_dir_all(project).unwrap();
}

#[test]
fn runs_and_checks_the_current_project_without_a_file_argument() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is valid")
        .as_nanos();
    let project = std::env::temp_dir().join(format!("orviscript-current-project-{id}"));
    fs::create_dir_all(project.join("src/nested")).unwrap();
    fs::write(
        project.join("orvi.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
    )
    .unwrap();
    fs::write(project.join("src/main.orvi"), "print(6 * 7)").unwrap();

    let checked = run_in(&["check"], &project.join("src/nested"));
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let executed = run_in(&["run"], &project);
    assert!(executed.status.success());
    assert_eq!(String::from_utf8_lossy(&executed.stdout).trim(), "42");

    fs::remove_dir_all(project).unwrap();
}

#[test]
fn runs_a_program_from_disk() {
    let path = source_file("print(6 * 7)");
    let output = run(&["run", path.to_str().expect("UTF-8 temporary path")]);
    fs::remove_file(path).expect("temporary source can be removed");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "42");
}

#[test]
fn checks_valid_source() {
    let path = source_file("let value = 42");
    let output = run(&["check", path.to_str().expect("UTF-8 temporary path")]);
    fs::remove_file(path).expect("temporary source can be removed");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("Check passed:"));
}

#[test]
fn prints_positioned_syntax_diagnostics() {
    let path = source_file("let value = @");
    let output = run(&["check", path.to_str().expect("UTF-8 temporary path")]);
    fs::remove_file(path).expect("temporary source can be removed");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(":1:13: error: unexpected character '@'"));
    assert!(stderr.contains('^'));
}

#[test]
fn prints_semantic_diagnostics() {
    let path = source_file("let value = missing;");
    let output = run(&["check", path.to_str().expect("UTF-8 temporary path")]);
    fs::remove_file(path).expect("temporary source can be removed");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("error: undefined name 'missing'"));
}

#[test]
fn formats_source_and_checks_the_result() {
    let path = source_file("let value=1+2");
    let path_text = path.to_str().expect("UTF-8 temporary path");

    let before = run(&["fmt", "--check", path_text]);
    assert_eq!(before.status.code(), Some(1));

    let formatted = run(&["fmt", path_text]);
    assert!(formatted.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), "let value = 1 + 2\n");

    let after = run(&["fmt", "--check", path_text]);
    fs::remove_file(path).expect("temporary source can be removed");
    assert!(after.status.success());
}

#[test]
fn checks_import_exports_and_cycles() {
    let first = source_file("const hidden = 0; export const available = 1;");
    let second = first.with_file_name(format!(
        "{}-entry.orvi",
        first.file_stem().unwrap().to_string_lossy()
    ));
    let import_name = first.file_name().unwrap().to_string_lossy();
    fs::write(
        &second,
        format!("import {{ missing }} from \"./{import_name}\";"),
    )
    .unwrap();
    let output = run(&["check", second.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("has no export 'missing'"));

    fs::write(
        &second,
        format!("import {{ hidden }} from \"./{import_name}\";"),
    )
    .unwrap();
    let output = run(&["check", second.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("has no export 'hidden'"));

    fs::write(
        &first,
        format!(
            "import {{ second }} from \"./{}\"; let first = 1;",
            second.file_name().unwrap().to_string_lossy()
        ),
    )
    .unwrap();
    fs::write(
        &second,
        format!("import {{ first }} from \"./{import_name}\"; let second = 2;"),
    )
    .unwrap();
    let output = run(&["check", second.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cyclic module import"));

    fs::remove_file(first).unwrap();
    fs::remove_file(second).unwrap();
}

#[test]
fn checks_imported_function_signatures() {
    let module = source_file("export fn double(value: int) -> int { return value * 2; }");
    let entry = module.with_file_name(format!(
        "{}-typed-entry.orvi",
        module.file_stem().unwrap().to_string_lossy()
    ));
    fs::write(
        &entry,
        format!(
            "import {{ double }} from \"./{}\"; double(\"two\");",
            module.file_name().unwrap().to_string_lossy()
        ),
    )
    .unwrap();
    let output = run(&["check", entry.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("expected int, received string"));
    fs::remove_file(module).unwrap();
    fs::remove_file(entry).unwrap();
}

#[test]
fn runs_test_tasks_and_returns_failure_status() {
    let passing = source_file(
        "task setup -> Result<string, string> { return Ok(\"ready\") } task test_cli -> Result<null, string> { needs: [setup]; print(task.result(\"setup\")?); return Ok(null) }",
    );
    let output = run(&["test", passing.to_str().unwrap()]);
    fs::remove_file(passing).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("test test_cli: passed"));

    let failing = source_file("task test_failure -> Result<null, string> { return Err(\"no\") }");
    let output = run(&["test", failing.to_str().unwrap()]);
    fs::remove_file(failing).unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("test test_failure: failed"));
}

#[test]
fn links_lists_runs_and_unlinks_global_task_aliases() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is valid")
        .as_nanos();
    let home = std::env::temp_dir().join(format!("orviscript-registry-{id}"));
    fs::create_dir_all(&home).unwrap();
    let script = home.join("release.orvi");
    fs::write(
        &script,
        "task deploy -> Result<null, string> { print(\"deployed\"); return Ok(null) }",
    )
    .unwrap();
    let script_text = script.to_str().unwrap();

    let output = run_with_home(&["task", "link", "hosting.deploy", script_text], &home);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = run_with_home(&["task", "list"], &home);
    assert!(String::from_utf8_lossy(&output.stdout).contains("hosting.deploy"));
    let output = run_with_home(&["task", "run", "hosting.deploy"], &home);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("deployed"));
    let output = run_with_home(&["task", "unlink", "hosting.deploy"], &home);
    assert!(output.status.success());
    let output = run_with_home(&["task", "list"], &home);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("hosting.deploy"));

    fs::remove_dir_all(home).unwrap();
}

#[test]
fn manages_and_runs_local_path_dependencies() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is valid")
        .as_nanos();
    let base = std::env::temp_dir().join(format!("orviscript-dependencies-{id}"));
    let app = base.join("app");
    let math = base.join("math");
    fs::create_dir_all(app.join("src")).unwrap();
    fs::create_dir_all(math.join("src")).unwrap();
    fs::write(
        app.join("orvi.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
    )
    .unwrap();
    fs::write(
        math.join("orvi.toml"),
        "[project]\nname = \"math\"\nversion = \"1.0.0\"\nentry = \"src/main.orvi\"\n",
    )
    .unwrap();
    fs::write(
        math.join("src/main.orvi"),
        "export fn double(value: int) -> int { return value * 2 }",
    )
    .unwrap();
    fs::write(
        app.join("src/main.orvi"),
        "import { double } from \"pkg:math\"; print(double(21))",
    )
    .unwrap();

    let added = run_in(&["add", "math", "--path", math.to_str().unwrap()], &app);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    assert!(app.join("orvi.lock").is_file());
    let dependencies = run_in(&["deps"], &app);
    assert!(dependencies.status.success());
    assert!(String::from_utf8_lossy(&dependencies.stdout).contains("math 1.0.0"));
    let checked = run_in(&["check"], &app);
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let executed = run_in(&["run"], &app);
    assert!(executed.status.success());
    assert_eq!(String::from_utf8_lossy(&executed.stdout).trim(), "42");
    let removed = run_in(&["remove", "math"], &app);
    assert!(removed.status.success());
    assert!(
        !fs::read_to_string(app.join("orvi.toml"))
            .unwrap()
            .contains("math =")
    );

    fs::remove_dir_all(base).unwrap();
}

#[test]
fn discovers_lists_and_filters_tests_across_a_project() {
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is valid")
        .as_nanos();
    let project = std::env::temp_dir().join(format!("orviscript-test-project-{id}"));
    fs::create_dir_all(project.join("src")).unwrap();
    fs::write(
        project.join("orvi.toml"),
        "[project]\nname = \"tests\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n",
    )
    .unwrap();
    fs::write(
        project.join("src/main.orvi"),
        "task test_fast -> Result<void, string> { return Ok() }",
    )
    .unwrap();
    fs::write(
        project.join("src/other.orvi"),
        "task test_slow -> Result<void, string> { return Ok() }",
    )
    .unwrap();

    let listed = run_in(&["test", "--list"], &project);
    assert!(listed.status.success());
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("test_fast"));
    assert!(stdout.contains("test_slow"));
    assert!(stdout.contains("Listed 2 test(s)."));

    let filtered = run_in(&["test", "--filter", "fast"], &project);
    assert!(
        filtered.status.success(),
        "{}",
        String::from_utf8_lossy(&filtered.stderr)
    );
    let stdout = String::from_utf8_lossy(&filtered.stdout);
    assert!(stdout.contains("test test_fast: passed"));
    assert!(!stdout.contains("test_slow"));
    assert!(stdout.contains("1 passed, 0 failed, 0 skipped, 1 total"));

    fs::remove_dir_all(project).unwrap();
}
