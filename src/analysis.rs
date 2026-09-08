//! Name-and-type analysis representation.
//!
//! This module establishes the analysis boundary and stable identities without
//! adding semantic data to the parser AST. It collects top-level names, resolves
//! type definitions, and resolves function signatures before later phases
//! analyze declaration bodies.

use crate::ast::{
    AssignmentTarget, AssignmentTargetSuffixKind, Block, Declaration, Expression,
    ExpressionBody, ExpressionBodyKind, ExpressionKind, FunctionDeclaration, Parameter,
    PrimitiveType, Program, Statement, StatementBody, StatementBodyKind, StatementKind, Type,
    TypeDeclaration, TypeKind, TypeMember, TypeMemberKind,
};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::source::{SourceFile, Span};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TypeId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DeferredId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum IntrinsicId {
    Print,
    Println,
    Panic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallableId {
    Function(FunctionId),
    Constructor(TypeDeclarationId),
    Intrinsic(IntrinsicId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallableResolution {
    Found(CallableId),
    Ambiguous {
        value: CallableId,
        constructor: TypeDeclarationId,
    },
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ResolvedType {
    Primitive(PrimitiveType),
    List(TypeId),
    Map { key: TypeId, value: TypeId },
    Nominal(TypeDeclarationId),
    Union(Box<[UnionAlternative]>),
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum IntrinsicArguments {
    OnePrintable,
    ZeroOrOnePrintable,
    Exact(Box<[TypeId]>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IntrinsicSignature {
    pub(crate) id: IntrinsicId,
    pub(crate) name: &'static str,
    pub(crate) arguments: IntrinsicArguments,
    pub(crate) result: TypeState,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ParameterSignature<'ast> {
    pub(crate) binding: BindingId,
    pub(crate) node: &'ast Parameter,
    pub(crate) ty: TypeState,
}

#[derive(Clone, Debug)]
pub(crate) struct FunctionSignature<'ast> {
    pub(crate) id: FunctionId,
    pub(crate) node: &'ast FunctionDeclaration,
    pub(crate) parameters: Vec<ParameterSignature<'ast>>,
    pub(crate) result: TypeState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemberStorage {
    Inline,
    Referenced,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct StructMember<'ast> {
    pub(crate) node: &'ast TypeMember,
    pub(crate) name: &'ast crate::ast::Identifier,
    pub(crate) storage: MemberStorage,
    pub(crate) ty: TypeState,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TupleMember<'ast> {
    pub(crate) node: &'ast TypeMember,
    pub(crate) position: usize,
    pub(crate) ty: TypeState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnionStyle {
    Untagged,
    Tagged,
}

#[derive(Clone, Debug)]
pub(crate) enum TypeDefinitionKind<'ast> {
    Struct(Box<[StructMember<'ast>]>),
    Tuple(Box<[TupleMember<'ast>]>),
    Union {
        style: UnionStyle,
        alternatives: Box<[UnionAlternative]>,
    },
    Invalid,
}

#[derive(Clone, Debug)]
pub(crate) struct TypeDefinition<'ast> {
    pub(crate) id: TypeDeclarationId,
    pub(crate) node: &'ast TypeDeclaration,
    pub(crate) kind: TypeDefinitionKind<'ast>,
}

#[derive(Clone, Copy, Debug)]
struct PendingMapKey<'ast> {
    node: &'ast Type,
    key: TypeId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TypeName {
    pub(crate) name: Box<str>,
    pub(crate) id: TypeDeclarationId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionName {
    pub(crate) name: Box<str>,
    pub(crate) id: FunctionId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntryPoint {
    Missing,
    Valid(FunctionId),
    Invalid(FunctionId),
    Duplicate {
        first: FunctionId,
        duplicate: FunctionId,
    },
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
    pub(crate) type_names: Vec<TypeName>,
    pub(crate) function_names: Vec<FunctionName>,
    pub(crate) type_definitions: Vec<TypeDefinition<'ast>>,
    pub(crate) function_signatures: Vec<FunctionSignature<'ast>>,
    pub(crate) intrinsic_signatures: Vec<IntrinsicSignature>,
    pub(crate) entry_point: EntryPoint,
    pub(crate) deferred: Vec<DeferredAnalysis<'ast>>,
    pub(crate) diagnostics: Diagnostics,
    pending_map_keys: Vec<PendingMapKey<'ast>>,
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

    pub(crate) fn type_by_name(&self, name: &str) -> Option<TypeDeclarationId> {
        self.type_names
            .iter()
            .find_map(|entry| (entry.name.as_ref() == name).then_some(entry.id))
    }

    pub(crate) fn function_by_name(&self, name: &str) -> Option<FunctionId> {
        self.function_names
            .iter()
            .find_map(|entry| (entry.name.as_ref() == name).then_some(entry.id))
    }

    pub(crate) fn intrinsic_by_name(&self, name: &str) -> Option<IntrinsicId> {
        self.intrinsic_signatures
            .iter()
            .find_map(|signature| (signature.name == name).then_some(signature.id))
    }

    pub(crate) fn function_signature(&self, id: FunctionId) -> &FunctionSignature<'ast> {
        let signature = &self.function_signatures[id.0];
        debug_assert_eq!(signature.id, id);
        signature
    }

    pub(crate) fn type_definition(&self, id: TypeDeclarationId) -> &TypeDefinition<'ast> {
        let definition = &self.type_definitions[id.0];
        debug_assert_eq!(definition.id, id);
        definition
    }

    pub(crate) fn intrinsic_signature(&self, id: IntrinsicId) -> &IntrinsicSignature {
        self.intrinsic_signatures
            .iter()
            .find(|signature| signature.id == id)
            .expect("every intrinsic identity has a registered signature")
    }

    pub(crate) fn callable_by_name(&self, name: &str) -> CallableResolution {
        let value = self
            .function_by_name(name)
            .map(CallableId::Function)
            .or_else(|| self.intrinsic_by_name(name).map(CallableId::Intrinsic));
        let constructor = self.type_by_name(name);
        match (value, constructor) {
            (Some(value), Some(constructor)) => {
                CallableResolution::Ambiguous { value, constructor }
            }
            (Some(value), None) => CallableResolution::Found(value),
            (None, Some(constructor)) => {
                CallableResolution::Found(CallableId::Constructor(constructor))
            }
            (None, None) => CallableResolution::Unknown,
        }
    }

    fn collect_top_level_names(&mut self) {
        for index in 0..self.declarations.len() {
            let record = self.declarations[index];
            match (record.id, record.node) {
                (DeclarationId::Type(id), DeclarationNode::Type(declaration)) => {
                    let name = self.identifier_text(declaration.name.span).to_owned();
                    if self.type_by_name(&name).is_some() {
                        self.diagnostics.push(Diagnostic::source(
                            self.source,
                            declaration.name.span,
                            format!("duplicate type declaration '{name}'"),
                        ));
                    } else {
                        self.type_names.push(TypeName {
                            name: name.into_boxed_str(),
                            id,
                        });
                    }
                }
                (DeclarationId::Function(id), DeclarationNode::Function(function)) => {
                    let name = self.identifier_text(function.name.span).to_owned();
                    if self.intrinsic_by_name(&name).is_some() {
                        self.diagnostics.push(Diagnostic::source(
                            self.source,
                            function.name.span,
                            format!("function name '{name}' is reserved for a compiler intrinsic"),
                        ));
                    } else if self.function_by_name(&name).is_some() {
                        self.diagnostics.push(Diagnostic::source(
                            self.source,
                            function.name.span,
                            format!("duplicate function declaration '{name}'"),
                        ));
                    } else {
                        self.function_names.push(FunctionName {
                            name: name.into_boxed_str(),
                            id,
                        });
                    }
                    self.check_parameter_names(function);
                }
                _ => unreachable!("declaration identity must match its AST node"),
            }
        }
    }

    fn check_parameter_names(&mut self, function: &'ast FunctionDeclaration) {
        let mut names: Vec<Box<str>> = Vec::new();
        for parameter in &function.parameters {
            let name = self.identifier_text(parameter.name.span).to_owned();
            if names
                .iter()
                .any(|existing| existing.as_ref() == name.as_str())
            {
                self.diagnostics.push(Diagnostic::source(
                    self.source,
                    parameter.name.span,
                    format!("duplicate parameter name '{name}'"),
                ));
            } else {
                names.push(name.into_boxed_str());
            }
        }
    }

    fn resolve_type_definitions(&mut self) {
        for index in 0..self.declarations.len() {
            let record = self.declarations[index];
            let (DeclarationId::Type(id), DeclarationNode::Type(declaration)) =
                (record.id, record.node)
            else {
                continue;
            };
            let kind = self.resolve_type_definition(declaration);
            self.type_definitions.push(TypeDefinition {
                id,
                node: declaration,
                kind,
            });
        }
    }

    fn resolve_type_definition(
        &mut self,
        declaration: &'ast TypeDeclaration,
    ) -> TypeDefinitionKind<'ast> {
        let has_named = declaration
            .members
            .iter()
            .any(|member| matches!(&member.kind, TypeMemberKind::Named { .. }));
        let has_unnamed = declaration
            .members
            .iter()
            .any(|member| matches!(&member.kind, TypeMemberKind::Unnamed(_)));
        let has_top_level_union = declaration.members.iter().any(|member| {
            let ty = match &member.kind {
                TypeMemberKind::Named { ty, .. } | TypeMemberKind::Unnamed(ty) => ty,
            };
            matches!(&ty.kind, TypeKind::Union(_))
        });

        if has_named && has_unnamed {
            self.error(
                declaration.span,
                "type declaration cannot mix named and unnamed members",
            );
            for member in &declaration.members {
                let ty = match &member.kind {
                    TypeMemberKind::Named { ty, .. } | TypeMemberKind::Unnamed(ty) => ty,
                };
                self.resolve_type(ty);
            }
            return TypeDefinitionKind::Invalid;
        }

        if has_top_level_union {
            let [member] = declaration.members.as_slice() else {
                self.error(
                    declaration.span,
                    "union declaration must contain one unnamed union type",
                );
                for member in &declaration.members {
                    let ty = match &member.kind {
                        TypeMemberKind::Named { ty, .. } | TypeMemberKind::Unnamed(ty) => ty,
                    };
                    self.resolve_type(ty);
                }
                return TypeDefinitionKind::Invalid;
            };
            let TypeMemberKind::Unnamed(ty) = &member.kind else {
                self.resolve_type(match &member.kind {
                    TypeMemberKind::Named { ty, .. } => ty,
                    TypeMemberKind::Unnamed(_) => unreachable!(),
                });
                self.error(
                    member.span,
                    "union declaration alternatives must be unnamed",
                );
                return TypeDefinitionKind::Invalid;
            };
            let state = self.resolve_type(ty);
            return match state {
                TypeState::Resolved(id) => match self.types.get(id) {
                    ResolvedType::Union(alternatives) => {
                        let alternatives = alternatives.clone();
                        let style = union_style(&alternatives);
                        TypeDefinitionKind::Union {
                            style,
                            alternatives,
                        }
                    }
                    _ => unreachable!("top-level union syntax must resolve to a union type"),
                },
                _ => TypeDefinitionKind::Invalid,
            };
        }

        if has_named {
            let mut names: Vec<Box<str>> = Vec::new();
            let mut members = Vec::with_capacity(declaration.members.len());
            for member in &declaration.members {
                let TypeMemberKind::Named {
                    name,
                    referenced,
                    ty,
                } = &member.kind
                else {
                    unreachable!("mixed member forms were rejected above")
                };
                let spelling = self.identifier_text(name.span).to_owned();
                if names
                    .iter()
                    .any(|existing| existing.as_ref() == spelling.as_str())
                {
                    self.error(name.span, format!("duplicate struct member '{spelling}'"));
                } else {
                    names.push(spelling.into_boxed_str());
                }
                let resolved = self.resolve_type(ty);
                let storage = if *referenced {
                    MemberStorage::Referenced
                } else {
                    MemberStorage::Inline
                };
                members.push(StructMember {
                    node: member,
                    name,
                    storage,
                    ty: resolved,
                });
            }
            TypeDefinitionKind::Struct(members.into_boxed_slice())
        } else {
            let mut members = Vec::with_capacity(declaration.members.len());
            for (position, member) in declaration.members.iter().enumerate() {
                let TypeMemberKind::Unnamed(ty) = &member.kind else {
                    unreachable!("mixed member forms were rejected above")
                };
                members.push(TupleMember {
                    node: member,
                    position,
                    ty: self.resolve_type(ty),
                });
            }
            TypeDefinitionKind::Tuple(members.into_boxed_slice())
        }
    }

    fn resolve_function_signatures(&mut self) {
        for index in 0..self.declarations.len() {
            let record = self.declarations[index];
            let (DeclarationId::Function(id), DeclarationNode::Function(function)) =
                (record.id, record.node)
            else {
                continue;
            };
            let mut parameters = Vec::with_capacity(function.parameters.len());
            for parameter in &function.parameters {
                let binding = self.parameter_binding(parameter);
                let ty = self.resolve_type(&parameter.ty);
                parameters.push(ParameterSignature {
                    binding,
                    node: parameter,
                    ty,
                });
            }
            let result = function
                .return_type
                .as_ref()
                .map_or(TypeState::NoValue, |ty| self.resolve_type(ty));
            self.function_signatures.push(FunctionSignature {
                id,
                node: function,
                parameters,
                result,
            });
        }
    }

    fn resolve_type(&mut self, node: &'ast Type) -> TypeState {
        let state = match &node.kind {
            TypeKind::Primitive(primitive) => {
                TypeState::Resolved(self.types.primitive(*primitive))
            }
            TypeKind::Named(identifier) => {
                let name = self.identifier_text(identifier.span).to_owned();
                match self.type_by_name(&name) {
                    Some(declaration) => {
                        TypeState::Resolved(self.types.intern(ResolvedType::Nominal(declaration)))
                    }
                    None => self.error(identifier.span, format!("unknown type '{name}'")),
                }
            }
            TypeKind::List(element) => match self.resolve_type(element) {
                TypeState::Resolved(element) => {
                    TypeState::Resolved(self.types.intern(ResolvedType::List(element)))
                }
                _ => TypeState::Error,
            },
            TypeKind::Map { key, value } => {
                let key_node = key.as_ref();
                let key_state = self.resolve_type(key);
                let value_state = self.resolve_type(value);
                match (key_state, value_state) {
                    (TypeState::Resolved(key), TypeState::Resolved(value)) => {
                        self.pending_map_keys.push(PendingMapKey {
                            node: key_node,
                            key,
                        });
                        TypeState::Resolved(self.types.intern(ResolvedType::Map { key, value }))
                    }
                    _ => TypeState::Error,
                }
            }
            TypeKind::Union(alternatives) => {
                let mut resolved = Vec::with_capacity(alternatives.len());
                let mut failed = false;
                let mut style = None;
                let mut untagged = Vec::new();
                let mut tags: Vec<Box<str>> = Vec::new();
                let mut saw_error = false;
                for alternative in alternatives {
                    match &alternative.kind {
                        TypeKind::Tagged { tag, payload } => {
                            let tag_span = tag.span;
                            let tag = self.identifier_text(tag.span).to_owned();
                            let payload_state = self.resolve_type(payload);
                            if tag == "Error" {
                                if saw_error {
                                    self.error(tag_span, "duplicate Error alternative");
                                    failed = true;
                                }
                                saw_error = true;
                                if !std::ptr::eq(alternative, alternatives.last().unwrap()) {
                                    self.error(alternative.span, "Error must be the final union alternative");
                                    failed = true;
                                }
                            } else {
                                if style == Some(UnionStyle::Untagged) {
                                    self.error(alternative.span, "tagged and untagged union alternatives cannot be mixed");
                                    failed = true;
                                }
                                style = Some(UnionStyle::Tagged);
                                if tags.iter().any(|existing| existing.as_ref() == tag.as_str()) {
                                    self.error(tag_span, format!("duplicate union tag '{tag}'"));
                                    failed = true;
                                } else {
                                    tags.push(tag.clone().into_boxed_str());
                                }
                            }
                            match payload_state {
                                TypeState::Resolved(payload) if tag == "Error" => {
                                    resolved.push(UnionAlternative::Error(payload));
                                }
                                TypeState::Resolved(payload) => {
                                    resolved.push(UnionAlternative::Tagged {
                                        tag: tag.into_boxed_str(),
                                        payload,
                                    });
                                }
                                _ => failed = true,
                            }
                        }
                        _ => {
                            if style == Some(UnionStyle::Tagged) {
                                self.error(alternative.span, "tagged and untagged union alternatives cannot be mixed");
                                failed = true;
                            }
                            style = Some(UnionStyle::Untagged);
                            match self.resolve_type(alternative) {
                                TypeState::Resolved(ty) => {
                                    if untagged.contains(&ty) {
                                        self.error(alternative.span, "duplicate untagged union alternative");
                                        failed = true;
                                    } else {
                                        untagged.push(ty);
                                    }
                                    resolved.push(UnionAlternative::Untagged(ty));
                                }
                                _ => failed = true,
                            }
                        }
                    }
                }
                if failed {
                    TypeState::Error
                } else {
                    resolved.sort();
                    TypeState::Resolved(
                        self.types
                            .intern(ResolvedType::Union(resolved.into_boxed_slice())),
                    )
                }
            }
            TypeKind::Tagged { payload, .. } => {
                self.resolve_type(payload);
                self.error(node.span, "tagged alternative is only valid within a union type")
            }
            TypeKind::Parenthesized(inner) => self.resolve_type(inner),
        };
        if let TypeKind::Union(alternatives) = &node.kind {
            for alternative in alternatives {
                if matches!(&alternative.kind, TypeKind::Tagged { .. }) {
                    self.annotate_type(alternative, state);
                }
            }
        }
        self.annotate_type(node, state);
        state
    }

    fn validate_referenced_storage(&mut self) {
        let mut invalid = Vec::new();
        for definition in &self.type_definitions {
            let TypeDefinitionKind::Struct(members) = &definition.kind else {
                continue;
            };
            for member in members {
                if member.storage == MemberStorage::Referenced {
                    if let TypeState::Resolved(ty) = member.ty
                        && !self.is_struct_type(ty)
                    {
                        invalid.push(member.node.span);
                    }
                }
            }
        }
        for span in invalid {
            self.error(span, "referenced storage '&' is permitted only for struct-valued members");
        }
    }

    fn is_struct_type(&self, ty: TypeId) -> bool {
        matches!(
            self.types.get(ty),
            ResolvedType::Nominal(id)
                if matches!(&self.type_definition(*id).kind, TypeDefinitionKind::Struct(_))
        )
    }

    fn validate_map_keys(&mut self) {
        let invalid = self
            .pending_map_keys
            .iter()
            .copied()
            .filter(|pending| !self.is_valid_map_key(pending.key, &mut Vec::new()))
            .map(|pending| pending.node.span)
            .collect::<Vec<_>>();
        for span in invalid {
            self.error(
                span,
                "map key type must be int, str, bool, or an immutable tuple of valid map keys",
            );
        }
    }

    fn is_valid_map_key(&self, ty: TypeId, visiting: &mut Vec<TypeDeclarationId>) -> bool {
        match self.types.get(ty) {
            ResolvedType::Primitive(
                PrimitiveType::Int | PrimitiveType::Str | PrimitiveType::Bool,
            ) => true,
            ResolvedType::Primitive(PrimitiveType::Float | PrimitiveType::Char) => false,
            ResolvedType::Nominal(id) => {
                if visiting.contains(id) {
                    return false;
                }
                let TypeDefinitionKind::Tuple(members) = &self.type_definition(*id).kind else {
                    return false;
                };
                visiting.push(*id);
                let valid = members.iter().all(|member| {
                    matches!(
                        member.ty,
                        TypeState::Resolved(member_ty)
                            if self.is_valid_map_key(member_ty, visiting)
                    )
                });
                visiting.pop();
                valid
            }
            ResolvedType::List(_) | ResolvedType::Map { .. } | ResolvedType::Union(_) => false,
        }
    }

    fn validate_inline_layouts(&mut self) {
        let recursive = self
            .type_definitions
            .iter()
            .filter_map(|definition| {
                self.layout_reaches(definition.id, definition.id, &mut Vec::new())
                    .then_some(definition.node.name.span)
            })
            .collect::<Vec<_>>();
        for span in recursive {
            let name = self.identifier_text(span).to_owned();
            self.error(
                span,
                format!("type '{name}' has an infinitely recursive inline layout"),
            );
        }
    }

    fn layout_reaches(
        &self,
        target: TypeDeclarationId,
        current: TypeDeclarationId,
        visited: &mut Vec<TypeDeclarationId>,
    ) -> bool {
        if visited.contains(&current) {
            return false;
        }
        visited.push(current);
        let mut dependencies = Vec::new();
        self.inline_dependencies(current, &mut dependencies);
        dependencies.into_iter().any(|dependency| {
            dependency == target || self.layout_reaches(target, dependency, visited)
        })
    }

    fn inline_dependencies(
        &self,
        id: TypeDeclarationId,
        dependencies: &mut Vec<TypeDeclarationId>,
    ) {
        match &self.type_definition(id).kind {
            TypeDefinitionKind::Struct(members) => {
                for member in members {
                    if member.storage == MemberStorage::Inline {
                        if let TypeState::Resolved(ty) = member.ty {
                            self.collect_inline_type_dependencies(ty, dependencies);
                        }
                    }
                }
            }
            TypeDefinitionKind::Tuple(members) => {
                for member in members {
                    if let TypeState::Resolved(ty) = member.ty {
                        self.collect_inline_type_dependencies(ty, dependencies);
                    }
                }
            }
            TypeDefinitionKind::Union { alternatives, .. } => {
                for alternative in alternatives {
                    let ty = match alternative {
                        UnionAlternative::Untagged(ty)
                        | UnionAlternative::Tagged { payload: ty, .. }
                        | UnionAlternative::Error(ty) => *ty,
                    };
                    self.collect_inline_type_dependencies(ty, dependencies);
                }
            }
            TypeDefinitionKind::Invalid => {}
        }
    }

    fn collect_inline_type_dependencies(
        &self,
        ty: TypeId,
        dependencies: &mut Vec<TypeDeclarationId>,
    ) {
        match self.types.get(ty) {
            ResolvedType::Primitive(_) | ResolvedType::List(_) | ResolvedType::Map { .. } => {}
            ResolvedType::Nominal(id) => dependencies.push(*id),
            ResolvedType::Union(alternatives) => {
                for alternative in alternatives.iter() {
                    let payload = match alternative {
                        UnionAlternative::Untagged(payload)
                        | UnionAlternative::Tagged { payload, .. }
                        | UnionAlternative::Error(payload) => *payload,
                    };
                    self.collect_inline_type_dependencies(payload, dependencies);
                }
            }
        }
    }

    fn parameter_binding(&self, parameter: &'ast Parameter) -> BindingId {
        self.bindings
            .iter()
            .find_map(|record| match record.node {
                BindingNode::Parameter(candidate) if std::ptr::eq(candidate, parameter) => {
                    Some(record.id)
                }
                _ => None,
            })
            .expect("every parameter receives a binding identity during the syntax census")
    }

    fn classify_entry_point(&self) -> EntryPoint {
        let mut mains = self.function_signatures.iter().filter(|signature| {
            self.identifier_text(signature.node.name.span) == "main"
        });
        let Some(first) = mains.next() else {
            return EntryPoint::Missing;
        };
        if let Some(duplicate) = mains.next() {
            return EntryPoint::Duplicate {
                first: first.id,
                duplicate: duplicate.id,
            };
        }
        if self.valid_main_signature(first) {
            EntryPoint::Valid(first.id)
        } else {
            EntryPoint::Invalid(first.id)
        }
    }

    fn valid_main_signature(&self, signature: &FunctionSignature<'_>) -> bool {
        let parameters_valid = match signature.parameters.as_slice() {
            [] => true,
            [parameter] => {
                !parameter.node.mutable
                    && self.identifier_text(parameter.node.name.span) == "args"
                    && matches!(
                        parameter.ty,
                        TypeState::Resolved(ty)
                            if matches!(
                                self.types.get(ty),
                                ResolvedType::List(element)
                                    if *element == self.types.primitive(PrimitiveType::Str)
                            )
                    )
            }
            _ => false,
        };
        let result_valid = signature.result == TypeState::NoValue
            || signature.result
                == TypeState::Resolved(self.types.primitive(PrimitiveType::Int));
        parameters_valid && result_valid
    }

    fn identifier_text(&self, span: Span) -> &str {
        &self.source.text[span.start..span.end]
    }
}

fn union_style(alternatives: &[UnionAlternative]) -> UnionStyle {
    if alternatives
        .iter()
        .any(|alternative| matches!(alternative, UnionAlternative::Tagged { .. }))
    {
        UnionStyle::Tagged
    } else {
        UnionStyle::Untagged
    }
}

/// Creates the analysis result, collects top-level namespaces, and resolves all
/// type definitions and function signatures before function bodies are considered.
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

    let types = TypeTable::new();
    let str_type = types.primitive(PrimitiveType::Str);
    let mut analysis = Analysis {
        source,
        program,
        declarations,
        bindings,
        types,
        type_annotations: Vec::new(),
        expression_annotations: Vec::new(),
        type_names: Vec::new(),
        function_names: Vec::new(),
        type_definitions: Vec::new(),
        function_signatures: Vec::new(),
        intrinsic_signatures: vec![
            IntrinsicSignature {
                id: IntrinsicId::Print,
                name: "print",
                arguments: IntrinsicArguments::OnePrintable,
                result: TypeState::NoValue,
            },
            IntrinsicSignature {
                id: IntrinsicId::Println,
                name: "println",
                arguments: IntrinsicArguments::ZeroOrOnePrintable,
                result: TypeState::NoValue,
            },
            IntrinsicSignature {
                id: IntrinsicId::Panic,
                name: "panic",
                arguments: IntrinsicArguments::Exact(Box::new([str_type])),
                result: TypeState::Never,
            },
        ],
        entry_point: EntryPoint::Missing,
        deferred: Vec::new(),
        diagnostics: Diagnostics::new(),
        pending_map_keys: Vec::new(),
    };
    analysis.collect_top_level_names();
    analysis.resolve_type_definitions();
    analysis.resolve_function_signatures();
    analysis.validate_referenced_storage();
    analysis.validate_map_keys();
    analysis.validate_inline_layouts();
    analysis.entry_point = analysis.classify_entry_point();
    analysis
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
            concat!(
                "type Pair(a int, b int); ",
                "fn first(arg int) { x := 1; for item in [x] { y := item; } } ",
                "fn second() {}",
            ),
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

    #[test]
    fn collects_all_names_before_resolving_function_signatures() {
        let source = source(
            "fn make(value Later) [Later] {} fn recurse() int {} type Later(int); fn main() {}",
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);

        let later = analysis.type_by_name("Later").unwrap();
        let make = analysis.function_by_name("make").unwrap();
        let recurse = analysis.function_by_name("recurse").unwrap();
        let signature = analysis.function_signature(make);
        let TypeState::Resolved(parameter) = signature.parameters[0].ty else {
            panic!("forward type reference should resolve");
        };
        assert_eq!(analysis.types.get(parameter), &ResolvedType::Nominal(later));
        let TypeState::Resolved(result) = signature.result else {
            panic!("container return type should resolve");
        };
        assert_eq!(
            analysis.types.get(result),
            &ResolvedType::List(parameter)
        );
        assert_eq!(
            analysis.callable_by_name("recurse"),
            CallableResolution::Found(CallableId::Function(recurse))
        );
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn keeps_type_and_value_namespaces_separate_and_calls_unambiguous() {
        let source = source(
            concat!(
                "type Same(int); type OnlyType(int); ",
                "fn Same(value int) int {} fn onlyFunction() {} fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);

        let same_type = analysis.type_by_name("Same").unwrap();
        let same_function = analysis.function_by_name("Same").unwrap();
        assert_eq!(
            analysis.callable_by_name("Same"),
            CallableResolution::Ambiguous {
                value: CallableId::Function(same_function),
                constructor: same_type,
            }
        );
        assert!(matches!(
            analysis.callable_by_name("OnlyType"),
            CallableResolution::Found(CallableId::Constructor(_))
        ));
        assert!(matches!(
            analysis.callable_by_name("onlyFunction"),
            CallableResolution::Found(CallableId::Function(_))
        ));
        assert_eq!(
            analysis.callable_by_name("missing"),
            CallableResolution::Unknown
        );
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn diagnoses_duplicates_unknown_types_and_intrinsic_collisions() {
        let source = source(
            concat!(
                "type Item(int); type Item(str); ",
                "fn f(a Missing, a int) {} fn f() {} fn print() {} fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();

        assert_eq!(analysis.diagnostics.len(), 5);
        for expected in [
            "duplicate type declaration 'Item'",
            "duplicate parameter name 'a'",
            "duplicate function declaration 'f'",
            "function name 'print' is reserved for a compiler intrinsic",
            "unknown type 'Missing'",
        ] {
            assert!(diagnostics.contains(expected), "{diagnostics}");
        }
    }

    #[test]
    fn registers_intrinsics_with_explicit_signature_rules() {
        let source = source("fn main() {}");
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);

        assert_eq!(analysis.intrinsic_signatures.len(), 3);
        assert_eq!(
            analysis.callable_by_name("print"),
            CallableResolution::Found(CallableId::Intrinsic(IntrinsicId::Print))
        );
        assert_eq!(
            analysis.callable_by_name("println"),
            CallableResolution::Found(CallableId::Intrinsic(IntrinsicId::Println))
        );
        assert_eq!(
            analysis.intrinsic_signature(IntrinsicId::Print).arguments,
            IntrinsicArguments::OnePrintable
        );
        assert_eq!(
            analysis
                .intrinsic_signature(IntrinsicId::Println)
                .arguments,
            IntrinsicArguments::ZeroOrOnePrintable
        );
        let panic = analysis.intrinsic_signature(IntrinsicId::Panic);
        assert_eq!(panic.result, TypeState::Never);
        assert_eq!(
            panic.arguments,
            IntrinsicArguments::Exact(Box::new([
                analysis.types.primitive(PrimitiveType::Str)
            ]))
        );
    }

    #[test]
    fn makes_recursive_function_names_available_independently_of_body_order() {
        let source = source(
            "fn first() int { second() } fn second() int { first() } fn main() {}",
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);

        let first = analysis.function_by_name("first").unwrap();
        let second = analysis.function_by_name("second").unwrap();
        assert_eq!(
            analysis.callable_by_name("first"),
            CallableResolution::Found(CallableId::Function(first))
        );
        assert_eq!(
            analysis.callable_by_name("second"),
            CallableResolution::Found(CallableId::Function(second))
        );
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn classifies_type_definitions_and_preserves_nominal_identity() {
        let source = source(
            concat!(
                "type Left(value int); type Right(value int); ",
                "type Pair(int, str); type Choice(int | str); fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);

        let left = analysis.type_by_name("Left").unwrap();
        let right = analysis.type_by_name("Right").unwrap();
        assert_ne!(ResolvedType::Nominal(left), ResolvedType::Nominal(right));
        assert!(matches!(
            &analysis.type_definition(left).kind,
            TypeDefinitionKind::Struct(members) if members.len() == 1
        ));
        assert!(matches!(
            &analysis.type_definition(analysis.type_by_name("Pair").unwrap()).kind,
            TypeDefinitionKind::Tuple(members) if members.len() == 2
        ));
        assert!(matches!(
            &analysis.type_definition(analysis.type_by_name("Choice").unwrap()).kind,
            TypeDefinitionKind::Union {
                style: UnionStyle::Untagged,
                alternatives,
            } if alternatives.len() == 2
        ));
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn validates_member_and_union_forms() {
        let source = source(
            concat!(
                "type Mixed(a int, str); type Fields(a int, a str); ",
                "type Duplicate(int | int); type Tags(A(int) | A(str)); ",
                "type Hybrid(A(int) | str); type BadError(Error(str) | int); ",
                "fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();

        for expected in [
            "cannot mix named and unnamed members",
            "duplicate struct member 'a'",
            "duplicate untagged union alternative",
            "duplicate union tag 'A'",
            "tagged and untagged union alternatives cannot be mixed",
            "Error must be the final union alternative",
        ] {
            assert!(diagnostics.contains(expected), "{diagnostics}");
        }
    }

    #[test]
    fn union_identity_ignores_order_but_preserves_explicit_nesting() {
        let source = source(
            concat!(
                "fn first(value int | str) {} fn second(value str | int) {} ",
                "fn nested(value (int | str) | bool) {} fn flat(value int | str | bool) {} ",
                "fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let parameter_type = |name| {
            analysis
                .function_signature(analysis.function_by_name(name).unwrap())
                .parameters[0]
                .ty
        };

        assert_eq!(parameter_type("first"), parameter_type("second"));
        assert_ne!(parameter_type("nested"), parameter_type("flat"));
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn validates_referenced_storage_and_recursive_inline_layouts() {
        let source = source(
            concat!(
                "type Node(next &Node); type Safe(items [Safe]); ",
                "type Pair(int, int); type BadReference(pair &Pair, count &int); ",
                "type A(b B); type B(A); type Link(C | int); type C(link Link); ",
                "fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();

        assert_eq!(diagnostics.matches("referenced storage '&'").count(), 2);
        for name in ["A", "B", "Link", "C"] {
            assert!(
                diagnostics.contains(&format!(
                    "type '{name}' has an infinitely recursive inline layout"
                )),
                "{diagnostics}",
            );
        }
        assert!(!diagnostics.contains("type 'Node' has an infinitely"));
        assert!(!diagnostics.contains("type 'Safe' has an infinitely"));
    }

    #[test]
    fn accepts_composed_tuple_map_keys_and_rejects_other_keys() {
        let source = source(
            concat!(
                "type Inner(str, bool); type Key(int, Inner); type Object(value int); ",
                "fn valid(table {Key: str}) {} ",
                "fn invalidObject(table {Object: str}) {} ",
                "fn invalidList(table {[int]: str}) {} ",
                "fn invalidFloat(table {float: str}) {} fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();

        assert_eq!(
            diagnostics
                .matches("map key type must be int, str, bool, or an immutable tuple")
                .count(),
            3
        );
    }

    #[test]
    fn classifies_all_supported_entry_point_signatures() {
        for text in [
            "fn main() {}",
            "fn main() int {}",
            "fn main(args [str]) {}",
            "fn main(args [str]) int {}",
        ] {
            let source = source(text);
            let program = parser::parse(&source).unwrap();
            let analysis = analyze(&source, &program);
            assert!(matches!(analysis.entry_point, EntryPoint::Valid(_)), "{text}");
        }

        for text in ["fn main(value str) {}", "fn main() str {}"] {
            let source = source(text);
            let program = parser::parse(&source).unwrap();
            let analysis = analyze(&source, &program);
            assert!(
                matches!(analysis.entry_point, EntryPoint::Invalid(_)),
                "{text}"
            );
        }

        let helper_source = source("fn helper() {}");
        let program = parser::parse(&helper_source).unwrap();
        assert_eq!(
            analyze(&helper_source, &program).entry_point,
            EntryPoint::Missing
        );

        let duplicate_source = source("fn main() {} fn main() {}");
        let program = parser::parse(&duplicate_source).unwrap();
        assert!(matches!(
            analyze(&duplicate_source, &program).entry_point,
            EntryPoint::Duplicate { .. }
        ));
    }
}
