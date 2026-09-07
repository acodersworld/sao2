use crate::source::Span;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    pub declarations: Vec<Declaration>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Declaration {
    Function(FunctionDeclaration),
}

impl Declaration {
    pub fn span(&self) -> Span {
        match self {
            Self::Function(function) => function.span,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionDeclaration {
    pub name: Identifier,
    pub parameters: Vec<Parameter>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Parameter {
    pub mutable: bool,
    pub name: Identifier,
    pub ty: Type,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Type {
    pub kind: TypeKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeKind {
    Primitive(PrimitiveType),
    Named(Identifier),
    List(Box<Type>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimitiveType {
    Int,
    Float,
    Str,
    Bool,
    Char,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Block {
    pub statements: Vec<Statement>,
    pub value: Option<Box<Expression>>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Statement {
    pub kind: StatementKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatementKind {
    Expression(Expression),
    Block(Block),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionKind {
    Identifier(Identifier),
    String(Vec<u8>),
    Call {
        callee: Box<Expression>,
        arguments: Vec<Expression>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identifier {
    pub span: Span,
}
