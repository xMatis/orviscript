use std::collections::HashMap;
use std::fmt;

use orvi_syntax::{
    Expression, ExpressionKind, Identifier, Program, Span, Statement, StatementKind,
};

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

#[derive(Clone, Copy, Debug)]
struct Symbol {
    kind: SymbolKind,
    mutable: bool,
    arity: Option<usize>,
    definition: Option<Span>,
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
            },
        )])],
        errors: Vec::new(),
        symbols: Vec::new(),
        function_depth: 0,
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
                    Symbol {
                        kind: SymbolKind::Function,
                        mutable: false,
                        arity: Some(parameters.len()),
                        definition: Some(name.span),
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

    fn function(&mut self, parameters: &[Identifier], body: &[Statement], _: Span) {
        self.scopes.push(HashMap::new());
        for parameter in parameters {
            self.declare(
                parameter,
                Symbol {
                    kind: SymbolKind::Parameter,
                    mutable: true,
                    arity: None,
                    definition: Some(parameter.span),
                },
            );
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
                if self.resolve_reference(name).is_none() {
                    self.undefined(name);
                }
            }
            ExpressionKind::Assign { name, value } => {
                self.expression(value);
                match self.resolve_reference(name) {
                    None => self.undefined(name),
                    Some(symbol) if !symbol.mutable => {
                        self.errors.push(SemanticError {
                            kind: SemanticErrorKind::ImmutableAssignment,
                            message: format!("cannot assign to immutable name '{}'", name.name),
                            span: name.span,
                        });
                    }
                    Some(_) => {}
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
            }
        }
    }

    fn declare(&mut self, name: &Identifier, symbol: Symbol) {
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
            kind: symbol.kind,
            is_definition: true,
        });
    }

    fn resolve(&self, name: &str) -> Option<Symbol> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
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
}
