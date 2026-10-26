use std::collections::{BTreeMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use orvi_project::{Project, add_path_dependency, remove_dependency, resolve_import};
use orvi_runtime::{ExecutionContext, TaskStatus, execute_file, execute_tasks};
use orvi_sema::analyze_program_with_modules;
use orvi_syntax::{Program, Span, StatementKind, format_source, line_column, parse};

const HELP: &str = "OrviScript

Usage: orviscript [OPTIONS]
       orviscript <COMMAND> [FILE]

Commands:
  init [DIRECTORY]
                Create a new OrviScript project (defaults to the current directory)
  run [FILE]    Run a file or the current project's entry
  check [FILE]  Check a file or the current project's entry
  test [FILE] [--filter TEXT] [--list]
                Run, filter or list test_ tasks (all project files by default)
  add <NAME> --path <DIRECTORY>
                Add a local path dependency and refresh orvi.lock
  remove <NAME> Remove a dependency and refresh orvi.lock
  install       Validate dependencies and generate orvi.lock
  deps          List resolved project dependencies
  fmt <FILE>    Format an OrviScript program in place
  fmt --check <FILE>
                Check whether an OrviScript program is formatted
  task link <ALIAS> <FILE>
                Link an alias to an OrviScript task file
  task list     List linked task aliases
  task run <ALIAS>
                Run the linked task named by the alias suffix
  task unlink <ALIAS>
                Remove a linked task alias

Options:
  -h, --help     Print help
  -V, --version  Print version";

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Help,
    Version,
    Init(PathBuf),
    Run(Option<PathBuf>),
    Check(Option<PathBuf>),
    Test {
        path: Option<PathBuf>,
        filter: Option<String>,
        list: bool,
    },
    Add {
        name: String,
        path: PathBuf,
    },
    Remove(String),
    Install,
    Dependencies,
    TaskLink {
        alias: String,
        path: PathBuf,
    },
    TaskList,
    TaskRun(String),
    TaskUnlink(String),
    Format {
        path: PathBuf,
        check: bool,
    },
}

fn parse_arguments(args: &[OsString]) -> Result<Command, String> {
    match args {
        [] => Ok(Command::Help),
        [argument] if argument == "-h" || argument == "--help" => Ok(Command::Help),
        [argument] if argument == "-V" || argument == "--version" => Ok(Command::Version),
        [command] if command == "init" => Ok(Command::Init(PathBuf::from("."))),
        [command, path] if command == "init" => Ok(Command::Init(PathBuf::from(path))),
        [command] if command == "run" => Ok(Command::Run(None)),
        [command] if command == "check" => Ok(Command::Check(None)),
        [command] if command == "test" => Ok(Command::Test {
            path: None,
            filter: None,
            list: false,
        }),
        [command, option] if command == "test" && option == "--list" => Ok(Command::Test {
            path: None,
            filter: None,
            list: true,
        }),
        [command, option, filter] if command == "test" && option == "--filter" => {
            Ok(Command::Test {
                path: None,
                filter: Some(filter.to_string_lossy().into_owned()),
                list: false,
            })
        }
        [command] if command == "install" => Ok(Command::Install),
        [command] if command == "deps" => Ok(Command::Dependencies),
        [command, name] if command == "remove" => {
            Ok(Command::Remove(name.to_string_lossy().into_owned()))
        }
        [command, name, option, path] if command == "add" && option == "--path" => {
            Ok(Command::Add {
                name: name.to_string_lossy().into_owned(),
                path: PathBuf::from(path),
            })
        }
        [command, path] if command == "run" => Ok(Command::Run(Some(PathBuf::from(path)))),
        [command, path] if command == "check" => Ok(Command::Check(Some(PathBuf::from(path)))),
        [command, path] if command == "test" => Ok(Command::Test {
            path: Some(PathBuf::from(path)),
            filter: None,
            list: false,
        }),
        [command, path, option] if command == "test" && option == "--list" => Ok(Command::Test {
            path: Some(PathBuf::from(path)),
            filter: None,
            list: true,
        }),
        [command, path, option, filter] if command == "test" && option == "--filter" => {
            Ok(Command::Test {
                path: Some(PathBuf::from(path)),
                filter: Some(filter.to_string_lossy().into_owned()),
                list: false,
            })
        }
        [command, action] if command == "task" && action == "list" => Ok(Command::TaskList),
        [command, action, alias] if command == "task" && action == "run" => {
            Ok(Command::TaskRun(alias.to_string_lossy().into_owned()))
        }
        [command, action, alias] if command == "task" && action == "unlink" => {
            Ok(Command::TaskUnlink(alias.to_string_lossy().into_owned()))
        }
        [command, action, alias, path] if command == "task" && action == "link" => {
            Ok(Command::TaskLink {
                alias: alias.to_string_lossy().into_owned(),
                path: PathBuf::from(path),
            })
        }
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
        [command] if command == "fmt" => Err("command 'fmt' requires a file".into()),
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
        Ok(Command::Init(path)) => init_project(&path),
        Ok(Command::Check(path)) => with_source_target(path, check_file),
        Ok(Command::Run(path)) => with_source_target(path, run_file),
        Ok(Command::Test { path, filter, list }) => test_command(path, filter.as_deref(), list),
        Ok(Command::Add { name, path }) => add_dependency(&name, &path),
        Ok(Command::Remove(name)) => remove_project_dependency(&name),
        Ok(Command::Install) => install_dependencies(),
        Ok(Command::Dependencies) => list_dependencies(),
        Ok(Command::TaskLink { alias, path }) => task_link(&alias, &path),
        Ok(Command::TaskList) => task_list(),
        Ok(Command::TaskRun(alias)) => task_run(&alias),
        Ok(Command::TaskUnlink(alias)) => task_unlink(&alias),
        Ok(Command::Format { path, check }) => format_file(&path, check),
        Err(message) => {
            eprintln!("error: {message}\n\nFor usage information, try 'orviscript --help'.");
            ExitCode::from(2)
        }
    }
}

fn with_source_target(target: Option<PathBuf>, operation: fn(&Path) -> ExitCode) -> ExitCode {
    let path = match target {
        Some(path) => path,
        None => {
            let current = match env::current_dir() {
                Ok(current) => current,
                Err(error) => {
                    eprintln!("error: could not determine current directory: {error}");
                    return ExitCode::from(1);
                }
            };
            match Project::discover(&current).and_then(|project| project.existing_entry_path()) {
                Ok(entry) => entry,
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::from(1);
                }
            }
        }
    };
    operation(&path)
}

fn init_project(path: &Path) -> ExitCode {
    let project_name = match path.file_name().and_then(|name| name.to_str()) {
        Some(name) if !name.is_empty() && name != "." => name.to_owned(),
        _ => env::current_dir()
            .ok()
            .and_then(|directory| directory.file_name().map(|name| name.to_owned()))
            .and_then(|name| name.to_str().map(str::to_owned))
            .unwrap_or_else(|| "orviscript-project".into()),
    };
    let manifest = path.join("orvi.toml");
    let source_directory = path.join("src");
    let entry = source_directory.join("main.orvi");
    let gitignore = path.join(".gitignore");
    let conflicts = [&manifest, &entry, &gitignore]
        .into_iter()
        .filter(|candidate| candidate.exists())
        .map(|candidate| candidate.display().to_string())
        .collect::<Vec<_>>();
    if !conflicts.is_empty() {
        eprintln!(
            "error: project initialization would overwrite: {}",
            conflicts.join(", ")
        );
        return ExitCode::from(1);
    }
    if let Err(error) = fs::create_dir_all(&source_directory) {
        eprintln!(
            "error: could not create project directory '{}': {error}",
            source_directory.display()
        );
        return ExitCode::from(1);
    }
    let escaped_name = project_name.replace('\\', "\\\\").replace('"', "\\\"");
    let files = [
        (
            &manifest,
            format!(
                "[project]\nname = \"{escaped_name}\"\nversion = \"0.1.0\"\nentry = \"src/main.orvi\"\n"
            ),
        ),
        (&entry, format!("print(\"Hello from {escaped_name}!\")\n")),
        (&gitignore, "/dist/\n/.orvi/\n".into()),
    ];
    for (file, contents) in files {
        if let Err(error) = fs::write(file, contents) {
            eprintln!("error: could not write '{}': {error}", file.display());
            return ExitCode::from(1);
        }
    }
    println!("Created OrviScript project at {}", path.display());
    println!("Run it with: orviscript run {}", entry.display());
    ExitCode::SUCCESS
}

fn current_project() -> Result<Project, String> {
    let current = env::current_dir()
        .map_err(|error| format!("could not determine current directory: {error}"))?;
    Project::discover(&current).map_err(|error| error.to_string())
}

fn add_dependency(name: &str, path: &Path) -> ExitCode {
    let project = match current_project() {
        Ok(project) => project,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    match add_path_dependency(&project, name, path).and_then(|project| project.write_lockfile()) {
        Ok(_) => {
            println!("Added dependency {name} from {}", path.display());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn remove_project_dependency(name: &str) -> ExitCode {
    let project = match current_project() {
        Ok(project) => project,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    match remove_dependency(&project, name).and_then(|project| project.write_lockfile()) {
        Ok(_) => {
            println!("Removed dependency {name}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn install_dependencies() -> ExitCode {
    let project = match current_project() {
        Ok(project) => project,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    match project.write_lockfile() {
        Ok(lockfile) => {
            println!(
                "Resolved {} package(s) into {}",
                lockfile.packages.len(),
                project.root.join(orvi_project::LOCK_FILE).display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn list_dependencies() -> ExitCode {
    let project = match current_project() {
        Ok(project) => project,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    match project.resolve_dependencies() {
        Ok(lockfile) => {
            if lockfile.packages.is_empty() {
                println!("No dependencies.");
            } else {
                for package in lockfile.packages {
                    println!("{} {} ({})", package.name, package.version, package.source);
                }
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
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

fn test_command(path: Option<PathBuf>, filter: Option<&str>, list: bool) -> ExitCode {
    let paths = match path {
        Some(path) => vec![path],
        None => {
            let project = match current_project() {
                Ok(project) => project,
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::from(1);
                }
            };
            match project.source_files() {
                Ok(paths) => paths,
                Err(error) => {
                    eprintln!("error: {error}");
                    return ExitCode::from(1);
                }
            }
        }
    };
    let mut passed = 0_usize;
    let mut failed = 0_usize;
    let mut skipped = 0_usize;
    let mut selected_count = 0_usize;
    let mut invalid = false;

    for path in paths {
        let Some((source, parsed)) = load_and_parse(&path) else {
            invalid = true;
            continue;
        };
        if report_syntax_errors(&path, &source, &parsed)
            || report_semantic_errors(&path, &source, &parsed.program)
            || report_import_errors(&path, &parsed.program)
        {
            invalid = true;
            continue;
        }
        let selected = parsed
            .program
            .statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                StatementKind::Task { name, .. }
                    if name.name.starts_with("test_")
                        && filter.is_none_or(|filter| name.name.contains(filter)) =>
                {
                    Some(name.name.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        if selected.is_empty() {
            continue;
        }
        selected_count += selected.len();
        if list {
            for name in selected {
                println!("{}::{name}", path.display());
            }
            continue;
        }
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let script_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        let script_dir = script_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| cwd.clone());
        let context = ExecutionContext {
            cwd,
            project_dir: find_project_dir(&script_dir),
            script_dir,
        };
        match execute_tasks(&parsed.program, context, script_path, &selected) {
            Ok(execution) => {
                for line in execution.output {
                    println!("{line}");
                }
                for report in execution.tasks {
                    if report.status == TaskStatus::Pending {
                        continue;
                    }
                    let is_test = selected.contains(&report.name);
                    let status = match report.status {
                        TaskStatus::Succeeded => {
                            if is_test {
                                passed += 1;
                            }
                            "passed"
                        }
                        TaskStatus::Failed => {
                            if is_test {
                                failed += 1;
                            }
                            "failed"
                        }
                        TaskStatus::Skipped => {
                            if is_test {
                                skipped += 1;
                            }
                            "skipped"
                        }
                        TaskStatus::Pending => "pending",
                        TaskStatus::Running => "running",
                    };
                    let kind = if is_test { "test" } else { "dependency" };
                    println!("{kind} {}: {status} ({})", report.name, path.display());
                    if let Some(error) = report.error {
                        eprintln!("  {error}");
                    }
                }
            }
            Err(error) => {
                report_error(&path, &source, error.span, &error.message);
                invalid = true;
            }
        }
    }

    if selected_count == 0 {
        eprintln!("error: no matching tasks with prefix 'test_'");
        return ExitCode::from(1);
    }
    if list {
        println!("Listed {selected_count} test(s).");
    } else {
        println!(
            "Test summary: {passed} passed, {failed} failed, {skipped} skipped, {selected_count} total"
        );
    }
    if invalid || failed > 0 || skipped > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn registry_path() -> Result<PathBuf, String> {
    if let Some(directory) = env::var_os("ORVISCRIPT_HOME") {
        return Ok(PathBuf::from(directory).join("tasks.json"));
    }
    #[cfg(windows)]
    let base = env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    base.map(|base| base.join("orviscript").join("tasks.json"))
        .ok_or_else(|| "could not determine the OrviScript configuration directory".into())
}

fn load_registry(path: &Path) -> Result<BTreeMap<String, String>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let source = fs::read_to_string(path)
        .map_err(|error| format!("could not read task registry '{}': {error}", path.display()))?;
    serde_json::from_str(&source)
        .map_err(|error| format!("invalid task registry '{}': {error}", path.display()))
}

fn save_registry(path: &Path, registry: &BTreeMap<String, String>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "task registry path has no parent directory".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "could not create task registry directory '{}': {error}",
            parent.display()
        )
    })?;
    let source = serde_json::to_string_pretty(registry)
        .map_err(|error| format!("could not encode task registry: {error}"))?;
    fs::write(path, format!("{source}\n")).map_err(|error| {
        format!(
            "could not write task registry '{}': {error}",
            path.display()
        )
    })
}

fn valid_task_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias.split('.').all(|part| {
            !part.is_empty()
                && part.chars().all(|character| {
                    character.is_alphanumeric() || character == '_' || character == '-'
                })
        })
}

fn task_link(alias: &str, path: &Path) -> ExitCode {
    if !valid_task_alias(alias) {
        eprintln!("error: invalid task alias '{alias}'");
        return ExitCode::from(1);
    }
    let canonical = match path.canonicalize() {
        Ok(path) if path.extension().and_then(|extension| extension.to_str()) == Some("orvi") => {
            path
        }
        Ok(_) => {
            eprintln!("error: task file must end with '.orvi'");
            return ExitCode::from(1);
        }
        Err(error) => {
            eprintln!(
                "error: could not resolve task file '{}': {error}",
                path.display()
            );
            return ExitCode::from(1);
        }
    };
    let Some((source, parsed)) = load_and_parse(&canonical) else {
        return ExitCode::from(1);
    };
    if report_syntax_errors(&canonical, &source, &parsed)
        || report_semantic_errors(&canonical, &source, &parsed.program)
        || report_import_errors(&canonical, &parsed.program)
    {
        return ExitCode::from(1);
    }
    let task_name = alias.rsplit('.').next().expect("valid alias has a suffix");
    if !parsed.program.statements.iter().any(|statement| {
        matches!(&statement.kind, StatementKind::Task { name, .. } if name.name == task_name)
    }) {
        eprintln!(
            "error: '{}' does not declare task '{task_name}'",
            canonical.display()
        );
        return ExitCode::from(1);
    }
    let registry_path = match registry_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    let mut registry = match load_registry(&registry_path) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    if registry.contains_key(alias) {
        eprintln!("error: task alias '{alias}' is already linked");
        return ExitCode::from(1);
    }
    registry.insert(alias.into(), canonical.to_string_lossy().into_owned());
    if let Err(error) = save_registry(&registry_path, &registry) {
        eprintln!("error: {error}");
        return ExitCode::from(1);
    }
    println!("Linked {alias} -> {}", canonical.display());
    ExitCode::SUCCESS
}

fn task_list() -> ExitCode {
    let path = match registry_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    match load_registry(&path) {
        Ok(registry) => {
            for (alias, script) in registry {
                println!("{alias}\t{script}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn task_unlink(alias: &str) -> ExitCode {
    let path = match registry_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    let mut registry = match load_registry(&path) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    if registry.remove(alias).is_none() {
        eprintln!("error: task alias '{alias}' is not linked");
        return ExitCode::from(1);
    }
    if let Err(error) = save_registry(&path, &registry) {
        eprintln!("error: {error}");
        return ExitCode::from(1);
    }
    println!("Unlinked {alias}");
    ExitCode::SUCCESS
}

fn task_run(alias: &str) -> ExitCode {
    let path = match registry_path().and_then(|path| load_registry(&path)) {
        Ok(registry) => match registry.get(alias) {
            Some(path) => PathBuf::from(path),
            None => {
                eprintln!("error: task alias '{alias}' is not linked");
                return ExitCode::from(1);
            }
        },
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    let task_name = alias.rsplit('.').next().unwrap_or(alias).to_owned();
    run_named_task(&path, &task_name)
}

fn run_named_task(path: &Path, task_name: &str) -> ExitCode {
    let Some((source, parsed)) = load_and_parse(path) else {
        return ExitCode::from(1);
    };
    if report_syntax_errors(path, &source, &parsed)
        || report_semantic_errors(path, &source, &parsed.program)
        || report_import_errors(path, &parsed.program)
    {
        return ExitCode::from(1);
    }
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let script_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let script_dir = script_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.clone());
    let context = ExecutionContext {
        cwd,
        project_dir: find_project_dir(&script_dir),
        script_dir,
    };
    let selected = [task_name.to_owned()];
    match execute_tasks(&parsed.program, context, script_path, &selected) {
        Ok(execution) => {
            for line in execution.output {
                println!("{line}");
            }
            for report in execution.tasks {
                if report.status == TaskStatus::Pending {
                    continue;
                }
                println!("task {}: {:?}", report.name, report.status);
                if let Some(error) = report.error {
                    eprintln!("  {error}");
                }
                if report.name == task_name && report.status != TaskStatus::Succeeded {
                    return ExitCode::from(1);
                }
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
        let imported_path = match resolve_import(module_path, source) {
            Ok(path) => path,
            Err(error) => {
                report_error(module_path, "", statement.span, &error.to_string());
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
        .filter_map(|statement| {
            let StatementKind::Export(declaration) = &statement.kind else {
                return None;
            };
            match &declaration.kind {
                StatementKind::Variable { name, .. }
                | StatementKind::Function { name, .. }
                | StatementKind::Struct { name, .. }
                | StatementKind::Interface { name, .. } => Some(name.as_str()),
                _ => None,
            }
        })
        .collect()
}

fn find_project_dir(script_dir: &Path) -> PathBuf {
    Project::discover(script_dir)
        .map(|project| project.root)
        .unwrap_or_else(|_| script_dir.to_path_buf())
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
    let modules = program
        .statements
        .iter()
        .filter_map(|statement| {
            let StatementKind::Import {
                source: import_source,
                ..
            } = &statement.kind
            else {
                return None;
            };
            if import_source.starts_with("std:") {
                return None;
            }
            let module_path = resolve_import(path, import_source).ok()?;
            let module_source = fs::read_to_string(module_path).ok()?;
            let parsed = parse(&module_source);
            (!parsed.has_errors()).then(|| (import_source.clone(), parsed.program))
        })
        .collect();
    let errors = analyze_program_with_modules(program, &modules).errors;
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
            Ok(Command::Run(Some(PathBuf::from("app.orvi"))))
        );
        assert_eq!(
            parse_arguments(&[OsString::from("run")]),
            Ok(Command::Run(None))
        );
    }
    #[test]
    fn parses_init_commands() {
        assert_eq!(
            parse_arguments(&[OsString::from("init")]),
            Ok(Command::Init(PathBuf::from(".")))
        );
        assert_eq!(
            parse_arguments(&[OsString::from("init"), OsString::from("demo")]),
            Ok(Command::Init(PathBuf::from("demo")))
        );
    }
    #[test]
    fn parses_check_command() {
        assert_eq!(
            parse_arguments(&[OsString::from("check"), OsString::from("app.orvi")]),
            Ok(Command::Check(Some(PathBuf::from("app.orvi"))))
        );
    }
    #[test]
    fn parses_test_and_dependency_commands() {
        assert_eq!(
            parse_arguments(&[
                OsString::from("test"),
                OsString::from("--filter"),
                OsString::from("fast"),
            ]),
            Ok(Command::Test {
                path: None,
                filter: Some("fast".into()),
                list: false,
            })
        );
        assert_eq!(
            parse_arguments(&[
                OsString::from("add"),
                OsString::from("math"),
                OsString::from("--path"),
                OsString::from("../math"),
            ]),
            Ok(Command::Add {
                name: "math".into(),
                path: PathBuf::from("../math"),
            })
        );
        assert_eq!(
            parse_arguments(&[OsString::from("install")]),
            Ok(Command::Install)
        );
    }
    #[test]
    fn parses_task_registry_commands() {
        assert_eq!(
            parse_arguments(&[OsString::from("task"), OsString::from("list")]),
            Ok(Command::TaskList)
        );
        assert_eq!(
            parse_arguments(&[
                OsString::from("task"),
                OsString::from("run"),
                OsString::from("hosting.deploy"),
            ]),
            Ok(Command::TaskRun("hosting.deploy".into()))
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
    fn rejects_missing_format_path() {
        assert_eq!(
            parse_arguments(&[OsString::from("fmt")]),
            Err("command 'fmt' requires a file".into())
        );
    }
}
