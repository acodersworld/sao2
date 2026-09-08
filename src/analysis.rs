//! Name-and-type analysis representation.
//!
//! This module deliberately does not perform name lookup or type inference yet.
//! It establishes the analysis boundary and stable identities which the later
//! milestone-3 phases fill in without adding semantic data to the parser AST.

use crate::ast::{
    AssignmentTarget, AssignmentTargetSuffixKind, Block, Declaration, Expression,
    ExpressionBody, ExpressionBodyKind, ExpressionKind, FunctionDeclaration, Parameter,
    PrimitiveType, Program, Statement, StatementBody, StatementBodyKind, StatementKind, Type,
    TypeDeclaration,
};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::source::{SourceFile, Span};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TypeDeclarationId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FunctionId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum DeclarationId {
    Type(TypeDeclarationId),
    Function(FunctionId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct BindingId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct TypeId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DeferredId(usize);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedType {
    Primitive(PrimitiveType),
    List(TypeId),
    Map { key: TypeId, value: TypeId },
    Nominal(TypeDeclarationId),
    Union(Box<[UnionAlternative]>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum UnionAlternative {
    Untagged(TypeId),
    Tagged { tag: Box<str>, payload: TypeId },
    Error(TypeId),
}

/// The outcome of analyzing a construct which may or may not produce a value.
///
/// `NoValue` and `Never` are deliberately outside `ResolvedType`: neither is a
/// language value type. `Error` suppresses cascades without manufacturing a
/// valid type, while `Deferred` makes later flow-sensitive work explicit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TypeState {
    Resolved(TypeId),
    NoValue,
    Never,
    Error,
    Deferred(DeferredId),
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DeclarationNode<'ast> {
    Type(&'ast TypeDeclaration),
    Function(&'ast FunctionDeclaration),
}

impl DeclarationNode<'_> {
    pub(crate) fn span(self) -> Span {
        match self {
            Self::Type(declaration) => declaration.span,
            Self::Function(declaration) => declaration.span,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum BindingNode<'ast> {
    Parameter(&'ast Parameter),
    Local(&'ast Statement),
    Loop(&'ast Statement),
}

impl BindingNode<'_> {
    pub(crate) fn span(self) -> Span {
        match self {
            Self::Parameter(parameter) => parameter.name.span,
            Self::Local(statement) => match &statement.kind {
                StatementKind::Local { name, .. } => name.span,
                _ => unreachable!("local binding must point to a local statement"),
            },
            Self::Loop(statement) => match &statement.kind {
                StatementKind::For { binding, .. } => binding.span,
                _ => unreachable!("loop binding must point to a for statement"),
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AstNode<'ast> {
    Declaration(DeclarationNode<'ast>),
    Binding(BindingNode<'ast>),
    Type(&'ast Type),
    Statement(&'ast Statement),
    Expression(&'ast Expression),
}

impl AstNode<'_> {
    pub(crate) fn span(self) -> Span {
        match self {
            Self::Declaration(node) => node.span(),
            Self::Binding(node) => node.span(),
            Self::Type(ty) => ty.span,
            Self::Statement(statement) => statement.span,
            Self::Expression(expression) => expression.span,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DeclarationRecord<'ast> {
    pub(crate) id: DeclarationId,
    pub(crate) node: DeclarationNode<'ast>,
    pub(crate) span: Span,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BindingRecord<'ast> {
    pub(crate) id: BindingId,
    pub(crate) node: BindingNode<'ast>,
    pub(crate) span: Span,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TypeAnnotation<'ast> {
    pub(crate) node: &'ast Type,
    pub(crate) state: TypeState,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ExpressionAnnotation<'ast> {
    pub(crate) node: &'ast Expression,
    pub(crate) state: TypeState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeferredReason {
    FlowDependentType,
    ExpectedType,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DeferredAnalysis<'ast> {
    pub(crate) id: DeferredId,
    pub(crate) node: AstNode<'ast>,
    pub(crate) reason: DeferredReason,
}

#[derive(Debug)]
pub(crate) struct TypeTable {
    entries: Vec<ResolvedType>,
}

impl TypeTable {
    const INT: TypeId = TypeId(0);
    const FLOAT: TypeId = TypeId(1);
    const STR: TypeId = TypeId(2);
    const BOOL: TypeId = TypeId(3);
    const CHAR: TypeId = TypeId(4);

    fn new() -> Self {
        Self {
            entries: vec![
                ResolvedType::Primitive(PrimitiveType::Int),
                ResolvedType::Primitive(PrimitiveType::Float),
                ResolvedType::Primitive(PrimitiveType::Str),
                ResolvedType::Primitive(PrimitiveType::Bool),
                ResolvedType::Primitive(PrimitiveType::Char),
            ],
        }
    }

    pub(crate) fn primitive(&self, primitive: PrimitiveType) -> TypeId {
        match primitive {
            PrimitiveType::Int => Self::INT,
            PrimitiveType::Float => Self::FLOAT,
            PrimitiveType::Str => Self::STR,
            PrimitiveType::Bool => Self::BOOL,
            PrimitiveType::Char => Self::CHAR,
        }
    }

    pub(crate) fn intern(&mut self, ty: ResolvedType) -> TypeId {
        if let Some(index) = self.entries.iter().position(|entry| entry == &ty) {
            return TypeId(index);
        }
        let id = TypeId(self.entries.len());
        self.entries.push(ty);
        id
    }

    pub(crate) fn get(&self, id: TypeId) -> &ResolvedType {
        &self.entries[id.0]
    }
}

/// Results owned by the name-and-type analysis stage.
///
/// References provide an explicit connection back to the immutable parser AST;
/// semantic annotations live only in this structure.
#[derive(Debug)]
pub(crate) struct Analysis<'source, 'ast> {
    pub(crate) source: &'source SourceFile,
    pub(crate) program: &'ast Program,
    pub(crate) declarations: Vec<DeclarationRecord<'ast>>,
    pub(crate) bindings: Vec<BindingRecord<'ast>>,
    pub(crate) types: TypeTable,
    pub(crate) type_annotations: Vec<TypeAnnotation<'ast>>,
    pub(crate) expression_annotations: Vec<ExpressionAnnotation<'ast>>,
    pub(crate) deferred: Vec<DeferredAnalysis<'ast>>,
    pub(crate) diagnostics: Diagnostics,
}

impl<'source, 'ast> Analysis<'source, 'ast> {
    pub(crate) fn annotate_type(&mut self, node: &'ast Type, state: TypeState) {
        self.type_annotations.push(TypeAnnotation { node, state });
    }

    pub(crate) fn annotate_expression(&mut self, node: &'ast Expression, state: TypeState) {
        self.expression_annotations
            .push(ExpressionAnnotation { node, state });
    }

    pub(crate) fn defer(
        &mut self,
        node: AstNode<'ast>,
        reason: DeferredReason,
    ) -> DeferredId {
        let id = DeferredId(self.deferred.len());
        self.deferred.push(DeferredAnalysis { id, node, reason });
        id
    }

    pub(crate) fn error(&mut self, span: Span, message: impl Into<String>) -> TypeState {
        self.diagnostics
            .push(Diagnostic::source(self.source, span, message));
        TypeState::Error
    }
}

/// Creates the analysis result and assigns deterministic identities to syntax
/// declarations and binding sites. Name lookup and typing begin in later phases.
pub(crate) fn analyze<'source, 'ast>(
    source: &'source SourceFile,
    program: &'ast Program,
) -> Analysis<'source, 'ast> {
    let mut declarations = Vec::with_capacity(program.declarations.len());
    let mut bindings = Vec::new();
    let mut next_type = 0;
    let mut next_function = 0;

    for declaration in &program.declarations {
        let (id, node) = match declaration {
            Declaration::Type(type_declaration) => {
                let id = DeclarationId::Type(TypeDeclarationId(next_type));
                next_type += 1;
                (id, DeclarationNode::Type(type_declaration))
            }
            Declaration::Function(function) => {
                let id = DeclarationId::Function(FunctionId(next_function));
                next_function += 1;
                for parameter in &function.parameters {
                    push_binding(&mut bindings, BindingNode::Parameter(parameter));
                }
                collect_block_bindings(&function.body, &mut bindings);
                (id, DeclarationNode::Function(function))
            }
        };
        declarations.push(DeclarationRecord {
            id,
            span: node.span(),
            node,
        });
    }

    Analysis {
        source,
        program,
        declarations,
        bindings,
        types: TypeTable::new(),
        type_annotations: Vec::new(),
        expression_annotations: Vec::new(),
        deferred: Vec::new(),
        diagnostics: Diagnostics::new(),
    }
}

fn push_binding<'ast>(bindings: &mut Vec<BindingRecord<'ast>>, node: BindingNode<'ast>) {
    bindings.push(BindingRecord {
        id: BindingId(bindings.len()),
        span: node.span(),
        node,
    });
}

fn collect_block_bindings<'ast>(block: &'ast Block, bindings: &mut Vec<BindingRecord<'ast>>) {
    for statement in &block.statements {
        collect_statement_bindings(statement, bindings);
    }
    if let Some(value) = &block.value {
        collect_expression_bindings(value, bindings);
    }
}

fn collect_statement_bindings<'ast>(
    statement: &'ast Statement,
    bindings: &mut Vec<BindingRecord<'ast>>,
) {
    match &statement.kind {
        StatementKind::Local { initializer, .. } => {
            push_binding(bindings, BindingNode::Local(statement));
            collect_expression_bindings(initializer, bindings);
        }
        StatementKind::Assignment { target, value, .. } => {
            collect_assignment_target_bindings(target, bindings);
            collect_expression_bindings(value, bindings);
        }
        StatementKind::Expression(expression) => collect_expression_bindings(expression, bindings),
        StatementKind::Return(value) => {
            if let Some(value) = value {
                collect_expression_bindings(value, bindings);
            }
        }
        StatementKind::Break | StatementKind::Continue => {}
        StatementKind::If {
            branches,
            else_body,
        } => {
            for branch in branches {
                collect_expression_bindings(&branch.condition, bindings);
                collect_statement_body_bindings(&branch.body, bindings);
            }
            if let Some(body) = else_body {
                collect_statement_body_bindings(body, bindings);
            }
        }
        StatementKind::While { condition, body } => {
            collect_expression_bindings(condition, bindings);
            collect_statement_body_bindings(body, bindings);
        }
        StatementKind::For {
            iterable, body, ..
        } => {
            push_binding(bindings, BindingNode::Loop(statement));
            collect_expression_bindings(iterable, bindings);
            collect_statement_body_bindings(body, bindings);
        }
        StatementKind::Switch {
            value,
            arms,
            else_body,
        } => {
            collect_expression_bindings(value, bindings);
            for arm in arms {
                collect_statement_body_bindings(&arm.body, bindings);
            }
            if let Some(body) = else_body {
                collect_statement_body_bindings(body, bindings);
            }
        }
        StatementKind::Block(block) => collect_block_bindings(block, bindings),
    }
}

fn collect_statement_body_bindings<'ast>(
    body: &'ast StatementBody,
    bindings: &mut Vec<BindingRecord<'ast>>,
) {
    match &body.kind {
        StatementBodyKind::Block(block) => collect_block_bindings(block, bindings),
        StatementBodyKind::Statement(statement) => collect_statement_bindings(statement, bindings),
    }
}

fn collect_assignment_target_bindings<'ast>(
    target: &'ast AssignmentTarget,
    bindings: &mut Vec<BindingRecord<'ast>>,
) {
    for suffix in &target.suffixes {
        if let AssignmentTargetSuffixKind::Index(expression) = &suffix.kind {
            collect_expression_bindings(expression, bindings);
        }
    }
}

fn collect_expression_bindings<'ast>(
    expression: &'ast Expression,
    bindings: &mut Vec<BindingRecord<'ast>>,
) {
    match &expression.kind {
        ExpressionKind::Identifier(_)
        | ExpressionKind::Integer
        | ExpressionKind::Float
        | ExpressionKind::String(_)
        | ExpressionKind::Character(_)
        | ExpressionKind::Boolean(_)
        | ExpressionKind::TypedEmptyList(_)
        | ExpressionKind::TypedEmptyMap(_) => {}
        ExpressionKind::Parenthesized(inner)
        | ExpressionKind::Unary { operand: inner, .. }
        | ExpressionKind::Try { value: inner, .. } => {
            collect_expression_bindings(inner, bindings);
        }
        ExpressionKind::List(elements) => {
            for element in elements {
                collect_expression_bindings(element, bindings);
            }
        }
        ExpressionKind::Map(entries) => {
            for entry in entries {
                collect_expression_bindings(&entry.key, bindings);
                collect_expression_bindings(&entry.value, bindings);
            }
        }
        ExpressionKind::Block(block) => collect_block_bindings(block, bindings),
        ExpressionKind::If {
            branches,
            else_branch,
        } => {
            for branch in branches {
                collect_expression_bindings(&branch.condition, bindings);
                collect_expression_body_bindings(&branch.body, bindings);
            }
            collect_expression_body_bindings(else_branch, bindings);
        }
        ExpressionKind::Binary { left, right, .. } => {
            collect_expression_bindings(left, bindings);
            collect_expression_bindings(right, bindings);
        }
        ExpressionKind::Is { value, .. } => collect_expression_bindings(value, bindings),
        ExpressionKind::Call { callee, arguments } => {
            collect_expression_bindings(callee, bindings);
            for argument in arguments {
                match &argument.kind {
                    crate::ast::ArgumentKind::Positional(value)
                    | crate::ast::ArgumentKind::Named { value, .. } => {
                        collect_expression_bindings(value, bindings);
                    }
                }
            }
        }
        ExpressionKind::Index { value, index } => {
            collect_expression_bindings(value, bindings);
            collect_expression_bindings(index, bindings);
        }
        ExpressionKind::Member { value, .. } => collect_expression_bindings(value, bindings),
    }
}

fn collect_expression_body_bindings<'ast>(
    body: &'ast ExpressionBody,
    bindings: &mut Vec<BindingRecord<'ast>>,
) {
    match &body.kind {
        ExpressionBodyKind::Block(block) => collect_block_bindings(block, bindings),
        ExpressionBodyKind::Expression(expression) => {
            collect_expression_bindings(expression, bindings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use std::path::PathBuf;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }

    #[test]
    fn assigns_stable_typed_declaration_and_binding_identities() {
        let source = source(
            "type Pair(a int, b int); fn first(arg int) { x := 1; for item in [x] { y := item; } } fn second() {}",
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);

        assert_eq!(analysis.declarations.len(), 3);
        assert_eq!(
            analysis.declarations[0].id,
            DeclarationId::Type(TypeDeclarationId(0))
        );
        assert_eq!(
            analysis.declarations[1].id,
            DeclarationId::Function(FunctionId(0))
        );
        assert_eq!(
            analysis.declarations[2].id,
            DeclarationId::Function(FunctionId(1))
        );
        assert_eq!(
            analysis
                .bindings
                .iter()
                .map(|binding| binding.id)
                .collect::<Vec<_>>(),
            vec![BindingId(0), BindingId(1), BindingId(2), BindingId(3)]
        );
        assert_eq!(
            analysis
                .bindings
                .iter()
                .map(|binding| &source.text[binding.span.start..binding.span.end])
                .collect::<Vec<_>>(),
            vec!["arg", "x", "item", "y"]
        );
    }

    #[test]
    fn keeps_semantic_types_and_non_value_states_distinct() {
        let source = source("fn main() {}");
        let program = parser::parse(&source).unwrap();
        let mut analysis = analyze(&source, &program);

        let int = analysis.types.primitive(PrimitiveType::Int);
        assert_eq!(
            analysis.types.get(int),
            &ResolvedType::Primitive(PrimitiveType::Int)
        );
        let list = analysis.types.intern(ResolvedType::List(int));
        assert_eq!(analysis.types.get(list), &ResolvedType::List(int));
        assert_eq!(analysis.types.intern(ResolvedType::List(int)), list);
        assert_ne!(TypeState::Resolved(int), TypeState::NoValue);
        assert_ne!(TypeState::NoValue, TypeState::Never);
        assert_ne!(TypeState::Never, TypeState::Error);
    }

    #[test]
    fn records_ast_connections_errors_and_deferred_work() {
        let source = source("fn main() { value := if true: 1 else: 2; }");
        let program = parser::parse(&source).unwrap();
        let mut analysis = analyze(&source, &program);
        let Declaration::Function(function) = &program.declarations[0] else {
            unreachable!();
        };
        let StatementKind::Local { initializer, .. } = &function.body.statements[0].kind else {
            unreachable!();
        };

        let deferred = analysis.defer(
            AstNode::Expression(initializer),
            DeferredReason::FlowDependentType,
        );
        analysis.annotate_expression(initializer, TypeState::Deferred(deferred));
        let error = analysis.error(initializer.span, "example analysis error");

        assert_eq!(analysis.deferred[0].id, deferred);
        assert_eq!(analysis.deferred[0].node.span(), initializer.span);
        assert_eq!(analysis.expression_annotations[0].state, TypeState::Deferred(deferred));
        assert_eq!(error, TypeState::Error);
        assert_eq!(analysis.diagnostics.len(), 1);
        assert!(analysis.diagnostics.to_string().contains("example analysis error"));
    }
}
