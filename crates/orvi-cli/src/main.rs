use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use orvi_runtime::{ExecutionContext, execute_file};
use orvi_sema::analyze;
use orvi_syntax::{Program, Span, StatementKind, format_source, line_column, parse};

const HELP: &str = "OrviScript

Usage: orviscript [OPTIONS]
       orviscript <COMMAND> <FILE>

Commands:
  run <FILE>    Run an OrviScript program
  check <FILE>  Check an OrviScript program for syntax and semantic errors
  fmt <FILE>    Format an OrviScript program in place
  fmt --check <FILE>
                Check whether an OrviScript program is formatted

Options:
  -h, --help     Print help
  -V, --version  Print version";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Version,
    Run(PathBuf),
    Check(PathBuf),
    Format { path: PathBuf, check: bool },
}

fn parse_arguments(args: &[OsString]) -> Result<Command, String> {
    match args {
        [] => Ok(Command::Help),
        [argument] if argument == "-h" || argument == "--help" => Ok(Command::Help),
        [argument] if argument == "-V" || argument == "--version" => Ok(Command::Version),
        [command, path] if command == "run" => Ok(Command::Run(PathBuf::from(path))),
        [command, path] if command == "check" => Ok(Command::Check(PathBuf::from(path))),
        [command, option] if command == "fmt" && option == "--check" => {
            Err("command 'fmt --check' requires a file".into())
        }
        [command, path] if command == "fmt" => Ok(Command::Format {
            path: PathBuf::from(path),
            check: false,
        }),
        [command, option, path] if command == "fmt" && option == "--check" => Ok(Command::Format {
            path: PathBuf::from(path),
            check: true,
        }),
        [command] if command == "run" || command == "check" || command == "fmt" => Err(format!(
            "command '{}' requires a file",
            command.to_string_lossy()
        )),
        [argument] => Err(format!(
            "unexpected argument '{}'",
            argument.to_string_lossy()
        )),
        _ => Err("unexpected arguments".into()),
    }
}

fn main() -> ExitCode {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    match parse_arguments(&args) {
        Ok(Command::Help) => {
            println!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Version) => {
            println!("orviscript {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Ok(Command::Check(path)) => check_file(&path),
        Ok(Command::Run(path)) => run_file(&path),
        Ok(Command::Format { path, check }) => format_file(&path, check),
        Err(message) => {
            eprintln!("error: {message}\n\nFor usage information, try 'orviscript --help'.");
            ExitCode::from(2)
        }
    }
}

fn check_file(path: &Path) -> ExitCode {
    let Some((source, parsed)) = load_and_parse(path) else {
        return ExitCode::from(1);
    };
    let has_syntax_errors = report_syntax_errors(path, &source, &parsed);
    let has_semantic_errors = !has_syntax_errors
        && (report_semantic_errors(path, &source, &parsed.program)
            || report_import_errors(path, &parsed.program));
    if has_syntax_errors || has_semantic_errors {
        ExitCode::from(1)
    } else {
        println!("Check passed: {}", path.display());
        ExitCode::SUCCESS
    }
}

fn run_file(path: &Path) -> ExitCode {
    let Some((source, parsed)) = load_and_parse(path) else {
        return ExitCode::from(1);
    };
    if report_syntax_errors(path, &source, &parsed) {
        return ExitCode::from(1);
    }
    if report_semantic_errors(path, &source, &parsed.program) {
        return ExitCode::from(1);
    }
    if report_import_errors(path, &parsed.program) {
        return ExitCode::from(1);
    }
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let script_dir = path
        .canonicalize()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| {
            path.parent()
                .map_or_else(|| cwd.clone(), |parent| cwd.join(parent))
        });
    let context = ExecutionContext {
        cwd,
        project_dir: find_project_dir(&script_dir),
        script_dir,
    };
    let entry_file = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    match execute_file(&parsed.program, context, entry_file) {
        Ok(execution) => {
            for line in execution.output {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            report_error(path, &source, error.span, &error.message);
            ExitCode::from(1)
        }
    }
}

fn report_import_errors(path: &Path, program: &Program) -> bool {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut visited = HashSet::new();
    let mut stack = vec![path.clone()];
    check_imports(&path, program, &mut visited, &mut stack)
}

fn check_imports(
    module_path: &Path,
    program: &Program,
    visited: &mut HashSet<PathBuf>,
    stack: &mut Vec<PathBuf>,
) -> bool {
    let mut has_errors = false;
    for statement in &program.statements {
        let StatementKind::Import { names, source } = &statement.kind else {
            continue;
        };
        if source.starts_with("std:") {
            continue;
        }
        let candidate = module_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(source);
        if candidate
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("orvi")
        {
            report_error(
                module_path,
                "",
                statement.span,
                "local module path must end with '.orvi'",
            );
            has_errors = true;
            continue;
        }
        let imported_path = match candidate.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                eprintln!(
                    "{}: error: could not resolve module '{}': {error}",
                    module_path.display(),
                    candidate.display()
                );
                has_errors = true;
                continue;
            }
        };
        if stack.contains(&imported_path) {
            eprintln!(
                "{}: error: cyclic module import involving '{}'",
                module_path.display(),
                imported_path.display()
            );
            has_errors = true;
            continue;
        }
        let source_text = match fs::read_to_string(&imported_path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!(
                    "{}: error: could not read module: {error}",
                    imported_path.display()
                );
                has_errors = true;
                continue;
            }
        };
        let parsed = parse(&source_text);
        if report_syntax_errors(&imported_path, &source_text, &parsed) {
            has_errors = true;
            continue;
        }
        if report_semantic_errors(&imported_path, &source_text, &parsed.program) {
            has_errors = true;
        }
        let exports = exported_names(&parsed.program);
        for name in names {
            if !exports.contains(name.as_str()) {
                report_error(
                    module_path,
                    "",
                    name.span,
                    &format!(
                        "module '{}' has no export '{}'",
                        imported_path.display(),
                        name.name
                    ),
                );
                has_errors = true;
            }
        }
        if visited.insert(imported_path.clone()) {
            stack.push(imported_path.clone());
            has_errors |= check_imports(&imported_path, &parsed.program, visited, stack);
            stack.pop();
        }
    }
    has_errors
}

fn exported_names(program: &Program) -> HashSet<&str> {
    program
        .statements
        .iter()
        .filter_map(|statement| match &statement.kind {
            StatementKind::Variable { name, .. }
            | StatementKind::Function { name, .. }
            | StatementKind::Struct { name, .. }
            | StatementKind::Interface { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn find_project_dir(script_dir: &Path) -> PathBuf {
    script_dir
        .ancestors()
        .find(|directory| directory.join("orvi.toml").is_file())
        .unwrap_or(script_dir)
        .to_path_buf()
}

fn format_file(path: &Path, check: bool) -> ExitCode {
    let Some((source, parsed)) = load_and_parse(path) else {
        return ExitCode::from(1);
    };
    if report_syntax_errors(path, &source, &parsed) {
        return ExitCode::from(1);
    }
    let formatted = match format_source(&source) {
        Ok(formatted) => formatted,
        Err(error) => {
            eprintln!("error: could not format '{}': {error}", path.display());
            return ExitCode::from(1);
        }
    };
    if check {
        if source == formatted {
            println!("Format check passed: {}", path.display());
            return ExitCode::SUCCESS;
        }
        eprintln!("error: '{}' is not formatted", path.display());
        return ExitCode::from(1);
    }
    if source != formatted
        && let Err(error) = fs::write(path, formatted)
    {
        eprintln!("error: could not write '{}': {error}", path.display());
        return ExitCode::from(1);
    }
    println!("Formatted: {}", path.display());
    ExitCode::SUCCESS
}

fn load_and_parse(path: &Path) -> Option<(String, orvi_syntax::Parsed)> {
    match fs::read_to_string(path) {
        Ok(source) => {
            let parsed = parse(&source);
            Some((source, parsed))
        }
        Err(error) => {
            eprintln!("error: could not read '{}': {error}", path.display());
            None
        }
    }
}

fn report_syntax_errors(path: &Path, source: &str, parsed: &orvi_syntax::Parsed) -> bool {
    for error in &parsed.lexical_errors {
        report_error(path, source, error.span, &error.to_string());
    }
    for error in &parsed.parse_errors {
        report_error(path, source, error.span, &error.message);
    }
    parsed.has_errors()
}

fn report_semantic_errors(path: &Path, source: &str, program: &orvi_syntax::Program) -> bool {
    let errors = analyze(program);
    for error in &errors {
        report_error(path, source, error.span, &error.message);
    }
    !errors.is_empty()
}

fn report_error(path: &Path, source: &str, span: Span, message: &str) {
    let position =
        line_column(source, span.start()).unwrap_or(orvi_syntax::LineColumn { line: 1, column: 1 });
    eprintln!(
        "{}:{}:{}: error: {message}",
        path.display(),
        position.line,
        position.column
    );
    if let Some(line) = source.lines().nth(position.line.saturating_sub(1)) {
        eprintln!("  {line}");
        eprintln!("  {}^", " ".repeat(position.column.saturating_sub(1)));
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, parse_arguments};
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn displays_help_without_arguments() {
        assert_eq!(parse_arguments(&[]), Ok(Command::Help));
    }
    #[test]
    fn parses_run_command() {
        assert_eq!(
            parse_arguments(&[OsString::from("run"), OsString::from("app.orvi")]),
            Ok(Command::Run(PathBuf::from("app.orvi")))
        );
    }
    #[test]
    fn parses_check_command() {
        assert_eq!(
            parse_arguments(&[OsString::from("check"), OsString::from("app.orvi")]),
            Ok(Command::Check(PathBuf::from("app.orvi")))
        );
    }
    #[test]
    fn parses_format_commands() {
        assert_eq!(
            parse_arguments(&[OsString::from("fmt"), OsString::from("app.orvi")]),
            Ok(Command::Format {
                path: PathBuf::from("app.orvi"),
                check: false,
            })
        );
        assert_eq!(
            parse_arguments(&[
                OsString::from("fmt"),
                OsString::from("--check"),
                OsString::from("app.orvi"),
            ]),
            Ok(Command::Format {
                path: PathBuf::from("app.orvi"),
                check: true,
            })
        );
    }
    #[test]
    fn rejects_missing_path() {
        assert_eq!(
            parse_arguments(&[OsString::from("run")]),
            Err("command 'run' requires a file".into())
        );
    }
}
