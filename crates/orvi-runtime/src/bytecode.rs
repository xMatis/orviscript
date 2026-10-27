use std::collections::HashMap;

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, Literal, Program, Span, Statement, StatementKind,
    TypeAnnotation, UnaryOperator,
};

use super::{Execution, RuntimeError, Value, display_type, value_matches_type};

pub(super) struct Bytecode {
    instructions: Vec<Instruction>,
    slot_types: Vec<Option<TypeAnnotation>>,
}

#[derive(Clone)]
enum Instruction {
    Push(Value),
    Load(usize),
    Store(usize, Span),
    Pop,
    Unary(UnaryOperator, Span),
    Binary(BinaryOperator, Span),
    JumpIfFalse(usize, Span),
    JumpIfTrue(usize, Span),
    RequireBool(Span),
    Jump(usize),
    Print(usize),
}

#[derive(Clone, Copy)]
struct Local {
    slot: usize,
    mutable: bool,
}

struct Compiler {
    instructions: Vec<Instruction>,
    scopes: Vec<HashMap<String, Local>>,
    slot_types: Vec<Option<TypeAnnotation>>,
}

pub(super) fn compile(program: &Program) -> Option<Bytecode> {
    let mut compiler = Compiler {
        instructions: Vec::new(),
        scopes: vec![HashMap::new()],
        slot_types: Vec::new(),
    };
    compiler.compile_statements(&program.statements)?;
    Some(Bytecode {
        instructions: compiler.instructions,
        slot_types: compiler.slot_types,
    })
}

impl Compiler {
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
                self.compile_expression(initializer)?;
                let slot = self.slot_types.len();
                self.slot_types.push(type_annotation.clone());
                self.scopes.last_mut()?.insert(
                    name.name.clone(),
                    Local {
                        slot,
                        mutable: *mutable,
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
                self.compile_statement(body)?;
                self.instructions.push(Instruction::Pop);
                self.instructions.push(Instruction::Jump(loop_start));
                self.patch_jump(exit_jump, self.instructions.len());
                self.instructions.push(Instruction::Push(Value::Null));
            }
            StatementKind::Expression(expression) => self.compile_expression(expression)?,
            StatementKind::Function { .. }
            | StatementKind::Struct { .. }
            | StatementKind::Interface { .. }
            | StatementKind::Import { .. }
            | StatementKind::Export(_)
            | StatementKind::Task { .. }
            | StatementKind::Return(_)
            | StatementKind::Defer(_)
            | StatementKind::Using { .. } => return None,
        }
        Some(())
    }

    fn compile_expression(&mut self, expression: &Expression) -> Option<()> {
        match &expression.kind {
            ExpressionKind::Literal(literal) => self
                .instructions
                .push(Instruction::Push(literal_value(literal))),
            ExpressionKind::Variable(name) => {
                let local = self.resolve(&name.name)?;
                self.instructions.push(Instruction::Load(local.slot));
            }
            ExpressionKind::Assign { name, value } => {
                let local = self.resolve(&name.name)?;
                if !local.mutable {
                    return None;
                }
                self.compile_expression(value)?;
                self.instructions
                    .push(Instruction::Store(local.slot, expression.span));
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
                        self.compile_expression(right)?;
                        self.instructions
                            .push(Instruction::Binary(*operator, expression.span));
                    }
                }
            }
            ExpressionKind::Call { callee, arguments } => {
                let ExpressionKind::Variable(name) = &callee.kind else {
                    return None;
                };
                if name.name != "print" || self.resolve("print").is_some() {
                    return None;
                }
                for argument in arguments {
                    self.compile_expression(argument)?;
                }
                self.instructions.push(Instruction::Print(arguments.len()));
            }
            ExpressionKind::List(_)
            | ExpressionKind::Map(_)
            | ExpressionKind::IndexAssign { .. }
            | ExpressionKind::MemberAssign { .. }
            | ExpressionKind::Index { .. }
            | ExpressionKind::Member { .. }
            | ExpressionKind::Match { .. }
            | ExpressionKind::Try(_)
            | ExpressionKind::Await(_) => return None,
        }
        Some(())
    }

    fn resolve(&self, name: &str) -> Option<Local> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
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
            | Instruction::Jump(destination) => {
                *destination = target;
            }
            _ => unreachable!("compiler only patches jumps"),
        }
    }
}

pub(super) fn execute(bytecode: &Bytecode) -> Result<Execution, RuntimeError> {
    let mut stack = Vec::with_capacity(32);
    let mut slots = vec![Value::Null; bytecode.slot_types.len()];
    let mut output = Vec::new();
    let mut instruction_pointer = 0;

    while let Some(instruction) = bytecode.instructions.get(instruction_pointer) {
        instruction_pointer += 1;
        match instruction {
            Instruction::Push(value) => stack.push(value.clone()),
            Instruction::Load(slot) => stack.push(slots[*slot].clone()),
            Instruction::Store(slot, span) => {
                let value = stack.last().expect("verified bytecode stack");
                if let Some(type_annotation) = &bytecode.slot_types[*slot]
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
        }
    }

    Ok(Execution {
        output,
        value: stack.pop().unwrap_or(Value::Null),
        collected_objects: 0,
    })
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
        BinaryOperator::Equal => Ok(Value::Bool(primitive_equal(&left, &right))),
        BinaryOperator::NotEqual => Ok(Value::Bool(!primitive_equal(&left, &right))),
        BinaryOperator::Less => compare(left, right, span, |a, b| a < b),
        BinaryOperator::LessEqual => compare(left, right, span, |a, b| a <= b),
        BinaryOperator::Greater => compare(left, right, span, |a, b| a > b),
        BinaryOperator::GreaterEqual => compare(left, right, span, |a, b| a >= b),
        BinaryOperator::And | BinaryOperator::Or => {
            Err(error("invalid logical operator state", span))
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

fn primitive_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Void, Value::Void) | (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Integer(a), Value::Float(b)) => *a as f64 == *b,
        (Value::Float(a), Value::Integer(b)) => *a == *b as f64,
        (Value::String(a), Value::String(b)) => a == b,
        _ => false,
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

    use super::{compile, execute};

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
    fn supports_typed_slots_and_short_circuit_boolean_operators() {
        assert_eq!(
            run(
                "let value: int = 4; value = value + 1; print(value); print(false && (1 / 0 == 0)); print(true || (1 / 0 == 0));"
            ),
            ["5", "false", "true"]
        );
    }
}
