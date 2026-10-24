use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, Literal, Program, Span, Statement, StatementKind,
    UnaryOperator,
};

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
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
        match self {
            Self::Null => formatter.write_str("null"),
            Self::Bool(value) => write!(formatter, "{value}"),
            Self::Integer(value) => write!(formatter, "{value}"),
            Self::Float(value) => write!(formatter, "{value}"),
            Self::String(value) => formatter.write_str(value),
            Self::Function(function) => write!(formatter, "<fn {}>", function.name),
            Self::NativePrint => formatter.write_str("<native fn print>"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RuntimeError {
    pub message: String,
    pub span: Span,
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
}

#[derive(Clone)]
pub struct UserFunction {
    name: String,
    parameters: Vec<String>,
    body: Vec<Statement>,
    closure: Environment,
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
    fn define(&self, name: String, value: Value, mutable: bool) {
        self.0
            .borrow_mut()
            .values
            .insert(name, Binding { value, mutable });
    }
    fn get(&self, name: &str) -> Option<Value> {
        let data = self.0.borrow();
        data.values
            .get(name)
            .map(|binding| binding.value.clone())
            .or_else(|| data.parent.as_ref().and_then(|parent| parent.get(name)))
    }
    fn assign(&self, name: &str, value: Value) -> Result<(), AssignError> {
        let parent = {
            let mut data = self.0.borrow_mut();
            if let Some(binding) = data.values.get_mut(name) {
                if !binding.mutable {
                    return Err(AssignError::Immutable);
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
}

enum AssignError {
    Undefined,
    Immutable,
}
enum Control {
    Continue(Value),
    Return(Value),
}

pub fn execute(program: &Program) -> Result<Execution, RuntimeError> {
    let globals = Environment::root();
    globals.define("print".into(), Value::NativePrint, false);
    let mut interpreter = Interpreter {
        environment: globals,
        output: Vec::new(),
        function_depth: 0,
    };
    let value = match interpreter.execute_statements(&program.statements)? {
        Control::Continue(value) => value,
        Control::Return(_) => {
            return Err(RuntimeError {
                message: "return is only valid inside a function".into(),
                span: Span::empty(0),
            });
        }
    };
    Ok(Execution {
        output: interpreter.output,
        value,
    })
}

struct Interpreter {
    environment: Environment,
    output: Vec<String>,
    function_depth: usize,
}

impl Interpreter {
    fn execute_statements(&mut self, statements: &[Statement]) -> Result<Control, RuntimeError> {
        let mut value = Value::Null;
        for statement in statements {
            match self.execute_statement(statement)? {
                Control::Continue(next) => value = next,
                returned @ Control::Return(_) => return Ok(returned),
            }
        }
        Ok(Control::Continue(value))
    }

    fn execute_statement(&mut self, statement: &Statement) -> Result<Control, RuntimeError> {
        match &statement.kind {
            StatementKind::Variable {
                name,
                mutable,
                initializer,
            } => {
                let value = self.evaluate(initializer)?;
                self.environment
                    .define(name.clone(), value.clone(), *mutable);
                Ok(Control::Continue(value))
            }
            StatementKind::Function {
                name,
                parameters,
                body,
            } => {
                let function = UserFunction {
                    name: name.clone(),
                    parameters: parameters.clone(),
                    body: body.clone(),
                    closure: self.environment.clone(),
                };
                self.environment
                    .define(name.clone(), Value::Function(Rc::new(function)), false);
                Ok(Control::Continue(Value::Null))
            }
            StatementKind::Block(statements) => {
                self.execute_block(statements, Environment::child(self.environment.clone()))
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
        let result = self.execute_statements(statements);
        self.environment = previous;
        result
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
            ExpressionKind::Variable(name) => self
                .environment
                .get(name)
                .ok_or_else(|| self.error(format!("undefined variable '{name}'"), expression.span)),
            ExpressionKind::Assign { name, value } => {
                let value = self.evaluate(value)?;
                self.environment
                    .assign(name, value.clone())
                    .map_err(|error| match error {
                        AssignError::Undefined => {
                            self.error(format!("undefined variable '{name}'"), expression.span)
                        }
                        AssignError::Immutable => self.error(
                            format!("cannot assign to constant '{name}'"),
                            expression.span,
                        ),
                    })?;
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
                let environment = Environment::child(function.closure.clone());
                for (name, value) in function.parameters.iter().cloned().zip(arguments) {
                    environment.define(name, value, true);
                }
                self.function_depth += 1;
                let result = self.execute_block(&function.body, environment);
                self.function_depth -= 1;
                match result? {
                    Control::Continue(_) => Ok(Value::Null),
                    Control::Return(value) => Ok(value),
                }
            }
            _ => Err(self.error("value is not callable", span)),
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
    fn error(&self, message: impl Into<String>, span: Span) -> RuntimeError {
        RuntimeError {
            message: message.into(),
            span,
        }
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
    match (left, right) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Integer(a), Value::Integer(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Integer(a), Value::Float(b)) => *a as f64 == *b,
        (Value::Float(a), Value::Integer(b)) => *a == *b as f64,
        (Value::String(a), Value::String(b)) => a == b,
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
        (Value::NativePrint, Value::NativePrint) => true,
        _ => false,
    }
}
