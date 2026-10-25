use crate::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    pub statements: Vec<Statement>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identifier {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeAnnotation {
    pub name: Identifier,
    pub arguments: Vec<TypeAnnotation>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub name: Identifier,
    pub type_annotation: Option<TypeAnnotation>,
}

impl Identifier {
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum StatementKind {
    Variable {
        name: Identifier,
        mutable: bool,
        type_annotation: Option<TypeAnnotation>,
        initializer: Expression,
    },
    Function {
        name: Identifier,
        parameters: Vec<Parameter>,
        return_type: Option<TypeAnnotation>,
        body: Vec<Statement>,
    },
    Block(Vec<Statement>),
    If {
        condition: Expression,
        then_branch: Box<Statement>,
        else_branch: Option<Box<Statement>>,
    },
    While {
        condition: Expression,
        body: Box<Statement>,
    },
    Return(Option<Expression>),
    Expression(Expression),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExpressionKind {
    Literal(Literal),
    Variable(Identifier),
    Assign {
        name: Identifier,
        value: Box<Expression>,
    },
    Unary {
        operator: UnaryOperator,
        operand: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        right: Box<Expression>,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    And,
    Or,
}
