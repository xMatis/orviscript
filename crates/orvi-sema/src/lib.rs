use std::collections::HashMap;
use std::fmt;

use orvi_syntax::{
    BinaryOperator, Expression, ExpressionKind, Identifier, Literal, Parameter, Program, Span,
    Statement, StatementKind, TypeAnnotation, UnaryOperator,
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
    Null,
    Bool,
    Int,
    Float,
    Number,
    String,
    Function {
        parameters: Vec<StaticType>,
        result: Box<StaticType>,
    },
}

pub fn analyze(program: &Program) -> Vec<SemanticError> {
    analyze_program(program).errors
}

pub fn analyze_program(program: &Program) -> Analysis {
    let mut analyzer = Analyzer {
        scopes: vec![HashMap::from([(
            "print".into(),
            Symbol {
                kind: SymbolKind::Function,
                mutable: false,
                arity: None,
                definition: None,
                value_type: StaticType::Function {
                    parameters: Vec::new(),
                    result: Box::new(StaticType::Null),
                },
            },
        )])],
        errors: Vec::new(),
        symbols: Vec::new(),
        function_depth: 0,
        current_return_type: None,
    };
    analyzer.statements(&program.statements, false);
    Analysis {
        errors: analyzer.errors,
        symbols: analyzer.symbols,
    }
}

struct Analyzer {
    scopes: Vec<HashMap<String, Symbol>>,
    errors: Vec<SemanticError>,
    symbols: Vec<SymbolOccurrence>,
    function_depth: usize,
    current_return_type: Option<StaticType>,
}

impl Analyzer {
    fn statements(&mut self, statements: &[Statement], create_scope: bool) {
        if create_scope {
            self.scopes.push(HashMap::new());
        }

        for statement in statements {
            if let StatementKind::Function {
                name,
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
                let result = self
                    .annotation_type(return_type.as_ref())
                    .unwrap_or(StaticType::Dynamic);
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
            if !matches!(&statement.kind, StatementKind::Function { .. }) {
                self.statement(statement);
            }
        }

        for statement in statements {
            if let StatementKind::Function {
                parameters,
                return_type,
                body,
                ..
            } = &statement.kind
            {
                self.function(parameters, return_type.as_ref(), body);
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
                    .map_or(StaticType::Null, |value| self.expression(value));
                if let Some(expected) = self.current_return_type.clone() {
                    self.require_assignable(&expected, &actual, statement.span);
                }
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
    ) {
        self.scopes.push(HashMap::new());
        for parameter in parameters {
            let value_type = parameter
                .type_annotation
                .as_ref()
                .and_then(static_type_from_annotation)
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
        self.current_return_type = return_type.and_then(static_type_from_annotation);
        self.function_depth += 1;
        self.statements(body, false);
        self.function_depth -= 1;
        self.current_return_type = previous_return_type;
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
                let callee_type = self.expression(callee);
                let argument_types = arguments
                    .iter()
                    .map(|argument| self.expression(argument))
                    .collect::<Vec<_>>();
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

    fn undefined(&mut self, name: &Identifier) {
        self.errors.push(SemanticError {
            kind: SemanticErrorKind::UndefinedName,
            message: format!("undefined name '{}'", name.name),
            span: name.span,
        });
    }

    fn annotation_type(&mut self, annotation: Option<&TypeAnnotation>) -> Option<StaticType> {
        let annotation = annotation?;
        let Some(value_type) = static_type_from_annotation(annotation) else {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::UnknownType,
                message: format!("unknown type '{}'", annotation.name.name),
                span: annotation.span,
            });
            return Some(StaticType::Dynamic);
        };
        if !annotation.arguments.is_empty() {
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
        if !expected.accepts(actual) {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::TypeMismatch,
                message: format!(
                    "expected {}, received {}",
                    expected.display_name(),
                    actual.display_name()
                ),
                span,
            });
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
        self == &Self::Dynamic
            || actual == &Self::Dynamic
            || self == actual
            || (*self == Self::Number && actual.is_numeric())
    }

    fn display_name(&self) -> String {
        match self {
            Self::Dynamic => "any".into(),
            Self::Null => "null".into(),
            Self::Bool => "bool".into(),
            Self::Int => "int".into(),
            Self::Float => "float".into(),
            Self::Number => "number".into(),
            Self::String => "string".into(),
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
}

fn static_type_from_annotation(annotation: &TypeAnnotation) -> Option<StaticType> {
    match annotation.name.as_str() {
        "any" => Some(StaticType::Dynamic),
        "null" => Some(StaticType::Null),
        "bool" => Some(StaticType::Bool),
        "int" => Some(StaticType::Int),
        "float" => Some(StaticType::Float),
        "number" => Some(StaticType::Number),
        "string" => Some(StaticType::String),
        "fn" => Some(StaticType::Function {
            parameters: Vec::new(),
            result: Box::new(StaticType::Dynamic),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{SemanticErrorKind, analyze, analyze_program};
    use orvi_syntax::parse;

    fn errors(source: &str) -> Vec<super::SemanticError> {
        let parsed = parse(source);
        assert!(!parsed.has_errors(), "test source must parse");
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
}
