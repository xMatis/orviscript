use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::rc::{Rc, Weak};

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, Literal, Parameter, Program, Span, Statement,
    StatementKind, StructField, TypeAnnotation, UnaryOperator,
};

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    List(Rc<RefCell<Vec<Value>>>),
    StructType(Rc<StructDefinition>),
    StructInstance(Rc<RefCell<StructInstance>>),
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
    closure: Environment,
}

pub struct StructInstance {
    definition: Rc<StructDefinition>,
    fields: HashMap<String, Value>,
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
    let globals = Environment::root();
    globals.define("print".into(), Value::NativePrint, false, None);
    let mut interpreter = Interpreter {
        environment: globals.clone(),
        output: Vec::new(),
        function_depth: 0,
        environments: vec![Rc::downgrade(&globals.0)],
        lists: Vec::new(),
        objects: Vec::new(),
        collected_objects: 0,
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
    interpreter.collect_finished(std::slice::from_ref(&value));
    Ok(Execution {
        output: interpreter.output,
        value,
        collected_objects: interpreter.collected_objects,
    })
}

struct Interpreter {
    environment: Environment,
    output: Vec<String>,
    function_depth: usize,
    environments: Vec<Weak<RefCell<EnvironmentData>>>,
    lists: Vec<Weak<RefCell<Vec<Value>>>>,
    objects: Vec<Weak<RefCell<StructInstance>>>,
    collected_objects: usize,
}

impl Interpreter {
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
                let definition = StructDefinition {
                    name: name.name.clone(),
                    fields: fields.clone(),
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
            ExpressionKind::List(values) => {
                let values = values
                    .iter()
                    .map(|value| self.evaluate(value))
                    .collect::<Result<Vec<_>, _>>()?;
                let values = Rc::new(RefCell::new(values));
                self.lists.push(Rc::downgrade(&values));
                Ok(Value::List(values))
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
                let index = self.list_index(index)?;
                let value = self.evaluate(value)?;
                let Value::List(values) = object else {
                    return Err(self.error("value is not indexable", expression.span));
                };
                let mut values = values.borrow_mut();
                let Some(slot) = values.get_mut(index) else {
                    return Err(self.error("list index is out of bounds", expression.span));
                };
                *slot = value.clone();
                Ok(value)
            }
            ExpressionKind::MemberAssign {
                object,
                field,
                value,
            } => {
                let object = self.evaluate(object)?;
                let value = self.evaluate(value)?;
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
                let index = self.list_index(index)?;
                let Value::List(values) = object else {
                    return Err(self.error("value is not indexable", expression.span));
                };
                let value =
                    values.borrow().get(index).cloned().ok_or_else(|| {
                        self.error("list index is out of bounds", expression.span)
                    })?;
                Ok(value)
            }
            ExpressionKind::Member { object, field } => {
                let object = self.evaluate(object)?;
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
                let value = match result? {
                    Control::Continue(_) => Value::Null,
                    Control::Return(value) => value,
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
        for value in temporary_roots {
            mark_value(value, &mut marked_environments, &mut marked_lists);
        }

        self.sweep(&marked_environments, &marked_lists);
    }

    fn collect_finished(&mut self, result_roots: &[Value]) {
        let mut marked_environments = HashSet::new();
        let mut marked_lists = HashSet::new();
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
        }
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
            Self::Function(_) | Self::NativePrint => "fn".into(),
            Self::StructType(definition) => format!("type {}", definition.name),
            Self::StructInstance(instance) => instance.borrow().definition.name.clone(),
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
        "fn" => matches!(value, Value::Function(_) | Value::NativePrint),
        name => matches!(
            value,
            Value::StructInstance(instance) if instance.borrow().definition.name == name
        ),
    }
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
        (Value::StructType(a), Value::StructType(b)) => Rc::ptr_eq(a, b),
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
        Value::StructType(definition) => format!("<struct {}>", definition.name),
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
        Value::Function(function) => format!("<fn {}>", function.name),
        Value::NativePrint => "<native fn print>".into(),
    }
}
