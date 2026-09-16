use crate::source::Span;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    pub declarations: Vec<Declaration>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Declaration {
    Type(TypeDeclaration),
    Function(FunctionDeclaration),
}

impl Declaration {
    #[allow(dead_code)] // Used by consumers of the public AST outside this crate.
    pub fn span(&self) -> Span {
        match self {
            Self::Type(type_declaration) => type_declaration.span,
            Self::Function(function) => function.span,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeDeclaration {
    pub name: Identifier,
    pub members: Vec<TypeMember>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeMember {
    pub kind: TypeMemberKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeMemberKind {
    Named {
        name: Identifier,
        referenced: bool,
        ty: Type,
    },
    Unnamed(Type),
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
    Unit,
    Primitive(PrimitiveType),
    Named(Identifier),
    List(Box<Type>),
    Map { key: Box<Type>, value: Box<Type> },
    Union(Box<[Type]>),
    Tagged { tag: Identifier, payload: Box<Type> },
    Parenthesized(Box<Type>),
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
    Local {
        mutable: bool,
        name: Identifier,
        initializer: Expression,
    },
    Assignment {
        target: AssignmentTarget,
        operator: AssignmentOperator,
        operator_span: Span,
        value: Expression,
    },
    Expression(Expression),
    Return(Option<Expression>),
    Break,
    Continue,
    If {
        branches: Vec<ConditionalStatementBranch>,
        else_body: Option<StatementBody>,
    },
    While {
        condition: Expression,
        body: StatementBody,
    },
    For {
        binding: Identifier,
        iterable: Expression,
        body: StatementBody,
    },
    Switch {
        value: Expression,
        arms: Vec<SwitchArm>,
        else_body: Option<StatementBody>,
    },
    Block(Block),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatementBody {
    pub kind: StatementBodyKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatementBodyKind {
    Block(Block),
    Statement(Box<Statement>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionalStatementBranch {
    pub condition: Expression,
    pub body: StatementBody,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwitchArm {
    pub label: Type,
    pub body: StatementBody,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentTarget {
    pub root: Identifier,
    pub suffixes: Vec<AssignmentTargetSuffix>,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssignmentTargetSuffix {
    pub kind: AssignmentTargetSuffixKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssignmentTargetSuffixKind {
    Member(Member),
    Index(Expression),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssignmentOperator {
    Assign,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    BitwiseAnd,
    BitwiseOr,
    BitwiseXor,
    ShiftLeft,
    ShiftRight,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expression {
    pub kind: ExpressionKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionKind {
    Unit,
    Identifier(Identifier),
    Integer,
    Float,
    String(Vec<u8>),
    Character(u8),
    Boolean(bool),
    Conversion {
        destination: PrimitiveType,
        operand: Box<Expression>,
    },
    Parenthesized(Box<Expression>),
    List(Vec<Expression>),
    Map(Vec<MapEntry>),
    TypedEmptyList(Type),
    TypedEmptyMap(Type),
    Block(Block),
    If {
        branches: Vec<ConditionalExpressionBranch>,
        else_branch: ExpressionBody,
    },
    Unary {
        operator: UnaryOperator,
        operator_span: Span,
        operand: Box<Expression>,
    },
    Binary {
        left: Box<Expression>,
        operator: BinaryOperator,
        operator_span: Span,
        right: Box<Expression>,
    },
    Is {
        value: Box<Expression>,
        operator_span: Span,
        ty: Type,
    },
    Call {
        callee: Box<Expression>,
        arguments: Vec<Argument>,
    },
    Index {
        value: Box<Expression>,
        index: Box<Expression>,
    },
    Member {
        value: Box<Expression>,
        member: Member,
    },
    Try {
        value: Box<Expression>,
        operator_span: Span,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Argument {
    pub kind: ArgumentKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArgumentKind {
    Positional(Expression),
    Named { name: Identifier, value: Expression },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapEntry {
    pub key: Expression,
    pub value: Expression,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionalExpressionBranch {
    pub condition: Expression,
    pub body: ExpressionBody,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionBody {
    pub kind: ExpressionBodyKind,
    pub span: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionBodyKind {
    Block(Block),
    Expression(Box<Expression>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Member {
    Named(Identifier),
    TupleIndex(Span),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnaryOperator {
    LogicalNot,
    BitwiseNot,
    Plus,
    Minus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    LogicalOr,
    LogicalAnd,
    BitwiseOr,
    BitwiseXor,
    BitwiseAnd,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    In,
    ShiftLeft,
    ShiftRight,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identifier {
    pub span: Span,
}
