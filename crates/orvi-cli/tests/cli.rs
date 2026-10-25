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
