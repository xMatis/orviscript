use std::collections::{HashMap, HashSet};
use std::fmt;

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, Identifier, Literal, Parameter, PatternKind,
    Program, Span, Statement, StatementKind, TypeAnnotation, UnaryOperator,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticErrorKind {
    UndefinedName,
    DuplicateName,
    ImmutableAssignment,
    ReturnOutsideFunction,
    ArgumentCount,
    UnknownType,
    TypeMismatch,
    NonExhaustiveMatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticError {
    pub kind: SemanticErrorKind,
    pub message: String,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    Variable,
    Constant,
    Function,
    Parameter,
    Struct,
    Interface,
    Import,
    Task,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymbolOccurrence {
    pub name: String,
    pub span: Span,
    pub definition: Span,
    pub kind: SymbolKind,
    pub is_definition: bool,
    pub type_name: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Analysis {
    pub errors: Vec<SemanticError>,
    pub symbols: Vec<SymbolOccurrence>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandardMember {
    pub name: &'static str,
    pub signature: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandardModule {
    pub name: &'static str,
    pub description: &'static str,
    pub members: &'static [StandardMember],
}

const JSON_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "parse",
        signature: "fn parse(text: string) -> Result<any, string>",
        description: "Parses a JSON string.",
    },
    StandardMember {
        name: "stringify",
        signature: "fn stringify(value: any) -> Result<string, string>",
        description: "Serializes a value as JSON.",
    },
];
const FS_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "read_text",
        signature: "fn read_text(path: string) -> Result<string, string>",
        description: "Reads a UTF-8 text file.",
    },
    StandardMember {
        name: "read_text_async",
        signature: "fn read_text_async(path: string) -> Future<Result<string, string>>",
        description: "Reads a UTF-8 text file without blocking the executor.",
    },
    StandardMember {
        name: "write_text",
        signature: "fn write_text(path: string, contents: string) -> Result<void, string>",
        description: "Writes a UTF-8 text file.",
    },
    StandardMember {
        name: "write_text_async",
        signature: "fn write_text_async(path: string, contents: string) -> Future<Result<void, string>>",
        description: "Writes a UTF-8 text file without blocking the executor.",
    },
    StandardMember {
        name: "exists",
        signature: "fn exists(path: string) -> bool",
        description: "Checks whether a path exists.",
    },
    StandardMember {
        name: "create_dir_all",
        signature: "fn create_dir_all(path: string) -> Result<void, string>",
        description: "Creates a directory and its missing parents.",
    },
    StandardMember {
        name: "remove_file",
        signature: "fn remove_file(path: string) -> Result<void, string>",
        description: "Removes a file.",
    },
];
const PATH_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "join",
        signature: "fn join(parts: list<string>) -> string",
        description: "Joins path components using the platform separator.",
    },
    StandardMember {
        name: "parent",
        signature: "fn parent(path: string) -> Option<string>",
        description: "Returns the parent path when one exists.",
    },
    StandardMember {
        name: "file_name",
        signature: "fn file_name(path: string) -> Option<string>",
        description: "Returns the final path component when one exists.",
    },
    StandardMember {
        name: "is_absolute",
        signature: "fn is_absolute(path: string) -> bool",
        description: "Checks whether a path is absolute.",
    },
];
const ENV_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "get",
        signature: "fn get(name: string) -> Option<string>",
        description: "Reads an environment variable.",
    },
    StandardMember {
        name: "current_dir",
        signature: "fn current_dir() -> Result<string, string>",
        description: "Returns the current working directory.",
    },
];
const PROCESS_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "run",
        signature: "fn run(program: string, args: list<string>) -> Result<map<any>, string>",
        description: "Runs a program directly without a shell.",
    },
    StandardMember {
        name: "run_async",
        signature: "fn run_async(program: string, args: list<string>) -> Future<Result<map<any>, string>>",
        description: "Runs a program without blocking the executor.",
    },
];
const TERMINAL_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "input",
        signature: "fn input(prompt: string) -> Result<string, string>",
        description: "Reads visible terminal input.",
    },
    StandardMember {
        name: "confirm",
        signature: "fn confirm(prompt: string) -> Result<bool, string>",
        description: "Reads a yes or no answer.",
    },
    StandardMember {
        name: "password",
        signature: "fn password(prompt: string) -> Result<string, string>",
        description: "Reads terminal input without echoing it.",
    },
];
const TASK_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "result",
        signature: "fn result(name: string) -> Result<any, string>",
        description: "Returns the result of a declared workflow dependency.",
    },
    StandardMember {
        name: "spawn",
        signature: "fn spawn(future: Future<any>) -> Task<any>",
        description: "Starts a future in the current structured-concurrency scope.",
    },
    StandardMember {
        name: "join",
        signature: "fn join(task: Task<any>) -> Future<Result<any, string>>",
        description: "Waits for a scoped task result.",
    },
    StandardMember {
        name: "cancel",
        signature: "fn cancel(task: Task<any>) -> bool",
        description: "Requests cooperative cancellation of a scoped task.",
    },
    StandardMember {
        name: "timeout",
        signature: "fn timeout(future: Future<any>, milliseconds: int) -> Future<Result<any, string>>",
        description: "Runs a future with a deadline.",
    },
];
const HTTP_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "get",
        signature: "fn get(url: string) -> Result<map<any>, string>",
        description: "Performs a bounded HTTP GET request.",
    },
    StandardMember {
        name: "get_async",
        signature: "fn get_async(url: string) -> Future<Result<map<any>, string>>",
        description: "Performs a bounded HTTP GET request without blocking the executor.",
    },
    StandardMember {
        name: "request",
        signature: "fn request(method: string, url: string, headers: map<string>, body: string) -> Result<map<any>, string>",
        description: "Performs a bounded HTTP request.",
    },
    StandardMember {
        name: "request_async",
        signature: "fn request_async(method: string, url: string, headers: map<string>, body: string) -> Future<Result<map<any>, string>>",
        description: "Performs a bounded HTTP request without blocking the executor.",
    },
];
const TIME_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "unix_ms",
        signature: "fn unix_ms() -> Result<int, string>",
        description: "Returns Unix time in milliseconds.",
    },
    StandardMember {
        name: "monotonic_ms",
        signature: "fn monotonic_ms() -> int",
        description: "Returns milliseconds from the process monotonic clock.",
    },
    StandardMember {
        name: "sleep_ms",
        signature: "fn sleep_ms(milliseconds: int) -> Result<void, string>",
        description: "Blocks the current thread for a bounded duration.",
    },
    StandardMember {
        name: "sleep_async",
        signature: "fn sleep_async(milliseconds: int) -> Future<Result<void, string>>",
        description: "Waits without blocking the executor.",
    },
];
const WORKER_MEMBERS: &[StandardMember] = &[
    StandardMember {
        name: "spawn",
        signature: "fn spawn(source: string, input: any) -> Worker<map<any>>",
        description: "Runs isolated OrviScript source on the bounded CPU worker pool.",
    },
    StandardMember {
        name: "join",
        signature: "fn join(worker: Worker<any>) -> Future<Result<map<any>, string>>",
        description: "Waits for a worker and returns its value and captured output.",
    },
    StandardMember {
        name: "cancel",
        signature: "fn cancel(worker: Worker<any>) -> bool",
        description: "Requests cooperative cancellation of a worker.",
    },
    StandardMember {
        name: "timeout",
        signature: "fn timeout(worker: Worker<any>, milliseconds: int) -> Future<Result<map<any>, string>>",
        description: "Waits for a worker until a deadline, then cancels it.",
    },
    StandardMember {
        name: "threads",
        signature: "fn threads() -> int",
        description: "Returns the bounded worker pool size.",
    },
    StandardMember {
        name: "status",
        signature: "fn status(worker: Worker<any>) -> string",
        description: "Returns the current worker state without waiting.",
    },
    StandardMember {
        name: "stats",
        signature: "fn stats() -> map<any>",
        description: "Returns worker pool capacity, load, and lifetime counters.",
    },
];

pub const STANDARD_MODULES: &[StandardModule] = &[
    StandardModule {
        name: "json",
        description: "JSON parsing and serialization",
        members: JSON_MEMBERS,
    },
    StandardModule {
        name: "fs",
        description: "Filesystem operations",
        members: FS_MEMBERS,
    },
    StandardModule {
        name: "path",
        description: "Cross-platform path operations",
        members: PATH_MEMBERS,
    },
    StandardModule {
        name: "env",
        description: "Process environment access",
        members: ENV_MEMBERS,
    },
    StandardModule {
        name: "process",
        description: "Direct child process execution",
        members: PROCESS_MEMBERS,
    },
    StandardModule {
        name: "terminal",
        description: "Interactive terminal input",
        members: TERMINAL_MEMBERS,
    },
    StandardModule {
        name: "task",
        description: "Workflow results and structured concurrency",
        members: TASK_MEMBERS,
    },
    StandardModule {
        name: "http",
        description: "Bounded HTTP client operations",
        members: HTTP_MEMBERS,
    },
    StandardModule {
        name: "time",
        description: "Wall-clock, monotonic time, and timers",
        members: TIME_MEMBERS,
    },
    StandardModule {
        name: "worker",
        description: "Isolated parallel CPU execution",
        members: WORKER_MEMBERS,
    },
];

pub fn standard_module(name: &str) -> Option<&'static StandardModule> {
    STANDARD_MODULES.iter().find(|module| module.name == name)
}

impl fmt::Display for SemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Debug)]
struct Symbol {
    kind: SymbolKind,
    mutable: bool,
    arity: Option<usize>,
    definition: Option<Span>,
    value_type: StaticType,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StaticType {
    Dynamic,
    Void,
    Null,
    Bool,
    Int,
    Float,
    Number,
    String,
    List(Box<StaticType>),
    Map(Box<StaticType>),
    Module(String),
    Struct(String),
    Interface(String),
    Option(Box<StaticType>),
    Result(Box<StaticType>, Box<StaticType>),
    Future(Box<StaticType>),
    Task(Box<StaticType>),
    Worker(Box<StaticType>),
    Function {
        parameters: Vec<StaticType>,
        result: Box<StaticType>,
    },
}

pub fn analyze(program: &Program) -> Vec<SemanticError> {
    analyze_program(program).errors
}

pub fn analyze_program(program: &Program) -> Analysis {
    analyze_program_with_modules(program, &HashMap::new())
}

pub fn analyze_worker_program(program: &Program) -> Analysis {
    analyze_program_internal(program, &HashMap::new(), true)
}

pub fn analyze_program_with_modules(
    program: &Program,
    modules: &HashMap<String, Program>,
) -> Analysis {
    analyze_program_internal(program, modules, false)
}

fn analyze_program_internal(
    program: &Program,
    modules: &HashMap<String, Program>,
    worker_mode: bool,
) -> Analysis {
    let struct_names = program
        .statements
        .iter()
        .filter_map(|statement| match &declaration_statement(statement).kind {
            StatementKind::Struct { name, .. } => Some(name.name.clone()),
            _ => None,
        })
        .collect();
    let interface_names = program
        .statements
        .iter()
        .filter_map(|statement| match &declaration_statement(statement).kind {
            StatementKind::Interface { name, .. } => Some(name.name.clone()),
            _ => None,
        })
        .collect();
    let mut analyzer = Analyzer {
        scopes: vec![builtin_symbols()],
        errors: Vec::new(),
        symbols: Vec::new(),
        function_depth: 0,
        current_return_type: None,
        current_function_async: false,
        in_cleanup_expression: false,
        struct_names,
        structs: HashMap::new(),
        interface_names,
        interfaces: HashMap::new(),
        task_names: program
            .statements
            .iter()
            .filter_map(|statement| match &declaration_statement(statement).kind {
                StatementKind::Task { name, .. } => Some(name.name.clone()),
                _ => None,
            })
            .collect(),
        current_task_needs: None,
        module_exports: modules
            .iter()
            .map(|(source, program)| (source.clone(), exported_symbols(program)))
            .collect(),
    };
    if worker_mode {
        analyzer.scopes[0].insert(
            "worker_input".into(),
            Symbol {
                kind: SymbolKind::Constant,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Dynamic,
            },
        );
    }
    analyzer.statements(&program.statements, false);
    analyzer.errors.extend(task_cycle_errors(program));
    Analysis {
        errors: analyzer.errors,
        symbols: analyzer.symbols,
    }
}

fn exported_symbols(program: &Program) -> HashMap<String, Symbol> {
    let struct_names = program
        .statements
        .iter()
        .filter_map(|statement| match &declaration_statement(statement).kind {
            StatementKind::Struct { name, .. } => Some(name.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let interface_names = program
        .statements
        .iter()
        .filter_map(|statement| match &declaration_statement(statement).kind {
            StatementKind::Interface { name, .. } => Some(name.name.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    program
        .statements
        .iter()
        .filter_map(|statement| {
            let StatementKind::Export(declaration) = &statement.kind else {
                return None;
            };
            let (name, symbol) = match &declaration.kind {
                StatementKind::Variable {
                    name,
                    mutable,
                    type_annotation,
                    ..
                } => (
                    name,
                    Symbol {
                        kind: if *mutable {
                            SymbolKind::Variable
                        } else {
                            SymbolKind::Constant
                        },
                        mutable: *mutable,
                        arity: None,
                        definition: None,
                        value_type: type_annotation
                            .as_ref()
                            .and_then(|annotation| {
                                static_type_from_annotation(
                                    annotation,
                                    &struct_names,
                                    &interface_names,
                                )
                            })
                            .unwrap_or(StaticType::Dynamic),
                    },
                ),
                StatementKind::Function {
                    name,
                    is_async,
                    parameters,
                    return_type,
                    ..
                } => (
                    name,
                    Symbol {
                        kind: SymbolKind::Function,
                        mutable: false,
                        arity: Some(parameters.len()),
                        definition: None,
                        value_type: StaticType::Function {
                            parameters: parameters
                                .iter()
                                .map(|parameter| {
                                    parameter
                                        .type_annotation
                                        .as_ref()
                                        .and_then(|annotation| {
                                            static_type_from_annotation(
                                                annotation,
                                                &struct_names,
                                                &interface_names,
                                            )
                                        })
                                        .unwrap_or(StaticType::Dynamic)
                                })
                                .collect(),
                            result: Box::new({
                                let result = return_type
                                    .as_ref()
                                    .and_then(|annotation| {
                                        static_type_from_annotation(
                                            annotation,
                                            &struct_names,
                                            &interface_names,
                                        )
                                    })
                                    .unwrap_or(StaticType::Dynamic);
                                if *is_async {
                                    StaticType::Future(Box::new(result))
                                } else {
                                    result
                                }
                            }),
                        },
                    },
                ),
                StatementKind::Struct { name, .. } => (
                    name,
                    Symbol {
                        kind: SymbolKind::Struct,
                        mutable: false,
                        arity: None,
                        definition: None,
                        value_type: StaticType::Struct(name.name.clone()),
                    },
                ),
                StatementKind::Interface { name, .. } => (
                    name,
                    Symbol {
                        kind: SymbolKind::Interface,
                        mutable: false,
                        arity: None,
                        definition: None,
                        value_type: StaticType::Interface(name.name.clone()),
                    },
                ),
                _ => return None,
            };
            Some((name.name.clone(), symbol))
        })
        .collect()
}

fn declaration_statement(statement: &Statement) -> &Statement {
    match &statement.kind {
        StatementKind::Export(declaration) => declaration_statement(declaration),
        _ => statement,
    }
}

fn task_cycle_errors(program: &Program) -> Vec<SemanticError> {
    fn visit(
        name: &str,
        tasks: &HashMap<String, (Vec<String>, Span)>,
        states: &mut HashMap<String, u8>,
        errors: &mut Vec<SemanticError>,
    ) {
        match states.get(name) {
            Some(1) => {
                let span = tasks.get(name).map_or(Span::empty(0), |task| task.1);
                errors.push(SemanticError {
                    kind: SemanticErrorKind::TypeMismatch,
                    message: format!("cyclic task dependency involving '{name}'"),
                    span,
                });
                return;
            }
            Some(2) => return,
            _ => {}
        }
        states.insert(name.to_owned(), 1);
        if let Some((needs, _)) = tasks.get(name) {
            for dependency in needs {
                if tasks.contains_key(dependency) {
                    visit(dependency, tasks, states, errors);
                }
            }
        }
        states.insert(name.to_owned(), 2);
    }

    let tasks = program
        .statements
        .iter()
        .filter_map(|statement| match &statement.kind {
            StatementKind::Task { name, needs, .. } => Some((
                name.name.clone(),
                (
                    needs.iter().map(|need| need.name.clone()).collect(),
                    name.span,
                ),
            )),
            _ => None,
        })
        .collect::<HashMap<_, _>>();
    let mut states = HashMap::new();
    let mut errors = Vec::new();
    for statement in &program.statements {
        if let StatementKind::Task { name, .. } = &statement.kind {
            visit(&name.name, &tasks, &mut states, &mut errors);
        }
    }
    errors
}

fn builtin_symbols() -> HashMap<String, Symbol> {
    let function = |parameter: StaticType, result: StaticType| Symbol {
        kind: SymbolKind::Function,
        mutable: false,
        arity: Some(1),
        definition: None,
        value_type: StaticType::Function {
            parameters: vec![parameter],
            result: Box::new(result),
        },
    };
    HashMap::from([
        (
            "print".into(),
            Symbol {
                kind: SymbolKind::Function,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Function {
                    parameters: Vec::new(),
                    result: Box::new(StaticType::Void),
                },
            },
        ),
        (
            "Some".into(),
            function(
                StaticType::Dynamic,
                StaticType::Option(Box::new(StaticType::Dynamic)),
            ),
        ),
        (
            "None".into(),
            Symbol {
                kind: SymbolKind::Constant,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Option(Box::new(StaticType::Dynamic)),
            },
        ),
        (
            "Ok".into(),
            Symbol {
                kind: SymbolKind::Function,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Function {
                    parameters: vec![StaticType::Dynamic],
                    result: Box::new(StaticType::Result(
                        Box::new(StaticType::Dynamic),
                        Box::new(StaticType::Dynamic),
                    )),
                },
            },
        ),
        (
            "Err".into(),
            function(
                StaticType::Dynamic,
                StaticType::Result(Box::new(StaticType::Dynamic), Box::new(StaticType::Dynamic)),
            ),
        ),
        (
            "json".into(),
            Symbol {
                kind: SymbolKind::Constant,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Module("json".into()),
            },
        ),
        ("fs".into(), module_symbol("fs")),
        ("path".into(), module_symbol("path")),
        ("env".into(), module_symbol("env")),
        ("process".into(), module_symbol("process")),
        ("terminal".into(), module_symbol("terminal")),
        ("task".into(), module_symbol("task")),
        ("http".into(), module_symbol("http")),
        ("time".into(), module_symbol("time")),
        ("worker".into(), module_symbol("worker")),
        (
            "context".into(),
            Symbol {
                kind: SymbolKind::Constant,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Map(Box::new(StaticType::String)),
            },
        ),
    ])
}

fn module_symbol(name: &str) -> Symbol {
    Symbol {
        kind: SymbolKind::Constant,
        mutable: false,
        arity: None,
        definition: None,
        value_type: StaticType::Module(name.into()),
    }
}

struct Analyzer {
    scopes: Vec<HashMap<String, Symbol>>,
    errors: Vec<SemanticError>,
    symbols: Vec<SymbolOccurrence>,
    function_depth: usize,
    current_return_type: Option<StaticType>,
    current_function_async: bool,
    in_cleanup_expression: bool,
    struct_names: HashSet<String>,
    structs: HashMap<String, HashMap<String, StaticType>>,
    interface_names: HashSet<String>,
    interfaces: HashMap<String, HashMap<String, StaticType>>,
    task_names: HashSet<String>,
    current_task_needs: Option<HashSet<String>>,
    module_exports: HashMap<String, HashMap<String, Symbol>>,
}

impl Analyzer {
    fn statements(&mut self, statements: &[Statement], create_scope: bool) {
        if create_scope {
            self.scopes.push(HashMap::new());
        }

        if self.function_depth > 0 || self.scopes.len() > 1 {
            for statement in statements {
                if matches!(statement.kind, StatementKind::Export(_)) {
                    self.type_error("export is only valid at module top level", statement.span);
                }
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Import { names, source } = &statement.kind {
                let standard_name = source.strip_prefix("std:");
                for name in names {
                    let value_type = if let Some(module) = standard_name {
                        if standard_module(module).is_none() {
                            self.errors.push(SemanticError {
                                kind: SemanticErrorKind::UndefinedName,
                                message: format!("unknown standard module '{source}'"),
                                span: name.span,
                            });
                            StaticType::Dynamic
                        } else {
                            module_member_type(module, name.as_str()).unwrap_or_else(|| {
                                self.errors.push(SemanticError {
                                    kind: SemanticErrorKind::UndefinedName,
                                    message: format!(
                                        "module '{source}' has no export '{}'",
                                        name.name
                                    ),
                                    span: name.span,
                                });
                                StaticType::Dynamic
                            })
                        }
                    } else if let Some(exports) = self.module_exports.get(source) {
                        match exports.get(name.as_str()) {
                            Some(symbol) => symbol.value_type.clone(),
                            None => StaticType::Dynamic,
                        }
                    } else {
                        StaticType::Dynamic
                    };
                    let arity = self
                        .module_exports
                        .get(source)
                        .and_then(|exports| exports.get(name.as_str()))
                        .and_then(|symbol| symbol.arity);
                    self.declare(
                        name,
                        Symbol {
                            kind: SymbolKind::Import,
                            mutable: false,
                            arity,
                            definition: Some(name.span),
                            value_type,
                        },
                    );
                }
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Task { name, .. } = &statement.kind {
                self.declare(
                    name,
                    Symbol {
                        kind: SymbolKind::Task,
                        mutable: false,
                        arity: None,
                        definition: Some(name.span),
                        value_type: StaticType::Dynamic,
                    },
                );
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Interface { name, fields } = &statement.kind {
                self.interface_names.insert(name.name.clone());
                let mut field_types = HashMap::new();
                for field in fields {
                    let field_type = self
                        .annotation_type(Some(&field.type_annotation))
                        .unwrap_or(StaticType::Dynamic);
                    if field_types
                        .insert(field.name.name.clone(), field_type)
                        .is_some()
                    {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::DuplicateName,
                            message: format!(
                                "field '{}' is already declared in interface '{}'",
                                field.name.name, name.name
                            ),
                            span: field.name.span,
                        });
                    }
                }
                self.interfaces.insert(name.name.clone(), field_types);
                self.declare(
                    name,
                    Symbol {
                        kind: SymbolKind::Interface,
                        mutable: false,
                        arity: None,
                        definition: Some(name.span),
                        value_type: StaticType::Interface(name.name.clone()),
                    },
                );
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Struct { name, fields } = &statement.kind {
                self.struct_names.insert(name.name.clone());
                let mut field_types = HashMap::new();
                let mut parameters = Vec::new();
                for field in fields {
                    let field_type = self
                        .annotation_type(Some(&field.type_annotation))
                        .unwrap_or(StaticType::Dynamic);
                    if field_types
                        .insert(field.name.name.clone(), field_type.clone())
                        .is_some()
                    {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::DuplicateName,
                            message: format!(
                                "field '{}' is already declared in struct '{}'",
                                field.name.name, name.name
                            ),
                            span: field.name.span,
                        });
                    }
                    parameters.push(field_type);
                }
                self.structs.insert(name.name.clone(), field_types);
                self.declare(
                    name,
                    Symbol {
                        kind: SymbolKind::Struct,
                        mutable: false,
                        arity: None,
                        definition: Some(name.span),
                        value_type: StaticType::Function {
                            parameters,
                            result: Box::new(StaticType::Struct(name.name.clone())),
                        },
                    },
                );
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Function {
                name,
                is_async,
                parameters,
                return_type,
                ..
            } = &statement.kind
            {
                let parameter_types = parameters
                    .iter()
                    .map(|parameter| {
                        self.annotation_type(parameter.type_annotation.as_ref())
                            .unwrap_or(StaticType::Dynamic)
                    })
                    .collect();
                let declared_result = self
                    .annotation_type(return_type.as_ref())
                    .unwrap_or(StaticType::Dynamic);
                let result = if *is_async {
                    StaticType::Future(Box::new(declared_result))
                } else {
                    declared_result
                };
                self.declare(
                    name,
                    Symbol {
                        kind: SymbolKind::Function,
                        mutable: false,
                        arity: Some(parameters.len()),
                        definition: Some(name.span),
                        value_type: StaticType::Function {
                            parameters: parameter_types,
                            result: Box::new(result),
                        },
                    },
                );
            }
        }

        for statement in statements {
            if !matches!(
                &declaration_statement(statement).kind,
                StatementKind::Function { .. } | StatementKind::Task { .. }
            ) {
                self.statement(statement);
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Function {
                is_async,
                parameters,
                return_type,
                body,
                ..
            } = &statement.kind
            {
                self.function(parameters, return_type.as_ref(), body, *is_async);
            }
        }

        for statement in statements {
            let statement = declaration_statement(statement);
            if let StatementKind::Task {
                needs,
                return_type,
                body,
                ..
            } = &statement.kind
            {
                let task_return = self.annotation_type(Some(return_type));
                if !matches!(
                    task_return,
                    Some(StaticType::Result(_, _)) | Some(StaticType::Dynamic)
                ) {
                    self.type_error("task return type must be Result<T, E>", return_type.span);
                }
                let mut dependencies = HashSet::new();
                for dependency in needs {
                    if !self.task_names.contains(dependency.as_str()) {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::UndefinedName,
                            message: format!("undefined task dependency '{}'", dependency.name),
                            span: dependency.span,
                        });
                    }
                    if !dependencies.insert(dependency.name.clone()) {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::DuplicateName,
                            message: format!("duplicate task dependency '{}'", dependency.name),
                            span: dependency.span,
                        });
                    }
                }
                let previous = self.current_task_needs.replace(dependencies);
                self.function(&[], Some(return_type), body, false);
                self.current_task_needs = previous;
            }
        }

        if create_scope {
            self.scopes.pop();
        }
    }

    fn statement(&mut self, statement: &Statement) {
        match &statement.kind {
            StatementKind::Variable {
                name,
                mutable,
                type_annotation,
                initializer,
            } => {
                let actual = self.expression(initializer);
                let declared = self.annotation_type(type_annotation.as_ref());
                if let Some(expected) = &declared {
                    self.require_assignable(expected, &actual, initializer.span);
                }
                self.declare(
                    name,
                    Symbol {
                        kind: if *mutable {
                            SymbolKind::Variable
                        } else {
                            SymbolKind::Constant
                        },
                        mutable: *mutable,
                        arity: None,
                        definition: Some(name.span),
                        value_type: declared.unwrap_or(actual),
                    },
                );
            }
            StatementKind::Function { .. } => unreachable!("function bodies are analyzed last"),
            StatementKind::Struct { name, fields } => {
                let mut saw_default = false;
                for field in fields {
                    if let Some(default) = &field.default {
                        saw_default = true;
                        let actual = self.expression(default);
                        let expected = self
                            .structs
                            .get(name.as_str())
                            .and_then(|fields| fields.get(field.name.as_str()))
                            .cloned()
                            .unwrap_or(StaticType::Dynamic);
                        self.require_assignable(&expected, &actual, default.span);
                    } else if saw_default {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::ArgumentCount,
                            message: "required struct fields must precede fields with defaults"
                                .into(),
                            span: field.span,
                        });
                    }
                }
            }
            StatementKind::Interface { .. } => {}
            StatementKind::Import { .. } => {}
            StatementKind::Export(declaration) => self.statement(declaration),
            StatementKind::Task { .. } => unreachable!("task bodies are analyzed last"),
            StatementKind::Block(statements) => self.statements(statements, true),
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_type = self.expression(condition);
                self.require_assignable(&StaticType::Bool, &condition_type, condition.span);
                self.statement(then_branch);
                if let Some(else_branch) = else_branch {
                    self.statement(else_branch);
                }
            }
            StatementKind::While { condition, body } => {
                let condition_type = self.expression(condition);
                self.require_assignable(&StaticType::Bool, &condition_type, condition.span);
                self.statement(body);
            }
            StatementKind::Return(value) => {
                if self.function_depth == 0 {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::ReturnOutsideFunction,
                        message: "return is only valid inside a function".into(),
                        span: statement.span,
                    });
                }
                let actual = value
                    .as_ref()
                    .map_or(StaticType::Void, |value| self.expression(value));
                if let Some(expected) = self.current_return_type.clone() {
                    self.require_assignable(&expected, &actual, statement.span);
                }
            }
            StatementKind::Defer(expression) => {
                let previous = self.in_cleanup_expression;
                self.in_cleanup_expression = true;
                self.expression(expression);
                self.in_cleanup_expression = previous;
            }
            StatementKind::Using {
                name,
                initializer,
                cleanup,
            } => {
                let value_type = self.expression(initializer);
                self.declare(
                    name,
                    Symbol {
                        kind: SymbolKind::Constant,
                        mutable: false,
                        arity: None,
                        definition: Some(name.span),
                        value_type,
                    },
                );
                let previous = self.in_cleanup_expression;
                self.in_cleanup_expression = true;
                self.expression(cleanup);
                self.in_cleanup_expression = previous;
            }
            StatementKind::Expression(expression) => {
                self.expression(expression);
            }
        }
    }

    fn function(
        &mut self,
        parameters: &[Parameter],
        return_type: Option<&TypeAnnotation>,
        body: &[Statement],
        is_async: bool,
    ) {
        self.scopes.push(HashMap::new());
        for parameter in parameters {
            let value_type = parameter
                .type_annotation
                .as_ref()
                .and_then(|annotation| {
                    static_type_from_annotation(
                        annotation,
                        &self.struct_names,
                        &self.interface_names,
                    )
                })
                .unwrap_or(StaticType::Dynamic);
            self.declare(
                &parameter.name,
                Symbol {
                    kind: SymbolKind::Parameter,
                    mutable: true,
                    arity: None,
                    definition: Some(parameter.name.span),
                    value_type,
                },
            );
        }
        let previous_return_type = self.current_return_type.clone();
        let previous_async = self.current_function_async;
        self.current_return_type = return_type.and_then(|annotation| {
            static_type_from_annotation(annotation, &self.struct_names, &self.interface_names)
        });
        self.current_function_async = is_async;
        self.function_depth += 1;
        self.statements(body, false);
        self.function_depth -= 1;
        self.current_return_type = previous_return_type;
        self.current_function_async = previous_async;
        self.scopes.pop();
    }

    fn expression(&mut self, expression: &Expression) -> StaticType {
        match &expression.kind {
            ExpressionKind::Literal(literal) => match literal {
                Literal::Null => StaticType::Null,
                Literal::Bool(_) => StaticType::Bool,
                Literal::Integer(_) => StaticType::Int,
                Literal::Float(_) => StaticType::Float,
                Literal::String(_) => StaticType::String,
            },
            ExpressionKind::List(values) => {
                let mut element_type = StaticType::Dynamic;
                for value in values {
                    let next = self.expression(value);
                    element_type = element_type.merge(&next);
                }
                StaticType::List(Box::new(element_type))
            }
            ExpressionKind::Map(entries) => {
                let mut value_type = StaticType::Dynamic;
                let mut keys = HashSet::new();
                for (key, value) in entries {
                    if !keys.insert(key) {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::DuplicateName,
                            message: format!("map key '{key}' is duplicated"),
                            span: value.span,
                        });
                    }
                    value_type = value_type.merge(&self.expression(value));
                }
                StaticType::Map(Box::new(value_type))
            }
            ExpressionKind::Variable(name) => match self.resolve_reference(name) {
                Some(symbol) => symbol.value_type,
                None => {
                    self.undefined(name);
                    StaticType::Dynamic
                }
            },
            ExpressionKind::Assign { name, value } => {
                let actual = self.expression(value);
                match self.resolve_reference(name) {
                    None => {
                        self.undefined(name);
                        StaticType::Dynamic
                    }
                    Some(symbol) if !symbol.mutable => {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::ImmutableAssignment,
                            message: format!("cannot assign to immutable name '{}'", name.name),
                            span: name.span,
                        });
                        symbol.value_type
                    }
                    Some(symbol) => {
                        self.require_assignable(&symbol.value_type, &actual, value.span);
                        symbol.value_type
                    }
                }
            }
            ExpressionKind::IndexAssign {
                object,
                index,
                value,
            } => {
                let object_type = self.expression(object);
                let index_type = self.expression(index);
                let value_type = self.expression(value);
                match object_type {
                    StaticType::List(element_type) => {
                        self.require_assignable(&StaticType::Int, &index_type, index.span);
                        self.require_assignable(&element_type, &value_type, value.span);
                        value_type
                    }
                    StaticType::Map(element_type) => {
                        self.require_assignable(&StaticType::String, &index_type, index.span);
                        self.require_assignable(&element_type, &value_type, value.span);
                        value_type
                    }
                    StaticType::Dynamic => StaticType::Dynamic,
                    _ => {
                        self.type_error("value is not indexable", object.span);
                        StaticType::Dynamic
                    }
                }
            }
            ExpressionKind::MemberAssign {
                object,
                field,
                value,
            } => {
                let object_type = self.expression(object);
                let value_type = self.expression(value);
                match object_type {
                    StaticType::Struct(name) | StaticType::Interface(name) => {
                        let fields = self
                            .structs
                            .get(&name)
                            .or_else(|| self.interfaces.get(&name));
                        if let Some(field_type) = fields
                            .and_then(|fields| fields.get(field.as_str()))
                            .cloned()
                        {
                            self.require_assignable(&field_type, &value_type, value.span);
                        } else {
                            self.type_error(
                                format!("type '{name}' has no field '{}'", field.name),
                                field.span,
                            );
                        }
                        value_type
                    }
                    StaticType::Map(element_type) => {
                        self.require_assignable(&element_type, &value_type, value.span);
                        value_type
                    }
                    StaticType::Module(name) => {
                        self.type_error(format!("module '{name}' is immutable"), field.span);
                        StaticType::Dynamic
                    }
                    StaticType::Dynamic => StaticType::Dynamic,
                    _ => {
                        self.type_error("value has no fields", object.span);
                        StaticType::Dynamic
                    }
                }
            }
            ExpressionKind::Unary { operator, operand } => {
                let operand_type = self.expression(operand);
                match operator {
                    UnaryOperator::Not => {
                        self.require_assignable(&StaticType::Bool, &operand_type, operand.span);
                        StaticType::Bool
                    }
                    UnaryOperator::Negate => {
                        if !operand_type.is_numeric() && operand_type != StaticType::Dynamic {
                            self.type_error("unary '-' requires a number", expression.span);
                        }
                        operand_type
                    }
                }
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let left_type = self.expression(left);
                let right_type = self.expression(right);
                self.binary_type(*operator, &left_type, &right_type, expression.span)
            }
            ExpressionKind::Call { callee, arguments } => {
                self.check_task_result_access(callee, arguments, expression.span);
                let callee_type = self.expression(callee);
                let argument_types = arguments
                    .iter()
                    .map(|argument| self.expression(argument))
                    .collect::<Vec<_>>();
                if let ExpressionKind::Variable(name) = &callee.kind
                    && name.name == "Ok"
                {
                    if arguments.len() > 1 {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::ArgumentCount,
                            message: format!(
                                "function 'Ok' expects 0 or 1 arguments but received {}",
                                arguments.len()
                            ),
                            span: expression.span,
                        });
                    }
                    return StaticType::Result(
                        Box::new(argument_types.first().cloned().unwrap_or(StaticType::Void)),
                        Box::new(StaticType::Dynamic),
                    );
                }
                if let ExpressionKind::Variable(name) = &callee.kind
                    && let Some(Symbol {
                        arity: Some(arity), ..
                    }) = self.resolve(name.as_str())
                    && arity != arguments.len()
                {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::ArgumentCount,
                        message: format!(
                            "function '{}' expects {arity} arguments but received {}",
                            name.name,
                            arguments.len()
                        ),
                        span: expression.span,
                    });
                }
                if let StaticType::Function { parameters, result } = callee_type {
                    if matches!(callee.kind, ExpressionKind::Member { .. })
                        && parameters.len() != arguments.len()
                    {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::ArgumentCount,
                            message: format!(
                                "function expects {} arguments but received {}",
                                parameters.len(),
                                arguments.len()
                            ),
                            span: expression.span,
                        });
                    }
                    for ((expected, actual), argument) in
                        parameters.iter().zip(&argument_types).zip(arguments)
                    {
                        self.require_assignable(expected, actual, argument.span);
                    }
                    *result
                } else {
                    if callee_type != StaticType::Dynamic {
                        self.type_error("value is not callable", callee.span);
                    }
                    StaticType::Dynamic
                }
            }
            ExpressionKind::Index { object, index } => {
                let object_type = self.expression(object);
                let index_type = self.expression(index);
                match object_type {
                    StaticType::List(element_type) => {
                        self.require_assignable(&StaticType::Int, &index_type, index.span);
                        *element_type
                    }
                    StaticType::Map(element_type) => {
                        self.require_assignable(&StaticType::String, &index_type, index.span);
                        *element_type
                    }
                    StaticType::Dynamic => StaticType::Dynamic,
                    _ => {
                        self.type_error("value is not indexable", object.span);
                        StaticType::Dynamic
                    }
                }
            }
            ExpressionKind::Member { object, field } => {
                let object_type = self.expression(object);
                match object_type {
                    StaticType::Struct(name) | StaticType::Interface(name) => {
                        let fields = self
                            .structs
                            .get(&name)
                            .or_else(|| self.interfaces.get(&name));
                        if let Some(field_type) = fields
                            .and_then(|fields| fields.get(field.as_str()))
                            .cloned()
                        {
                            field_type
                        } else {
                            self.type_error(
                                format!("type '{name}' has no field '{}'", field.name),
                                field.span,
                            );
                            StaticType::Dynamic
                        }
                    }
                    StaticType::Map(element_type) => *element_type,
                    StaticType::Module(module) => {
                        if let Some(value_type) = module_member_type(&module, field.as_str()) {
                            value_type
                        } else {
                            self.type_error(
                                format!("module '{module}' has no member '{}'", field.name),
                                field.span,
                            );
                            StaticType::Dynamic
                        }
                    }
                    StaticType::Dynamic => StaticType::Dynamic,
                    _ => {
                        self.type_error("value has no fields", object.span);
                        StaticType::Dynamic
                    }
                }
            }
            ExpressionKind::Match { value, arms } => {
                let subject_type = self.expression(value);
                let mut result_type = StaticType::Dynamic;
                let mut variants = HashSet::new();
                let mut has_wildcard = false;
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    match &arm.pattern.kind {
                        PatternKind::Wildcard => has_wildcard = true,
                        PatternKind::Literal(literal) => {
                            let pattern_type = literal_type(literal);
                            if !subject_type.accepts(&pattern_type) {
                                self.type_error(
                                    format!(
                                        "pattern type {} cannot match {}",
                                        pattern_type.display_name(),
                                        subject_type.display_name()
                                    ),
                                    arm.pattern.span,
                                );
                            }
                        }
                        PatternKind::Variant { name, binding } => {
                            variants.insert(name.name.clone());
                            let payload = self.variant_payload(&subject_type, name);
                            match (binding, payload) {
                                (Some(binding), Some(Some(value_type))) => self.declare(
                                    binding,
                                    Symbol {
                                        kind: SymbolKind::Constant,
                                        mutable: false,
                                        arity: None,
                                        definition: Some(binding.span),
                                        value_type,
                                    },
                                ),
                                (Some(binding), _) => self.type_error(
                                    format!("variant '{}' has no payload", name.name),
                                    binding.span,
                                ),
                                (None, Some(Some(_))) => self.type_error(
                                    format!("variant '{}' requires a binding", name.name),
                                    name.span,
                                ),
                                (None, Some(None)) => {}
                                (_, None) => self.type_error(
                                    format!(
                                        "variant '{}' cannot match {}",
                                        name.name,
                                        subject_type.display_name()
                                    ),
                                    name.span,
                                ),
                            }
                        }
                    }
                    let arm_type = self.expression(&arm.value);
                    result_type = result_type.merge(&arm_type);
                    self.scopes.pop();
                }
                if !has_wildcard && !self.match_is_exhaustive(&subject_type, &variants) {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::NonExhaustiveMatch,
                        message: "match expression is not exhaustive".into(),
                        span: expression.span,
                    });
                }
                result_type
            }
            ExpressionKind::Try(value) => {
                let value_type = self.expression(value);
                match &value_type {
                    StaticType::Option(inner) => {
                        let expected = StaticType::Option(Box::new(StaticType::Dynamic));
                        self.require_try_return_type(&expected, expression.span);
                        (**inner).clone()
                    }
                    StaticType::Result(ok, error) => {
                        let expected =
                            StaticType::Result(Box::new(StaticType::Dynamic), error.clone());
                        self.require_try_return_type(&expected, expression.span);
                        (**ok).clone()
                    }
                    StaticType::Dynamic => StaticType::Dynamic,
                    _ => {
                        self.type_error("operator '?' requires Option or Result", expression.span);
                        StaticType::Dynamic
                    }
                }
            }
            ExpressionKind::Await(value) => {
                let value_type = self.expression(value);
                if self.function_depth > 0 && !self.current_function_async {
                    self.type_error(
                        "'await' is only valid at module top level or inside an async function",
                        expression.span,
                    );
                }
                if self.in_cleanup_expression {
                    self.type_error(
                        "'await' is not allowed in defer or using cleanup",
                        expression.span,
                    );
                }
                match value_type {
                    StaticType::Future(result) => *result,
                    StaticType::Dynamic => StaticType::Dynamic,
                    other => {
                        self.type_error(
                            format!(
                                "'await' requires Future<T>, received {}",
                                other.display_name()
                            ),
                            expression.span,
                        );
                        StaticType::Dynamic
                    }
                }
            }
        }
    }

    fn binary_type(
        &mut self,
        operator: BinaryOperator,
        left: &StaticType,
        right: &StaticType,
        span: Span,
    ) -> StaticType {
        match operator {
            BinaryOperator::Equal | BinaryOperator::NotEqual => StaticType::Bool,
            BinaryOperator::And | BinaryOperator::Or => {
                self.require_assignable(&StaticType::Bool, left, span);
                self.require_assignable(&StaticType::Bool, right, span);
                StaticType::Bool
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => {
                if (!left.is_numeric() && *left != StaticType::Dynamic)
                    || (!right.is_numeric() && *right != StaticType::Dynamic)
                {
                    self.type_error("comparison requires numbers", span);
                }
                StaticType::Bool
            }
            BinaryOperator::Add if *left == StaticType::String && *right == StaticType::String => {
                StaticType::String
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder => {
                if (!left.is_numeric() && *left != StaticType::Dynamic)
                    || (!right.is_numeric() && *right != StaticType::Dynamic)
                {
                    self.type_error("operator requires numbers", span);
                    return StaticType::Dynamic;
                }
                if *left == StaticType::Int && *right == StaticType::Int {
                    StaticType::Int
                } else if *left == StaticType::Dynamic || *right == StaticType::Dynamic {
                    StaticType::Dynamic
                } else {
                    StaticType::Number
                }
            }
        }
    }

    fn variant_payload(
        &self,
        subject: &StaticType,
        name: &Identifier,
    ) -> Option<Option<StaticType>> {
        match (subject, name.as_str()) {
            (StaticType::Option(value), "Some") => Some(Some((**value).clone())),
            (StaticType::Option(_), "None") => Some(None),
            (StaticType::Result(value, _), "Ok") => Some(Some((**value).clone())),
            (StaticType::Result(_, error), "Err") => Some(Some((**error).clone())),
            (StaticType::Dynamic, "Some" | "Ok" | "Err") => Some(Some(StaticType::Dynamic)),
            (StaticType::Dynamic, "None") => Some(None),
            _ => None,
        }
    }

    fn match_is_exhaustive(&self, subject: &StaticType, variants: &HashSet<String>) -> bool {
        match subject {
            StaticType::Option(_) => variants.contains("Some") && variants.contains("None"),
            StaticType::Result(_, _) => variants.contains("Ok") && variants.contains("Err"),
            _ => false,
        }
    }

    fn require_try_return_type(&mut self, propagated: &StaticType, span: Span) {
        let Some(expected) = self.current_return_type.clone() else {
            self.type_error(
                "operator '?' requires a function with an Option or Result return type",
                span,
            );
            return;
        };
        if !self.is_assignable(&expected, propagated) {
            self.type_error(
                format!(
                    "operator '?' cannot propagate {} from a function returning {}",
                    propagated.display_name(),
                    expected.display_name()
                ),
                span,
            );
        }
    }

    fn declare(&mut self, name: &Identifier, symbol: Symbol) {
        let occurrence_kind = symbol.kind;
        let occurrence_type = symbol.value_type.display_name();
        let scope = self.scopes.last_mut().expect("analyzer always has a scope");
        if scope.insert(name.name.clone(), symbol).is_some() {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::DuplicateName,
                message: format!("name '{}' is already declared in this scope", name.name),
                span: name.span,
            });
        }
        self.symbols.push(SymbolOccurrence {
            name: name.name.clone(),
            span: name.span,
            definition: name.span,
            kind: occurrence_kind,
            is_definition: true,
            type_name: Some(occurrence_type),
        });
    }

    fn resolve(&self, name: &str) -> Option<Symbol> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn resolve_reference(&mut self, name: &Identifier) -> Option<Symbol> {
        let symbol = self.resolve(name.as_str())?;
        if let Some(definition) = symbol.definition {
            self.symbols.push(SymbolOccurrence {
                name: name.name.clone(),
                span: name.span,
                definition,
                kind: symbol.kind,
                is_definition: false,
                type_name: Some(symbol.value_type.display_name()),
            });
        }
        Some(symbol)
    }

    fn check_task_result_access(
        &mut self,
        callee: &Expression,
        arguments: &[Expression],
        span: Span,
    ) {
        let ExpressionKind::Member { object, field } = &callee.kind else {
            return;
        };
        let ExpressionKind::Variable(module) = &object.kind else {
            return;
        };
        if module.name != "task" || field.name != "result" {
            return;
        }
        let Some(needs) = &self.current_task_needs else {
            self.type_error("task.result is only valid inside a task", span);
            return;
        };
        let [
            Expression {
                kind: ExpressionKind::Literal(Literal::String(name)),
                ..
            },
        ] = arguments
        else {
            self.type_error(
                "task.result requires one string literal dependency name",
                span,
            );
            return;
        };
        if !needs.contains(name) {
            self.type_error(
                format!("task.result('{name}') is not a declared dependency"),
                span,
            );
        }
    }

    fn undefined(&mut self, name: &Identifier) {
        self.errors.push(SemanticError {
            kind: SemanticErrorKind::UndefinedName,
            message: format!("undefined name '{}'", name.name),
            span: name.span,
        });
    }

    fn annotation_type(&mut self, annotation: Option<&TypeAnnotation>) -> Option<StaticType> {
        let annotation = annotation?;
        let Some(value_type) =
            static_type_from_annotation(annotation, &self.struct_names, &self.interface_names)
        else {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::UnknownType,
                message: format!("unknown type '{}'", annotation.name.name),
                span: annotation.span,
            });
            return Some(StaticType::Dynamic);
        };
        if !matches!(
            annotation.name.as_str(),
            "list" | "map" | "Option" | "Result" | "Future" | "Task" | "Worker"
        ) && !annotation.arguments.is_empty()
        {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::UnknownType,
                message: format!(
                    "type '{}' does not accept type arguments",
                    annotation.name.name
                ),
                span: annotation.span,
            });
        }
        Some(value_type)
    }

    fn require_assignable(&mut self, expected: &StaticType, actual: &StaticType, span: Span) {
        if !self.is_assignable(expected, actual) {
            let hint = if matches!(actual, StaticType::Future(_))
                && !matches!(expected, StaticType::Future(_) | StaticType::Dynamic)
            {
                "; did you forget 'await'?"
            } else {
                ""
            };
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::TypeMismatch,
                message: format!(
                    "expected {}, received {}{hint}",
                    expected.display_name(),
                    actual.display_name()
                ),
                span,
            });
        }
    }

    fn is_assignable(&self, expected: &StaticType, actual: &StaticType) -> bool {
        match (expected, actual) {
            (StaticType::List(expected), StaticType::List(actual))
            | (StaticType::Map(expected), StaticType::Map(actual))
            | (StaticType::Option(expected), StaticType::Option(actual))
            | (StaticType::Future(expected), StaticType::Future(actual))
            | (StaticType::Task(expected), StaticType::Task(actual))
            | (StaticType::Worker(expected), StaticType::Worker(actual)) => {
                self.is_assignable(expected, actual)
            }
            (
                StaticType::Result(expected_value, expected_error),
                StaticType::Result(actual_value, actual_error),
            ) => {
                self.is_assignable(expected_value, actual_value)
                    && self.is_assignable(expected_error, actual_error)
            }
            (StaticType::Interface(interface), StaticType::Struct(structure)) => self
                .interfaces
                .get(interface)
                .zip(self.structs.get(structure))
                .is_some_and(|(required, available)| {
                    required.iter().all(|(name, expected)| {
                        available
                            .get(name)
                            .is_some_and(|actual| self.is_assignable(expected, actual))
                    })
                }),
            (StaticType::Interface(expected), StaticType::Interface(actual)) => {
                if expected == actual {
                    true
                } else {
                    self.interfaces
                        .get(expected)
                        .zip(self.interfaces.get(actual))
                        .is_some_and(|(required, available)| {
                            required.iter().all(|(name, expected)| {
                                available
                                    .get(name)
                                    .is_some_and(|actual| self.is_assignable(expected, actual))
                            })
                        })
                }
            }
            _ => expected.accepts(actual),
        }
    }

    fn type_error(&mut self, message: impl Into<String>, span: Span) {
        self.errors.push(SemanticError {
            kind: SemanticErrorKind::TypeMismatch,
            message: message.into(),
            span,
        });
    }
}

impl StaticType {
    fn is_numeric(&self) -> bool {
        matches!(self, Self::Int | Self::Float | Self::Number)
    }

    fn accepts(&self, actual: &Self) -> bool {
        if let (Self::List(expected), Self::List(actual)) = (self, actual) {
            return expected.accepts(actual);
        }
        if let (Self::Map(expected), Self::Map(actual)) = (self, actual) {
            return expected.accepts(actual);
        }
        if let (Self::Option(expected), Self::Option(actual)) = (self, actual) {
            return expected.accepts(actual);
        }
        if let (Self::Result(expected_ok, expected_error), Self::Result(actual_ok, actual_error)) =
            (self, actual)
        {
            return expected_ok.accepts(actual_ok) && expected_error.accepts(actual_error);
        }
        if let (Self::Future(expected), Self::Future(actual)) = (self, actual) {
            return expected.accepts(actual);
        }
        if let (Self::Task(expected), Self::Task(actual)) = (self, actual) {
            return expected.accepts(actual);
        }
        if let (Self::Worker(expected), Self::Worker(actual)) = (self, actual) {
            return expected.accepts(actual);
        }
        self == &Self::Dynamic
            || actual == &Self::Dynamic
            || self == actual
            || (*self == Self::Number && actual.is_numeric())
    }

    fn display_name(&self) -> String {
        match self {
            Self::Dynamic => "any".into(),
            Self::Void => "void".into(),
            Self::Null => "null".into(),
            Self::Bool => "bool".into(),
            Self::Int => "int".into(),
            Self::Float => "float".into(),
            Self::Number => "number".into(),
            Self::String => "string".into(),
            Self::List(element) => format!("list<{}>", element.display_name()),
            Self::Map(element) => format!("map<{}>", element.display_name()),
            Self::Module(name) => format!("module {name}"),
            Self::Struct(name) => name.clone(),
            Self::Interface(name) => name.clone(),
            Self::Option(value) => format!("Option<{}>", value.display_name()),
            Self::Result(value, error) => {
                format!("Result<{}, {}>", value.display_name(), error.display_name())
            }
            Self::Future(value) => format!("Future<{}>", value.display_name()),
            Self::Task(value) => format!("Task<{}>", value.display_name()),
            Self::Worker(value) => format!("Worker<{}>", value.display_name()),
            Self::Function { parameters, result } => format!(
                "fn({}) -> {}",
                parameters
                    .iter()
                    .map(Self::display_name)
                    .collect::<Vec<_>>()
                    .join(", "),
                result.display_name()
            ),
        }
    }

    fn merge(&self, other: &Self) -> Self {
        if self == &Self::Dynamic {
            return other.clone();
        }
        if other == &Self::Dynamic || self == other {
            return self.clone();
        }
        if self.is_numeric() && other.is_numeric() {
            return Self::Number;
        }
        Self::Dynamic
    }
}

fn static_type_from_annotation(
    annotation: &TypeAnnotation,
    struct_names: &HashSet<String>,
    interface_names: &HashSet<String>,
) -> Option<StaticType> {
    match annotation.name.as_str() {
        "any" => Some(StaticType::Dynamic),
        "void" => Some(StaticType::Void),
        "null" => Some(StaticType::Null),
        "bool" => Some(StaticType::Bool),
        "int" => Some(StaticType::Int),
        "float" => Some(StaticType::Float),
        "number" => Some(StaticType::Number),
        "string" => Some(StaticType::String),
        "list" if annotation.arguments.len() == 1 => {
            static_type_from_annotation(&annotation.arguments[0], struct_names, interface_names)
                .map(|element| StaticType::List(Box::new(element)))
        }
        "map" if annotation.arguments.len() == 1 => {
            static_type_from_annotation(&annotation.arguments[0], struct_names, interface_names)
                .map(|value| StaticType::Map(Box::new(value)))
        }
        "Option" if annotation.arguments.len() == 1 => {
            static_type_from_annotation(&annotation.arguments[0], struct_names, interface_names)
                .map(|value| StaticType::Option(Box::new(value)))
        }
        "Result" if annotation.arguments.len() == 2 => {
            let value = static_type_from_annotation(
                &annotation.arguments[0],
                struct_names,
                interface_names,
            )?;
            let error = static_type_from_annotation(
                &annotation.arguments[1],
                struct_names,
                interface_names,
            )?;
            Some(StaticType::Result(Box::new(value), Box::new(error)))
        }
        "Future" if annotation.arguments.len() == 1 => {
            static_type_from_annotation(&annotation.arguments[0], struct_names, interface_names)
                .map(|value| StaticType::Future(Box::new(value)))
        }
        "Task" if annotation.arguments.len() == 1 => {
            static_type_from_annotation(&annotation.arguments[0], struct_names, interface_names)
                .map(|value| StaticType::Task(Box::new(value)))
        }
        "Worker" if annotation.arguments.len() == 1 => {
            static_type_from_annotation(&annotation.arguments[0], struct_names, interface_names)
                .map(|value| StaticType::Worker(Box::new(value)))
        }
        "fn" => Some(StaticType::Function {
            parameters: Vec::new(),
            result: Box::new(StaticType::Dynamic),
        }),
        name if struct_names.contains(name) && annotation.arguments.is_empty() => {
            Some(StaticType::Struct(name.into()))
        }
        name if interface_names.contains(name) && annotation.arguments.is_empty() => {
            Some(StaticType::Interface(name.into()))
        }
        _ => None,
    }
}

fn module_member_type(module: &str, member: &str) -> Option<StaticType> {
    match (module, member) {
        ("json", "parse") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::Dynamic),
        )),
        ("json", "stringify") => Some(function_type(
            vec![StaticType::Dynamic],
            result_type(StaticType::String),
        )),
        ("fs", "read_text") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::String),
        )),
        ("fs", "read_text_async") => Some(function_type(
            vec![StaticType::String],
            StaticType::Future(Box::new(result_type(StaticType::String))),
        )),
        ("fs", "write_text") => Some(function_type(
            vec![StaticType::String, StaticType::String],
            result_type(StaticType::Void),
        )),
        ("fs", "write_text_async") => Some(function_type(
            vec![StaticType::String, StaticType::String],
            StaticType::Future(Box::new(result_type(StaticType::Void))),
        )),
        ("fs", "exists") => Some(function_type(vec![StaticType::String], StaticType::Bool)),
        ("fs", "create_dir_all" | "remove_file") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::Void),
        )),
        ("path", "join") => Some(function_type(
            vec![StaticType::List(Box::new(StaticType::String))],
            StaticType::String,
        )),
        ("path", "parent" | "file_name") => Some(function_type(
            vec![StaticType::String],
            option_type(StaticType::String),
        )),
        ("path", "is_absolute") => Some(function_type(vec![StaticType::String], StaticType::Bool)),
        ("env", "get") => Some(function_type(
            vec![StaticType::String],
            option_type(StaticType::String),
        )),
        ("env", "current_dir") => Some(function_type(Vec::new(), result_type(StaticType::String))),
        ("process", "run") => Some(function_type(
            vec![
                StaticType::String,
                StaticType::List(Box::new(StaticType::String)),
            ],
            result_type(StaticType::Map(Box::new(StaticType::Dynamic))),
        )),
        ("process", "run_async") => Some(function_type(
            vec![
                StaticType::String,
                StaticType::List(Box::new(StaticType::String)),
            ],
            StaticType::Future(Box::new(result_type(StaticType::Map(Box::new(
                StaticType::Dynamic,
            ))))),
        )),
        ("terminal", "input" | "password") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::String),
        )),
        ("terminal", "confirm") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::Bool),
        )),
        ("task", "result") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::Dynamic),
        )),
        ("task", "spawn") => Some(function_type(
            vec![StaticType::Future(Box::new(StaticType::Dynamic))],
            StaticType::Task(Box::new(StaticType::Dynamic)),
        )),
        ("task", "join") => Some(function_type(
            vec![StaticType::Task(Box::new(StaticType::Dynamic))],
            StaticType::Future(Box::new(result_type(StaticType::Dynamic))),
        )),
        ("task", "cancel") => Some(function_type(
            vec![StaticType::Task(Box::new(StaticType::Dynamic))],
            StaticType::Bool,
        )),
        ("task", "timeout") => Some(function_type(
            vec![
                StaticType::Future(Box::new(StaticType::Dynamic)),
                StaticType::Int,
            ],
            StaticType::Future(Box::new(result_type(StaticType::Dynamic))),
        )),
        ("worker", "spawn") => Some(function_type(
            vec![StaticType::String, StaticType::Dynamic],
            StaticType::Worker(Box::new(StaticType::Map(Box::new(StaticType::Dynamic)))),
        )),
        ("worker", "join") => Some(function_type(
            vec![StaticType::Worker(Box::new(StaticType::Dynamic))],
            StaticType::Future(Box::new(result_type(StaticType::Map(Box::new(
                StaticType::Dynamic,
            ))))),
        )),
        ("worker", "cancel") => Some(function_type(
            vec![StaticType::Worker(Box::new(StaticType::Dynamic))],
            StaticType::Bool,
        )),
        ("worker", "timeout") => Some(function_type(
            vec![
                StaticType::Worker(Box::new(StaticType::Dynamic)),
                StaticType::Int,
            ],
            StaticType::Future(Box::new(result_type(StaticType::Map(Box::new(
                StaticType::Dynamic,
            ))))),
        )),
        ("worker", "threads") => Some(function_type(Vec::new(), StaticType::Int)),
        ("worker", "status") => Some(function_type(
            vec![StaticType::Worker(Box::new(StaticType::Dynamic))],
            StaticType::String,
        )),
        ("worker", "stats") => Some(function_type(
            Vec::new(),
            StaticType::Map(Box::new(StaticType::Dynamic)),
        )),
        ("http", "get") => Some(function_type(
            vec![StaticType::String],
            result_type(StaticType::Map(Box::new(StaticType::Dynamic))),
        )),
        ("http", "get_async") => Some(function_type(
            vec![StaticType::String],
            StaticType::Future(Box::new(result_type(StaticType::Map(Box::new(
                StaticType::Dynamic,
            ))))),
        )),
        ("http", "request") => Some(function_type(
            vec![
                StaticType::String,
                StaticType::String,
                StaticType::Map(Box::new(StaticType::String)),
                StaticType::String,
            ],
            result_type(StaticType::Map(Box::new(StaticType::Dynamic))),
        )),
        ("http", "request_async") => Some(function_type(
            vec![
                StaticType::String,
                StaticType::String,
                StaticType::Map(Box::new(StaticType::String)),
                StaticType::String,
            ],
            StaticType::Future(Box::new(result_type(StaticType::Map(Box::new(
                StaticType::Dynamic,
            ))))),
        )),
        ("time", "unix_ms") => Some(function_type(Vec::new(), result_type(StaticType::Int))),
        ("time", "monotonic_ms") => Some(function_type(Vec::new(), StaticType::Int)),
        ("time", "sleep_ms") => Some(function_type(
            vec![StaticType::Int],
            result_type(StaticType::Void),
        )),
        ("time", "sleep_async") => Some(function_type(
            vec![StaticType::Int],
            StaticType::Future(Box::new(result_type(StaticType::Void))),
        )),
        _ => None,
    }
}

fn function_type(parameters: Vec<StaticType>, result: StaticType) -> StaticType {
    StaticType::Function {
        parameters,
        result: Box::new(result),
    }
}

fn result_type(value: StaticType) -> StaticType {
    StaticType::Result(Box::new(value), Box::new(StaticType::String))
}

fn option_type(value: StaticType) -> StaticType {
    StaticType::Option(Box::new(value))
}

fn literal_type(literal: &Literal) -> StaticType {
    match literal {
        Literal::Null => StaticType::Null,
        Literal::Bool(_) => StaticType::Bool,
        Literal::Integer(_) => StaticType::Int,
        Literal::Float(_) => StaticType::Float,
        Literal::String(_) => StaticType::String,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        STANDARD_MODULES, SemanticErrorKind, analyze, analyze_program,
        analyze_program_with_modules, analyze_worker_program, module_member_type,
    };
    use orvi_syntax::parse;
    use std::collections::HashMap;

    #[test]
    fn standard_library_catalog_matches_semantic_types() {
        for module in STANDARD_MODULES {
            assert!(
                !module.members.is_empty(),
                "{} must expose members",
                module.name
            );
            for member in module.members {
                assert!(
                    module_member_type(module.name, member.name).is_some(),
                    "std:{}.{} is missing a semantic type",
                    module.name,
                    member.name
                );
            }
        }
    }

    fn errors(source: &str) -> Vec<super::SemanticError> {
        let parsed = parse(source);
        assert!(
            !parsed.has_errors(),
            "test source must parse: {:?}",
            parsed.parse_errors
        );
        analyze(&parsed.program)
    }

    #[test]
    fn accepts_scopes_functions_and_builtin_print() {
        assert!(
            errors("fn add(a, b) { return a + b; } let result = add(1, 2); print(result);")
                .is_empty()
        );
    }

    #[test]
    fn checks_async_functions_futures_and_await_context() {
        assert!(
            errors(
                "async fn compute() -> int { return 42 } let pending: Future<int> = compute(); let value: int = await pending;",
            )
            .is_empty()
        );
        assert!(
            errors(
                "async fn inner() -> int { return 1 } async fn outer() -> int { return await inner() }",
            )
            .is_empty()
        );
        assert!(
            errors(
                "async fn inner() -> Result<int, string> { return Ok(1) } async fn outer() -> Result<int, string> { let value = (await inner())?; return Ok(value) }",
            )
            .is_empty()
        );

        let invalid = errors(
            "async fn work() -> int { defer await work(); return await 1 } fn sync() { await work() }",
        );
        assert!(
            invalid
                .iter()
                .any(|error| error.message.contains("cleanup"))
        );
        assert!(
            invalid
                .iter()
                .any(|error| error.message.contains("requires Future"))
        );
        assert!(
            invalid
                .iter()
                .any(|error| error.message.contains("module top level"))
        );

        let missing_await = errors("async fn work() -> int { return 1 } let value: int = work();");
        assert!(
            missing_await
                .iter()
                .any(|error| error.message.contains("forget 'await'"))
        );
        assert!(
            errors(
                "async fn work() -> Result<int, string> { (await time.sleep_async(1))?; return Ok(1) } async fn main() -> Result<int, string> { let handle: Task<Result<int, string>> = task.spawn(work()); task.cancel(handle); return (await task.join(handle))? } await main();",
            )
            .is_empty()
        );
        assert!(
            errors(
                "async fn work() -> int { return 1 } let timed: Result<int, string> = await task.timeout(work(), 1);",
            )
            .is_empty()
        );
    }

    #[test]
    fn checks_worker_handles_and_worker_input() {
        let worker_errors = errors(
            "let handle: Worker<map<any>> = worker.spawn(\"worker_input\", 7); let result: Future<Result<map<any>, string>> = worker.join(handle);",
        );
        assert!(worker_errors.is_empty(), "{worker_errors:?}");
        let worker = parse("worker_input");
        assert!(analyze_worker_program(&worker.program).errors.is_empty());
        assert_eq!(errors("print(worker_input)").len(), 1);
    }

    #[test]
    fn reports_undefined_and_duplicate_names() {
        let errors = errors("let value = missing; let value = 2;");
        assert_eq!(errors[0].kind, SemanticErrorKind::UndefinedName);
        assert_eq!(errors[1].kind, SemanticErrorKind::DuplicateName);
    }

    #[test]
    fn rejects_assignment_to_constants_and_functions() {
        let errors = errors("const value = 1; value = 2; fn work() {} work = 3;");
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.kind == SemanticErrorKind::ImmutableAssignment)
        );
    }

    #[test]
    fn validates_returns_and_argument_counts() {
        let errors = errors("fn one(value) { return value; } one(); return 1;");
        assert_eq!(errors[0].kind, SemanticErrorKind::ArgumentCount);
        assert_eq!(errors[1].kind, SemanticErrorKind::ReturnOutsideFunction);
    }

    #[test]
    fn closures_can_capture_names_declared_later_in_the_same_scope() {
        assert!(errors("fn show() { print(value); } let value = 42; show();").is_empty());
    }

    #[test]
    fn indexes_definitions_and_resolved_references() {
        let parsed = parse("let value = 1; { let value = 2; print(value); } print(value);");
        let analysis = analyze_program(&parsed.program);
        let definitions = analysis
            .symbols
            .iter()
            .filter(|symbol| symbol.is_definition)
            .collect::<Vec<_>>();
        let references = analysis
            .symbols
            .iter()
            .filter(|symbol| !symbol.is_definition)
            .collect::<Vec<_>>();

        assert_eq!(definitions.len(), 2);
        assert_eq!(references.len(), 2);
        assert_eq!(references[0].definition, definitions[1].definition);
        assert_eq!(references[1].definition, definitions[0].definition);
    }

    #[test]
    fn checks_declared_binding_and_function_types() {
        let errors = errors(
            "let count: int = \"wrong\"; fn twice(value: int) -> int { return value * 2; } twice(\"wrong\");",
        );
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.kind == SemanticErrorKind::TypeMismatch)
        );
    }

    #[test]
    fn reports_unknown_types() {
        let errors = errors("let value: missing = 1;");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind, SemanticErrorKind::UnknownType);
    }

    #[test]
    fn checks_list_element_and_index_types() {
        assert!(errors("let values: list<int> = [1, 2]; values[0] = 3;").is_empty());
        let errors = errors("let values: list<int> = [1, 2]; values[0] = \"wrong\";");
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind, SemanticErrorKind::TypeMismatch);
    }

    #[test]
    fn checks_struct_constructors_and_fields() {
        assert!(
            errors("struct Server { host: string; port: int = 22 } let server: Server = Server(\"localhost\"); server.port = 80;")
                .is_empty()
        );
        let errors = errors(
            "struct Server { host: string; port: int = 22 } let server = Server(1); server.port = \"wrong\"; server.missing;",
        );
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .all(|error| error.kind == SemanticErrorKind::TypeMismatch)
        );
    }

    #[test]
    fn checks_option_result_match_and_propagation() {
        let source = "fn read(value: Option<int>) -> Option<int> { let inner = value?; return Some(match inner { 0 => 1, _ => inner }) } fn work(value: Result<int, string>) -> Result<int, string> { return Ok(value? + 1) }";
        assert!(errors(source).is_empty());

        let errors = errors(
            "fn bad(value: Option<int>) -> int { return value? } match Some(1) { Some(value) => value }",
        );
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .any(|error| error.kind == SemanticErrorKind::TypeMismatch)
        );
        assert!(
            errors
                .iter()
                .any(|error| error.kind == SemanticErrorKind::NonExhaustiveMatch)
        );
    }

    #[test]
    fn analyzes_deferred_and_using_cleanup_in_scope() {
        let source = "fn work(value: Option<int>) -> Option<int> { defer print(\"done\"); using handle = \"db\" => print(\"close\", handle); let inner = value?; return Some(inner) }";
        assert!(errors(source).is_empty());
    }

    #[test]
    fn checks_structural_interface_contracts_and_fields() {
        let source = "interface Named { name: string } struct Job { name: string; retries: int = 3 } fn label(value: Named) -> string { return value.name } print(label(Job(\"release\")))";
        assert!(errors(source).is_empty());

        let errors = errors(
            "interface Named { name: string } struct Job { title: string } let value: Named = Job(\"release\")",
        );
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].kind, SemanticErrorKind::TypeMismatch);
    }

    #[test]
    fn checks_map_keys_and_values() {
        assert!(
            errors("let values: map<int> = { first: 1 }; values[\"second\"] = 2; values.third = 3")
                .is_empty()
        );
        let errors =
            errors("let values: map<int> = { first: 1 }; values[0] = 2; values.second = \"wrong\"");
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.kind == SemanticErrorKind::TypeMismatch)
        );
    }

    #[test]
    fn checks_json_module_calls() {
        assert!(
            errors("fn read() -> Result<any, string> { return json.parse(\"{}\") }").is_empty()
        );
        let errors = errors("json.parse(1); json.missing");
        assert_eq!(errors.len(), 2);
        assert!(
            errors
                .iter()
                .all(|error| error.kind == SemanticErrorKind::TypeMismatch)
        );
    }

    #[test]
    fn checks_automation_module_calls() {
        let source = "fs.read_text(\"file\"); fs.write_text(\"file\", \"text\"); path.join([\"a\", \"b\"]); env.get(\"NAME\"); env.current_dir(); process.run(\"tool\", [\"arg\"]); terminal.confirm(\"Continue?\"); print(context.cwd)";
        assert!(errors(source).is_empty());

        let errors = errors("fs.write_text(\"only-one\"); process.run(1, [])");
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn checks_http_and_time_module_calls() {
        assert!(
            errors(
                "fn fetch() -> Result<map<any>, string> { return http.get(\"https://example.com\") } fn clock() -> Result<int, string> { return time.unix_ms() } time.sleep_ms(0); print(time.monotonic_ms())"
            )
            .is_empty()
        );
        let errors = errors(
            "http.get(1); http.request(\"GET\", \"https://example.com\", [], \"\"); time.sleep_ms(\"soon\")",
        );
        assert!(errors.len() >= 3);
    }

    #[test]
    fn declares_named_imports_and_checks_standard_exports() {
        assert!(errors("import { read_text } from \"std:fs\"; read_text(\"file\")").is_empty());
        assert!(errors("import { value } from \"./module.orvi\"; print(value)").is_empty());
        let missing_export = errors("import { missing } from \"std:fs\";");
        assert_eq!(missing_export.len(), 1);
        assert_eq!(missing_export[0].kind, SemanticErrorKind::UndefinedName);
        let missing_module = errors("import { value } from \"std:missing\";");
        assert_eq!(missing_module.len(), 1);
        assert_eq!(
            missing_module[0].message,
            "unknown standard module 'std:missing'"
        );
    }

    #[test]
    fn checks_calls_against_imported_function_signatures() {
        let module = parse("export fn double(value: int) -> int { return value * 2; }");
        let entry = parse("import { double } from \"./math.orvi\"; let bad = double(\"two\", 2);");
        let analysis = analyze_program_with_modules(
            &entry.program,
            &HashMap::from([("./math.orvi".into(), module.program)]),
        );
        assert!(analysis.errors.iter().any(|error| {
            error.kind == SemanticErrorKind::ArgumentCount
                && error.message.contains("expects 1 arguments")
        }));
        assert!(analysis.errors.iter().any(|error| {
            error.kind == SemanticErrorKind::TypeMismatch
                && error.message.contains("expected int, received string")
        }));
    }

    #[test]
    fn checks_void_and_explicit_exports() {
        assert!(
            errors(
                "export fn finish() -> void { return } fn work() -> Result<void, string> { return Ok() }",
            )
            .is_empty()
        );
        let bad_return = errors("fn bad() -> void { return null }");
        assert_eq!(bad_return.len(), 1);
        assert_eq!(bad_return[0].kind, SemanticErrorKind::TypeMismatch);

        let nested_export = errors("fn outer() -> void { export fn inner() -> void {} }");
        assert_eq!(nested_export.len(), 1);
        assert!(nested_export[0].message.contains("module top level"));
    }

    #[test]
    fn checks_task_results_and_dependencies() {
        let source = r#"
            task build -> Result<string, string> { return Ok("dist") }
            task deploy -> Result<string, string> {
                needs: [build];
                return Ok(task.result("build")?);
            }
        "#;
        assert!(errors(source).is_empty());

        let errors =
            errors("task bad -> int { needs: [missing]; task.result(\"other\"); return 1 }");
        assert_eq!(errors.len(), 3);
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("must be Result"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("undefined task"))
        );
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("not a declared dependency"))
        );
    }
}
