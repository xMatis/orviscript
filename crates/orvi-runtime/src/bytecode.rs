use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, ForSource, Literal, Parameter, PatternKind,
    Program, Span, Statement, StatementKind, TypeAnnotation, UnaryOperator,
};

use super::{
    Execution, MapEntries, RuntimeError, Value, VariantTag, display_type, mark_value,
    value_matches_type, values_equal,
};

#[derive(Clone)]
pub(super) struct Bytecode {
    instructions: Vec<Instruction>,
    slot_types: Vec<Option<TypeAnnotation>>,
    initial_values: Vec<Value>,
    functions: Vec<BytecodeFunction>,
}

#[derive(Clone)]
struct BytecodeFunction {
    name: String,
    instructions: Vec<Instruction>,
    slot_types: Vec<Option<TypeAnnotation>>,
    parameters: usize,
    return_type: Option<TypeAnnotation>,
}

#[derive(Default)]
struct VmHeap {
    lists: Vec<Weak<RefCell<Vec<Value>>>>,
    maps: Vec<Weak<RefCell<MapEntries>>>,
    slot_pool: Vec<Vec<Value>>,
    stack_pool: Vec<Vec<Value>>,
}

#[derive(Clone)]
enum Instruction {
    Push(Value),
    Load(usize),
    Store(usize, Span),
    Bind(usize, Span),
    Pop,
    Unary(UnaryOperator, Span),
    Binary(BinaryOperator, Span),
    IntegerBinary(BinaryOperator, Span),
    IntegerConstant(BinaryOperator, i64, Span),
    IntegerSlotConstant(usize, BinaryOperator, i64, Span),
    IntegerSlotBinary(usize, BinaryOperator, usize, Span),
    JumpIfFalse(usize, Span),
    JumpIfTrue(usize, Span),
    RequireBool(Span),
    Jump(usize),
    Print(usize),
    Call(usize, usize, Span),
    Return,
    BuildList(usize),
    SnapshotList(Span),
    ListLength(Span),
    BuildMap(Vec<String>),
    Index(Span),
    SetIndex(Span),
    Member(String, Span),
    SetMember(String, Span),
    Construct(VariantTag, usize, Span),
    Try(Span),
    MatchPattern {
        pattern: CompiledPattern,
        binding: Option<usize>,
        target: usize,
    },
    MatchFail(Span),
    RequireType(TypeAnnotation, Span),
}

#[derive(Clone)]
enum CompiledPattern {
    Variant(VariantTag),
    Literal(Value),
    Wildcard,
}

#[derive(Clone, Copy)]
struct Local {
    slot: usize,
    mutable: bool,
    primitive: Option<PrimitiveKind>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PrimitiveKind {
    Bool,
    Integer,
    Float,
    String,
}

struct Compiler {
    instructions: Vec<Instruction>,
    scopes: Vec<HashMap<String, Local>>,
    slot_types: Vec<Option<TypeAnnotation>>,
    functions: HashMap<String, FunctionReference>,
    in_function: bool,
    stack_parameter: Option<String>,
    stack_parameter_type: Option<PrimitiveKind>,
    loop_stack: Vec<LoopContext>,
}

#[derive(Default)]
struct LoopContext {
    break_jumps: Vec<usize>,
    continue_jumps: Vec<usize>,
}

#[derive(Clone)]
struct FunctionReference {
    index: usize,
    arity: usize,
    inline: Option<InlineFunction>,
}

#[derive(Clone)]
struct InlineFunction {
    parameters: Vec<Parameter>,
    expression: Expression,
    return_type: Option<TypeAnnotation>,
}

pub(super) fn compile(program: &Program) -> Option<Bytecode> {
    compile_with_bindings(program, Vec::new())
}

thread_local! {
    static WORKER_CACHE: RefCell<HashMap<String, Bytecode>> = RefCell::new(HashMap::new());
}

pub(super) fn compile_worker_cached(
    source: &str,
    program: &Program,
    input: Value,
) -> Option<Bytecode> {
    WORKER_CACHE.with(|cache| {
        if let Some(bytecode) = cache.borrow().get(source) {
            let mut bytecode = bytecode.clone();
            bytecode.initial_values[0] = input;
            return Some(bytecode);
        }
        let bytecode = compile_with_bindings(program, vec![("worker_input".into(), input)])?;
        let mut cache = cache.borrow_mut();
        if cache.len() >= 64 {
            cache.clear();
        }
        let mut template = bytecode.clone();
        template.initial_values[0] = Value::Null;
        cache.insert(source.into(), template);
        Some(bytecode)
    })
}

pub(super) fn compile_with_bindings(
    program: &Program,
    bindings: Vec<(String, Value)>,
) -> Option<Bytecode> {
    let functions = collect_function_references(program)?;
    let mut root_scope = HashMap::new();
    let mut slot_types = Vec::with_capacity(bindings.len());
    let mut initial_values = Vec::with_capacity(bindings.len());
    for (name, value) in bindings {
        let slot = slot_types.len();
        root_scope.insert(
            name,
            Local {
                slot,
                mutable: false,
                primitive: primitive_from_value(&value),
            },
        );
        slot_types.push(None);
        initial_values.push(value);
    }
    let mut compiler = Compiler {
        instructions: Vec::new(),
        scopes: vec![root_scope],
        slot_types,
        functions: functions.clone(),
        in_function: false,
        stack_parameter: None,
        stack_parameter_type: None,
        loop_stack: Vec::new(),
    };
    compiler.compile_statements(&program.statements)?;
    let compiled_functions = compile_functions(program, &functions)?;
    Some(Bytecode {
        instructions: compiler.instructions,
        slot_types: compiler.slot_types,
        initial_values,
        functions: compiled_functions,
    })
}

fn collect_function_references(program: &Program) -> Option<HashMap<String, FunctionReference>> {
    let mut functions = HashMap::new();
    for statement in &program.statements {
        let declaration = match &statement.kind {
            StatementKind::Export(declaration) => declaration.as_ref(),
            _ => statement,
        };
        if let StatementKind::Function {
            name,
            is_async,
            parameters,
            return_type,
            body,
            ..
        } = &declaration.kind
        {
            if *is_async || functions.contains_key(name.as_str()) {
                return None;
            }
            let index = functions.len();
            functions.insert(
                name.name.clone(),
                FunctionReference {
                    index,
                    arity: parameters.len(),
                    inline: inline_function(parameters, return_type, body),
                },
            );
        }
    }
    Some(functions)
}

fn inline_function(
    parameters: &[Parameter],
    return_type: &Option<TypeAnnotation>,
    body: &[Statement],
) -> Option<InlineFunction> {
    let [
        Statement {
            kind: StatementKind::Return(Some(expression)),
            ..
        },
    ] = body
    else {
        return None;
    };
    let parameter_names = parameters
        .iter()
        .map(|parameter| parameter.name.name.as_str())
        .collect::<std::collections::HashSet<_>>();
    if !is_inline_expression(expression, &parameter_names, 0) {
        return None;
    }
    Some(InlineFunction {
        parameters: parameters.to_vec(),
        expression: expression.clone(),
        return_type: return_type.clone(),
    })
}

fn is_inline_expression(
    expression: &Expression,
    parameters: &std::collections::HashSet<&str>,
    depth: usize,
) -> bool {
    if depth > 16 {
        return false;
    }
    match &expression.kind {
        ExpressionKind::Literal(_) => true,
        ExpressionKind::Variable(name) => parameters.contains(name.as_str()),
        ExpressionKind::Unary { operand, .. } => {
            is_inline_expression(operand, parameters, depth + 1)
        }
        ExpressionKind::Binary { left, right, .. } => {
            is_inline_expression(left, parameters, depth + 1)
                && is_inline_expression(right, parameters, depth + 1)
        }
        _ => false,
    }
}

fn can_inline_parameter_from_stack(inline: &InlineFunction) -> bool {
    let [parameter] = inline.parameters.as_slice() else {
        return false;
    };
    let mut occurrences = 0;
    count_parameter_occurrences(
        &inline.expression,
        parameter.name.as_str(),
        &mut occurrences,
    );
    occurrences == 1
        && matches!(
            first_expression_leaf(&inline.expression),
            ExpressionKind::Variable(name) if name.name == parameter.name.name
        )
}

fn count_parameter_occurrences(expression: &Expression, parameter: &str, count: &mut usize) {
    match &expression.kind {
        ExpressionKind::Variable(name) if name.name == parameter => *count += 1,
        ExpressionKind::Unary { operand, .. } => {
            count_parameter_occurrences(operand, parameter, count);
        }
        ExpressionKind::Binary { left, right, .. } => {
            count_parameter_occurrences(left, parameter, count);
            count_parameter_occurrences(right, parameter, count);
        }
        _ => {}
    }
}

fn first_expression_leaf(expression: &Expression) -> &ExpressionKind {
    match &expression.kind {
        ExpressionKind::Unary { operand, .. } => first_expression_leaf(operand),
        ExpressionKind::Binary { left, .. } => first_expression_leaf(left),
        kind => kind,
    }
}

fn primitive_from_annotation(annotation: &TypeAnnotation) -> Option<PrimitiveKind> {
    match annotation.name.as_str() {
        "bool" => Some(PrimitiveKind::Bool),
        "int" => Some(PrimitiveKind::Integer),
        "float" => Some(PrimitiveKind::Float),
        "string" => Some(PrimitiveKind::String),
        _ => None,
    }
}

fn primitive_from_value(value: &Value) -> Option<PrimitiveKind> {
    match value {
        Value::Bool(_) => Some(PrimitiveKind::Bool),
        Value::Integer(_) => Some(PrimitiveKind::Integer),
        Value::Float(_) => Some(PrimitiveKind::Float),
        Value::String(_) => Some(PrimitiveKind::String),
        _ => None,
    }
}

fn compile_functions(
    program: &Program,
    references: &HashMap<String, FunctionReference>,
) -> Option<Vec<BytecodeFunction>> {
    let mut functions = Vec::with_capacity(references.len());
    for statement in &program.statements {
        let declaration = match &statement.kind {
            StatementKind::Export(declaration) => declaration.as_ref(),
            _ => statement,
        };
        let StatementKind::Function {
            name,
            is_async: false,
            parameters,
            return_type,
            body,
        } = &declaration.kind
        else {
            continue;
        };
        let mut compiler = Compiler::for_function(parameters, references.clone());
        compiler.compile_function_body(body)?;
        let reference = references.get(name.as_str())?;
        if reference.index != functions.len() {
            return None;
        }
        functions.push(BytecodeFunction {
            name: name.name.clone(),
            instructions: compiler.instructions,
            slot_types: compiler.slot_types,
            parameters: parameters.len(),
            return_type: return_type.clone(),
        });
    }
    Some(functions)
}

impl Compiler {
    fn for_function(
        parameters: &[Parameter],
        functions: HashMap<String, FunctionReference>,
    ) -> Self {
        let mut scope = HashMap::new();
        let mut slot_types = Vec::with_capacity(parameters.len());
        for parameter in parameters {
            let slot = slot_types.len();
            scope.insert(
                parameter.name.name.clone(),
                Local {
                    slot,
                    mutable: false,
                    primitive: parameter
                        .type_annotation
                        .as_ref()
                        .and_then(primitive_from_annotation),
                },
            );
            slot_types.push(parameter.type_annotation.clone());
        }
        Self {
            instructions: Vec::new(),
            scopes: vec![scope],
            slot_types,
            functions,
            in_function: true,
            stack_parameter: None,
            stack_parameter_type: None,
            loop_stack: Vec::new(),
        }
    }

    fn compile_function_body(&mut self, statements: &[Statement]) -> Option<()> {
        self.compile_statements(statements)?;
        self.instructions.push(Instruction::Pop);
        self.instructions.push(Instruction::Push(Value::Void));
        self.instructions.push(Instruction::Return);
        Some(())
    }

    fn compile_statements(&mut self, statements: &[Statement]) -> Option<()> {
        if statements.is_empty() {
            self.instructions.push(Instruction::Push(Value::Null));
            return Some(());
        }
        for (index, statement) in statements.iter().enumerate() {
            self.compile_statement(statement)?;
            if index + 1 != statements.len() {
                self.instructions.push(Instruction::Pop);
            }
        }
        Some(())
    }

    fn compile_statement(&mut self, statement: &Statement) -> Option<()> {
        match &statement.kind {
            StatementKind::Variable {
                name,
                mutable,
                type_annotation,
                initializer,
            } => {
                let primitive = type_annotation
                    .as_ref()
                    .and_then(primitive_from_annotation)
                    .or_else(|| self.expression_primitive(initializer));
                self.compile_expression(initializer)?;
                let slot = self.slot_types.len();
                self.slot_types.push(type_annotation.clone());
                self.scopes.last_mut()?.insert(
                    name.name.clone(),
                    Local {
                        slot,
                        mutable: *mutable,
                        primitive,
                    },
                );
                self.instructions
                    .push(Instruction::Store(slot, initializer.span));
            }
            StatementKind::Block(statements) => {
                self.scopes.push(HashMap::new());
                self.compile_statements(statements)?;
                self.scopes.pop();
            }
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.compile_expression(condition)?;
                let false_jump = self.emit(Instruction::JumpIfFalse(usize::MAX, condition.span));
                self.compile_statement(then_branch)?;
                let end_jump = self.emit(Instruction::Jump(usize::MAX));
                self.patch_jump(false_jump, self.instructions.len());
                if let Some(else_branch) = else_branch {
                    self.compile_statement(else_branch)?;
                } else {
                    self.instructions.push(Instruction::Push(Value::Null));
                }
                self.patch_jump(end_jump, self.instructions.len());
            }
            StatementKind::While { condition, body } => {
                let loop_start = self.instructions.len();
                self.compile_expression(condition)?;
                let exit_jump = self.emit(Instruction::JumpIfFalse(usize::MAX, condition.span));
                self.loop_stack.push(LoopContext::default());
                self.compile_statement(body)?;
                let loop_context = self.loop_stack.pop()?;
                self.instructions.push(Instruction::Pop);
                self.instructions.push(Instruction::Jump(loop_start));
                let exit = self.instructions.len();
                self.patch_jump(exit_jump, exit);
                for jump in loop_context.continue_jumps {
                    self.patch_jump(jump, loop_start);
                }
                for jump in loop_context.break_jumps {
                    self.patch_jump(jump, exit);
                }
                self.instructions.push(Instruction::Push(Value::Null));
            }
            StatementKind::For { name, source, body } => {
                self.scopes.push(HashMap::new());
                let item_slot = self.slot_types.len();
                self.slot_types.push(None);
                let item_primitive = match source {
                    ForSource::Range { .. } => Some(PrimitiveKind::Integer),
                    ForSource::Iterable(_) => None,
                };
                self.scopes.last_mut()?.insert(
                    name.name.clone(),
                    Local {
                        slot: item_slot,
                        mutable: false,
                        primitive: item_primitive,
                    },
                );
                match source {
                    ForSource::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let current_slot = item_slot;
                        self.compile_expression(start)?;
                        self.instructions
                            .push(Instruction::Store(current_slot, start.span));
                        self.instructions.push(Instruction::Pop);
                        let end_constant = match end.kind {
                            ExpressionKind::Literal(Literal::Integer(value)) => Some(value),
                            _ => None,
                        };
                        let end_slot = if end_constant.is_none() {
                            let slot = self.slot_types.len();
                            self.slot_types.push(None);
                            self.compile_expression(end)?;
                            self.instructions.push(Instruction::Store(slot, end.span));
                            self.instructions.push(Instruction::Pop);
                            Some(slot)
                        } else {
                            None
                        };

                        let loop_start = self.instructions.len();
                        self.instructions.push(Instruction::Load(current_slot));
                        let comparison = if *inclusive {
                            BinaryOperator::LessEqual
                        } else {
                            BinaryOperator::Less
                        };
                        if let Some(end_constant) = end_constant {
                            self.instructions.push(Instruction::IntegerConstant(
                                comparison,
                                end_constant,
                                statement.span,
                            ));
                        } else {
                            self.instructions
                                .push(Instruction::Load(end_slot.expect("dynamic range end slot")));
                            self.instructions
                                .push(Instruction::IntegerBinary(comparison, statement.span));
                        }
                        let exit_jump =
                            self.emit(Instruction::JumpIfFalse(usize::MAX, statement.span));
                        self.loop_stack.push(LoopContext::default());
                        self.compile_statement(body)?;
                        let loop_context = self.loop_stack.pop()?;
                        self.instructions.push(Instruction::Pop);
                        let continue_target = self.instructions.len();
                        let inclusive_end_jump = if *inclusive {
                            self.instructions.push(Instruction::Load(current_slot));
                            if let Some(end_constant) = end_constant {
                                self.instructions.push(Instruction::IntegerConstant(
                                    BinaryOperator::Equal,
                                    end_constant,
                                    statement.span,
                                ));
                            } else {
                                self.instructions.push(Instruction::Load(
                                    end_slot.expect("dynamic range end slot"),
                                ));
                                self.instructions.push(Instruction::IntegerBinary(
                                    BinaryOperator::Equal,
                                    statement.span,
                                ));
                            }
                            Some(self.emit(Instruction::JumpIfTrue(usize::MAX, statement.span)))
                        } else {
                            None
                        };
                        self.instructions.push(Instruction::Load(current_slot));
                        self.instructions.push(Instruction::IntegerConstant(
                            BinaryOperator::Add,
                            1,
                            statement.span,
                        ));
                        self.instructions
                            .push(Instruction::Store(current_slot, statement.span));
                        self.instructions.push(Instruction::Pop);
                        self.instructions.push(Instruction::Jump(loop_start));
                        let exit = self.instructions.len();
                        self.patch_jump(exit_jump, exit);
                        if let Some(inclusive_end_jump) = inclusive_end_jump {
                            self.patch_jump(inclusive_end_jump, exit);
                        }
                        for jump in loop_context.continue_jumps {
                            self.patch_jump(jump, continue_target);
                        }
                        for jump in loop_context.break_jumps {
                            self.patch_jump(jump, exit);
                        }
                    }
                    ForSource::Iterable(iterable) => {
                        let collection_slot = self.slot_types.len();
                        self.slot_types.push(None);
                        self.compile_expression(iterable)?;
                        self.instructions
                            .push(Instruction::SnapshotList(iterable.span));
                        self.instructions
                            .push(Instruction::Store(collection_slot, iterable.span));
                        self.instructions.push(Instruction::Pop);
                        let index_slot = self.slot_types.len();
                        self.slot_types.push(None);
                        self.instructions.push(Instruction::Push(Value::Integer(0)));
                        self.instructions
                            .push(Instruction::Store(index_slot, iterable.span));
                        self.instructions.push(Instruction::Pop);

                        let loop_start = self.instructions.len();
                        self.instructions.push(Instruction::Load(index_slot));
                        self.instructions.push(Instruction::Load(collection_slot));
                        self.instructions
                            .push(Instruction::ListLength(iterable.span));
                        self.instructions.push(Instruction::IntegerBinary(
                            BinaryOperator::Less,
                            statement.span,
                        ));
                        let exit_jump =
                            self.emit(Instruction::JumpIfFalse(usize::MAX, statement.span));
                        self.instructions.push(Instruction::Load(collection_slot));
                        self.instructions.push(Instruction::Load(index_slot));
                        self.instructions.push(Instruction::Index(iterable.span));
                        self.instructions
                            .push(Instruction::Store(item_slot, name.span));
                        self.instructions.push(Instruction::Pop);
                        self.loop_stack.push(LoopContext::default());
                        self.compile_statement(body)?;
                        let loop_context = self.loop_stack.pop()?;
                        self.instructions.push(Instruction::Pop);
                        let continue_target = self.instructions.len();
                        self.instructions.push(Instruction::Load(index_slot));
                        self.instructions.push(Instruction::IntegerConstant(
                            BinaryOperator::Add,
                            1,
                            statement.span,
                        ));
                        self.instructions
                            .push(Instruction::Store(index_slot, statement.span));
                        self.instructions.push(Instruction::Pop);
                        self.instructions.push(Instruction::Jump(loop_start));
                        let exit = self.instructions.len();
                        self.patch_jump(exit_jump, exit);
                        for jump in loop_context.continue_jumps {
                            self.patch_jump(jump, continue_target);
                        }
                        for jump in loop_context.break_jumps {
                            self.patch_jump(jump, exit);
                        }
                    }
                }
                self.scopes.pop();
                self.instructions.push(Instruction::Push(Value::Null));
            }
            StatementKind::Expression(expression) => self.compile_expression(expression)?,
            StatementKind::Function {
                is_async: false, ..
            } if !self.in_function && self.scopes.len() == 1 => {
                self.instructions.push(Instruction::Push(Value::Null));
            }
            StatementKind::Export(declaration) => self.compile_statement(declaration)?,
            StatementKind::Break => {
                let jump = self.emit(Instruction::Jump(usize::MAX));
                self.loop_stack.last_mut()?.break_jumps.push(jump);
            }
            StatementKind::Continue => {
                let jump = self.emit(Instruction::Jump(usize::MAX));
                self.loop_stack.last_mut()?.continue_jumps.push(jump);
            }
            StatementKind::Return(value) => {
                if !self.in_function {
                    return None;
                }
                if let Some(value) = value {
                    self.compile_expression(value)?;
                } else {
                    self.instructions.push(Instruction::Push(Value::Void));
                }
                self.instructions.push(Instruction::Return);
            }
            StatementKind::Function { .. }
            | StatementKind::Struct { .. }
            | StatementKind::Interface { .. }
            | StatementKind::Import { .. }
            | StatementKind::Task { .. }
            | StatementKind::Defer(_)
            | StatementKind::Using { .. } => return None,
        }
        Some(())
    }

    fn expression_primitive(&self, expression: &Expression) -> Option<PrimitiveKind> {
        match &expression.kind {
            ExpressionKind::Literal(Literal::Bool(_)) => Some(PrimitiveKind::Bool),
            ExpressionKind::Literal(Literal::Integer(_)) => Some(PrimitiveKind::Integer),
            ExpressionKind::Literal(Literal::Float(_)) => Some(PrimitiveKind::Float),
            ExpressionKind::Literal(Literal::String(_)) => Some(PrimitiveKind::String),
            ExpressionKind::Literal(Literal::Null) => None,
            ExpressionKind::Variable(name) => {
                if self.stack_parameter.as_deref() == Some(name.as_str()) {
                    self.stack_parameter_type
                } else {
                    self.resolve(name.as_str())
                        .and_then(|local| local.primitive)
                }
            }
            ExpressionKind::Assign { name, .. } => self
                .resolve(name.as_str())
                .and_then(|local| local.primitive),
            ExpressionKind::Unary { operator, operand } => match operator {
                UnaryOperator::Not
                    if self.expression_primitive(operand) == Some(PrimitiveKind::Bool) =>
                {
                    Some(PrimitiveKind::Bool)
                }
                UnaryOperator::Negate => match self.expression_primitive(operand) {
                    Some(kind @ (PrimitiveKind::Integer | PrimitiveKind::Float)) => Some(kind),
                    _ => None,
                },
                _ => None,
            },
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.expression_primitive(left);
                let right = self.expression_primitive(right);
                match operator {
                    BinaryOperator::Add
                        if left == Some(PrimitiveKind::String)
                            && right == Some(PrimitiveKind::String) =>
                    {
                        Some(PrimitiveKind::String)
                    }
                    BinaryOperator::Add
                    | BinaryOperator::Subtract
                    | BinaryOperator::Multiply
                    | BinaryOperator::Divide => match (left, right) {
                        (Some(PrimitiveKind::Integer), Some(PrimitiveKind::Integer)) => {
                            Some(PrimitiveKind::Integer)
                        }
                        (
                            Some(PrimitiveKind::Integer | PrimitiveKind::Float),
                            Some(PrimitiveKind::Integer | PrimitiveKind::Float),
                        ) => Some(PrimitiveKind::Float),
                        _ => None,
                    },
                    BinaryOperator::Remainder
                        if left == Some(PrimitiveKind::Integer)
                            && right == Some(PrimitiveKind::Integer) =>
                    {
                        Some(PrimitiveKind::Integer)
                    }
                    BinaryOperator::Equal | BinaryOperator::NotEqual => Some(PrimitiveKind::Bool),
                    BinaryOperator::Less
                    | BinaryOperator::LessEqual
                    | BinaryOperator::Greater
                    | BinaryOperator::GreaterEqual
                        if matches!(
                            (left, right),
                            (
                                Some(PrimitiveKind::Integer | PrimitiveKind::Float),
                                Some(PrimitiveKind::Integer | PrimitiveKind::Float)
                            )
                        ) =>
                    {
                        Some(PrimitiveKind::Bool)
                    }
                    BinaryOperator::And | BinaryOperator::Or
                        if left == Some(PrimitiveKind::Bool)
                            && right == Some(PrimitiveKind::Bool) =>
                    {
                        Some(PrimitiveKind::Bool)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn compile_expression(&mut self, expression: &Expression) -> Option<()> {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self
                .instructions
                .push(Instruction::Push(literal_value(literal))),
            ExpressionKind::List(values) => {
                for value in values {
                    self.compile_expression(value)?;
                }
                self.instructions.push(Instruction::BuildList(values.len()));
            }
            ExpressionKind::Map(values) => {
                for (_, value) in values {
                    self.compile_expression(value)?;
                }
                self.instructions.push(Instruction::BuildMap(
                    values.iter().map(|(key, _)| key.clone()).collect(),
                ));
            }
            ExpressionKind::Variable(name) => {
                if self.stack_parameter.as_deref() == Some(name.as_str()) {
                    self.stack_parameter = None;
                    self.stack_parameter_type = None;
                } else if let Some(local) = self.resolve(&name.name) {
                    self.instructions.push(Instruction::Load(local.slot));
                } else if name.name == "None" {
                    self.instructions.push(Instruction::Push(Value::Variant {
                        tag: VariantTag::None,
                        payload: None,
                    }));
                } else {
                    return None;
                }
            }
            ExpressionKind::Assign { name, value } => {
                let local = self.resolve(&name.name)?;
                if !local.mutable {
                    return None;
                }
                let instruction_start = self.instructions.len();
                self.compile_expression(value)?;
                if !self.fuse_integer_slot_assignment(
                    instruction_start,
                    local.slot,
                    expression.span,
                ) {
                    self.instructions
                        .push(Instruction::Store(local.slot, expression.span));
                }
            }
            ExpressionKind::IndexAssign {
                object,
                index,
                value,
            } => {
                self.compile_expression(object)?;
                self.compile_expression(value)?;
                self.compile_expression(index)?;
                self.instructions
                    .push(Instruction::SetIndex(expression.span));
            }
            ExpressionKind::MemberAssign {
                object,
                field,
                value,
            } => {
                self.compile_expression(object)?;
                self.compile_expression(value)?;
                self.instructions
                    .push(Instruction::SetMember(field.name.clone(), expression.span));
            }
            ExpressionKind::Unary { operator, operand } => {
                self.compile_expression(operand)?;
                self.instructions
                    .push(Instruction::Unary(*operator, expression.span));
            }
            ExpressionKind::Binary {
                left,
                operator,
                right,
            } => {
                let left_primitive = self.expression_primitive(left);
                let right_primitive = self.expression_primitive(right);
                self.compile_expression(left)?;
                match operator {
                    BinaryOperator::And => {
                        let short_circuit =
                            self.emit(Instruction::JumpIfFalse(usize::MAX, left.span));
                        self.compile_expression(right)?;
                        self.instructions.push(Instruction::RequireBool(right.span));
                        let end = self.emit(Instruction::Jump(usize::MAX));
                        self.patch_jump(short_circuit, self.instructions.len());
                        self.instructions
                            .push(Instruction::Push(Value::Bool(false)));
                        self.patch_jump(end, self.instructions.len());
                    }
                    BinaryOperator::Or => {
                        let short_circuit =
                            self.emit(Instruction::JumpIfTrue(usize::MAX, left.span));
                        self.compile_expression(right)?;
                        self.instructions.push(Instruction::RequireBool(right.span));
                        let end = self.emit(Instruction::Jump(usize::MAX));
                        self.patch_jump(short_circuit, self.instructions.len());
                        self.instructions.push(Instruction::Push(Value::Bool(true)));
                        self.patch_jump(end, self.instructions.len());
                    }
                    _ => {
                        if left_primitive == Some(PrimitiveKind::Integer)
                            && right_primitive == Some(PrimitiveKind::Integer)
                            && let ExpressionKind::Literal(Literal::Integer(constant)) = right.kind
                        {
                            self.instructions.push(Instruction::IntegerConstant(
                                *operator,
                                constant,
                                expression.span,
                            ));
                        } else {
                            self.compile_expression(right)?;
                            if left_primitive == Some(PrimitiveKind::Integer)
                                && right_primitive == Some(PrimitiveKind::Integer)
                            {
                                self.instructions
                                    .push(Instruction::IntegerBinary(*operator, expression.span));
                            } else {
                                self.instructions
                                    .push(Instruction::Binary(*operator, expression.span));
                            }
                        }
                    }
                }
            }
            ExpressionKind::Call { callee, arguments } => {
                let ExpressionKind::Variable(name) = &callee.kind else {
                    return None;
                };
                for argument in arguments {
                    self.compile_expression(argument)?;
                }
                if name.name == "print" && self.resolve("print").is_none() {
                    self.instructions.push(Instruction::Print(arguments.len()));
                } else if let Some(tag) = constructor_tag(name.as_str()) {
                    if (tag == VariantTag::Ok && arguments.len() <= 1)
                        || (tag != VariantTag::Ok && arguments.len() == 1)
                    {
                        self.instructions.push(Instruction::Construct(
                            tag,
                            arguments.len(),
                            expression.span,
                        ));
                    } else {
                        return None;
                    }
                } else {
                    let function = self.functions.get(name.as_str())?.clone();
                    if function.arity != arguments.len() {
                        return None;
                    }
                    if let Some(inline) = function.inline {
                        if can_inline_parameter_from_stack(&inline) {
                            let parameter = &inline.parameters[0];
                            if let Some(parameter_type) = &parameter.type_annotation {
                                self.instructions.push(Instruction::RequireType(
                                    parameter_type.clone(),
                                    expression.span,
                                ));
                            }
                            self.stack_parameter = Some(parameter.name.name.clone());
                            self.stack_parameter_type = parameter
                                .type_annotation
                                .as_ref()
                                .and_then(primitive_from_annotation);
                            self.compile_expression(&inline.expression)?;
                            if self.stack_parameter.take().is_some() {
                                return None;
                            }
                            self.stack_parameter_type = None;
                        } else {
                            let mut scope = HashMap::with_capacity(inline.parameters.len());
                            for parameter in inline.parameters.iter().rev() {
                                let slot = self.slot_types.len();
                                self.slot_types.push(parameter.type_annotation.clone());
                                self.instructions
                                    .push(Instruction::Bind(slot, expression.span));
                                scope.insert(
                                    parameter.name.name.clone(),
                                    Local {
                                        slot,
                                        mutable: false,
                                        primitive: parameter
                                            .type_annotation
                                            .as_ref()
                                            .and_then(primitive_from_annotation),
                                    },
                                );
                            }
                            self.scopes.push(scope);
                            self.compile_expression(&inline.expression)?;
                            self.scopes.pop();
                        }
                        if let Some(return_type) = inline.return_type {
                            self.instructions
                                .push(Instruction::RequireType(return_type, expression.span));
                        }
                    } else {
                        self.instructions.push(Instruction::Call(
                            function.index,
                            arguments.len(),
                            expression.span,
                        ));
                    }
                }
            }
            ExpressionKind::Index { object, index } => {
                self.compile_expression(object)?;
                self.compile_expression(index)?;
                self.instructions.push(Instruction::Index(expression.span));
            }
            ExpressionKind::Member { object, field } => {
                self.compile_expression(object)?;
                self.instructions
                    .push(Instruction::Member(field.name.clone(), expression.span));
            }
            ExpressionKind::Match { value, arms } => {
                self.compile_expression(value)?;
                let subject = self.slot_types.len();
                self.slot_types.push(None);
                self.instructions
                    .push(Instruction::Store(subject, value.span));
                self.instructions.push(Instruction::Pop);
                let mut end_jumps = Vec::with_capacity(arms.len());
                for arm in arms {
                    self.instructions.push(Instruction::Load(subject));
                    self.scopes.push(HashMap::new());
                    let (pattern, binding) = compile_pattern(
                        &arm.pattern.kind,
                        &mut self.slot_types,
                        self.scopes.last_mut()?,
                    )?;
                    let pattern_index = self.emit(Instruction::MatchPattern {
                        pattern,
                        binding,
                        target: usize::MAX,
                    });
                    self.compile_expression(&arm.value)?;
                    self.scopes.pop();
                    end_jumps.push(self.emit(Instruction::Jump(usize::MAX)));
                    self.patch_jump(pattern_index, self.instructions.len());
                }
                self.instructions
                    .push(Instruction::MatchFail(expression.span));
                let end = self.instructions.len();
                for jump in end_jumps {
                    self.patch_jump(jump, end);
                }
            }
            ExpressionKind::Try(value) => {
                if !self.in_function {
                    return None;
                }
                self.compile_expression(value)?;
                self.instructions.push(Instruction::Try(expression.span));
            }
            ExpressionKind::Await(_) => return None,
        }
        Some(())
    }

    fn resolve(&self, name: &str) -> Option<Local> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn fuse_integer_slot_assignment(&mut self, start: usize, slot: usize, span: Span) -> bool {
        let tail = &self.instructions[start..];
        let fused = match tail {
            [
                Instruction::Load(loaded),
                Instruction::IntegerConstant(operator, constant, _),
            ] if *loaded == slot => Some(Instruction::IntegerSlotConstant(
                slot, *operator, *constant, span,
            )),
            [
                Instruction::Load(loaded),
                Instruction::RequireType(parameter_type, _),
                Instruction::IntegerConstant(operator, constant, _),
                Instruction::RequireType(return_type, _),
            ] if *loaded == slot
                && primitive_from_annotation(parameter_type) == Some(PrimitiveKind::Integer)
                && primitive_from_annotation(return_type) == Some(PrimitiveKind::Integer) =>
            {
                Some(Instruction::IntegerSlotConstant(
                    slot, *operator, *constant, span,
                ))
            }
            [
                Instruction::Load(loaded),
                Instruction::Load(right),
                Instruction::IntegerBinary(operator, _),
            ] if *loaded == slot => Some(Instruction::IntegerSlotBinary(
                slot, *operator, *right, span,
            )),
            _ => None,
        };
        let Some(instruction) = fused else {
            return false;
        };
        self.instructions.truncate(start);
        self.instructions.push(instruction);
        true
    }

    fn emit(&mut self, instruction: Instruction) -> usize {
        let index = self.instructions.len();
        self.instructions.push(instruction);
        index
    }

    fn patch_jump(&mut self, index: usize, target: usize) {
        match &mut self.instructions[index] {
            Instruction::JumpIfFalse(destination, _)
            | Instruction::JumpIfTrue(destination, _)
            | Instruction::Jump(destination)
            | Instruction::MatchPattern {
                target: destination,
                ..
            } => {
                *destination = target;
            }
            _ => unreachable!("compiler only patches jumps"),
        }
    }
}

fn constructor_tag(name: &str) -> Option<VariantTag> {
    match name {
        "Some" => Some(VariantTag::Some),
        "Ok" => Some(VariantTag::Ok),
        "Err" => Some(VariantTag::Err),
        _ => None,
    }
}

fn compile_pattern(
    pattern: &PatternKind,
    slot_types: &mut Vec<Option<TypeAnnotation>>,
    scope: &mut HashMap<String, Local>,
) -> Option<(CompiledPattern, Option<usize>)> {
    match pattern {
        PatternKind::Variant { name, binding } => {
            let tag = match name.as_str() {
                "Some" => VariantTag::Some,
                "None" => VariantTag::None,
                "Ok" => VariantTag::Ok,
                "Err" => VariantTag::Err,
                _ => return None,
            };
            let binding = binding.as_ref().map(|binding| {
                let slot = slot_types.len();
                slot_types.push(None);
                scope.insert(
                    binding.name.clone(),
                    Local {
                        slot,
                        mutable: false,
                        primitive: None,
                    },
                );
                slot
            });
            Some((CompiledPattern::Variant(tag), binding))
        }
        PatternKind::Literal(literal) => {
            Some((CompiledPattern::Literal(literal_value(literal)), None))
        }
        PatternKind::Wildcard => Some((CompiledPattern::Wildcard, None)),
    }
}

pub(super) fn execute(bytecode: &Bytecode) -> Result<Execution, RuntimeError> {
    execute_inner(bytecode, None)
}

pub(super) fn execute_with_cancellation(
    bytecode: &Bytecode,
    cancellation: &AtomicBool,
) -> Result<Execution, RuntimeError> {
    execute_inner(bytecode, Some(cancellation))
}

fn execute_inner(
    bytecode: &Bytecode,
    cancellation: Option<&AtomicBool>,
) -> Result<Execution, RuntimeError> {
    let mut slots = vec![Value::Null; bytecode.slot_types.len()];
    for (slot, value) in bytecode.initial_values.iter().cloned().enumerate() {
        slots[slot] = value;
    }
    let mut output = Vec::new();
    let mut heap = VmHeap::default();
    let value = execute_chunk(
        bytecode,
        &bytecode.instructions,
        &bytecode.slot_types,
        slots,
        cancellation,
        &mut output,
        &mut heap,
        0,
    )?;
    let collected_objects = collect_cycles(&value, &heap);

    Ok(Execution {
        output,
        value,
        collected_objects,
    })
}

#[allow(clippy::too_many_arguments)]
fn execute_chunk(
    bytecode: &Bytecode,
    instructions: &[Instruction],
    slot_types: &[Option<TypeAnnotation>],
    mut slots: Vec<Value>,
    cancellation: Option<&AtomicBool>,
    output: &mut Vec<String>,
    heap: &mut VmHeap,
    call_depth: usize,
) -> Result<Value, RuntimeError> {
    let mut stack = heap
        .stack_pool
        .pop()
        .unwrap_or_else(|| Vec::with_capacity(32));
    let result = execute_chunk_inner(
        bytecode,
        instructions,
        slot_types,
        &mut slots,
        cancellation,
        output,
        heap,
        call_depth,
        &mut stack,
    );
    stack.clear();
    heap.stack_pool.push(stack);
    slots.clear();
    heap.slot_pool.push(slots);
    result
}

#[allow(clippy::too_many_arguments)]
fn execute_chunk_inner(
    bytecode: &Bytecode,
    instructions: &[Instruction],
    slot_types: &[Option<TypeAnnotation>],
    slots: &mut [Value],
    cancellation: Option<&AtomicBool>,
    output: &mut Vec<String>,
    heap: &mut VmHeap,
    call_depth: usize,
    stack: &mut Vec<Value>,
) -> Result<Value, RuntimeError> {
    const MAX_CALL_DEPTH: usize = 128;
    if call_depth > MAX_CALL_DEPTH {
        return Err(error(
            "maximum bytecode call depth exceeded",
            Span::empty(0),
        ));
    }
    let mut instruction_pointer = 0;
    let mut executed_instructions = 0_usize;

    while let Some(instruction) = instructions.get(instruction_pointer) {
        if executed_instructions & 0xff == 0
            && cancellation.is_some_and(|token| token.load(Ordering::Acquire))
        {
            return Err(error("execution was cancelled", Span::empty(0)));
        }
        executed_instructions = executed_instructions.wrapping_add(1);
        instruction_pointer += 1;
        match instruction {
            Instruction::Push(value) => stack.push(value.clone()),
            Instruction::Load(slot) => stack.push(slots[*slot].clone()),
            Instruction::Store(slot, span) => {
                let value = stack.last().expect("verified bytecode stack");
                if let Some(type_annotation) = &slot_types[*slot]
                    && !value_matches_type(value, type_annotation)
                {
                    return Err(error(
                        format!(
                            "expected {}, received {}",
                            display_type(type_annotation),
                            value.type_name()
                        ),
                        *span,
                    ));
                }
                slots[*slot] = value.clone();
            }
            Instruction::Bind(slot, span) => {
                let value = stack.pop().expect("verified bytecode stack");
                if let Some(type_annotation) = &slot_types[*slot]
                    && !value_matches_type(&value, type_annotation)
                {
                    return Err(error(
                        format!(
                            "expected {}, received {}",
                            display_type(type_annotation),
                            value.type_name()
                        ),
                        *span,
                    ));
                }
                slots[*slot] = value;
            }
            Instruction::Pop => {
                stack.pop().expect("verified bytecode stack");
            }
            Instruction::Unary(operator, span) => {
                let value = stack.pop().expect("verified bytecode stack");
                stack.push(unary(*operator, value, *span)?);
            }
            Instruction::Binary(operator, span) => {
                let right = stack.pop().expect("verified bytecode stack");
                let left = stack.pop().expect("verified bytecode stack");
                stack.push(binary(*operator, left, right, *span)?);
            }
            Instruction::IntegerBinary(operator, span) => {
                let right = stack.pop().expect("verified bytecode stack");
                let left = stack.pop().expect("verified bytecode stack");
                let (Value::Integer(left), Value::Integer(right)) = (left, right) else {
                    return Err(error("integer instruction received a non-int value", *span));
                };
                stack.push(integer_binary(*operator, left, right, *span)?);
            }
            Instruction::IntegerConstant(operator, right, span) => {
                let left = stack.pop().expect("verified bytecode stack");
                let Value::Integer(left) = left else {
                    return Err(error("integer instruction received a non-int value", *span));
                };
                stack.push(integer_binary(*operator, left, *right, *span)?);
            }
            Instruction::IntegerSlotConstant(slot, operator, right, span) => {
                let Value::Integer(left) = &slots[*slot] else {
                    return Err(error("integer instruction received a non-int value", *span));
                };
                let result = integer_binary(*operator, *left, *right, *span)?;
                slots[*slot] = result.clone();
                stack.push(result);
            }
            Instruction::IntegerSlotBinary(left_slot, operator, right_slot, span) => {
                let (Value::Integer(left), Value::Integer(right)) =
                    (&slots[*left_slot], &slots[*right_slot])
                else {
                    return Err(error("integer instruction received a non-int value", *span));
                };
                let result = integer_binary(*operator, *left, *right, *span)?;
                slots[*left_slot] = result.clone();
                stack.push(result);
            }
            Instruction::JumpIfFalse(target, span) => {
                let value = stack.pop().expect("verified bytecode stack");
                let Value::Bool(condition) = value else {
                    return Err(error("condition must be bool", *span));
                };
                if !condition {
                    instruction_pointer = *target;
                }
            }
            Instruction::JumpIfTrue(target, span) => {
                let value = stack.pop().expect("verified bytecode stack");
                let Value::Bool(condition) = value else {
                    return Err(error("condition must be bool", *span));
                };
                if condition {
                    instruction_pointer = *target;
                }
            }
            Instruction::RequireBool(span) => {
                if !matches!(stack.last(), Some(Value::Bool(_))) {
                    return Err(error("condition must be bool", *span));
                }
            }
            Instruction::RequireType(expected, span) => {
                let value = stack.last().expect("verified bytecode stack");
                if !value_matches_type(value, expected) {
                    return Err(error(
                        format!(
                            "expected {}, received {}",
                            display_type(expected),
                            value.type_name()
                        ),
                        *span,
                    ));
                }
            }
            Instruction::Jump(target) => instruction_pointer = *target,
            Instruction::Print(argument_count) => {
                let start = stack.len() - argument_count;
                let line = stack[start..]
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ");
                stack.truncate(start);
                output.push(line);
                stack.push(Value::Void);
            }
            Instruction::Call(function, argument_count, span) => {
                let function = &bytecode.functions[*function];
                if *argument_count != function.parameters {
                    return Err(error(
                        format!(
                            "function '{}' expects {} arguments, received {}",
                            function.name, function.parameters, argument_count
                        ),
                        *span,
                    ));
                }
                let mut function_slots = heap.slot_pool.pop().unwrap_or_default();
                function_slots.resize(function.slot_types.len(), Value::Null);
                for index in (0..*argument_count).rev() {
                    let argument = stack.pop().expect("verified bytecode call stack");
                    if let Some(expected) = &function.slot_types[index]
                        && !value_matches_type(&argument, expected)
                    {
                        return Err(error(
                            format!(
                                "expected {}, received {}",
                                display_type(expected),
                                argument.type_name()
                            ),
                            *span,
                        ));
                    }
                    function_slots[index] = argument;
                }
                let result = execute_chunk(
                    bytecode,
                    &function.instructions,
                    &function.slot_types,
                    function_slots,
                    cancellation,
                    output,
                    heap,
                    call_depth + 1,
                )?;
                if let Some(expected) = &function.return_type
                    && !value_matches_type(&result, expected)
                {
                    return Err(error(
                        format!(
                            "expected {}, received {}",
                            display_type(expected),
                            result.type_name()
                        ),
                        *span,
                    ));
                }
                stack.push(result);
            }
            Instruction::Return => return Ok(stack.pop().unwrap_or(Value::Void)),
            Instruction::BuildList(length) => {
                let start = stack.len() - length;
                let values = stack.split_off(start);
                let values = Rc::new(RefCell::new(values));
                heap.lists.push(Rc::downgrade(&values));
                stack.push(Value::List(values));
            }
            Instruction::SnapshotList(span) => {
                let value = stack.pop().expect("verified bytecode stack");
                let Value::List(values) = value else {
                    return Err(error("for expects a list", *span));
                };
                let snapshot = Rc::new(RefCell::new(values.borrow().clone()));
                heap.lists.push(Rc::downgrade(&snapshot));
                stack.push(Value::List(snapshot));
            }
            Instruction::ListLength(span) => {
                let value = stack.pop().expect("verified bytecode stack");
                let Value::List(values) = value else {
                    return Err(error("for expects a list", *span));
                };
                let length = i64::try_from(values.borrow().len())
                    .map_err(|_| error("list is too large to iterate", *span))?;
                stack.push(Value::Integer(length));
            }
            Instruction::BuildMap(keys) => {
                let start = stack.len() - keys.len();
                let values = stack.split_off(start);
                let values = Rc::new(RefCell::new(keys.iter().cloned().zip(values).collect()));
                heap.maps.push(Rc::downgrade(&values));
                stack.push(Value::Map(values));
            }
            Instruction::Index(span) => {
                let index = stack.pop().expect("verified bytecode stack");
                let object = stack.pop().expect("verified bytecode stack");
                stack.push(read_index(object, index, *span)?);
            }
            Instruction::SetIndex(span) => {
                let index = stack.pop().expect("verified bytecode stack");
                let value = stack.pop().expect("verified bytecode stack");
                let object = stack.pop().expect("verified bytecode stack");
                write_index(object, index, value.clone(), *span)?;
                stack.push(value);
            }
            Instruction::Member(field, span) => {
                let object = stack.pop().expect("verified bytecode stack");
                stack.push(read_member(object, field, *span)?);
            }
            Instruction::SetMember(field, span) => {
                let value = stack.pop().expect("verified bytecode stack");
                let object = stack.pop().expect("verified bytecode stack");
                write_member(object, field, value.clone(), *span)?;
                stack.push(value);
            }
            Instruction::Construct(tag, argument_count, span) => {
                let payload = match argument_count {
                    0 if *tag == VariantTag::Ok => Some(Box::new(Value::Void)),
                    1 => Some(Box::new(
                        stack.pop().expect("verified bytecode constructor stack"),
                    )),
                    _ => return Err(error("invalid variant constructor arity", *span)),
                };
                stack.push(Value::Variant { tag: *tag, payload });
            }
            Instruction::Try(span) => {
                let value = stack.pop().expect("verified bytecode stack");
                match value {
                    Value::Variant {
                        tag: VariantTag::Some | VariantTag::Ok,
                        payload: Some(value),
                    } => stack.push(*value),
                    value @ Value::Variant {
                        tag: VariantTag::None | VariantTag::Err,
                        ..
                    } => return Ok(value),
                    _ => return Err(error("operator '?' requires Option or Result", *span)),
                }
            }
            Instruction::MatchPattern {
                pattern,
                binding,
                target,
            } => {
                let value = stack.pop().expect("verified bytecode stack");
                let (matches, payload) = match pattern {
                    CompiledPattern::Wildcard => (true, None),
                    CompiledPattern::Literal(expected) => (values_equal(&value, expected), None),
                    CompiledPattern::Variant(expected) => match value {
                        Value::Variant { tag, payload } if tag == *expected => (
                            binding.is_none() || payload.is_some(),
                            payload.map(|value| *value),
                        ),
                        _ => (false, None),
                    },
                };
                if matches {
                    if let Some(binding) = binding {
                        slots[*binding] = payload.expect("matched binding has a payload");
                    }
                } else {
                    instruction_pointer = *target;
                }
            }
            Instruction::MatchFail(span) => {
                return Err(error("match expression has no matching arm", *span));
            }
        }
    }
    Ok(stack.pop().unwrap_or(Value::Null))
}

fn read_index(object: Value, index: Value, span: Span) -> Result<Value, RuntimeError> {
    match object {
        Value::List(values) => {
            let index = list_index(index, span)?;
            let value = values.borrow().get(index).cloned();
            value.ok_or_else(|| error("list index is out of bounds", span))
        }
        Value::Map(values) => {
            let key = map_key(index, span)?;
            let value = values
                .borrow()
                .iter()
                .find(|(candidate, _)| candidate == &key)
                .map(|(_, value)| value.clone());
            value.ok_or_else(|| error(format!("map has no key '{key}'"), span))
        }
        _ => Err(error("value is not indexable", span)),
    }
}

fn collect_cycles(root: &Value, heap: &VmHeap) -> usize {
    let mut marked_environments = HashSet::new();
    let mut marked_values = HashSet::new();
    mark_value(root, &mut marked_environments, &mut marked_values);
    let mut collected = 0;
    for entry in &heap.lists {
        let Some(values) = entry.upgrade() else {
            continue;
        };
        let identity = Rc::as_ptr(&values) as usize;
        if !marked_values.contains(&identity) {
            values.borrow_mut().clear();
            collected += 1;
        }
    }
    for entry in &heap.maps {
        let Some(values) = entry.upgrade() else {
            continue;
        };
        let identity = Rc::as_ptr(&values) as usize;
        if !marked_values.contains(&identity) {
            values.borrow_mut().clear();
            collected += 1;
        }
    }
    collected
}

fn write_index(object: Value, index: Value, value: Value, span: Span) -> Result<(), RuntimeError> {
    match object {
        Value::List(values) => {
            let index = list_index(index, span)?;
            let mut values = values.borrow_mut();
            let slot = values
                .get_mut(index)
                .ok_or_else(|| error("list index is out of bounds", span))?;
            *slot = value;
            Ok(())
        }
        Value::Map(values) => {
            let key = map_key(index, span)?;
            let mut values = values.borrow_mut();
            if let Some((_, slot)) = values.iter_mut().find(|(candidate, _)| candidate == &key) {
                *slot = value;
            } else {
                values.push((key, value));
            }
            Ok(())
        }
        _ => Err(error("value is not indexable", span)),
    }
}

fn read_member(object: Value, field: &str, span: Span) -> Result<Value, RuntimeError> {
    let Value::Map(values) = object else {
        return Err(error("value has no fields", span));
    };
    let value = values
        .borrow()
        .iter()
        .find(|(candidate, _)| candidate == field)
        .map(|(_, value)| value.clone());
    value.ok_or_else(|| error(format!("map has no key '{field}'"), span))
}

fn write_member(object: Value, field: &str, value: Value, span: Span) -> Result<(), RuntimeError> {
    let Value::Map(values) = object else {
        return Err(error("value has no fields", span));
    };
    let mut values = values.borrow_mut();
    if let Some((_, slot)) = values.iter_mut().find(|(candidate, _)| candidate == field) {
        *slot = value;
    } else {
        values.push((field.into(), value));
    }
    Ok(())
}

fn list_index(value: Value, span: Span) -> Result<usize, RuntimeError> {
    match value {
        Value::Integer(value) => {
            usize::try_from(value).map_err(|_| error("list index must be non-negative", span))
        }
        _ => Err(error("list index must be int", span)),
    }
}

fn map_key(value: Value, span: Span) -> Result<String, RuntimeError> {
    match value {
        Value::String(value) => Ok(value),
        _ => Err(error("map index must be string", span)),
    }
}

fn literal_value(literal: &Literal) -> Value {
    match literal {
        Literal::Null => Value::Null,
        Literal::Bool(value) => Value::Bool(*value),
        Literal::Integer(value) => Value::Integer(*value),
        Literal::Float(value) => Value::Float(*value),
        Literal::String(value) => Value::String(value.clone()),
    }
}

fn unary(operator: UnaryOperator, value: Value, span: Span) -> Result<Value, RuntimeError> {
    match (operator, value) {
        (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (UnaryOperator::Negate, Value::Integer(value)) => value
            .checked_neg()
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
        (UnaryOperator::Not, _) => Err(error("operator '!' requires bool", span)),
        (UnaryOperator::Negate, _) => Err(error("unary '-' requires a number", span)),
    }
}

fn binary(
    operator: BinaryOperator,
    left: Value,
    right: Value,
    span: Span,
) -> Result<Value, RuntimeError> {
    match operator {
        BinaryOperator::Add => match (left, right) {
            (Value::String(left), Value::String(right)) => Ok(Value::String(left + &right)),
            (left, right) => numeric(left, right, span, i64::checked_add, |a, b| a + b),
        },
        BinaryOperator::Subtract => numeric(left, right, span, i64::checked_sub, |a, b| a - b),
        BinaryOperator::Multiply => numeric(left, right, span, i64::checked_mul, |a, b| a * b),
        BinaryOperator::Divide => divide(left, right, span),
        BinaryOperator::Remainder => remainder(left, right, span),
        BinaryOperator::Equal => Ok(Value::Bool(values_equal(&left, &right))),
        BinaryOperator::NotEqual => Ok(Value::Bool(!values_equal(&left, &right))),
        BinaryOperator::Less => compare(left, right, span, |a, b| a < b),
        BinaryOperator::LessEqual => compare(left, right, span, |a, b| a <= b),
        BinaryOperator::Greater => compare(left, right, span, |a, b| a > b),
        BinaryOperator::GreaterEqual => compare(left, right, span, |a, b| a >= b),
        BinaryOperator::And | BinaryOperator::Or => {
            Err(error("invalid logical operator state", span))
        }
    }
}

fn integer_binary(
    operator: BinaryOperator,
    left: i64,
    right: i64,
    span: Span,
) -> Result<Value, RuntimeError> {
    match operator {
        BinaryOperator::Add => left
            .checked_add(right)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        BinaryOperator::Subtract => left
            .checked_sub(right)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        BinaryOperator::Multiply => left
            .checked_mul(right)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        BinaryOperator::Divide if right == 0 => Err(error("division by zero", span)),
        BinaryOperator::Divide => left
            .checked_div(right)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        BinaryOperator::Remainder if right == 0 => Err(error("division by zero", span)),
        BinaryOperator::Remainder => left
            .checked_rem(right)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        BinaryOperator::Equal => Ok(Value::Bool(left == right)),
        BinaryOperator::NotEqual => Ok(Value::Bool(left != right)),
        BinaryOperator::Less => Ok(Value::Bool(left < right)),
        BinaryOperator::LessEqual => Ok(Value::Bool(left <= right)),
        BinaryOperator::Greater => Ok(Value::Bool(left > right)),
        BinaryOperator::GreaterEqual => Ok(Value::Bool(left >= right)),
        BinaryOperator::And | BinaryOperator::Or => {
            Err(error("invalid integer operator state", span))
        }
    }
}

fn numeric(
    left: Value,
    right: Value,
    span: Span,
    integer: fn(i64, i64) -> Option<i64>,
    float: fn(f64, f64) -> f64,
) -> Result<Value, RuntimeError> {
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => integer(a, b)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        (Value::Integer(a), Value::Float(b)) => Ok(Value::Float(float(a as f64, b))),
        (Value::Float(a), Value::Integer(b)) => Ok(Value::Float(float(a, b as f64))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Float(float(a, b))),
        _ => Err(error("operator requires numbers", span)),
    }
}

fn divide(left: Value, right: Value, span: Span) -> Result<Value, RuntimeError> {
    if matches!(right, Value::Integer(0)) || matches!(right, Value::Float(value) if value == 0.0) {
        return Err(error("division by zero", span));
    }
    match (left, right) {
        (Value::Integer(a), Value::Integer(b)) => a
            .checked_div(b)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        (left, right) => numeric(left, right, span, i64::checked_div, |a, b| a / b),
    }
}

fn remainder(left: Value, right: Value, span: Span) -> Result<Value, RuntimeError> {
    match (left, right) {
        (_, Value::Integer(0)) => Err(error("division by zero", span)),
        (Value::Integer(a), Value::Integer(b)) => a
            .checked_rem(b)
            .map(Value::Integer)
            .ok_or_else(|| error("integer overflow", span)),
        _ => Err(error("operator '%' requires integers", span)),
    }
}

fn compare(
    left: Value,
    right: Value,
    span: Span,
    operation: fn(f64, f64) -> bool,
) -> Result<Value, RuntimeError> {
    let left = number(left).ok_or_else(|| error("comparison requires numbers", span))?;
    let right = number(right).ok_or_else(|| error("comparison requires numbers", span))?;
    Ok(Value::Bool(operation(left, right)))
}

fn number(value: Value) -> Option<f64> {
    match value {
        Value::Integer(value) => Some(value as f64),
        Value::Float(value) => Some(value),
        _ => None,
    }
}

fn error(message: impl Into<String>, span: Span) -> RuntimeError {
    RuntimeError {
        message: message.into(),
        span,
        propagated: None,
    }
}

#[cfg(test)]
mod tests {
    use orvi_syntax::parse;

    use super::{compile, compile_worker_cached, execute};
    use crate::Value;

    fn run(source: &str) -> Vec<String> {
        let parsed = parse(source);
        assert!(!parsed.has_errors());
        let bytecode = compile(&parsed.program).expect("source should use the bytecode fast path");
        execute(&bytecode).expect("bytecode should execute").output
    }

    #[test]
    fn executes_loops_branches_and_shadowed_locals() {
        let output = run(
            "let index = 0; let total = 0; while index < 10 { let value = index; total = total + value; index = index + 1; } if total == 45 { let total = 7; print(total); } print(total);",
        );
        assert_eq!(output, ["7", "45"]);
    }

    #[test]
    fn executes_for_loops_on_the_bytecode_fast_path() {
        let source = "let total = 0; for value in [2, 3, 5] { total = total + value; } for index in 1..=3 { total = total + index; } for skipped in 5..2 { total = total + skipped; } for edge in 9223372036854775807..=9223372036854775807 { total = total + 1; } print(total); let values = [1, 2]; let snapshot_total = 0; for value in values { values[1] = 9; snapshot_total = snapshot_total + value; } print(snapshot_total);";
        let parsed = parse(source);
        assert!(!parsed.has_errors(), "{:?}", parsed.parse_errors);
        let bytecode = compile(&parsed.program).expect("for loops compile to bytecode");
        assert!(
            bytecode
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, super::Instruction::ListLength(_)))
        );
        assert_eq!(execute(&bytecode).unwrap().output, ["17", "3"]);
    }

    #[test]
    fn compiles_nested_break_and_continue_to_bytecode_jumps() {
        let output = run(
            "let total = 0; for outer in 0..3 { for inner in 0..5 { if inner == 1 { continue; } if inner == 3 { break; } total = total + 1; } } print(total); let index = 0; let hits = 0; while true { index = index + 1; if index < 3 { continue; } hits = hits + 1; if index == 4 { break; } } print(hits);",
        );
        assert_eq!(output, ["6", "2"]);
    }

    #[test]
    fn supports_primitive_operations_and_reports_runtime_errors() {
        assert_eq!(
            run("print(2 * 3 + 1); print(\"orvi\" + \"script\");"),
            ["7", "orviscript"]
        );

        let parsed = parse("print(1 / 0);");
        let error = execute(&compile(&parsed.program).unwrap()).unwrap_err();
        assert_eq!(error.message, "division by zero");
    }

    #[test]
    fn specializes_integer_operations_without_changing_results() {
        let parsed = parse(
            "let left = 20; let right = 3; print(left + 2); print(left + right); print(left - right); print(left * right); print(left / right); print(left % right); print(left == right); print(left != right); print(left < right); print(left <= right); print(left > right); print(left >= right); print(left + -2); left = left + 2; print(left); left = left + right; print(left); print(\"orvi\" + \"script\");",
        );
        let bytecode = compile(&parsed.program).expect("integer operations compile");
        assert!(
            bytecode
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, super::Instruction::IntegerConstant(..)))
        );
        assert!(
            bytecode
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, super::Instruction::IntegerBinary(..)))
        );
        assert!(
            bytecode.instructions.iter().any(|instruction| matches!(
                instruction,
                super::Instruction::IntegerSlotConstant(..)
            ))
        );
        assert!(
            bytecode.instructions.iter().any(|instruction| matches!(
                instruction,
                super::Instruction::IntegerSlotBinary(..)
            ))
        );
        assert!(
            bytecode
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, super::Instruction::Binary(..)))
        );
        assert_eq!(
            execute(&bytecode).unwrap().output,
            [
                "22",
                "23",
                "17",
                "60",
                "6",
                "2",
                "false",
                "true",
                "false",
                "false",
                "true",
                "true",
                "18",
                "22",
                "25",
                "orviscript"
            ]
        );

        let parsed = parse("print(9223372036854775807 + 1);");
        let error = execute(&compile(&parsed.program).unwrap()).unwrap_err();
        assert_eq!(error.message, "integer overflow");
    }

    #[test]
    fn supports_typed_slots_and_short_circuit_boolean_operators() {
        assert_eq!(
            run(
                "let value: int = 4; value = value + 1; print(value); print(false && (1 / 0 == 0)); print(true || (1 / 0 == 0));"
            ),
            ["5", "false", "true"]
        );
    }

    #[test]
    fn executes_typed_functions_recursion_and_early_returns() {
        assert_eq!(
            run(
                "fn factorial(value: int) -> int { if value <= 1 { return 1; } return value * factorial(value - 1); } fn classify(value: int) -> string { if value > 0 { return \"positive\"; } return \"other\"; } print(factorial(10)); print(classify(7));"
            ),
            ["3628800", "positive"]
        );
    }

    #[test]
    fn inlines_safe_leaf_functions_without_changing_call_semantics() {
        let parsed = parse(
            "fn combine(left: int, right: int) -> int { return left * 10 + right; } fn twice(value: int) -> int { return value + value; } let left = 99; let counter = 0; print(combine(2, 3)); print(twice(counter = counter + 1)); print(counter); print(left);",
        );
        assert!(!parsed.has_errors());
        let bytecode = compile(&parsed.program).expect("leaf functions compile");
        assert!(
            !bytecode
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, super::Instruction::Call(..))),
            "eligible leaf calls should be expanded into the caller"
        );
        assert_eq!(
            execute(&bytecode).expect("inlined calls execute").output,
            ["23", "2", "1", "99"]
        );
    }

    #[test]
    fn keeps_complex_functions_as_calls_and_checks_inlined_types() {
        let parsed = parse(
            "fn step(value: int) -> int { let next = value + 1; return next; } print(step(41));",
        );
        let bytecode = compile(&parsed.program).expect("complex function compiles");
        assert!(
            bytecode
                .instructions
                .iter()
                .any(|instruction| matches!(instruction, super::Instruction::Call(..)))
        );
        assert_eq!(execute(&bytecode).unwrap().output, ["42"]);

        let parsed =
            parse("fn identity(value: int) -> string { return value; } print(identity(1));");
        let error = execute(&compile(&parsed.program).expect("inline type check compiles"))
            .expect_err("return type must still be enforced after inlining");
        assert_eq!(error.message, "expected string, received int");

        let parsed =
            parse("fn increment(value: int) -> int { return value + 1; } print(increment(\"x\"));");
        let error = execute(&compile(&parsed.program).expect("inline argument check compiles"))
            .expect_err("parameter type must still be enforced after inlining");
        assert_eq!(error.message, "expected int, received string");
    }

    #[test]
    fn rejects_excessive_bytecode_recursion() {
        let parsed = parse("fn recurse() -> void { recurse(); } recurse();");
        let error = execute(&compile(&parsed.program).expect("function compiles"))
            .expect_err("recursion limit is enforced");
        assert_eq!(error.message, "maximum bytecode call depth exceeded");
    }

    #[test]
    fn executes_lists_maps_indexing_fields_and_mutation() {
        assert_eq!(
            run(
                "let values: list<int> = [1, 2, 3]; values[1] = 7; let record: map<int> = { answer: 40 }; record.answer = record[\"answer\"] + 2; record[\"extra\"] = values[1]; print(values); print(record.answer); print(record.extra); print(values == [1, 7, 3]);"
            ),
            ["[1, 7, 3]", "42", "7", "true"]
        );
    }

    #[test]
    fn executes_variants_match_and_error_propagation() {
        assert_eq!(
            run(
                "fn checked(value: int) -> Result<int, string> { if value < 0 { return Err(\"negative\"); } return Ok(value); } fn doubled(value: int) -> Result<int, string> { let checked_value = checked(value)?; return Ok(checked_value * 2); } print(doubled(21)); print(doubled(-1)); print(match Some(5) { Some(value) => value * 2, None => 0 }); print(match Err(\"failed\") { Ok(value) => value, Err(message) => message });"
            ),
            ["Ok(42)", "Err(negative)", "10", "failed"]
        );
    }

    #[test]
    fn caches_worker_chunks_without_caching_worker_input() {
        let source = "worker_input + 1";
        let parsed = parse(source);
        let first = compile_worker_cached(source, &parsed.program, Value::Integer(1))
            .expect("worker source compiles");
        let second = compile_worker_cached(source, &parsed.program, Value::Integer(41))
            .expect("worker source is loaded from cache");
        assert_eq!(execute(&first).unwrap().value.to_string(), "2");
        assert_eq!(execute(&second).unwrap().value.to_string(), "42");
    }
}
