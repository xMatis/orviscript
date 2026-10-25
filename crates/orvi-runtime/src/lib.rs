use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::{Rc, Weak};

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, Literal, Parameter, PatternKind, Program, Span,
    Statement, StatementKind, StructField, TypeAnnotation, UnaryOperator,
};

type MapEntries = Vec<(String, Value)>;

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Rc<RefCell<Vec<Value>>>),
    Map(Rc<RefCell<MapEntries>>),
    StructType(Rc<StructDefinition>),
    InterfaceType(Rc<InterfaceDefinition>),
    StructInstance(Rc<RefCell<StructInstance>>),
    Variant {
        tag: VariantTag,
        payload: Option<Box<Value>>,
    },
    NativeConstructor(VariantTag),
    NativeModule(NativeModule),
    NativeFunction(NativeFunction),
    Function(Rc<UserFunction>),
    NativePrint,
}

impl fmt::Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self}")
    }
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&format_value(self, &mut HashSet::new()))
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeError {
    pub message: String,
    pub span: Span,
    propagated: Option<Value>,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RuntimeError {}

#[derive(Clone, Debug)]
pub struct Execution {
    pub output: Vec<String>,
    pub value: Value,
    pub collected_objects: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
}

#[derive(Clone, Debug)]
pub struct TaskReport {
    pub name: String,
    pub status: TaskStatus,
    pub result: Option<Value>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TaskExecution {
    pub output: Vec<String>,
    pub tasks: Vec<TaskReport>,
}

#[derive(Clone)]
struct TaskDefinition {
    name: String,
    needs: Vec<String>,
    return_type: TypeAnnotation,
    body: Vec<Statement>,
    span: Span,
}

#[derive(Clone)]
pub struct UserFunction {
    name: String,
    parameters: Vec<Parameter>,
    return_type: Option<TypeAnnotation>,
    body: Vec<Statement>,
    closure: Environment,
}

#[derive(Clone)]
pub struct StructDefinition {
    name: String,
    fields: Vec<StructField>,
    interfaces: RefCell<HashSet<String>>,
    closure: Environment,
}

#[derive(Clone)]
pub struct InterfaceDefinition {
    name: String,
    fields: Vec<StructField>,
}

pub struct StructInstance {
    definition: Rc<StructDefinition>,
    fields: HashMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VariantTag {
    Some,
    None,
    Ok,
    Err,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeModule {
    Json,
    Fs,
    Path,
    Env,
    Process,
    Terminal,
    Task,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFunction {
    JsonParse,
    JsonStringify,
    FsReadText,
    FsWriteText,
    FsExists,
    FsCreateDirAll,
    FsRemoveFile,
    PathJoin,
    PathParent,
    PathFileName,
    PathIsAbsolute,
    EnvGet,
    EnvCurrentDir,
    ProcessRun,
    TerminalInput,
    TerminalConfirm,
    TerminalPassword,
    TaskResult,
}

#[derive(Clone, Debug)]
pub struct ExecutionContext {
    pub cwd: PathBuf,
    pub script_dir: PathBuf,
    pub project_dir: PathBuf,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            cwd: cwd.clone(),
            script_dir: cwd.clone(),
            project_dir: cwd,
        }
    }
}

#[derive(Clone)]
struct Environment(Rc<RefCell<EnvironmentData>>);

struct EnvironmentData {
    values: HashMap<String, Binding>,
    parent: Option<Environment>,
}
#[derive(Clone)]
struct Binding {
    value: Value,
    mutable: bool,
    type_annotation: Option<TypeAnnotation>,
}

impl Environment {
    fn root() -> Self {
        Self(Rc::new(RefCell::new(EnvironmentData {
            values: HashMap::new(),
            parent: None,
        })))
    }
    fn child(parent: Self) -> Self {
        Self(Rc::new(RefCell::new(EnvironmentData {
            values: HashMap::new(),
            parent: Some(parent),
        })))
    }
    fn define(
        &self,
        name: String,
        value: Value,
        mutable: bool,
        type_annotation: Option<TypeAnnotation>,
    ) {
        self.0.borrow_mut().values.insert(
            name,
            Binding {
                value,
                mutable,
                type_annotation,
            },
        );
    }
    fn get(&self, name: &str) -> Option<Value> {
        let data = self.0.borrow();
        data.values
            .get(name)
            .map(|binding| binding.value.clone())
            .or_else(|| data.parent.as_ref().and_then(|parent| parent.get(name)))
    }
    fn get_local_binding(&self, name: &str) -> Option<Binding> {
        self.0.borrow().values.get(name).cloned()
    }
    fn assign(&self, name: &str, value: Value) -> Result<(), AssignError> {
        let parent = {
            let mut data = self.0.borrow_mut();
            if let Some(binding) = data.values.get_mut(name) {
                if !binding.mutable {
                    return Err(AssignError::Immutable);
                }
                if let Some(type_annotation) = &binding.type_annotation
                    && !value_matches_type(&value, type_annotation)
                {
                    return Err(AssignError::TypeMismatch {
                        expected: display_type(type_annotation),
                        actual: value.type_name(),
                    });
                }
                binding.value = value;
                return Ok(());
            }
            data.parent.clone()
        };
        parent.map_or(Err(AssignError::Undefined), |parent| {
            parent.assign(name, value)
        })
    }

    fn interface_definitions(&self) -> Vec<Rc<InterfaceDefinition>> {
        let (mut interfaces, parent) = {
            let data = self.0.borrow();
            let interfaces = data
                .values
                .values()
                .filter_map(|binding| match &binding.value {
                    Value::InterfaceType(definition) => Some(definition.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (interfaces, data.parent.clone())
        };
        if let Some(parent) = parent {
            interfaces.extend(parent.interface_definitions());
        }
        interfaces
    }

    fn register_interface(&self, interface: &InterfaceDefinition) {
        let parent = {
            let data = self.0.borrow();
            for binding in data.values.values() {
                if let Value::StructType(structure) = &binding.value
                    && struct_satisfies_interface(&structure.fields, &interface.fields)
                {
                    structure
                        .interfaces
                        .borrow_mut()
                        .insert(interface.name.clone());
                }
            }
            data.parent.clone()
        };
        if let Some(parent) = parent {
            parent.register_interface(interface);
        }
    }
}

enum AssignError {
    Undefined,
    Immutable,
    TypeMismatch { expected: String, actual: String },
}
enum Control {
    Continue(Value),
    Return(Value),
}

pub fn execute(program: &Program) -> Result<Execution, RuntimeError> {
    execute_internal(program, ExecutionContext::default(), None)
}

pub fn execute_with_context(
    program: &Program,
    context: ExecutionContext,
) -> Result<Execution, RuntimeError> {
    execute_internal(program, context, None)
}

pub fn execute_file(
    program: &Program,
    context: ExecutionContext,
    entry_file: PathBuf,
) -> Result<Execution, RuntimeError> {
    execute_internal(program, context, Some(entry_file))
}

fn execute_internal(
    program: &Program,
    context: ExecutionContext,
    entry_file: Option<PathBuf>,
) -> Result<Execution, RuntimeError> {
    let mut interpreter = Interpreter::new(context, entry_file);
    let result = interpreter.execute_statements(&program.statements);
    let cleanup = interpreter.run_deferred();
    let control = match result {
        Ok(control) => {
            cleanup?;
            control
        }
        Err(error) => return Err(error),
    };
    let value = match control {
        Control::Continue(value) => value,
        Control::Return(_) => {
            return Err(RuntimeError {
                message: "return is only valid inside a function".into(),
                span: Span::empty(0),
                propagated: None,
            });
        }
    };
    interpreter.collect_finished(std::slice::from_ref(&value));
    Ok(Execution {
        output: interpreter.output,
        value,
        collected_objects: interpreter.collected_objects,
    })
}

pub fn execute_tasks(
    program: &Program,
    context: ExecutionContext,
    entry_file: PathBuf,
    selected: &[String],
) -> Result<TaskExecution, RuntimeError> {
    let mut interpreter = Interpreter::new(context, Some(entry_file));
    let control = interpreter.execute_statements(&program.statements)?;
    if matches!(control, Control::Return(_)) {
        return Err(interpreter.error("return is only valid inside a function", Span::empty(0)));
    }
    let tasks = collect_tasks(program)?;
    let reports = interpreter.run_tasks(&tasks, selected)?;
    interpreter.run_deferred()?;
    Ok(TaskExecution {
        output: interpreter.output,
        tasks: reports,
    })
}

struct Interpreter {
    environment: Environment,
    globals: Environment,
    output: Vec<String>,
    function_depth: usize,
    environments: Vec<Weak<RefCell<EnvironmentData>>>,
    lists: Vec<Weak<RefCell<Vec<Value>>>>,
    maps: Vec<Weak<RefCell<MapEntries>>>,
    objects: Vec<Weak<RefCell<StructInstance>>>,
    collected_objects: usize,
    deferred: Vec<Vec<Expression>>,
    context: ExecutionContext,
    current_files: Vec<PathBuf>,
    module_cache: HashMap<PathBuf, Environment>,
    modules_in_progress: HashSet<PathBuf>,
    task_results: HashMap<String, Value>,
    active_task_needs: Option<HashSet<String>>,
}

fn collect_tasks(program: &Program) -> Result<Vec<TaskDefinition>, RuntimeError> {
    let mut tasks = Vec::new();
    let mut names = HashSet::new();
    for statement in &program.statements {
        let StatementKind::Task {
            name,
            needs,
            return_type,
            body,
        } = &statement.kind
        else {
            continue;
        };
        if !names.insert(name.name.clone()) {
            return Err(RuntimeError {
                message: format!("task '{}' is already declared", name.name),
                span: name.span,
                propagated: None,
            });
        }
        tasks.push(TaskDefinition {
            name: name.name.clone(),
            needs: needs.iter().map(|need| need.name.clone()).collect(),
            return_type: return_type.clone(),
            body: body.clone(),
            span: statement.span,
        });
    }
    Ok(tasks)
}

fn validate_task_cycles(
    tasks: &[TaskDefinition],
    definitions: &HashMap<String, TaskDefinition>,
) -> Result<(), RuntimeError> {
    fn visit(
        name: &str,
        definitions: &HashMap<String, TaskDefinition>,
        states: &mut HashMap<String, u8>,
    ) -> Result<(), RuntimeError> {
        match states.get(name) {
            Some(1) => {
                let task = definitions.get(name).expect("task validated");
                return Err(RuntimeError {
                    message: format!("cyclic task dependency involving '{name}'"),
                    span: task.span,
                    propagated: None,
                });
            }
            Some(2) => return Ok(()),
            _ => {}
        }
        states.insert(name.to_owned(), 1);
        let task = definitions.get(name).expect("task validated");
        for dependency in &task.needs {
            visit(dependency, definitions, states)?;
        }
        states.insert(name.to_owned(), 2);
        Ok(())
    }

    let mut states = HashMap::new();
    for task in tasks {
        visit(&task.name, definitions, &mut states)?;
    }
    Ok(())
}

impl Interpreter {
    fn new(context: ExecutionContext, entry_file: Option<PathBuf>) -> Self {
        let globals = Environment::root();
        globals.define("print".into(), Value::NativePrint, false, None);
        globals.define(
            "Some".into(),
            Value::NativeConstructor(VariantTag::Some),
            false,
            None,
        );
        globals.define(
            "None".into(),
            Value::Variant {
                tag: VariantTag::None,
                payload: None,
            },
            false,
            None,
        );
        globals.define(
            "Ok".into(),
            Value::NativeConstructor(VariantTag::Ok),
            false,
            None,
        );
        globals.define(
            "Err".into(),
            Value::NativeConstructor(VariantTag::Err),
            false,
            None,
        );
        for (name, module) in [
            ("json", NativeModule::Json),
            ("fs", NativeModule::Fs),
            ("path", NativeModule::Path),
            ("env", NativeModule::Env),
            ("process", NativeModule::Process),
            ("terminal", NativeModule::Terminal),
            ("task", NativeModule::Task),
        ] {
            globals.define(name.into(), Value::NativeModule(module), false, None);
        }
        let context_value = Value::Map(Rc::new(RefCell::new(vec![
            (
                "cwd".into(),
                Value::String(context.cwd.to_string_lossy().into_owned()),
            ),
            (
                "script_dir".into(),
                Value::String(context.script_dir.to_string_lossy().into_owned()),
            ),
            (
                "project_dir".into(),
                Value::String(context.project_dir.to_string_lossy().into_owned()),
            ),
        ])));
        globals.define("context".into(), context_value, false, None);
        Self {
            environment: globals.clone(),
            globals: globals.clone(),
            output: Vec::new(),
            function_depth: 0,
            environments: vec![Rc::downgrade(&globals.0)],
            lists: Vec::new(),
            maps: Vec::new(),
            objects: Vec::new(),
            collected_objects: 0,
            deferred: vec![Vec::new()],
            context,
            current_files: entry_file.into_iter().collect(),
            module_cache: HashMap::new(),
            modules_in_progress: HashSet::new(),
            task_results: HashMap::new(),
            active_task_needs: None,
        }
    }
    fn execute_statements(&mut self, statements: &[Statement]) -> Result<Control, RuntimeError> {
        let mut value = Value::Null;
        for statement in statements {
            match self.execute_statement(statement)? {
                Control::Continue(next) => {
                    self.collect_garbage(std::slice::from_ref(&next));
                    value = next;
                }
                Control::Return(value) => {
                    self.collect_garbage(std::slice::from_ref(&value));
                    return Ok(Control::Return(value));
                }
            }
        }
        Ok(Control::Continue(value))
    }

    fn run_tasks(
        &mut self,
        tasks: &[TaskDefinition],
        selected: &[String],
    ) -> Result<Vec<TaskReport>, RuntimeError> {
        let definitions = tasks
            .iter()
            .map(|task| (task.name.clone(), task.clone()))
            .collect::<HashMap<_, _>>();
        for task in tasks {
            for dependency in &task.needs {
                if !definitions.contains_key(dependency) {
                    return Err(self.error(
                        format!(
                            "task '{}' requires undefined task '{dependency}'",
                            task.name
                        ),
                        task.span,
                    ));
                }
            }
        }
        validate_task_cycles(tasks, &definitions)?;
        let targets = if selected.is_empty() {
            tasks
                .iter()
                .map(|task| task.name.clone())
                .collect::<Vec<_>>()
        } else {
            selected.to_vec()
        };
        let mut reports = tasks
            .iter()
            .map(|task| {
                (
                    task.name.clone(),
                    TaskReport {
                        name: task.name.clone(),
                        status: TaskStatus::Pending,
                        result: None,
                        error: None,
                    },
                )
            })
            .collect::<HashMap<_, _>>();
        for target in targets {
            if !definitions.contains_key(&target) {
                return Err(self.error(format!("unknown task '{target}'"), Span::empty(0)));
            }
            self.run_task(&target, &definitions, &mut reports);
        }
        Ok(tasks
            .iter()
            .map(|task| reports.remove(&task.name).expect("all tasks have reports"))
            .collect())
    }

    fn run_task(
        &mut self,
        name: &str,
        definitions: &HashMap<String, TaskDefinition>,
        reports: &mut HashMap<String, TaskReport>,
    ) {
        if reports
            .get(name)
            .is_some_and(|report| report.status != TaskStatus::Pending)
        {
            return;
        }
        let definition = definitions.get(name).expect("task validated").clone();
        for dependency in &definition.needs {
            self.run_task(dependency, definitions, reports);
            if reports
                .get(dependency)
                .is_some_and(|report| report.status != TaskStatus::Succeeded)
            {
                let report = reports.get_mut(name).expect("task report exists");
                report.status = TaskStatus::Skipped;
                report.error = Some(format!("dependency '{dependency}' did not succeed"));
                return;
            }
        }
        reports.get_mut(name).expect("task report exists").status = TaskStatus::Running;
        match self.execute_task(&definition) {
            Ok(value) => {
                let succeeded = matches!(
                    value,
                    Value::Variant {
                        tag: VariantTag::Ok,
                        ..
                    }
                );
                self.task_results.insert(name.to_owned(), value.clone());
                let report = reports.get_mut(name).expect("task report exists");
                report.status = if succeeded {
                    TaskStatus::Succeeded
                } else {
                    TaskStatus::Failed
                };
                if !succeeded {
                    report.error = Some(format!("task returned {value}"));
                }
                report.result = Some(value);
            }
            Err(error) => {
                let report = reports.get_mut(name).expect("task report exists");
                report.status = TaskStatus::Failed;
                report.error = Some(error.message);
            }
        }
    }

    fn execute_task(&mut self, task: &TaskDefinition) -> Result<Value, RuntimeError> {
        let previous_needs = self
            .active_task_needs
            .replace(task.needs.iter().cloned().collect());
        self.function_depth += 1;
        let environment = self.child_environment(self.globals.clone());
        let result = self.execute_block(&task.body, environment);
        self.function_depth -= 1;
        self.active_task_needs = previous_needs;
        let value = match result {
            Ok(Control::Continue(_)) => Value::Null,
            Ok(Control::Return(value)) => value,
            Err(mut error) => match error.propagated.take() {
                Some(value) => value,
                None => return Err(error),
            },
        };
        self.require_type(&value, Some(&task.return_type), task.span)?;
        if !matches!(
            value,
            Value::Variant {
                tag: VariantTag::Ok | VariantTag::Err,
                ..
            }
        ) {
            return Err(self.error("task must return Result<T, E>", task.span));
        }
        Ok(value)
    }

    fn execute_statement(&mut self, statement: &Statement) -> Result<Control, RuntimeError> {
        match &statement.kind {
            StatementKind::Variable {
                name,
                mutable,
                type_annotation,
                initializer,
            } => {
                let value = self.evaluate(initializer)?;
                self.require_type(&value, type_annotation.as_ref(), initializer.span)?;
                self.environment.define(
                    name.name.clone(),
                    value.clone(),
                    *mutable,
                    type_annotation.clone(),
                );
                Ok(Control::Continue(value))
            }
            StatementKind::Function {
                name,
                parameters,
                return_type,
                body,
            } => {
                let function = UserFunction {
                    name: name.name.clone(),
                    parameters: parameters.clone(),
                    return_type: return_type.clone(),
                    body: body.clone(),
                    closure: self.environment.clone(),
                };
                self.environment.define(
                    name.name.clone(),
                    Value::Function(Rc::new(function)),
                    false,
                    None,
                );
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Struct { name, fields } => {
                let interfaces = self
                    .environment
                    .interface_definitions()
                    .into_iter()
                    .filter(|interface| struct_satisfies_interface(fields, &interface.fields))
                    .map(|interface| interface.name.clone())
                    .collect();
                let definition = StructDefinition {
                    name: name.name.clone(),
                    fields: fields.clone(),
                    interfaces: RefCell::new(interfaces),
                    closure: self.environment.clone(),
                };
                self.environment.define(
                    name.name.clone(),
                    Value::StructType(Rc::new(definition)),
                    false,
                    None,
                );
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Interface { name, fields } => {
                let definition = Rc::new(InterfaceDefinition {
                    name: name.name.clone(),
                    fields: fields.clone(),
                });
                self.environment.register_interface(&definition);
                self.environment.define(
                    name.name.clone(),
                    Value::InterfaceType(definition),
                    false,
                    None,
                );
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Import { names, source } => {
                if let Some(module) = source.strip_prefix("std:") {
                    let module = native_module(module).ok_or_else(|| {
                        self.error(
                            format!("unknown standard module '{source}'"),
                            statement.span,
                        )
                    })?;
                    for name in names {
                        let function = module.member(name.as_str()).ok_or_else(|| {
                            self.error(
                                format!("module '{source}' has no export '{}'", name.name),
                                name.span,
                            )
                        })?;
                        self.environment.define(
                            name.name.clone(),
                            Value::NativeFunction(function),
                            false,
                            None,
                        );
                    }
                    return Ok(Control::Continue(Value::Null));
                }

                let path = self.resolve_module_path(source, statement.span)?;
                let module = self.load_module(&path, statement.span)?;
                for name in names {
                    let binding = module.get_local_binding(name.as_str()).ok_or_else(|| {
                        self.error(
                            format!("module '{}' has no export '{}'", path.display(), name.name),
                            name.span,
                        )
                    })?;
                    self.environment.define(
                        name.name.clone(),
                        binding.value,
                        false,
                        binding.type_annotation,
                    );
                }
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Task { .. } => Ok(Control::Continue(Value::Null)),
            StatementKind::Block(statements) => {
                let environment = self.child_environment(self.environment.clone());
                self.execute_block(statements, environment)
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let condition_value = self.evaluate(condition)?;
                if self.boolean(condition_value, condition.span)? {
                    self.execute_statement(then_branch)
                } else if let Some(branch) = else_branch {
                    self.execute_statement(branch)
                } else {
                    Ok(Control::Continue(Value::Null))
                }
            }
            StatementKind::While { condition, body } => {
                loop {
                    let condition_value = self.evaluate(condition)?;
                    if !self.boolean(condition_value, condition.span)? {
                        break;
                    }
                    if let returned @ Control::Return(_) = self.execute_statement(body)? {
                        return Ok(returned);
                    }
                }
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Return(expression) => {
                if self.function_depth == 0 {
                    return Err(
                        self.error("return is only valid inside a function", statement.span)
                    );
                }
                let value = expression
                    .as_ref()
                    .map_or(Ok(Value::Null), |expression| self.evaluate(expression))?;
                Ok(Control::Return(value))
            }
            StatementKind::Defer(expression) => {
                self.deferred
                    .last_mut()
                    .expect("interpreter always has a cleanup scope")
                    .push(expression.clone());
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Using {
                name,
                initializer,
                cleanup,
            } => {
                let value = self.evaluate(initializer)?;
                self.environment
                    .define(name.name.clone(), value.clone(), false, None);
                self.deferred
                    .last_mut()
                    .expect("interpreter always has a cleanup scope")
                    .push(cleanup.clone());
                Ok(Control::Continue(value))
            }
            StatementKind::Expression(expression) => {
                Ok(Control::Continue(self.evaluate(expression)?))
            }
        }
    }

    fn execute_block(
        &mut self,
        statements: &[Statement],
        environment: Environment,
    ) -> Result<Control, RuntimeError> {
        let previous = std::mem::replace(&mut self.environment, environment);
        self.deferred.push(Vec::new());
        let result = self.execute_statements(statements);
        let cleanup = self.run_deferred();
        self.deferred.pop();
        self.environment = previous;
        match result {
            Ok(control) => cleanup.map(|()| control),
            Err(error) => Err(error),
        }
    }

    fn resolve_module_path(&self, source: &str, span: Span) -> Result<PathBuf, RuntimeError> {
        let source_path = PathBuf::from(source);
        let path = if source_path.is_absolute() {
            source_path
        } else {
            let Some(current_file) = self.current_files.last() else {
                return Err(self.error("local imports require a source file path", span));
            };
            current_file
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(source_path)
        };
        if path.extension().and_then(|extension| extension.to_str()) != Some("orvi") {
            return Err(self.error("local module path must end with '.orvi'", span));
        }
        path.canonicalize().map_err(|error| {
            self.error(
                format!("could not resolve module '{}': {error}", path.display()),
                span,
            )
        })
    }

    fn load_module(&mut self, path: &Path, span: Span) -> Result<Environment, RuntimeError> {
        if let Some(environment) = self.module_cache.get(path) {
            return Ok(environment.clone());
        }
        if !self.modules_in_progress.insert(path.to_path_buf()) {
            return Err(self.error(
                format!("cyclic module import involving '{}'", path.display()),
                span,
            ));
        }

        let result = (|| {
            let source = std::fs::read_to_string(path).map_err(|error| {
                self.error(
                    format!("could not read module '{}': {error}", path.display()),
                    span,
                )
            })?;
            let parsed = orvi_syntax::parse(&source);
            if let Some(error) = parsed.lexical_errors.first() {
                return Err(self.error(format!("module '{}': {error}", path.display()), span));
            }
            if let Some(error) = parsed.parse_errors.first() {
                return Err(self.error(
                    format!("module '{}': {}", path.display(), error.message),
                    span,
                ));
            }

            let environment = self.child_environment(self.globals.clone());
            let previous = std::mem::replace(&mut self.environment, environment.clone());
            self.current_files.push(path.to_path_buf());
            self.deferred.push(Vec::new());
            let execution = self.execute_statements(&parsed.program.statements);
            let cleanup = self.run_deferred();
            self.deferred.pop();
            self.current_files.pop();
            self.environment = previous;
            let control = execution.and_then(|control| cleanup.map(|()| control))?;
            if matches!(control, Control::Return(_)) {
                return Err(self.error("return is not valid at module top level", span));
            }
            self.module_cache
                .insert(path.to_path_buf(), environment.clone());
            Ok(environment)
        })();

        self.modules_in_progress.remove(path);
        result
    }

    fn run_deferred(&mut self) -> Result<(), RuntimeError> {
        let deferred = std::mem::take(
            self.deferred
                .last_mut()
                .expect("interpreter always has a cleanup scope"),
        );
        let mut first_error = None;
        for expression in deferred.into_iter().rev() {
            if let Err(error) = self.evaluate(&expression)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn evaluate(&mut self, expression: &Expression) -> Result<Value, RuntimeError> {
        match &expression.kind {
            ExpressionKind::Literal(literal) => Ok(match literal {
                Literal::Null => Value::Null,
                Literal::Bool(value) => Value::Bool(*value),
                Literal::Integer(value) => Value::Integer(*value),
                Literal::Float(value) => Value::Float(*value),
                Literal::String(value) => Value::String(value.clone()),
            }),
            ExpressionKind::List(values) => {
                let values = values
                    .iter()
                    .map(|value| self.evaluate(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = Rc::new(RefCell::new(values));
                self.lists.push(Rc::downgrade(&values));
                Ok(Value::List(values))
            }
            ExpressionKind::Map(entries) => {
                let mut values = Vec::with_capacity(entries.len());
                for (key, value) in entries {
                    values.push((key.clone(), self.evaluate(value)?));
                }
                let values = Rc::new(RefCell::new(values));
                self.maps.push(Rc::downgrade(&values));
                Ok(Value::Map(values))
            }
            ExpressionKind::Variable(name) => {
                self.environment.get(name.as_str()).ok_or_else(|| {
                    self.error(
                        format!("undefined variable '{}'", name.name),
                        expression.span,
                    )
                })
            }
            ExpressionKind::Assign { name, value } => {
                let value = self.evaluate(value)?;
                self.environment
                    .assign(name.as_str(), value.clone())
                    .map_err(|error| match error {
                        AssignError::Undefined => self.error(
                            format!("undefined variable '{}'", name.name),
                            expression.span,
                        ),
                        AssignError::Immutable => self.error(
                            format!("cannot assign to constant '{}'", name.name),
                            expression.span,
                        ),
                        AssignError::TypeMismatch { expected, actual } => self.error(
                            format!("expected {expected}, received {actual}"),
                            expression.span,
                        ),
                    })?;
                Ok(value)
            }
            ExpressionKind::IndexAssign {
                object,
                index,
                value,
            } => {
                let object = self.evaluate(object)?;
                let value = self.evaluate(value)?;
                match object {
                    Value::List(values) => {
                        let index = self.list_index(index)?;
                        let mut values = values.borrow_mut();
                        let Some(slot) = values.get_mut(index) else {
                            return Err(self.error("list index is out of bounds", expression.span));
                        };
                        *slot = value.clone();
                        Ok(value)
                    }
                    Value::Map(values) => {
                        let key = self.map_key(index)?;
                        let mut values = values.borrow_mut();
                        if let Some((_, slot)) =
                            values.iter_mut().find(|(candidate, _)| candidate == &key)
                        {
                            *slot = value.clone();
                        } else {
                            values.push((key, value.clone()));
                        }
                        Ok(value)
                    }
                    _ => Err(self.error("value is not indexable", expression.span)),
                }
            }
            ExpressionKind::MemberAssign {
                object,
                field,
                value,
            } => {
                let object = self.evaluate(object)?;
                let value = self.evaluate(value)?;
                if let Value::Map(values) = object {
                    let mut values = values.borrow_mut();
                    if let Some((_, slot)) = values
                        .iter_mut()
                        .find(|(candidate, _)| candidate == field.as_str())
                    {
                        *slot = value.clone();
                    } else {
                        values.push((field.name.clone(), value.clone()));
                    }
                    return Ok(value);
                }
                let Value::StructInstance(instance) = object else {
                    return Err(self.error("value has no fields", expression.span));
                };
                let mut instance = instance.borrow_mut();
                let Some(field_definition) = instance
                    .definition
                    .fields
                    .iter()
                    .find(|candidate| candidate.name.name == field.name)
                else {
                    return Err(self.error(
                        format!(
                            "struct '{}' has no field '{}'",
                            instance.definition.name, field.name
                        ),
                        field.span,
                    ));
                };
                self.require_type(
                    &value,
                    Some(&field_definition.type_annotation),
                    expression.span,
                )?;
                instance.fields.insert(field.name.clone(), value.clone());
                Ok(value)
            }
            ExpressionKind::Unary { operator, operand } => {
                let value = self.evaluate(operand)?;
                match (operator, value) {
                    (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                    (UnaryOperator::Negate, Value::Integer(value)) => value
                        .checked_neg()
                        .map(Value::Integer)
                        .ok_or_else(|| self.error("integer overflow", expression.span)),
                    (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                    (UnaryOperator::Not, _) => {
                        Err(self.error("operator '!' requires bool", expression.span))
                    }
                    (UnaryOperator::Negate, _) => {
                        Err(self.error("unary '-' requires a number", expression.span))
                    }
                }
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => self.evaluate_binary(left, *operator, right, expression.span),
            ExpressionKind::Call { callee, arguments } => {
                let callee = self.evaluate(callee)?;
                let arguments = arguments
                    .iter()
                    .map(|argument| self.evaluate(argument))
                    .collect::<Result<Vec<_>, _>>()?;
                self.call(callee, arguments, expression.span)
            }
            ExpressionKind::Index { object, index } => {
                let object = self.evaluate(object)?;
                match object {
                    Value::List(values) => {
                        let index = self.list_index(index)?;
                        values.borrow().get(index).cloned().ok_or_else(|| {
                            self.error("list index is out of bounds", expression.span)
                        })
                    }
                    Value::Map(values) => {
                        let key = self.map_key(index)?;
                        values
                            .borrow()
                            .iter()
                            .find(|(candidate, _)| candidate == &key)
                            .map(|(_, value)| value.clone())
                            .ok_or_else(|| {
                                self.error(format!("map has no key '{key}'"), expression.span)
                            })
                    }
                    _ => Err(self.error("value is not indexable", expression.span)),
                }
            }
            ExpressionKind::Member { object, field } => {
                let object = self.evaluate(object)?;
                if let Value::NativeModule(module) = object {
                    return module
                        .member(field.as_str())
                        .map(Value::NativeFunction)
                        .ok_or_else(|| {
                            self.error(
                                format!(
                                    "module '{}' has no member '{}'",
                                    module.name(),
                                    field.name
                                ),
                                field.span,
                            )
                        });
                }
                if let Value::Map(values) = object {
                    return values
                        .borrow()
                        .iter()
                        .find(|(candidate, _)| candidate == field.as_str())
                        .map(|(_, value)| value.clone())
                        .ok_or_else(|| {
                            self.error(format!("map has no key '{}'", field.name), expression.span)
                        });
                }
                let Value::StructInstance(instance) = object else {
                    return Err(self.error("value has no fields", expression.span));
                };
                let instance = instance.borrow();
                instance.fields.get(field.as_str()).cloned().ok_or_else(|| {
                    self.error(
                        format!(
                            "struct '{}' has no field '{}'",
                            instance.definition.name, field.name
                        ),
                        field.span,
                    )
                })
            }
            ExpressionKind::Match { value, arms } => {
                let value = self.evaluate(value)?;
                for arm in arms {
                    let Some(binding_value) = pattern_binding(&arm.pattern.kind, &value) else {
                        continue;
                    };
                    let environment = self.child_environment(self.environment.clone());
                    if let PatternKind::Variant {
                        binding: Some(binding),
                        ..
                    } = &arm.pattern.kind
                    {
                        let Some(binding_value) = binding_value else {
                            continue;
                        };
                        environment.define(binding.name.clone(), binding_value, false, None);
                    }
                    let previous = std::mem::replace(&mut self.environment, environment);
                    let result = self.evaluate(&arm.value);
                    self.environment = previous;
                    return result;
                }
                Err(self.error("match expression has no matching arm", expression.span))
            }
            ExpressionKind::Try(value) => match self.evaluate(value)? {
                Value::Variant {
                    tag: VariantTag::Some | VariantTag::Ok,
                    payload: Some(value),
                } => Ok(*value),
                value @ Value::Variant {
                    tag: VariantTag::None | VariantTag::Err,
                    ..
                } => Err(RuntimeError {
                    message: "value propagated by '?'".into(),
                    span: expression.span,
                    propagated: Some(value),
                }),
                _ => Err(self.error("operator '?' requires Option or Result", expression.span)),
            },
        }
    }

    fn evaluate_binary(
        &mut self,
        left: &Expression,
        operator: BinaryOperator,
        right: &Expression,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        let left_value = self.evaluate(left)?;
        if operator == BinaryOperator::And {
            return if self.boolean(left_value, left.span)? {
                let right_value = self.evaluate(right)?;
                Ok(Value::Bool(self.boolean(right_value, right.span)?))
            } else {
                Ok(Value::Bool(false))
            };
        }
        if operator == BinaryOperator::Or {
            return if self.boolean(left_value, left.span)? {
                Ok(Value::Bool(true))
            } else {
                let right_value = self.evaluate(right)?;
                Ok(Value::Bool(self.boolean(right_value, right.span)?))
            };
        }
        let right_value = self.evaluate(right)?;
        match operator {
            BinaryOperator::Add => self.add(left_value, right_value, span),
            BinaryOperator::Subtract => {
                self.numeric(left_value, right_value, span, i64::checked_sub, |a, b| {
                    a - b
                })
            }
            BinaryOperator::Multiply => {
                self.numeric(left_value, right_value, span, i64::checked_mul, |a, b| {
                    a * b
                })
            }
            BinaryOperator::Divide => self.divide(left_value, right_value, span),
            BinaryOperator::Remainder => self.remainder(left_value, right_value, span),
            BinaryOperator::Equal => Ok(Value::Bool(values_equal(&left_value, &right_value))),
            BinaryOperator::NotEqual => Ok(Value::Bool(!values_equal(&left_value, &right_value))),
            BinaryOperator::Less => self.compare(left_value, right_value, span, |a, b| a < b),
            BinaryOperator::LessEqual => self.compare(left_value, right_value, span, |a, b| a <= b),
            BinaryOperator::Greater => self.compare(left_value, right_value, span, |a, b| a > b),
            BinaryOperator::GreaterEqual => {
                self.compare(left_value, right_value, span, |a, b| a >= b)
            }
            BinaryOperator::And | BinaryOperator::Or => {
                Err(self.error("invalid logical operator state", span))
            }
        }
    }

    fn call(
        &mut self,
        callee: Value,
        arguments: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        match callee {
            Value::NativePrint => {
                self.output.push(
                    arguments
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
                Ok(Value::Null)
            }
            Value::NativeConstructor(tag) => {
                if arguments.len() != 1 {
                    return Err(self.error(
                        format!(
                            "constructor '{}' expects 1 argument, received {}",
                            tag.name(),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                Ok(Value::Variant {
                    tag,
                    payload: arguments.into_iter().next().map(Box::new),
                })
            }
            Value::NativeFunction(function) => self.call_native(function, arguments, span),
            Value::Function(function) => {
                if arguments.len() != function.parameters.len() {
                    return Err(self.error(
                        format!(
                            "function '{}' expects {} arguments, received {}",
                            function.name,
                            function.parameters.len(),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let environment = self.child_environment(function.closure.clone());
                for (parameter, value) in function.parameters.iter().zip(arguments) {
                    self.require_type(
                        &value,
                        parameter.type_annotation.as_ref(),
                        parameter.name.span,
                    )?;
                    environment.define(
                        parameter.name.name.clone(),
                        value,
                        true,
                        parameter.type_annotation.clone(),
                    );
                }
                self.function_depth += 1;
                let result = self.execute_block(&function.body, environment);
                self.function_depth -= 1;
                let value = match result {
                    Ok(Control::Continue(_)) => Value::Null,
                    Ok(Control::Return(value)) => value,
                    Err(mut error) => {
                        let Some(value) = error.propagated.take() else {
                            return Err(error);
                        };
                        value
                    }
                };
                self.require_type(&value, function.return_type.as_ref(), span)?;
                Ok(value)
            }
            Value::StructType(definition) => {
                let required = definition
                    .fields
                    .iter()
                    .take_while(|field| field.default.is_none())
                    .count();
                if arguments.len() < required || arguments.len() > definition.fields.len() {
                    return Err(self.error(
                        format!(
                            "struct '{}' expects between {required} and {} arguments, received {}",
                            definition.name,
                            definition.fields.len(),
                            arguments.len()
                        ),
                        span,
                    ));
                }
                let environment = self.child_environment(definition.closure.clone());
                let previous = std::mem::replace(&mut self.environment, environment);
                let values = (|| {
                    let mut values = HashMap::new();
                    for (index, field) in definition.fields.iter().enumerate() {
                        let value = if let Some(value) = arguments.get(index) {
                            value.clone()
                        } else if let Some(default) = &field.default {
                            self.evaluate(default)?
                        } else {
                            return Err(self.error("missing required struct field", field.span));
                        };
                        self.require_type(&value, Some(&field.type_annotation), field.span)?;
                        self.environment.define(
                            field.name.name.clone(),
                            value.clone(),
                            false,
                            Some(field.type_annotation.clone()),
                        );
                        values.insert(field.name.name.clone(), value);
                    }
                    Ok(values)
                })();
                self.environment = previous;
                let values = values?;
                let instance = Rc::new(RefCell::new(StructInstance {
                    definition,
                    fields: values,
                }));
                self.objects.push(Rc::downgrade(&instance));
                Ok(Value::StructInstance(instance))
            }
            _ => Err(self.error("value is not callable", span)),
        }
    }

    fn call_native(
        &mut self,
        function: NativeFunction,
        arguments: Vec<Value>,
        span: Span,
    ) -> Result<Value, RuntimeError> {
        if arguments.len() != function.arity() {
            return Err(self.error(
                format!(
                    "function '{}' expects {} arguments, received {}",
                    function.name(),
                    function.arity(),
                    arguments.len()
                ),
                span,
            ));
        }
        let mut arguments = arguments.into_iter();
        match function {
            NativeFunction::JsonParse => {
                let value = arguments.next().expect("checked argument count");
                let Value::String(source) = value else {
                    return Err(self.error("json.parse expects string", span));
                };
                match serde_json::from_str(&source) {
                    Ok(value) => Ok(Value::Variant {
                        tag: VariantTag::Ok,
                        payload: Some(Box::new(self.json_to_value(value))),
                    }),
                    Err(error) => Ok(Value::Variant {
                        tag: VariantTag::Err,
                        payload: Some(Box::new(Value::String(error.to_string()))),
                    }),
                }
            }
            NativeFunction::JsonStringify => {
                let value = arguments.next().expect("checked argument count");
                match value_to_json(&value, &mut HashSet::new()).and_then(|value| {
                    serde_json::to_string(&value).map_err(|error| error.to_string())
                }) {
                    Ok(json) => Ok(Value::Variant {
                        tag: VariantTag::Ok,
                        payload: Some(Box::new(Value::String(json))),
                    }),
                    Err(error) => Ok(Value::Variant {
                        tag: VariantTag::Err,
                        payload: Some(Box::new(Value::String(error))),
                    }),
                }
            }
            NativeFunction::FsReadText => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(result_value(
                    std::fs::read_to_string(self.resolve_path(path)).map(Value::String),
                ))
            }
            NativeFunction::FsWriteText => {
                let path = self.string_argument(arguments.next(), function, span)?;
                let contents = self.string_argument(arguments.next(), function, span)?;
                Ok(result_value(
                    std::fs::write(self.resolve_path(path), contents).map(|()| Value::Null),
                ))
            }
            NativeFunction::FsExists => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(Value::Bool(self.resolve_path(path).exists()))
            }
            NativeFunction::FsCreateDirAll => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(result_value(
                    std::fs::create_dir_all(self.resolve_path(path)).map(|()| Value::Null),
                ))
            }
            NativeFunction::FsRemoveFile => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(result_value(
                    std::fs::remove_file(self.resolve_path(path)).map(|()| Value::Null),
                ))
            }
            NativeFunction::PathJoin => {
                let parts = self.string_list_argument(arguments.next(), function, span)?;
                let mut path = PathBuf::new();
                for part in parts {
                    path.push(part);
                }
                Ok(Value::String(path.to_string_lossy().into_owned()))
            }
            NativeFunction::PathParent => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(option_value(Path::new(&path).parent().map(|path| {
                    Value::String(path.to_string_lossy().into_owned())
                })))
            }
            NativeFunction::PathFileName => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(option_value(Path::new(&path).file_name().map(|name| {
                    Value::String(name.to_string_lossy().into_owned())
                })))
            }
            NativeFunction::PathIsAbsolute => {
                let path = self.string_argument(arguments.next(), function, span)?;
                Ok(Value::Bool(Path::new(&path).is_absolute()))
            }
            NativeFunction::EnvGet => {
                let name = self.string_argument(arguments.next(), function, span)?;
                Ok(option_value(std::env::var(name).ok().map(Value::String)))
            }
            NativeFunction::EnvCurrentDir => Ok(ok_value(Value::String(
                self.context.cwd.to_string_lossy().into_owned(),
            ))),
            NativeFunction::ProcessRun => {
                let program = self.string_argument(arguments.next(), function, span)?;
                let args = self.string_list_argument(arguments.next(), function, span)?;
                match Command::new(program)
                    .args(args)
                    .current_dir(&self.context.cwd)
                    .output()
                {
                    Ok(output) => {
                        let values = Rc::new(RefCell::new(vec![
                            (
                                "exit_code".into(),
                                output
                                    .status
                                    .code()
                                    .map_or(Value::Null, |code| Value::Integer(i64::from(code))),
                            ),
                            ("success".into(), Value::Bool(output.status.success())),
                            (
                                "stdout".into(),
                                Value::String(String::from_utf8_lossy(&output.stdout).into_owned()),
                            ),
                            (
                                "stderr".into(),
                                Value::String(String::from_utf8_lossy(&output.stderr).into_owned()),
                            ),
                        ]));
                        self.maps.push(Rc::downgrade(&values));
                        Ok(ok_value(Value::Map(values)))
                    }
                    Err(error) => Ok(err_value(error.to_string())),
                }
            }
            NativeFunction::TerminalInput | NativeFunction::TerminalConfirm => {
                let prompt = self.string_argument(arguments.next(), function, span)?;
                if !io::stdin().is_terminal() {
                    return Ok(err_value("terminal input is not interactive"));
                }
                print!("{prompt}");
                if let Err(error) = io::stdout().flush() {
                    return Ok(err_value(error.to_string()));
                }
                let mut input = String::new();
                if let Err(error) = io::stdin().read_line(&mut input) {
                    return Ok(err_value(error.to_string()));
                }
                let input = input.trim_end_matches(['\r', '\n']);
                if function == NativeFunction::TerminalInput {
                    Ok(ok_value(Value::String(input.into())))
                } else {
                    match input.to_ascii_lowercase().as_str() {
                        "y" | "yes" => Ok(ok_value(Value::Bool(true))),
                        "n" | "no" => Ok(ok_value(Value::Bool(false))),
                        _ => Ok(err_value("expected yes or no")),
                    }
                }
            }
            NativeFunction::TerminalPassword => {
                let prompt = self.string_argument(arguments.next(), function, span)?;
                if !io::stdin().is_terminal() {
                    return Ok(err_value("terminal input is not interactive"));
                }
                match rpassword::prompt_password(prompt) {
                    Ok(password) => Ok(ok_value(Value::String(password))),
                    Err(error) => Ok(err_value(error.to_string())),
                }
            }
            NativeFunction::TaskResult => {
                let name = self.string_argument(arguments.next(), function, span)?;
                let Some(needs) = &self.active_task_needs else {
                    return Err(self.error("task.result is only valid inside a task", span));
                };
                if !needs.contains(&name) {
                    return Err(self.error(
                        format!("task.result('{name}') is not a declared dependency"),
                        span,
                    ));
                }
                self.task_results.get(&name).cloned().ok_or_else(|| {
                    self.error(format!("task dependency '{name}' has no result"), span)
                })
            }
        }
    }

    fn string_argument(
        &self,
        value: Option<Value>,
        function: NativeFunction,
        span: Span,
    ) -> Result<String, RuntimeError> {
        match value.expect("checked argument count") {
            Value::String(value) => Ok(value),
            value => Err(self.error(
                format!(
                    "{} expects string, received {}",
                    function.name(),
                    value.type_name()
                ),
                span,
            )),
        }
    }

    fn string_list_argument(
        &self,
        value: Option<Value>,
        function: NativeFunction,
        span: Span,
    ) -> Result<Vec<String>, RuntimeError> {
        let value = value.expect("checked argument count");
        let Value::List(values) = value else {
            return Err(self.error(format!("{} expects list<string>", function.name()), span));
        };
        values
            .borrow()
            .iter()
            .map(|value| match value {
                Value::String(value) => Ok(value.clone()),
                value => Err(self.error(
                    format!(
                        "{} expects list<string>, received element {}",
                        function.name(),
                        value.type_name()
                    ),
                    span,
                )),
            })
            .collect()
    }

    fn resolve_path(&self, path: String) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            self.context.cwd.join(path)
        }
    }

    fn json_to_value(&mut self, value: serde_json::Value) -> Value {
        match value {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(value) => Value::Bool(value),
            serde_json::Value::Number(value) => value.as_i64().map_or_else(
                || {
                    Value::Float(
                        value
                            .as_f64()
                            .expect("JSON numbers are representable as f64"),
                    )
                },
                Value::Integer,
            ),
            serde_json::Value::String(value) => Value::String(value),
            serde_json::Value::Array(values) => {
                let values = Rc::new(RefCell::new(
                    values
                        .into_iter()
                        .map(|value| self.json_to_value(value))
                        .collect(),
                ));
                self.lists.push(Rc::downgrade(&values));
                Value::List(values)
            }
            serde_json::Value::Object(values) => {
                let values = Rc::new(RefCell::new(
                    values
                        .into_iter()
                        .map(|(key, value)| (key, self.json_to_value(value)))
                        .collect(),
                ));
                self.maps.push(Rc::downgrade(&values));
                Value::Map(values)
            }
        }
    }

    fn add(&self, left: Value, right: Value, span: Span) -> Result<Value, RuntimeError> {
        match (left, right) {
            (Value::String(left), Value::String(right)) => Ok(Value::String(left + &right)),
            (left, right) => self.numeric(left, right, span, i64::checked_add, |a, b| a + b),
        }
    }

    fn numeric(
        &self,
        left: Value,
        right: Value,
        span: Span,
        integer: fn(i64, i64) -> Option<i64>,
        float: fn(f64, f64) -> f64,
    ) -> Result<Value, RuntimeError> {
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => integer(a, b)
                .map(Value::Integer)
                .ok_or_else(|| self.error("integer overflow", span)),
            (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(float(a as f64, b))),
            (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(float(a, b as f64))),
            (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float(a, b))),
            _ => Err(self.error("operator requires numbers", span)),
        }
    }

    fn divide(&self, left: Value, right: Value, span: Span) -> Result<Value, RuntimeError> {
        if matches!(right, Value::Integer(0))
            || matches!(right, Value::Float(value) if value == 0.0)
        {
            return Err(self.error("division by zero", span));
        }
        match (left, right) {
            (Value::Integer(a), Value::Integer(b)) => a
                .checked_div(b)
                .map(Value::Integer)
                .ok_or_else(|| self.error("integer overflow", span)),
            (left, right) => self.numeric(left, right, span, i64::checked_div, |a, b| a / b),
        }
    }

    fn remainder(&self, left: Value, right: Value, span: Span) -> Result<Value, RuntimeError> {
        match (left, right) {
            (_, Value::Integer(0)) => Err(self.error("division by zero", span)),
            (Value::Integer(a), Value::Integer(b)) => a
                .checked_rem(b)
                .map(Value::Integer)
                .ok_or_else(|| self.error("integer overflow", span)),
            _ => Err(self.error("operator '%' requires integers", span)),
        }
    }

    fn compare(
        &self,
        left: Value,
        right: Value,
        span: Span,
        compare: fn(f64, f64) -> bool,
    ) -> Result<Value, RuntimeError> {
        let (left, right) =
            numbers(left, right).ok_or_else(|| self.error("comparison requires numbers", span))?;
        Ok(Value::Bool(compare(left, right)))
    }

    fn boolean(&self, value: Value, span: Span) -> Result<bool, RuntimeError> {
        if let Value::Bool(value) = value {
            Ok(value)
        } else {
            Err(self.error("condition must be bool", span))
        }
    }
    fn child_environment(&mut self, parent: Environment) -> Environment {
        let environment = Environment::child(parent);
        self.environments.push(Rc::downgrade(&environment.0));
        environment
    }

    fn collect_garbage(&mut self, temporary_roots: &[Value]) {
        let mut marked_environments = HashSet::new();
        let mut marked_lists = HashSet::new();
        mark_environment(
            &self.environment,
            &mut marked_environments,
            &mut marked_lists,
        );
        for environment in self.module_cache.values() {
            mark_environment(environment, &mut marked_environments, &mut marked_lists);
        }
        for value in temporary_roots {
            mark_value(value, &mut marked_environments, &mut marked_lists);
        }

        self.sweep(&marked_environments, &marked_lists);
    }

    fn collect_finished(&mut self, result_roots: &[Value]) {
        let mut marked_environments = HashSet::new();
        let mut marked_lists = HashSet::new();
        for environment in self.module_cache.values() {
            mark_environment(environment, &mut marked_environments, &mut marked_lists);
        }
        for value in result_roots {
            mark_value(value, &mut marked_environments, &mut marked_lists);
        }
        self.sweep(&marked_environments, &marked_lists);
    }

    fn sweep(&mut self, marked_environments: &HashSet<usize>, marked_lists: &HashSet<usize>) {
        self.lists.retain(|entry| {
            let Some(values) = entry.upgrade() else {
                return false;
            };
            let identity = Rc::as_ptr(&values) as usize;
            if !marked_lists.contains(&identity) {
                values.borrow_mut().clear();
                self.collected_objects += 1;
            }
            Rc::strong_count(&values) > 1
        });
        self.maps.retain(|entry| {
            let Some(values) = entry.upgrade() else {
                return false;
            };
            let identity = Rc::as_ptr(&values) as usize;
            if !marked_lists.contains(&identity) {
                values.borrow_mut().clear();
                self.collected_objects += 1;
            }
            Rc::strong_count(&values) > 1
        });
        self.objects.retain(|entry| {
            let Some(instance) = entry.upgrade() else {
                return false;
            };
            let identity = Rc::as_ptr(&instance) as usize;
            if !marked_lists.contains(&identity) {
                instance.borrow_mut().fields.clear();
                self.collected_objects += 1;
            }
            Rc::strong_count(&instance) > 1
        });
        self.environments.retain(|entry| {
            let Some(environment) = entry.upgrade() else {
                return false;
            };
            let identity = Rc::as_ptr(&environment) as usize;
            if !marked_environments.contains(&identity) {
                let mut data = environment.borrow_mut();
                data.values.clear();
                data.parent = None;
                self.collected_objects += 1;
            }
            Rc::strong_count(&environment) > 1
        });
    }
    fn list_index(&mut self, expression: &Expression) -> Result<usize, RuntimeError> {
        match self.evaluate(expression)? {
            Value::Integer(value) => usize::try_from(value)
                .map_err(|_| self.error("list index must be non-negative", expression.span)),
            _ => Err(self.error("list index must be int", expression.span)),
        }
    }
    fn map_key(&mut self, expression: &Expression) -> Result<String, RuntimeError> {
        match self.evaluate(expression)? {
            Value::String(value) => Ok(value),
            _ => Err(self.error("map index must be string", expression.span)),
        }
    }
    fn require_type(
        &self,
        value: &Value,
        type_annotation: Option<&TypeAnnotation>,
        span: Span,
    ) -> Result<(), RuntimeError> {
        let Some(type_annotation) = type_annotation else {
            return Ok(());
        };
        if value_matches_type(value, type_annotation) {
            Ok(())
        } else {
            Err(self.error(
                format!(
                    "expected {}, received {}",
                    display_type(type_annotation),
                    value.type_name()
                ),
                span,
            ))
        }
    }
    fn error(&self, message: impl Into<String>, span: Span) -> RuntimeError {
        RuntimeError {
            message: message.into(),
            span,
            propagated: None,
        }
    }
}

impl VariantTag {
    fn name(self) -> &'static str {
        match self {
            Self::Some => "Some",
            Self::None => "None",
            Self::Ok => "Ok",
            Self::Err => "Err",
        }
    }
}

fn ok_value(value: Value) -> Value {
    Value::Variant {
        tag: VariantTag::Ok,
        payload: Some(Box::new(value)),
    }
}

fn err_value(error: impl ToString) -> Value {
    Value::Variant {
        tag: VariantTag::Err,
        payload: Some(Box::new(Value::String(error.to_string()))),
    }
}

fn result_value<E: ToString>(result: Result<Value, E>) -> Value {
    match result {
        Ok(value) => ok_value(value),
        Err(error) => err_value(error),
    }
}

fn option_value(value: Option<Value>) -> Value {
    value.map_or(
        Value::Variant {
            tag: VariantTag::None,
            payload: None,
        },
        |value| Value::Variant {
            tag: VariantTag::Some,
            payload: Some(Box::new(value)),
        },
    )
}

impl NativeModule {
    fn name(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Fs => "fs",
            Self::Path => "path",
            Self::Env => "env",
            Self::Process => "process",
            Self::Terminal => "terminal",
            Self::Task => "task",
        }
    }

    fn member(self, name: &str) -> Option<NativeFunction> {
        match (self, name) {
            (Self::Json, "parse") => Some(NativeFunction::JsonParse),
            (Self::Json, "stringify") => Some(NativeFunction::JsonStringify),
            (Self::Fs, "read_text") => Some(NativeFunction::FsReadText),
            (Self::Fs, "write_text") => Some(NativeFunction::FsWriteText),
            (Self::Fs, "exists") => Some(NativeFunction::FsExists),
            (Self::Fs, "create_dir_all") => Some(NativeFunction::FsCreateDirAll),
            (Self::Fs, "remove_file") => Some(NativeFunction::FsRemoveFile),
            (Self::Path, "join") => Some(NativeFunction::PathJoin),
            (Self::Path, "parent") => Some(NativeFunction::PathParent),
            (Self::Path, "file_name") => Some(NativeFunction::PathFileName),
            (Self::Path, "is_absolute") => Some(NativeFunction::PathIsAbsolute),
            (Self::Env, "get") => Some(NativeFunction::EnvGet),
            (Self::Env, "current_dir") => Some(NativeFunction::EnvCurrentDir),
            (Self::Process, "run") => Some(NativeFunction::ProcessRun),
            (Self::Terminal, "input") => Some(NativeFunction::TerminalInput),
            (Self::Terminal, "confirm") => Some(NativeFunction::TerminalConfirm),
            (Self::Terminal, "password") => Some(NativeFunction::TerminalPassword),
            (Self::Task, "result") => Some(NativeFunction::TaskResult),
            _ => None,
        }
    }
}

fn native_module(name: &str) -> Option<NativeModule> {
    match name {
        "json" => Some(NativeModule::Json),
        "fs" => Some(NativeModule::Fs),
        "path" => Some(NativeModule::Path),
        "env" => Some(NativeModule::Env),
        "process" => Some(NativeModule::Process),
        "terminal" => Some(NativeModule::Terminal),
        "task" => Some(NativeModule::Task),
        _ => None,
    }
}

impl NativeFunction {
    fn name(self) -> &'static str {
        match self {
            Self::JsonParse => "json.parse",
            Self::JsonStringify => "json.stringify",
            Self::FsReadText => "fs.read_text",
            Self::FsWriteText => "fs.write_text",
            Self::FsExists => "fs.exists",
            Self::FsCreateDirAll => "fs.create_dir_all",
            Self::FsRemoveFile => "fs.remove_file",
            Self::PathJoin => "path.join",
            Self::PathParent => "path.parent",
            Self::PathFileName => "path.file_name",
            Self::PathIsAbsolute => "path.is_absolute",
            Self::EnvGet => "env.get",
            Self::EnvCurrentDir => "env.current_dir",
            Self::ProcessRun => "process.run",
            Self::TerminalInput => "terminal.input",
            Self::TerminalConfirm => "terminal.confirm",
            Self::TerminalPassword => "terminal.password",
            Self::TaskResult => "task.result",
        }
    }

    fn arity(self) -> usize {
        match self {
            Self::FsWriteText | Self::ProcessRun => 2,
            Self::EnvCurrentDir => 0,
            _ => 1,
        }
    }
}

fn pattern_binding(pattern: &PatternKind, value: &Value) -> Option<Option<Value>> {
    match pattern {
        PatternKind::Wildcard => Some(None),
        PatternKind::Literal(literal) => {
            let literal = match literal {
                Literal::Null => Value::Null,
                Literal::Bool(value) => Value::Bool(*value),
                Literal::Integer(value) => Value::Integer(*value),
                Literal::Float(value) => Value::Float(*value),
                Literal::String(value) => Value::String(value.clone()),
            };
            values_equal(&literal, value).then_some(None)
        }
        PatternKind::Variant { name, .. } => {
            let Value::Variant { tag, payload } = value else {
                return None;
            };
            (*tag == variant_tag(name.as_str())?).then(|| payload.as_deref().cloned())
        }
    }
}

fn variant_tag(name: &str) -> Option<VariantTag> {
    match name {
        "Some" => Some(VariantTag::Some),
        "None" => Some(VariantTag::None),
        "Ok" => Some(VariantTag::Ok),
        "Err" => Some(VariantTag::Err),
        _ => None,
    }
}

fn mark_environment(
    environment: &Environment,
    marked_environments: &mut HashSet<usize>,
    marked_lists: &mut HashSet<usize>,
) {
    let identity = Rc::as_ptr(&environment.0) as usize;
    if !marked_environments.insert(identity) {
        return;
    }
    let (parent, values) = {
        let data = environment.0.borrow();
        (
            data.parent.clone(),
            data.values
                .values()
                .map(|binding| binding.value.clone())
                .collect::<Vec<_>>(),
        )
    };
    if let Some(parent) = parent {
        mark_environment(&parent, marked_environments, marked_lists);
    }
    for value in &values {
        mark_value(value, marked_environments, marked_lists);
    }
}

fn mark_value(
    value: &Value,
    marked_environments: &mut HashSet<usize>,
    marked_lists: &mut HashSet<usize>,
) {
    match value {
        Value::List(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !marked_lists.insert(identity) {
                return;
            }
            for value in values.borrow().iter() {
                mark_value(value, marked_environments, marked_lists);
            }
        }
        Value::Map(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !marked_lists.insert(identity) {
                return;
            }
            for (_, value) in values.borrow().iter() {
                mark_value(value, marked_environments, marked_lists);
            }
        }
        Value::StructInstance(instance) => {
            let identity = Rc::as_ptr(instance) as usize;
            if !marked_lists.insert(identity) {
                return;
            }
            let (closure, values) = {
                let instance = instance.borrow();
                (
                    instance.definition.closure.clone(),
                    instance.fields.values().cloned().collect::<Vec<_>>(),
                )
            };
            mark_environment(&closure, marked_environments, marked_lists);
            for value in &values {
                mark_value(value, marked_environments, marked_lists);
            }
        }
        Value::StructType(definition) => {
            mark_environment(&definition.closure, marked_environments, marked_lists)
        }
        Value::Function(function) => {
            mark_environment(&function.closure, marked_environments, marked_lists)
        }
        Value::Variant {
            payload: Some(value),
            ..
        } => mark_value(value, marked_environments, marked_lists),
        _ => {}
    }
}

impl Value {
    fn type_name(&self) -> String {
        match self {
            Self::Null => "null".into(),
            Self::Bool(_) => "bool".into(),
            Self::Integer(_) => "int".into(),
            Self::Float(_) => "float".into(),
            Self::String(_) => "string".into(),
            Self::List(_) => "list".into(),
            Self::Map(_) => "map".into(),
            Self::Function(_) | Self::NativePrint | Self::NativeFunction(_) => "fn".into(),
            Self::NativeModule(module) => format!("module {}", module.name()),
            Self::StructType(definition) => format!("type {}", definition.name),
            Self::InterfaceType(definition) => format!("type {}", definition.name),
            Self::StructInstance(instance) => instance.borrow().definition.name.clone(),
            Self::Variant { tag, .. } => match tag {
                VariantTag::Some | VariantTag::None => "Option".into(),
                VariantTag::Ok | VariantTag::Err => "Result".into(),
            },
            Self::NativeConstructor(_) => "fn".into(),
        }
    }
}

fn value_matches_type(value: &Value, type_annotation: &TypeAnnotation) -> bool {
    value_matches_type_inner(value, type_annotation, &mut HashSet::new())
}

fn value_matches_type_inner(
    value: &Value,
    type_annotation: &TypeAnnotation,
    visited: &mut HashSet<usize>,
) -> bool {
    match type_annotation.name.as_str() {
        "any" => true,
        "null" => matches!(value, Value::Null),
        "bool" => matches!(value, Value::Bool(_)),
        "int" => matches!(value, Value::Integer(_)),
        "float" => matches!(value, Value::Float(_)),
        "number" => matches!(value, Value::Integer(_) | Value::Float(_)),
        "string" => matches!(value, Value::String(_)),
        "list" => {
            let Value::List(values) = value else {
                return false;
            };
            let [element_type] = type_annotation.arguments.as_slice() else {
                return false;
            };
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return true;
            }
            let matches = values
                .borrow()
                .iter()
                .all(|value| value_matches_type_inner(value, element_type, visited));
            visited.remove(&identity);
            matches
        }
        "map" => {
            let Value::Map(values) = value else {
                return false;
            };
            let [element_type] = type_annotation.arguments.as_slice() else {
                return false;
            };
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return true;
            }
            let matches = values
                .borrow()
                .iter()
                .all(|(_, value)| value_matches_type_inner(value, element_type, visited));
            visited.remove(&identity);
            matches
        }
        "fn" => matches!(
            value,
            Value::Function(_)
                | Value::NativePrint
                | Value::NativeConstructor(_)
                | Value::NativeFunction(_)
        ),
        "Option" => match value {
            Value::Variant {
                tag: VariantTag::None,
                ..
            } => true,
            Value::Variant {
                tag: VariantTag::Some,
                payload: Some(payload),
            } => type_annotation
                .arguments
                .first()
                .is_some_and(|inner| value_matches_type_inner(payload, inner, visited)),
            _ => false,
        },
        "Result" => match value {
            Value::Variant {
                tag: VariantTag::Ok,
                payload: Some(payload),
            } => type_annotation
                .arguments
                .first()
                .is_some_and(|inner| value_matches_type_inner(payload, inner, visited)),
            Value::Variant {
                tag: VariantTag::Err,
                payload: Some(payload),
            } => type_annotation
                .arguments
                .get(1)
                .is_some_and(|inner| value_matches_type_inner(payload, inner, visited)),
            _ => false,
        },
        name => matches!(value, Value::StructInstance(instance) if {
            let instance = instance.borrow();
            instance.definition.name == name
                || instance.definition.interfaces.borrow().contains(name)
        }),
    }
}

fn struct_satisfies_interface(fields: &[StructField], required: &[StructField]) -> bool {
    required.iter().all(|required_field| {
        fields.iter().any(|field| {
            field.name.name == required_field.name.name
                && type_annotations_match(&field.type_annotation, &required_field.type_annotation)
        })
    })
}

fn type_annotations_match(left: &TypeAnnotation, right: &TypeAnnotation) -> bool {
    left.name.name == right.name.name
        && left.arguments.len() == right.arguments.len()
        && left
            .arguments
            .iter()
            .zip(&right.arguments)
            .all(|(left, right)| type_annotations_match(left, right))
}

fn display_type(type_annotation: &TypeAnnotation) -> String {
    if type_annotation.arguments.is_empty() {
        return type_annotation.name.name.clone();
    }
    format!(
        "{}<{}>",
        type_annotation.name.name,
        type_annotation
            .arguments
            .iter()
            .map(display_type)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn value_to_json(value: &Value, visited: &mut HashSet<usize>) -> Result<serde_json::Value, String> {
    match value {
        Value::Null => Ok(serde_json::Value::Null),
        Value::Bool(value) => Ok(serde_json::Value::Bool(*value)),
        Value::Integer(value) => Ok(serde_json::Value::Number((*value).into())),
        Value::Float(value) => serde_json::Number::from_f64(*value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| "JSON cannot represent NaN or infinity".into()),
        Value::String(value) => Ok(serde_json::Value::String(value.clone())),
        Value::List(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return Err("JSON cannot represent cyclic values".into());
            }
            let result = values
                .borrow()
                .iter()
                .map(|value| value_to_json(value, visited))
                .collect::<Result<Vec<_>, _>>()
                .map(serde_json::Value::Array);
            visited.remove(&identity);
            result
        }
        Value::Map(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return Err("JSON cannot represent cyclic values".into());
            }
            let result = values
                .borrow()
                .iter()
                .map(|(key, value)| Ok((key.clone(), value_to_json(value, visited)?)))
                .collect::<Result<serde_json::Map<_, _>, String>>()
                .map(serde_json::Value::Object);
            visited.remove(&identity);
            result
        }
        _ => Err(format!("JSON cannot represent {}", value.type_name())),
    }
}

fn numbers(left: Value, right: Value) -> Option<(f64, f64)> {
    let left = match left {
        Value::Integer(value) => value as f64,
        Value::Float(value) => value,
        _ => return None,
    };
    let right = match right {
        Value::Integer(value) => value as f64,
        Value::Float(value) => value,
        _ => return None,
    };
    Some((left, right))
}

fn values_equal(left: &Value, right: &Value) -> bool {
    values_equal_inner(left, right, &mut HashSet::new())
}

fn values_equal_inner(left: &Value, right: &Value, visited: &mut HashSet<(usize, usize)>) -> bool {
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Integer(a), Value::Float(b)) => *a as f64 == *b,
        (Value::Float(a), Value::Integer(b)) => *a == *b as f64,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            let identity = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
            if !visited.insert(identity) {
                return true;
            }
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.iter()
                    .zip(b.iter())
                    .all(|(a, b)| values_equal_inner(a, b, visited))
        }
        (Value::Map(a), Value::Map(b)) => {
            let identity = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
            if !visited.insert(identity) {
                return true;
            }
            let a = a.borrow();
            let b = b.borrow();
            a.len() == b.len()
                && a.iter().all(|(key, value)| {
                    b.iter()
                        .find(|(candidate, _)| candidate == key)
                        .is_some_and(|(_, other)| values_equal_inner(value, other, visited))
                })
        }
        (Value::StructType(a), Value::StructType(b)) => Rc::ptr_eq(a, b),
        (Value::InterfaceType(a), Value::InterfaceType(b)) => Rc::ptr_eq(a, b),
        (Value::StructInstance(a), Value::StructInstance(b)) => {
            let identity = (Rc::as_ptr(a) as usize, Rc::as_ptr(b) as usize);
            if !visited.insert(identity) {
                return true;
            }
            let a = a.borrow();
            let b = b.borrow();
            a.definition.name == b.definition.name
                && a.fields.len() == b.fields.len()
                && a.fields.iter().all(|(name, value)| {
                    b.fields
                        .get(name)
                        .is_some_and(|other| values_equal_inner(value, other, visited))
                })
        }
        (
            Value::Variant {
                tag: a_tag,
                payload: a_payload,
            },
            Value::Variant {
                tag: b_tag,
                payload: b_payload,
            },
        ) => {
            a_tag == b_tag
                && match (a_payload, b_payload) {
                    (None, None) => true,
                    (Some(a), Some(b)) => values_equal_inner(a, b, visited),
                    _ => false,
                }
        }
        (Value::NativeConstructor(a), Value::NativeConstructor(b)) => a == b,
        (Value::NativeModule(a), Value::NativeModule(b)) => a == b,
        (Value::NativeFunction(a), Value::NativeFunction(b)) => a == b,
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
        (Value::NativePrint, Value::NativePrint) => true,
        _ => false,
    }
}

fn format_value(value: &Value, visited: &mut HashSet<usize>) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Integer(value) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::List(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return "<cycle>".into();
            }
            let formatted = values
                .borrow()
                .iter()
                .map(|value| format_value(value, visited))
                .collect::<Vec<_>>()
                .join(", ");
            visited.remove(&identity);
            format!("[{formatted}]")
        }
        Value::Map(values) => {
            let identity = Rc::as_ptr(values) as usize;
            if !visited.insert(identity) {
                return "<cycle>".into();
            }
            let formatted = values
                .borrow()
                .iter()
                .map(|(key, value)| format!("{key}: {}", format_value(value, visited)))
                .collect::<Vec<_>>()
                .join(", ");
            visited.remove(&identity);
            format!("{{ {formatted} }}")
        }
        Value::StructType(definition) => format!("<struct {}>", definition.name),
        Value::InterfaceType(definition) => format!("<interface {}>", definition.name),
        Value::StructInstance(instance) => {
            let identity = Rc::as_ptr(instance) as usize;
            if !visited.insert(identity) {
                return "<cycle>".into();
            }
            let instance = instance.borrow();
            let formatted = instance
                .definition
                .fields
                .iter()
                .filter_map(|field| {
                    instance.fields.get(field.name.as_str()).map(|value| {
                        format!("{}: {}", field.name.name, format_value(value, visited))
                    })
                })
                .collect::<Vec<_>>()
                .join(", ");
            visited.remove(&identity);
            format!("{} {{ {formatted} }}", instance.definition.name)
        }
        Value::Variant { tag, payload } => payload.as_ref().map_or_else(
            || tag.name().into(),
            |value| format!("{}({})", tag.name(), format_value(value, visited)),
        ),
        Value::NativeConstructor(tag) => format!("<native fn {}>", tag.name()),
        Value::NativeModule(module) => format!("<module {}>", module.name()),
        Value::NativeFunction(function) => format!("<native fn {}>", function.name()),
        Value::Function(function) => format!("<fn {}>", function.name),
        Value::NativePrint => "<native fn print>".into(),
    }
}
