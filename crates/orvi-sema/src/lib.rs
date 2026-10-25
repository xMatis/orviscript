use std::collections::HashMap;
use std::fmt;

use orvi_syntax::{Expression, ExpressionKind, Program, Span, Statement, StatementKind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticErrorKind {
    UndefinedName,
    DuplicateName,
    ImmutableAssignment,
    ReturnOutsideFunction,
    ArgumentCount,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticError {
    pub kind: SemanticErrorKind,
    pub message: String,
    pub span: Span,
}

impl fmt::Display for SemanticError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Copy, Debug)]
enum Symbol {
    Variable { mutable: bool },
    Function { arity: usize },
    VariadicFunction,
}

pub fn analyze(program: &Program) -> Vec<SemanticError> {
    let mut analyzer = Analyzer {
        scopes: vec![HashMap::from([("print".into(), Symbol::VariadicFunction)])],
        errors: Vec::new(),
        function_depth: 0,
    };
    analyzer.statements(&program.statements, false);
    analyzer.errors
}

struct Analyzer {
    scopes: Vec<HashMap<String, Symbol>>,
    errors: Vec<SemanticError>,
    function_depth: usize,
}

impl Analyzer {
    fn statements(&mut self, statements: &[Statement], create_scope: bool) {
        if create_scope {
            self.scopes.push(HashMap::new());
        }

        for statement in statements {
            if let StatementKind::Function {
                name, parameters, ..
            } = &statement.kind
            {
                self.declare(
                    name,
                    Symbol::Function {
                        arity: parameters.len(),
                    },
                    statement.span,
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
                parameters, body, ..
            } = &statement.kind
            {
                self.function(parameters, body, statement.span);
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
                initializer,
            } => {
                self.expression(initializer);
                self.declare(name, Symbol::Variable { mutable: *mutable }, statement.span);
            }
            StatementKind::Function { .. } => unreachable!("function bodies are analyzed last"),
            StatementKind::Block(statements) => self.statements(statements, true),
            StatementKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expression(condition);
                self.statement(then_branch);
                if let Some(else_branch) = else_branch {
                    self.statement(else_branch);
                }
            }
            StatementKind::While { condition, body } => {
                self.expression(condition);
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
                if let Some(value) = value {
                    self.expression(value);
                }
            }
            StatementKind::Expression(expression) => self.expression(expression),
        }
    }

    fn function(&mut self, parameters: &[String], body: &[Statement], span: Span) {
        self.scopes.push(HashMap::new());
        for parameter in parameters {
            self.declare(parameter, Symbol::Variable { mutable: true }, span);
        }
        self.function_depth += 1;
        self.statements(body, false);
        self.function_depth -= 1;
        self.scopes.pop();
    }

    fn expression(&mut self, expression: &Expression) {
        match &expression.kind {
            ExpressionKind::Literal(_) => {}
            ExpressionKind::Variable(name) => {
                if self.resolve(name).is_none() {
                    self.undefined(name, expression.span);
                }
            }
            ExpressionKind::Assign { name, value } => {
                self.expression(value);
                match self.resolve(name) {
                    None => self.undefined(name, expression.span),
                    Some(
                        Symbol::Variable { mutable: false }
                        | Symbol::Function { .. }
                        | Symbol::VariadicFunction,
                    ) => {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::ImmutableAssignment,
                            message: format!("cannot assign to immutable name '{name}'"),
                            span: expression.span,
                        });
                    }
                    Some(Symbol::Variable { mutable: true }) => {}
                }
            }
            ExpressionKind::Unary { operand, .. } => self.expression(operand),
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.expression(callee);
                for argument in arguments {
                    self.expression(argument);
                }
                if let ExpressionKind::Variable(name) = &callee.kind
                    && let Some(Symbol::Function { arity }) = self.resolve(name)
                    && arity != arguments.len()
                {
                    self.errors.push(SemanticError {
                        kind: SemanticErrorKind::ArgumentCount,
                        message: format!(
                            "function '{name}' expects {arity} arguments but received {}",
                            arguments.len()
                        ),
                        span: expression.span,
                    });
                }
            }
        }
    }

    fn declare(&mut self, name: &str, symbol: Symbol, span: Span) {
        let scope = self.scopes.last_mut().expect("analyzer always has a scope");
        if scope.insert(name.into(), symbol).is_some() {
            self.errors.push(SemanticError {
                kind: SemanticErrorKind::DuplicateName,
                message: format!("name '{name}' is already declared in this scope"),
                span,
            });
        }
    }

    fn resolve(&self, name: &str) -> Option<Symbol> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn undefined(&mut self, name: &str, span: Span) {
        self.errors.push(SemanticError {
            kind: SemanticErrorKind::UndefinedName,
            message: format!("undefined name '{name}'"),
            span,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{SemanticErrorKind, analyze};
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
}
