//! Name-and-type analysis representation.
//!
//! This module establishes the analysis boundary and stable identities without
//! adding semantic data to the parser AST. It collects top-level names, resolves
//! type definitions and function signatures, resolves lexical bindings and
//! callable targets, and records expression types and constructor choices.

use crate::ast::{
    ArgumentKind, AssignmentOperator, AssignmentTarget, AssignmentTargetSuffixKind, BinaryOperator,
    Block, Declaration, Expression, ExpressionBody, ExpressionBodyKind, ExpressionKind,
    FunctionDeclaration, Identifier, Member, Parameter, PrimitiveType, Program, Statement,
    StatementBody, StatementBodyKind, StatementKind, Type, TypeDeclaration, TypeKind, TypeMember,
    TypeMemberKind, UnaryOperator,
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

impl BindingId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TypeId(usize);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DeferredId(usize);

impl DeferredId {
    pub(crate) fn index(self) -> usize {
        self.0
    }
}

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
    AssignmentTarget(&'ast AssignmentTarget),
}

impl AstNode<'_> {
    pub(crate) fn span(self) -> Span {
        match self {
            Self::Declaration(node) => node.span(),
            Self::Binding(node) => node.span(),
            Self::Type(ty) => ty.span,
            Self::Statement(statement) => statement.span,
            Self::Expression(expression) => expression.span,
            Self::AssignmentTarget(target) => target.span,
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
    pub(crate) mutable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BindingAccess {
    Read,
    Write,
    ReadWrite,
    Mutate,
    ReadMutate,
    Call,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NameResolution {
    Binding(BindingId),
    Callable(CallableId),
    ErrorConstructor,
    AmbiguousErrorConstructor {
        declared: CallableId,
    },
    AmbiguousCall {
        value: CallableId,
        constructor: TypeDeclarationId,
    },
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct NameUse<'ast> {
    pub(crate) node: &'ast Identifier,
    pub(crate) access: BindingAccess,
    pub(crate) resolution: NameResolution,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CallTarget {
    Binding(BindingId),
    Callable(CallableId),
    Builtin(BuiltinMethod),
    QualifiedConstructor(TypeDeclarationId),
    ErrorConstructor,
    AmbiguousErrorConstructor {
        declared: CallableId,
    },
    Ambiguous {
        value: CallableId,
        constructor: TypeDeclarationId,
    },
    Expression,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CallResolution<'ast> {
    pub(crate) node: &'ast Expression,
    pub(crate) callee: &'ast Expression,
    pub(crate) target: CallTarget,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuiltinMethod {
    ListAppend,
    ListRemoveIndex,
    ListLen,
    MapRemoveKey,
    MapLen,
    StrLen,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LiteralValue {
    Integer(u64),
    Float(f64),
    String(Box<[u8]>),
    Character(u8),
    Boolean(bool),
}

#[derive(Clone, Debug)]
pub(crate) struct LiteralAnnotation<'ast> {
    pub(crate) node: &'ast Expression,
    pub(crate) value: LiteralValue,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct BindingTypeAnnotation {
    pub(crate) binding: BindingId,
    pub(crate) state: TypeState,
}

#[derive(Clone, Debug)]
pub(crate) struct AssignmentTargetAnnotation<'ast> {
    pub(crate) node: &'ast AssignmentTarget,
    pub(crate) root: Option<BindingId>,
    pub(crate) steps: Box<[AccessPathStep<'ast>]>,
    pub(crate) state: TypeState,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum AccessPathStep<'ast> {
    StructMember {
        suffix: &'ast crate::ast::AssignmentTargetSuffix,
        declaration: TypeDeclarationId,
        member: &'ast TypeMember,
        storage: MemberStorage,
        state: TypeState,
    },
    TupleMember {
        suffix: &'ast crate::ast::AssignmentTargetSuffix,
        declaration: TypeDeclarationId,
        member: &'ast TypeMember,
        position: usize,
        state: TypeState,
    },
    ListIndex {
        suffix: &'ast crate::ast::AssignmentTargetSuffix,
        element: TypeId,
        state: TypeState,
    },
    MapIndex {
        suffix: &'ast crate::ast::AssignmentTargetSuffix,
        key: TypeId,
        value: TypeId,
        state: TypeState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ConstructorKind {
    Struct {
        declaration: TypeDeclarationId,
        argument_members: Box<[usize]>,
    },
    Tuple(TypeDeclarationId),
    Union {
        declaration: TypeDeclarationId,
        alternative: UnionAlternative,
    },
    TaggedUnion {
        declaration: TypeDeclarationId,
        alternative: UnionAlternative,
    },
    Error {
        union_type: TypeId,
        alternative: UnionAlternative,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ConstructorResolution<'ast> {
    pub(crate) node: &'ast Expression,
    pub(crate) kind: ConstructorKind,
}

#[derive(Clone, Debug)]
pub(crate) struct UnionInjection<'ast> {
    pub(crate) node: &'ast Expression,
    pub(crate) union_type: TypeId,
    pub(crate) alternative: UnionAlternative,
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
pub(crate) enum DeferredReason {
    FlowDependentType,
    ExpectedType,
    Constructor,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct DeferredAnalysis<'ast> {
    pub(crate) id: DeferredId,
    pub(crate) node: AstNode<'ast>,
    pub(crate) reason: DeferredReason,
    pub(crate) resolved: bool,
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
///
/// A diagnostic-free result is the milestone-4 resolved-AST input, not yet a
/// complete semantic proof. Milestone 5 must enforce mutability and control-flow
/// rules, validate return paths and exhaustive switches, perform union narrowing
/// and postfix-`?` analysis, and complete every unresolved flow-dependent record
/// before the affected expression can be lowered.
#[derive(Debug)]
pub(crate) struct Analysis<'source, 'ast> {
    pub(crate) source: &'source SourceFile,
    pub(crate) program: &'ast Program,
    pub(crate) declarations: Vec<DeclarationRecord<'ast>>,
    pub(crate) bindings: Vec<BindingRecord<'ast>>,
    pub(crate) name_uses: Vec<NameUse<'ast>>,
    pub(crate) calls: Vec<CallResolution<'ast>>,
    pub(crate) literals: Vec<LiteralAnnotation<'ast>>,
    pub(crate) binding_types: Vec<BindingTypeAnnotation>,
    pub(crate) assignment_targets: Vec<AssignmentTargetAnnotation<'ast>>,
    pub(crate) constructors: Vec<ConstructorResolution<'ast>>,
    pub(crate) union_injections: Vec<UnionInjection<'ast>>,
    pub(crate) types: TypeTable,
    pub(crate) type_annotations: Vec<TypeAnnotation<'ast>>,
    pub(crate) expression_annotations: Vec<ExpressionAnnotation<'ast>>,
    pub(crate) type_names: Vec<TypeName>,
    pub(crate) function_names: Vec<FunctionName>,
    pub(crate) type_definitions: Vec<TypeDefinition<'ast>>,
    pub(crate) function_signatures: Vec<FunctionSignature<'ast>>,
    pub(crate) intrinsic_signatures: Vec<IntrinsicSignature>,
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

    pub(crate) fn resolve_deferred(&mut self, id: DeferredId) {
        let deferred = &mut self.deferred[id.index()];
        assert_eq!(deferred.id, id);
        assert!(!deferred.resolved, "deferred analysis record resolved more than once");
        deferred.resolved = true;
    }

    pub(crate) fn defer(
        &mut self,
        node: AstNode<'ast>,
        reason: DeferredReason,
    ) -> DeferredId {
        let id = DeferredId(self.deferred.len());
        self.deferred.push(DeferredAnalysis {
            id,
            node,
            reason,
            resolved: false,
        });
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

    pub(crate) fn binding(&self, id: BindingId) -> &BindingRecord<'ast> {
        let binding = &self.bindings[id.0];
        debug_assert_eq!(binding.id, id);
        binding
    }

    pub(crate) fn name_use(&self, node: &'ast Identifier) -> Option<&NameUse<'ast>> {
        self.name_uses
            .iter()
            .find(|name_use| std::ptr::eq(name_use.node, node))
    }

    pub(crate) fn call_resolution(
        &self,
        node: &'ast Expression,
    ) -> Option<&CallResolution<'ast>> {
        self.calls
            .iter()
            .find(|resolution| std::ptr::eq(resolution.node, node))
    }

    pub(crate) fn expression_annotation(
        &self,
        node: &'ast Expression,
    ) -> Option<&ExpressionAnnotation<'ast>> {
        self.expression_annotations
            .iter()
            .rev()
            .find(|annotation| std::ptr::eq(annotation.node, node))
    }

    pub(crate) fn literal(&self, node: &'ast Expression) -> Option<&LiteralValue> {
        self.literals
            .iter()
            .find_map(|literal| std::ptr::eq(literal.node, node).then_some(&literal.value))
    }

    pub(crate) fn binding_type(&self, id: BindingId) -> TypeState {
        let binding = self.binding_types[id.0];
        debug_assert_eq!(binding.binding, id);
        binding.state
    }

    pub(crate) fn assignment_target(
        &self,
        node: &'ast AssignmentTarget,
    ) -> Option<&AssignmentTargetAnnotation<'ast>> {
        self.assignment_targets
            .iter()
            .rev()
            .find(|annotation| std::ptr::eq(annotation.node, node))
    }

    fn assignment_target_annotation(
        &self,
        node: &'ast AssignmentTarget,
        state: TypeState,
    ) -> AssignmentTargetAnnotation<'ast> {
        let root = self.name_use(&node.root).and_then(|name_use| match name_use.resolution {
            NameResolution::Binding(binding) => Some(binding),
            _ => None,
        });
        let mut current = root.map_or(TypeState::Error, |binding| self.binding_type(binding));
        let mut steps = Vec::with_capacity(node.suffixes.len());
        for suffix in &node.suffixes {
            let TypeState::Resolved(receiver) = current else {
                break;
            };
            let step = match (self.types.get(receiver), &suffix.kind) {
                (ResolvedType::List(element), AssignmentTargetSuffixKind::Index(_)) => {
                    current = TypeState::Resolved(*element);
                    AccessPathStep::ListIndex {
                        suffix,
                        element: *element,
                        state: current,
                    }
                }
                (
                    ResolvedType::Map { key, value },
                    AssignmentTargetSuffixKind::Index(_),
                ) => {
                    current = TypeState::Resolved(*value);
                    AccessPathStep::MapIndex {
                        suffix,
                        key: *key,
                        value: *value,
                        state: current,
                    }
                }
                (
                    ResolvedType::Nominal(declaration),
                    AssignmentTargetSuffixKind::Member(Member::Named(name)),
                ) => {
                    let TypeDefinitionKind::Struct(members) =
                        &self.type_definition(*declaration).kind
                    else {
                        break;
                    };
                    let spelling = self.identifier_text(name.span);
                    let Some(member) = members
                        .iter()
                        .find(|member| self.identifier_text(member.name.span) == spelling)
                    else {
                        break;
                    };
                    current = member.ty;
                    AccessPathStep::StructMember {
                        suffix,
                        declaration: *declaration,
                        member: member.node,
                        storage: member.storage,
                        state: current,
                    }
                }
                (
                    ResolvedType::Nominal(declaration),
                    AssignmentTargetSuffixKind::Member(Member::TupleIndex(span)),
                ) => {
                    let TypeDefinitionKind::Tuple(members) =
                        &self.type_definition(*declaration).kind
                    else {
                        break;
                    };
                    let Some(position) = self
                        .identifier_text(*span)
                        .replace('_', "")
                        .parse::<usize>()
                        .ok()
                    else {
                        break;
                    };
                    let Some(member) = members.get(position) else {
                        break;
                    };
                    current = member.ty;
                    AccessPathStep::TupleMember {
                        suffix,
                        declaration: *declaration,
                        member: member.node,
                        position,
                        state: current,
                    }
                }
                _ => break,
            };
            steps.push(step);
        }
        AssignmentTargetAnnotation {
            node,
            root,
            steps: steps.into_boxed_slice(),
            state,
        }
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

    fn finalize_call_targets(&mut self) {
        let targets = self
            .calls
            .iter()
            .map(|call| self.final_call_target(*call))
            .collect::<Vec<_>>();
        for (call, target) in self.calls.iter_mut().zip(targets) {
            call.target = target;
        }
    }

    fn final_call_target(&self, call: CallResolution<'ast>) -> CallTarget {
        match call.target {
            CallTarget::Callable(CallableId::Function(_)) | CallTarget::Builtin(_) => {
                return call.target;
            }
            _ => {}
        }
        if let ExpressionKind::Identifier(identifier) = &call.callee.kind
            && let Some(NameResolution::Callable(CallableId::Function(function))) =
                self.name_use(identifier).map(|name_use| name_use.resolution)
        {
            return CallTarget::Callable(CallableId::Function(function));
        }
        let ExpressionKind::Member {
            value: receiver,
            member: Member::Named(name),
        } = &call.callee.kind
        else {
            return call.target;
        };
        let Some(TypeState::Resolved(receiver)) = self
            .expression_annotation(receiver)
            .map(|annotation| annotation.state)
        else {
            return call.target;
        };
        let spelling = self.identifier_text(name.span);
        let method = match self.types.get(receiver) {
            ResolvedType::List(_) => match spelling {
                "append" => Some(BuiltinMethod::ListAppend),
                "removeIndex" => Some(BuiltinMethod::ListRemoveIndex),
                "len" => Some(BuiltinMethod::ListLen),
                _ => None,
            },
            ResolvedType::Map { .. } => match spelling {
                "removeKey" => Some(BuiltinMethod::MapRemoveKey),
                "len" => Some(BuiltinMethod::MapLen),
                _ => None,
            },
            ResolvedType::Primitive(PrimitiveType::Str) if spelling == "len" => {
                Some(BuiltinMethod::StrLen)
            }
            _ => None,
        };
        method.map_or(call.target, CallTarget::Builtin)
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

    fn reject_reserved_error_bindings(&mut self) {
        let mut spans = Vec::new();
        for declaration in &self.program.declarations {
            match declaration {
                Declaration::Type(declaration) => {
                    spans.push(declaration.name.span);
                    for member in &declaration.members {
                        if let TypeMemberKind::Named { name, .. } = &member.kind {
                            spans.push(name.span);
                        }
                    }
                }
                Declaration::Function(function) => {
                    spans.push(function.name.span);
                }
            }
        }
        spans.extend(self.bindings.iter().map(|binding| binding.span));
        for span in spans {
            if self.identifier_text(span) == "Error" {
                self.diagnostics.push(Diagnostic::source(
                    self.source,
                    span,
                    "'Error' is reserved for the built-in error alternative and constructor",
                ));
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

    pub(crate) fn local_binding(&self, statement: &'ast Statement) -> Option<BindingId> {
        self.bindings.iter().find_map(|record| match record.node {
            BindingNode::Local(candidate) if std::ptr::eq(candidate, statement) => Some(record.id),
            _ => None,
        })
    }

    fn statement_binding(&self, statement: &'ast Statement) -> BindingId {
        self.bindings
            .iter()
            .find_map(|record| match record.node {
                BindingNode::Local(candidate) | BindingNode::Loop(candidate)
                    if std::ptr::eq(candidate, statement) =>
                {
                    Some(record.id)
                }
                _ => None,
            })
            .expect("every local and loop binding receives an identity during the syntax census")
    }

    fn resolve_function_bodies(&mut self) {
        let functions: Vec<&'ast FunctionDeclaration> = self
            .declarations
            .iter()
            .filter_map(|record| match record.node {
                DeclarationNode::Function(function) => Some(function),
                DeclarationNode::Type(_) => None,
            })
            .collect::<Vec<_>>();
        for function in functions {
            BodyResolver::new(self).resolve_function(function);
        }
    }

    fn infer_function_bodies(&mut self) {
        let parameter_types = self
            .function_signatures
            .iter()
            .flat_map(|signature| {
                signature
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.binding, parameter.ty))
            })
            .collect::<Vec<_>>();
        for (binding, state) in parameter_types {
            self.binding_types[binding.0].state = state;
        }
        let functions: Vec<&'ast FunctionDeclaration> = self
            .declarations
            .iter()
            .filter_map(|record| match record.node {
                DeclarationNode::Function(function) => Some(function),
                DeclarationNode::Type(_) => None,
            })
            .collect();
        for function in functions {
            TypeInferrer::new(self).infer_function(function);
        }
    }

    fn resolve_expected_types(&mut self) {
        let functions = self
            .function_signatures
            .iter()
            .map(|signature| (signature.node, signature.result))
            .collect::<Vec<_>>();
        for (function, result) in functions {
            let expected = match result {
                TypeState::Resolved(ty) => Some(ty),
                _ => None,
            };
            ExpectedTypeResolver::new(self, result)
                .resolve_block(&function.body, expected);
        }
    }

    pub(crate) fn identifier_text(&self, span: Span) -> &str {
        &self.source.text[span.start..span.end]
    }
}

#[derive(Debug)]
struct Scope {
    bindings: Vec<(Box<str>, BindingId)>,
    accepts_locals: bool,
}

impl Scope {
    fn lexical() -> Self {
        Self {
            bindings: Vec::new(),
            accepts_locals: true,
        }
    }

    fn loop_binding(name: Box<str>, binding: BindingId) -> Self {
        Self {
            bindings: vec![(name, binding)],
            accepts_locals: false,
        }
    }
}

struct BodyResolver<'analysis, 'source, 'ast> {
    analysis: &'analysis mut Analysis<'source, 'ast>,
    scopes: Vec<Scope>,
}

impl<'analysis, 'source, 'ast> BodyResolver<'analysis, 'source, 'ast> {
    fn new(analysis: &'analysis mut Analysis<'source, 'ast>) -> Self {
        Self {
            analysis,
            scopes: Vec::new(),
        }
    }

    fn resolve_function(&mut self, function: &'ast FunctionDeclaration) {
        self.scopes.push(Scope::lexical());
        for parameter in &function.parameters {
            let name = self.analysis.identifier_text(parameter.name.span).to_owned();
            let binding = self.analysis.parameter_binding(parameter);
            let scope = self.scopes.last_mut().unwrap();
            if !scope
                .bindings
                .iter()
                .any(|(existing, _)| existing.as_ref() == name.as_str())
            {
                scope.bindings.push((name.into_boxed_str(), binding));
            }
        }
        self.resolve_block(&function.body);
        self.scopes.pop();
    }

    fn resolve_block(&mut self, block: &'ast Block) {
        self.scopes.push(Scope::lexical());
        for statement in &block.statements {
            self.resolve_statement(statement);
        }
        if let Some(value) = &block.value {
            self.resolve_expression(value);
        }
        self.scopes.pop();
    }

    fn resolve_statement(&mut self, statement: &'ast Statement) {
        match &statement.kind {
            StatementKind::Local {
                name, initializer, ..
            } => {
                self.resolve_expression(initializer);
                let binding = self.analysis.statement_binding(statement);
                self.declare_local(name, binding);
            }
            StatementKind::Assignment {
                target,
                operator,
                value,
                ..
            } => {
                self.resolve_assignment_target(target, *operator);
                self.resolve_expression(value);
            }
            StatementKind::Expression(expression) => self.resolve_expression(expression),
            StatementKind::Return(value) => {
                if let Some(value) = value {
                    self.resolve_expression(value);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
            StatementKind::If {
                branches,
                else_body,
            } => {
                for branch in branches {
                    self.resolve_expression(&branch.condition);
                    self.resolve_statement_body(&branch.body);
                }
                if let Some(body) = else_body {
                    self.resolve_statement_body(body);
                }
            }
            StatementKind::While { condition, body } => {
                self.resolve_expression(condition);
                self.resolve_statement_body(body);
            }
            StatementKind::For {
                binding,
                iterable,
                body,
            } => {
                self.resolve_expression(iterable);
                let id = self.analysis.statement_binding(statement);
                let name = self.analysis.identifier_text(binding.span).to_owned();
                self.scopes
                    .push(Scope::loop_binding(name.into_boxed_str(), id));
                self.resolve_statement_body(body);
                self.scopes.pop();
            }
            StatementKind::Switch {
                value,
                arms,
                else_body,
            } => {
                self.resolve_expression(value);
                for arm in arms {
                    self.resolve_statement_body(&arm.body);
                }
                if let Some(body) = else_body {
                    self.resolve_statement_body(body);
                }
            }
            StatementKind::Block(block) => self.resolve_block(block),
        }
    }

    fn resolve_statement_body(&mut self, body: &'ast StatementBody) {
        match &body.kind {
            StatementBodyKind::Block(block) => self.resolve_block(block),
            StatementBodyKind::Statement(statement) => self.resolve_statement(statement),
        }
    }

    fn resolve_assignment_target(
        &mut self,
        target: &'ast AssignmentTarget,
        operator: AssignmentOperator,
    ) {
        let access = match (operator, target.suffixes.is_empty()) {
            (AssignmentOperator::Assign, true) => BindingAccess::Write,
            (AssignmentOperator::Assign, false) => BindingAccess::Mutate,
            (_, true) => BindingAccess::ReadWrite,
            (_, false) => BindingAccess::ReadMutate,
        };
        self.resolve_binding_name(&target.root, access);
        for suffix in &target.suffixes {
            if let AssignmentTargetSuffixKind::Index(index) = &suffix.kind {
                self.resolve_expression(index);
            }
        }
    }

    fn resolve_expression(&mut self, expression: &'ast Expression) {
        match &expression.kind {
            ExpressionKind::Identifier(identifier) => {
                self.resolve_value_name(identifier, BindingAccess::Read);
            }
            ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_)
            | ExpressionKind::TypedEmptyList(_)
            | ExpressionKind::TypedEmptyMap(_) => {}
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Unary { operand: inner, .. }
            | ExpressionKind::Try { value: inner, .. } => self.resolve_expression(inner),
            ExpressionKind::List(elements) => {
                for element in elements {
                    self.resolve_expression(element);
                }
            }
            ExpressionKind::Map(entries) => {
                for entry in entries {
                    self.resolve_expression(&entry.key);
                    self.resolve_expression(&entry.value);
                }
            }
            ExpressionKind::Block(block) => self.resolve_block(block),
            ExpressionKind::If {
                branches,
                else_branch,
            } => {
                for branch in branches {
                    self.resolve_expression(&branch.condition);
                    self.resolve_expression_body(&branch.body);
                }
                self.resolve_expression_body(else_branch);
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.resolve_expression(left);
                self.resolve_expression(right);
            }
            ExpressionKind::Is { value, .. } => self.resolve_expression(value),
            ExpressionKind::Call { callee, arguments } => {
                let target = self.resolve_call_callee(callee);
                self.analysis.calls.push(CallResolution {
                    node: expression,
                    callee,
                    target,
                });
                for argument in arguments {
                    match &argument.kind {
                        ArgumentKind::Positional(value)
                        | ArgumentKind::Named { value, .. } => self.resolve_expression(value),
                    }
                }
            }
            ExpressionKind::Index { value, index } => {
                self.resolve_expression(value);
                self.resolve_expression(index);
            }
            ExpressionKind::Member { value, .. } => self.resolve_expression(value),
        }
    }

    fn resolve_expression_body(&mut self, body: &'ast ExpressionBody) {
        match &body.kind {
            ExpressionBodyKind::Block(block) => self.resolve_block(block),
            ExpressionBodyKind::Expression(expression) => self.resolve_expression(expression),
        }
    }

    fn resolve_call_callee(&mut self, callee: &'ast Expression) -> CallTarget {
        let ExpressionKind::Identifier(identifier) = &callee.kind else {
            if let ExpressionKind::Member {
                value,
                member: Member::Named(_),
            } = &callee.kind
                && let ExpressionKind::Identifier(root) = &value.kind
            {
                let name = self.analysis.identifier_text(root.span).to_owned();
                if self.lookup(&name).is_none()
                    && let Some(constructor) = self.analysis.type_by_name(&name)
                {
                    self.record_name(
                        root,
                        BindingAccess::Call,
                        NameResolution::Callable(CallableId::Constructor(constructor)),
                    );
                    return CallTarget::QualifiedConstructor(constructor);
                }
            }
            self.resolve_expression(callee);
            return CallTarget::Expression;
        };
        let name = self.analysis.identifier_text(identifier.span).to_owned();
        if let Some(binding) = self.lookup(&name) {
            self.record_name(identifier, BindingAccess::Call, NameResolution::Binding(binding));
            return CallTarget::Binding(binding);
        }
        match self.analysis.callable_by_name(&name) {
            CallableResolution::Found(callable) if name == "Error" => {
                self.analysis.error(
                    identifier.span,
                    "call target 'Error' is ambiguous between a declared callable and the special error constructor",
                );
                self.record_name(
                    identifier,
                    BindingAccess::Call,
                    NameResolution::AmbiguousErrorConstructor { declared: callable },
                );
                CallTarget::AmbiguousErrorConstructor { declared: callable }
            }
            CallableResolution::Found(callable) => {
                self.record_name(
                    identifier,
                    BindingAccess::Call,
                    NameResolution::Callable(callable),
                );
                CallTarget::Callable(callable)
            }
            CallableResolution::Ambiguous { value, constructor } => {
                self.analysis.error(
                    identifier.span,
                    format!(
                        "call target '{name}' is ambiguous between a value declaration and a type constructor"
                    ),
                );
                self.record_name(
                    identifier,
                    BindingAccess::Call,
                    NameResolution::AmbiguousCall { value, constructor },
                );
                CallTarget::Ambiguous { value, constructor }
            }
            CallableResolution::Unknown => {
                if name == "Error" {
                    self.record_name(
                        identifier,
                        BindingAccess::Call,
                        NameResolution::ErrorConstructor,
                    );
                    CallTarget::ErrorConstructor
                } else {
                    self.unknown(identifier, &name, BindingAccess::Call);
                    CallTarget::Unknown
                }
            }
        }
    }

    fn resolve_value_name(&mut self, identifier: &'ast Identifier, access: BindingAccess) {
        let name = self.analysis.identifier_text(identifier.span).to_owned();
        if let Some(binding) = self.lookup(&name) {
            self.record_name(identifier, access, NameResolution::Binding(binding));
            return;
        }
        if let Some(function) = self.analysis.function_by_name(&name) {
            let callable = CallableId::Function(function);
            self.analysis.error(
                identifier.span,
                format!("function '{name}' can only be used as a direct call target"),
            );
            self.record_name(identifier, access, NameResolution::Callable(callable));
        } else if let Some(intrinsic) = self.analysis.intrinsic_by_name(&name) {
            let callable = CallableId::Intrinsic(intrinsic);
            self.analysis.error(
                identifier.span,
                format!("intrinsic '{name}' can only be used as a direct call target"),
            );
            self.record_name(identifier, access, NameResolution::Callable(callable));
        } else {
            self.unknown(identifier, &name, access);
        }
    }

    fn resolve_binding_name(&mut self, identifier: &'ast Identifier, access: BindingAccess) {
        let name = self.analysis.identifier_text(identifier.span).to_owned();
        if let Some(binding) = self.lookup(&name) {
            self.record_name(identifier, access, NameResolution::Binding(binding));
        } else {
            self.unknown(identifier, &name, access);
        }
    }

    fn unknown(&mut self, identifier: &'ast Identifier, name: &str, access: BindingAccess) {
        self.analysis
            .error(identifier.span, format!("unknown value '{name}'"));
        self.record_name(identifier, access, NameResolution::Unknown);
    }

    fn record_name(
        &mut self,
        identifier: &'ast Identifier,
        access: BindingAccess,
        resolution: NameResolution,
    ) {
        self.analysis.name_uses.push(NameUse {
            node: identifier,
            access,
            resolution,
        });
    }

    fn lookup(&self, name: &str) -> Option<BindingId> {
        self.scopes.iter().rev().find_map(|scope| {
            scope
                .bindings
                .iter()
                .rev()
                .find_map(|(candidate, binding)| (candidate.as_ref() == name).then_some(*binding))
        })
    }

    fn declare_local(&mut self, identifier: &'ast Identifier, binding: BindingId) {
        let name = self
            .analysis
            .identifier_text(identifier.span)
            .to_owned()
            .into_boxed_str();
        self.scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.accepts_locals)
            .expect("a function body always has a lexical scope")
            .bindings
            .push((name, binding));
    }
}

struct TypeInferrer<'analysis, 'source, 'ast> {
    analysis: &'analysis mut Analysis<'source, 'ast>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Compatibility {
    Match,
    Deferred,
    Invalid,
}

impl<'analysis, 'source, 'ast> TypeInferrer<'analysis, 'source, 'ast> {
    fn new(analysis: &'analysis mut Analysis<'source, 'ast>) -> Self {
        Self { analysis }
    }

    fn infer_function(&mut self, function: &'ast FunctionDeclaration) {
        self.infer_block(&function.body);
    }

    fn infer_block(&mut self, block: &'ast Block) -> TypeState {
        for statement in &block.statements {
            self.infer_statement(statement);
        }
        block
            .value
            .as_ref()
            .map_or(TypeState::NoValue, |value| self.infer_expression(value))
    }

    fn infer_statement(&mut self, statement: &'ast Statement) {
        match &statement.kind {
            StatementKind::Local { initializer, .. } => {
                let state = match self.infer_expression(initializer) {
                    TypeState::NoValue => self.analysis.error(
                        initializer.span,
                        "local initializer must produce a value",
                    ),
                    state => state,
                };
                let binding = self.analysis.statement_binding(statement);
                self.analysis.binding_types[binding.0].state = state;
            }
            StatementKind::Assignment { target, value, .. } => {
                self.infer_assignment_target(target);
                self.infer_expression(value);
            }
            StatementKind::Expression(expression) => {
                self.infer_expression(expression);
            }
            StatementKind::Return(value) => {
                if let Some(value) = value {
                    self.infer_expression(value);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
            StatementKind::If {
                branches,
                else_body,
            } => {
                for branch in branches {
                    let condition = self.infer_expression(&branch.condition);
                    self.require_bool(condition, branch.condition.span);
                    self.infer_statement_body(&branch.body);
                }
                if let Some(body) = else_body {
                    self.infer_statement_body(body);
                }
            }
            StatementKind::While { condition, body } => {
                let condition_state = self.infer_expression(condition);
                self.require_bool(condition_state, condition.span);
                self.infer_statement_body(body);
            }
            StatementKind::For { iterable, body, .. } => {
                let iterable_state = self.infer_expression(iterable);
                let binding = self.analysis.statement_binding(statement);
                self.analysis.binding_types[binding.0].state =
                    self.iteration_binding_type(iterable_state, iterable);
                self.infer_statement_body(body);
            }
            StatementKind::Switch {
                value,
                arms,
                else_body,
            } => {
                let value_state = self.infer_expression(value);
                if !matches!(value_state, TypeState::Error | TypeState::Never) {
                    self.analysis.defer(
                        AstNode::Statement(statement),
                        DeferredReason::FlowDependentType,
                    );
                }
                for arm in arms {
                    self.infer_statement_body(&arm.body);
                }
                if let Some(body) = else_body {
                    self.infer_statement_body(body);
                }
            }
            StatementKind::Block(block) => {
                self.infer_block(block);
            }
        }
    }

    fn infer_statement_body(&mut self, body: &'ast StatementBody) {
        match &body.kind {
            StatementBodyKind::Block(block) => {
                self.infer_block(block);
            }
            StatementBodyKind::Statement(statement) => self.infer_statement(statement),
        }
    }

    fn infer_assignment_target(&mut self, target: &'ast AssignmentTarget) -> TypeState {
        let mut state = self.infer_identifier(&target.root);
        for suffix in &target.suffixes {
            state = match &suffix.kind {
                AssignmentTargetSuffixKind::Index(index) => {
                    let index_state = self.infer_expression(index);
                    self.infer_target_index(target, state, index_state, suffix.span)
                }
                AssignmentTargetSuffixKind::Member(member) => {
                    self.infer_target_member(target, state, *member)
                }
            };
        }
        let annotation = self.analysis.assignment_target_annotation(target, state);
        self.analysis.assignment_targets.push(annotation);
        state
    }

    fn infer_target_index(
        &mut self,
        target: &'ast AssignmentTarget,
        value_state: TypeState,
        index_state: TypeState,
        span: Span,
    ) -> TypeState {
        let (TypeState::Resolved(value_type), TypeState::Resolved(index_type)) =
            (value_state, index_state)
        else {
            return self.combine_target_states(target, value_state, index_state);
        };
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        match self.analysis.types.get(value_type).clone() {
            ResolvedType::List(element) if index_type == int => TypeState::Resolved(element),
            ResolvedType::Map { key, value } if index_type == key => TypeState::Resolved(value),
            _ => self.analysis.error(
                span,
                "index assignment is not defined for these operand types",
            ),
        }
    }

    fn infer_target_member(
        &mut self,
        target: &'ast AssignmentTarget,
        value_state: TypeState,
        member: Member,
    ) -> TypeState {
        let TypeState::Resolved(value_type) = value_state else {
            return self.non_value_target(target, value_state);
        };
        match self.analysis.types.get(value_type).clone() {
            ResolvedType::Nominal(declaration) => {
                let kind = self.analysis.type_definition(declaration).kind.clone();
                match (kind, member) {
                    (TypeDefinitionKind::Struct(members), Member::Named(name)) => {
                        let spelling = self.analysis.identifier_text(name.span).to_owned();
                        members
                            .iter()
                            .find(|field| {
                                self.analysis.identifier_text(field.name.span) == spelling.as_str()
                            })
                            .map_or_else(
                                || {
                                    self.analysis.error(
                                        name.span,
                                        format!("struct has no member '{spelling}'"),
                                    )
                                },
                                |field| field.ty,
                            )
                    }
                    (TypeDefinitionKind::Tuple(members), Member::TupleIndex(span)) => self
                        .parse_tuple_index(span)
                        .and_then(|index| members.get(index))
                        .map_or_else(
                            || self.analysis.error(span, "tuple member index is out of range"),
                            |field| field.ty,
                        ),
                    (TypeDefinitionKind::Union { .. }, _) => {
                        self.defer_target(target, DeferredReason::FlowDependentType)
                    }
                    (TypeDefinitionKind::Invalid, _) => TypeState::Error,
                    (TypeDefinitionKind::Struct(_), Member::TupleIndex(span)) => self
                        .analysis
                        .error(span, "struct members must be accessed by name"),
                    (TypeDefinitionKind::Tuple(_), Member::Named(name)) => self
                        .analysis
                        .error(name.span, "tuple members must be accessed by position"),
                }
            }
            ResolvedType::Union(_) => {
                self.defer_target(target, DeferredReason::FlowDependentType)
            }
            _ => self.analysis.error(
                target.span,
                "assignment receiver has no assignable member",
            ),
        }
    }

    fn infer_expression(&mut self, expression: &'ast Expression) -> TypeState {
        let state = match &expression.kind {
            ExpressionKind::Identifier(identifier) => self.infer_identifier(identifier),
            ExpressionKind::Integer => self.infer_integer(expression, i64::MAX as u128),
            ExpressionKind::Float => self.infer_float(expression),
            ExpressionKind::String(value) => {
                self.analysis.literals.push(LiteralAnnotation {
                    node: expression,
                    value: LiteralValue::String(value.clone().into_boxed_slice()),
                });
                self.primitive(PrimitiveType::Str)
            }
            ExpressionKind::Character(value) => {
                self.analysis.literals.push(LiteralAnnotation {
                    node: expression,
                    value: LiteralValue::Character(*value),
                });
                self.primitive(PrimitiveType::Char)
            }
            ExpressionKind::Boolean(value) => {
                self.analysis.literals.push(LiteralAnnotation {
                    node: expression,
                    value: LiteralValue::Boolean(*value),
                });
                self.primitive(PrimitiveType::Bool)
            }
            ExpressionKind::Parenthesized(inner) => self.infer_expression(inner),
            ExpressionKind::List(elements) => {
                let children = elements
                    .iter()
                    .map(|element| self.infer_expression(element))
                    .collect::<Vec<_>>();
                if children.contains(&TypeState::Error) {
                    TypeState::Error
                } else if children.contains(&TypeState::Never) {
                    TypeState::Never
                } else if children.contains(&TypeState::NoValue) {
                    self.analysis.error(
                        expression.span,
                        "list elements must produce values",
                    )
                } else {
                    self.defer_expression(expression, DeferredReason::ExpectedType)
                }
            }
            ExpressionKind::Map(entries) => {
                let mut failed = false;
                let mut never = false;
                for entry in entries {
                    for state in [
                        self.infer_expression(&entry.key),
                        self.infer_expression(&entry.value),
                    ] {
                        failed |= state == TypeState::Error;
                        never |= state == TypeState::Never;
                        if state == TypeState::NoValue {
                            self.analysis.error(
                                expression.span,
                                "map keys and values must produce values",
                            );
                            failed = true;
                        }
                    }
                }
                if failed {
                    TypeState::Error
                } else if never {
                    TypeState::Never
                } else {
                    self.defer_expression(expression, DeferredReason::ExpectedType)
                }
            }
            ExpressionKind::TypedEmptyList(_) | ExpressionKind::TypedEmptyMap(_) => {
                self.defer_expression(expression, DeferredReason::ExpectedType)
            }
            ExpressionKind::Block(block) => self.infer_block(block),
            ExpressionKind::If {
                branches,
                else_branch,
            } => self.infer_if_expression(expression, branches, else_branch),
            ExpressionKind::Unary {
                operator,
                operator_span,
                operand,
            } => self.infer_unary(expression, *operator, *operator_span, operand),
            ExpressionKind::Binary {
                left,
                operator,
                operator_span,
                right,
            } => self.infer_binary(
                expression,
                left,
                *operator,
                *operator_span,
                right,
            ),
            ExpressionKind::Is { value, .. } => {
                let value = self.infer_expression(value);
                self.flow_dependent(expression, value)
            }
            ExpressionKind::Call { callee, arguments } => {
                self.infer_call(expression, callee, arguments)
            }
            ExpressionKind::Index { value, index } => {
                self.infer_index(expression, value, index)
            }
            ExpressionKind::Member { value, member } => {
                self.infer_member(expression, value, *member)
            }
            ExpressionKind::Try { value, .. } => {
                let value = self.infer_expression(value);
                self.flow_dependent(expression, value)
            }
        };
        self.analysis.annotate_expression(expression, state);
        state
    }

    fn infer_identifier(&self, identifier: &'ast Identifier) -> TypeState {
        match self.analysis.name_use(identifier).map(|name_use| name_use.resolution) {
            Some(NameResolution::Binding(binding)) => self.analysis.binding_type(binding),
            Some(NameResolution::Callable(_)
            | NameResolution::ErrorConstructor
            | NameResolution::AmbiguousErrorConstructor { .. }
            | NameResolution::AmbiguousCall { .. }
            | NameResolution::Unknown)
            | None => TypeState::Error,
        }
    }

    fn infer_integer(&mut self, expression: &'ast Expression, maximum: u128) -> TypeState {
        let text = self.analysis.identifier_text(expression.span);
        let (radix, digits) = if let Some(digits) = text.strip_prefix("0x") {
            (16, digits)
        } else if let Some(digits) = text.strip_prefix("0b") {
            (2, digits)
        } else {
            (10, text)
        };
        let digits = digits.replace('_', "");
        match u128::from_str_radix(&digits, radix) {
            Ok(value) if value <= maximum => {
                self.analysis.literals.push(LiteralAnnotation {
                    node: expression,
                    value: LiteralValue::Integer(value as u64),
                });
                self.primitive(PrimitiveType::Int)
            }
            _ => self.analysis.error(
                expression.span,
                "integer literal is outside the signed 64-bit range",
            ),
        }
    }

    fn infer_float(&mut self, expression: &'ast Expression) -> TypeState {
        let text = self
            .analysis
            .identifier_text(expression.span)
            .replace('_', "");
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => {
                self.analysis.literals.push(LiteralAnnotation {
                    node: expression,
                    value: LiteralValue::Float(value),
                });
                self.primitive(PrimitiveType::Float)
            }
            _ => self.analysis.error(
                expression.span,
                "floating-point literal is not a finite binary64 value",
            ),
        }
    }

    fn infer_unary(
        &mut self,
        expression: &'ast Expression,
        operator: UnaryOperator,
        operator_span: Span,
        operand: &'ast Expression,
    ) -> TypeState {
        if operator == UnaryOperator::Minus && matches!(&operand.kind, ExpressionKind::Integer) {
            let state = self.infer_integer(operand, (i64::MAX as u128) + 1);
            self.analysis.annotate_expression(operand, state);
            return state;
        }
        let operand_state = self.infer_expression(operand);
        let TypeState::Resolved(operand_type) = operand_state else {
            return self.non_value_operand(expression, operand_state);
        };
        let valid = match operator {
            UnaryOperator::LogicalNot => self.is_primitive(operand_type, PrimitiveType::Bool),
            UnaryOperator::BitwiseNot => self.is_primitive(operand_type, PrimitiveType::Int),
            UnaryOperator::Plus | UnaryOperator::Minus => {
                self.is_numeric(operand_type)
            }
        };
        if valid {
            TypeState::Resolved(operand_type)
        } else {
            self.analysis.error(
                operator_span,
                "unary operator is not defined for this operand type",
            )
        }
    }

    fn infer_binary(
        &mut self,
        expression: &'ast Expression,
        left: &'ast Expression,
        operator: BinaryOperator,
        operator_span: Span,
        right: &'ast Expression,
    ) -> TypeState {
        let left_state = self.infer_expression(left);
        let right_state = self.infer_expression(right);
        let bool_type = self.analysis.types.primitive(PrimitiveType::Bool);
        if matches!(operator, BinaryOperator::LogicalOr | BinaryOperator::LogicalAnd)
            && left_state == TypeState::Resolved(bool_type)
            && right_state == TypeState::Never
        {
            // The right operand may be skipped. Semantic flow analysis records
            // its divergent path, but normal short-circuit completion still
            // gives the complete expression type bool.
            return TypeState::Resolved(bool_type);
        }
        let (TypeState::Resolved(left_type), TypeState::Resolved(right_type)) =
            (left_state, right_state)
        else {
            return self.combine_operand_states(expression, left_state, right_state);
        };
        let int_type = self.analysis.types.primitive(PrimitiveType::Int);
        let result = match operator {
            BinaryOperator::LogicalOr | BinaryOperator::LogicalAnd
                if left_type == bool_type && right_type == bool_type =>
            {
                Some(bool_type)
            }
            BinaryOperator::BitwiseOr
            | BinaryOperator::BitwiseXor
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight
                if left_type == int_type && right_type == int_type =>
            {
                Some(int_type)
            }
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
                if left_type == right_type && self.is_numeric(left_type) =>
            {
                Some(left_type)
            }
            BinaryOperator::Remainder if left_type == int_type && right_type == int_type => {
                Some(int_type)
            }
            BinaryOperator::Equal | BinaryOperator::NotEqual
                if left_type == right_type && self.supports_equality(left_type) =>
            {
                Some(bool_type)
            }
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
                if left_type == right_type && self.supports_ordering(left_type) =>
            {
                Some(bool_type)
            }
            BinaryOperator::In if self.membership_matches(left_type, right_type) => {
                Some(bool_type)
            }
            _ => None,
        };
        result.map_or_else(
            || {
                self.analysis.error(
                    operator_span,
                    "binary operator is not defined for these operand types",
                )
            },
            TypeState::Resolved,
        )
    }

    fn infer_if_expression(
        &mut self,
        expression: &'ast Expression,
        branches: &'ast [crate::ast::ConditionalExpressionBranch],
        else_branch: &'ast ExpressionBody,
    ) -> TypeState {
        let mut states = Vec::with_capacity(branches.len() + 1);
        let mut invalid_condition = false;
        let mut deferred_condition = false;
        for branch in branches {
            let condition = self.infer_expression(&branch.condition);
            deferred_condition |= matches!(condition, TypeState::Deferred(_));
            invalid_condition |= !self.require_bool(condition, branch.condition.span);
            states.push(self.infer_expression_body(&branch.body));
        }
        states.push(self.infer_expression_body(else_branch));
        if invalid_condition || states.contains(&TypeState::Error) {
            return TypeState::Error;
        }
        if deferred_condition
            || states.iter().any(|state| matches!(state, TypeState::Deferred(_)))
        {
            return self.defer_expression(expression, DeferredReason::ExpectedType);
        }
        if states.contains(&TypeState::NoValue) {
            return self.analysis.error(
                expression.span,
                "every if-expression branch must produce a value",
            );
        }
        let resolved = states.iter().find_map(|state| match state {
            TypeState::Resolved(ty) => Some(*ty),
            _ => None,
        });
        match resolved {
            None => TypeState::Never,
            Some(expected)
                if states.iter().all(|state| {
                    *state == TypeState::Never || *state == TypeState::Resolved(expected)
                }) => TypeState::Resolved(expected),
            Some(_) => self.defer_expression(expression, DeferredReason::ExpectedType),
        }
    }

    fn infer_expression_body(&mut self, body: &'ast ExpressionBody) -> TypeState {
        match &body.kind {
            ExpressionBodyKind::Block(block) => self.infer_block(block),
            ExpressionBodyKind::Expression(expression) => self.infer_expression(expression),
        }
    }

    fn infer_call(
        &mut self,
        expression: &'ast Expression,
        callee: &'ast Expression,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let target = self
            .analysis
            .call_resolution(expression)
            .map(|resolution| resolution.target)
            .unwrap_or(CallTarget::Unknown);
        match target {
            CallTarget::Callable(CallableId::Function(function)) => {
                self.infer_function_call(expression, function, arguments)
            }
            CallTarget::Callable(CallableId::Intrinsic(intrinsic)) => {
                self.infer_intrinsic_call(expression, intrinsic, arguments)
            }
            CallTarget::Callable(CallableId::Constructor(_))
            | CallTarget::QualifiedConstructor(_)
            | CallTarget::ErrorConstructor => {
                let states = self.infer_arguments(arguments);
                if states.contains(&TypeState::Error) {
                    TypeState::Error
                } else if states.contains(&TypeState::Never) {
                    TypeState::Never
                } else {
                    self.defer_expression(expression, DeferredReason::Constructor)
                }
            }
            CallTarget::Binding(_) => {
                self.infer_arguments(arguments);
                self.analysis.error(callee.span, "value is not callable")
            }
            CallTarget::Expression => {
                if let ExpressionKind::Member { value, member } = &callee.kind {
                    self.infer_method_call(expression, value, *member, arguments)
                } else {
                    self.infer_expression(callee);
                    self.infer_arguments(arguments);
                    self.analysis.error(callee.span, "expression is not callable")
                }
            }
            CallTarget::Builtin(_) => unreachable!("built-in targets are selected during inference"),
            CallTarget::Ambiguous { .. }
            | CallTarget::AmbiguousErrorConstructor { .. }
            | CallTarget::Unknown => {
                self.infer_arguments(arguments);
                TypeState::Error
            }
        }
    }

    fn infer_function_call(
        &mut self,
        expression: &'ast Expression,
        function: FunctionId,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let states = self.infer_arguments(arguments);
        let (parameters, result) = {
            let signature = self.analysis.function_signature(function);
            (
                signature
                    .parameters
                    .iter()
                    .map(|parameter| parameter.ty)
                    .collect::<Vec<_>>(),
                signature.result,
            )
        };
        let mut failed = false;
        let mut needs_expected_type = false;
        for argument in arguments {
            if matches!(&argument.kind, ArgumentKind::Named { .. }) {
                self.analysis.error(
                    argument.span,
                    "named arguments are permitted only for struct constructors",
                );
                failed = true;
            }
        }
        if states.len() != parameters.len() {
            self.analysis.error(
                expression.span,
                format!(
                    "function expects {} argument(s), but {} were provided",
                    parameters.len(),
                    states.len()
                ),
            );
            failed = true;
        }
        for ((state, parameter), argument) in
            states.iter().zip(parameters).zip(arguments.iter())
        {
            match self.argument_compatibility(*state, parameter, argument.span) {
                Compatibility::Match => {}
                Compatibility::Deferred => needs_expected_type = true,
                Compatibility::Invalid => failed = true,
            }
        }
        self.call_result(
            expression,
            &states,
            failed,
            needs_expected_type,
            result,
        )
    }

    fn infer_intrinsic_call(
        &mut self,
        expression: &'ast Expression,
        intrinsic: IntrinsicId,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let states = self.infer_arguments(arguments);
        let signature = self.analysis.intrinsic_signature(intrinsic).clone();
        let mut failed = false;
        let mut needs_expected_type = false;
        for argument in arguments {
            if matches!(&argument.kind, ArgumentKind::Named { .. }) {
                self.analysis
                    .error(argument.span, "intrinsics do not accept named arguments");
                failed = true;
            }
        }
        match signature.arguments {
            IntrinsicArguments::OnePrintable => {
                if states.len() != 1 {
                    self.analysis
                        .error(expression.span, "print expects exactly one argument");
                    failed = true;
                } else if let TypeState::Resolved(ty) = states[0]
                    && !self.is_printable(ty, &mut Vec::new())
                {
                    self.analysis.error(
                        arguments[0].span,
                        "print argument must be a primitive or printable tuple",
                    );
                    failed = true;
                }
            }
            IntrinsicArguments::ZeroOrOnePrintable => {
                if states.len() > 1 {
                    self.analysis
                        .error(expression.span, "println expects zero or one argument");
                    failed = true;
                } else if let Some(TypeState::Resolved(ty)) = states.first()
                    && !self.is_printable(*ty, &mut Vec::new())
                {
                    self.analysis.error(
                        arguments[0].span,
                        "println argument must be a primitive or printable tuple",
                    );
                    failed = true;
                }
            }
            IntrinsicArguments::Exact(expected) => {
                if states.len() != expected.len() {
                    self.analysis.error(
                        expression.span,
                        format!(
                            "intrinsic expects {} argument(s), but {} were provided",
                            expected.len(),
                            states.len()
                        ),
                    );
                    failed = true;
                }
                for (state, expected) in states.iter().zip(expected.iter()) {
                    match self.argument_compatibility(
                        *state,
                        TypeState::Resolved(*expected),
                        expression.span,
                    ) {
                        Compatibility::Match => {}
                        Compatibility::Deferred => needs_expected_type = true,
                        Compatibility::Invalid => failed = true,
                    }
                }
            }
        }
        self.call_result(
            expression,
            &states,
            failed,
            needs_expected_type,
            signature.result,
        )
    }

    fn infer_method_call(
        &mut self,
        expression: &'ast Expression,
        receiver: &'ast Expression,
        member: Member,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let receiver_state = self.infer_expression(receiver);
        let states = self.infer_arguments(arguments);
        let TypeState::Resolved(receiver_type) = receiver_state else {
            return self.non_value_operand(expression, receiver_state);
        };
        if self.is_union_type(receiver_type) {
            return self.defer_expression(expression, DeferredReason::FlowDependentType);
        }
        let Member::Named(name) = member else {
            return self.analysis.error(expression.span, "tuple member is not callable");
        };
        let spelling = self.analysis.identifier_text(name.span).to_owned();
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let method = match self.analysis.types.get(receiver_type).clone() {
            ResolvedType::List(element) => match spelling.as_str() {
                "append" => Some((BuiltinMethod::ListAppend, Some(element), TypeState::NoValue)),
                "removeIndex" => Some((BuiltinMethod::ListRemoveIndex, Some(int), TypeState::NoValue)),
                "len" => Some((BuiltinMethod::ListLen, None, TypeState::Resolved(int))),
                _ => None,
            },
            ResolvedType::Map { key, .. } => match spelling.as_str() {
                "removeKey" => Some((BuiltinMethod::MapRemoveKey, Some(key), TypeState::NoValue)),
                "len" => Some((BuiltinMethod::MapLen, None, TypeState::Resolved(int))),
                _ => None,
            },
            ResolvedType::Primitive(PrimitiveType::Str) if spelling == "len" => {
                Some((BuiltinMethod::StrLen, None, TypeState::Resolved(int)))
            }
            _ => None,
        };
        let Some((method, expected, result)) = method else {
            return self.analysis.error(
                name.span,
                format!("type has no built-in method '{spelling}'"),
            );
        };
        if let Some(call) = self
            .analysis
            .calls
            .iter_mut()
            .find(|call| std::ptr::eq(call.node, expression))
        {
            call.target = CallTarget::Builtin(method);
        }
        let expected_count = usize::from(expected.is_some());
        let mut failed = states.len() != expected_count;
        let mut needs_expected_type = false;
        if failed {
            self.analysis.error(
                expression.span,
                format!(
                    "method '{spelling}' expects {expected_count} argument(s), but {} were provided",
                    states.len()
                ),
            );
        }
        for argument in arguments {
            if matches!(&argument.kind, ArgumentKind::Named { .. }) {
                self.analysis
                    .error(argument.span, "built-in methods do not accept named arguments");
                failed = true;
            }
        }
        if let Some(expected) = expected
            && let Some(state) = states.first()
        {
            match self.argument_compatibility(
                *state,
                TypeState::Resolved(expected),
                arguments[0].span,
            ) {
                Compatibility::Match => {}
                Compatibility::Deferred => needs_expected_type = true,
                Compatibility::Invalid => failed = true,
            }
        }
        self.call_result(
            expression,
            &states,
            failed,
            needs_expected_type,
            result,
        )
    }

    fn infer_arguments(&mut self, arguments: &'ast [crate::ast::Argument]) -> Vec<TypeState> {
        arguments
            .iter()
            .map(|argument| match &argument.kind {
                ArgumentKind::Positional(value) | ArgumentKind::Named { value, .. } => {
                    self.infer_expression(value)
                }
            })
            .collect()
    }

    fn call_result(
        &mut self,
        expression: &'ast Expression,
        arguments: &[TypeState],
        failed: bool,
        needs_expected_type: bool,
        result: TypeState,
    ) -> TypeState {
        if failed || arguments.contains(&TypeState::Error) {
            TypeState::Error
        } else if arguments.contains(&TypeState::Never) {
            TypeState::Never
        } else if needs_expected_type
            || arguments
            .iter()
            .any(|state| matches!(state, TypeState::Deferred(_)))
        {
            self.defer_expression(expression, DeferredReason::ExpectedType)
        } else {
            result
        }
    }

    fn argument_compatibility(
        &mut self,
        actual: TypeState,
        expected: TypeState,
        span: Span,
    ) -> Compatibility {
        match (actual, expected) {
            (TypeState::Resolved(actual), TypeState::Resolved(expected)) if actual == expected => {
                Compatibility::Match
            }
            (TypeState::Resolved(_), TypeState::Resolved(expected))
                if self.is_union_type(expected) =>
            {
                Compatibility::Deferred
            }
            (TypeState::Error, _) | (_, TypeState::Error) | (TypeState::Never, _) => {
                Compatibility::Match
            }
            (TypeState::Deferred(_), _) => Compatibility::Deferred,
            (TypeState::NoValue, _) => {
                self.analysis
                    .error(span, "argument expression does not produce a value");
                Compatibility::Invalid
            }
            _ => {
                self.analysis
                    .error(span, "argument type does not match parameter type");
                Compatibility::Invalid
            }
        }
    }

    fn infer_index(
        &mut self,
        expression: &'ast Expression,
        value: &'ast Expression,
        index: &'ast Expression,
    ) -> TypeState {
        let value_state = self.infer_expression(value);
        let index_state = self.infer_expression(index);
        let (TypeState::Resolved(value_type), TypeState::Resolved(index_type)) =
            (value_state, index_state)
        else {
            return self.combine_operand_states(expression, value_state, index_state);
        };
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        match self.analysis.types.get(value_type).clone() {
            ResolvedType::List(element) if index_type == int => TypeState::Resolved(element),
            ResolvedType::Map { key, value } if index_type == key => TypeState::Resolved(value),
            ResolvedType::Primitive(PrimitiveType::Str) if index_type == int => {
                self.primitive(PrimitiveType::Char)
            }
            _ => self.analysis.error(
                expression.span,
                "index operation is not defined for these operand types",
            ),
        }
    }

    fn infer_member(
        &mut self,
        expression: &'ast Expression,
        value: &'ast Expression,
        member: Member,
    ) -> TypeState {
        let value_state = self.infer_expression(value);
        let TypeState::Resolved(value_type) = value_state else {
            return self.non_value_operand(expression, value_state);
        };
        match self.analysis.types.get(value_type).clone() {
            ResolvedType::Nominal(declaration) => {
                let kind = self.analysis.type_definition(declaration).kind.clone();
                match (kind, member) {
                    (TypeDefinitionKind::Struct(members), Member::Named(name)) => {
                        let spelling = self.analysis.identifier_text(name.span).to_owned();
                        members
                            .iter()
                            .find(|field| {
                                self.analysis.identifier_text(field.name.span) == spelling.as_str()
                            })
                            .map_or_else(
                                || {
                                    self.analysis.error(
                                        name.span,
                                        format!("struct has no member '{spelling}'"),
                                    )
                                },
                                |field| field.ty,
                            )
                    }
                    (TypeDefinitionKind::Tuple(members), Member::TupleIndex(span)) => {
                        let index = self.parse_tuple_index(span);
                        index
                            .and_then(|index| members.get(index))
                            .map_or_else(
                                || self.analysis.error(span, "tuple member index is out of range"),
                                |field| field.ty,
                            )
                    }
                    (TypeDefinitionKind::Union { .. }, _) => {
                        self.defer_expression(expression, DeferredReason::FlowDependentType)
                    }
                    (TypeDefinitionKind::Invalid, _) => TypeState::Error,
                    (TypeDefinitionKind::Struct(_), Member::TupleIndex(span)) => self
                        .analysis
                        .error(span, "struct members must be accessed by name"),
                    (TypeDefinitionKind::Tuple(_), Member::Named(name)) => self
                        .analysis
                        .error(name.span, "tuple members must be accessed by position"),
                }
            }
            ResolvedType::List(_) => self.infer_method_member(
                member,
                &["append", "removeIndex", "len"],
            ),
            ResolvedType::Map { .. } => {
                self.infer_method_member(member, &["removeKey", "len"])
            }
            ResolvedType::Primitive(PrimitiveType::Str) => {
                self.infer_method_member(member, &["len"])
            }
            ResolvedType::Union(_) => {
                self.defer_expression(expression, DeferredReason::FlowDependentType)
            }
            ResolvedType::Primitive(_) => self.analysis.error(
                expression.span,
                "primitive value has no accessible member",
            ),
        }
    }

    fn infer_method_member(&mut self, member: Member, methods: &[&str]) -> TypeState {
        match member {
            Member::Named(name) => {
                let spelling = self.analysis.identifier_text(name.span).to_owned();
                if methods.contains(&spelling.as_str()) {
                    self.analysis.error(
                        name.span,
                        "container methods must be used as direct call targets",
                    )
                } else {
                    self.analysis.error(
                        name.span,
                        format!("container has no member '{spelling}'"),
                    )
                }
            }
            Member::TupleIndex(span) => self
                .analysis
                .error(span, "container members must be accessed by name"),
        }
    }

    fn iteration_binding_type(
        &mut self,
        state: TypeState,
        iterable: &'ast Expression,
    ) -> TypeState {
        match state {
            TypeState::Resolved(ty) => match self.analysis.types.get(ty).clone() {
                ResolvedType::List(element) => TypeState::Resolved(element),
                ResolvedType::Map { key, .. } => TypeState::Resolved(key),
                _ => self
                    .analysis
                    .error(iterable.span, "for loop requires a list or map iterable"),
            },
            TypeState::Deferred(deferred) => TypeState::Deferred(deferred),
            TypeState::NoValue => self.analysis.error(
                iterable.span,
                "for loop iterable must produce a value",
            ),
            other => other,
        }
    }

    fn require_bool(&mut self, state: TypeState, span: Span) -> bool {
        match state {
            TypeState::Resolved(ty) if self.is_primitive(ty, PrimitiveType::Bool) => true,
            TypeState::Error | TypeState::Never | TypeState::Deferred(_) => true,
            _ => {
                self.analysis.error(span, "condition must have type bool");
                false
            }
        }
    }

    fn non_value_operand(
        &mut self,
        expression: &'ast Expression,
        state: TypeState,
    ) -> TypeState {
        match state {
            TypeState::Error => TypeState::Error,
            TypeState::Never => TypeState::Never,
            TypeState::Deferred(_) => {
                self.defer_expression(expression, DeferredReason::FlowDependentType)
            }
            TypeState::NoValue => self.analysis.error(
                expression.span,
                "operand expression does not produce a value",
            ),
            TypeState::Resolved(_) => unreachable!(),
        }
    }

    fn flow_dependent(
        &mut self,
        expression: &'ast Expression,
        state: TypeState,
    ) -> TypeState {
        match state {
            TypeState::Resolved(_) | TypeState::Deferred(_) => {
                self.defer_expression(expression, DeferredReason::FlowDependentType)
            }
            other => self.non_value_operand(expression, other),
        }
    }

    fn combine_operand_states(
        &mut self,
        expression: &'ast Expression,
        left: TypeState,
        right: TypeState,
    ) -> TypeState {
        if left == TypeState::Error || right == TypeState::Error {
            TypeState::Error
        } else if left == TypeState::Never || right == TypeState::Never {
            TypeState::Never
        } else if matches!(left, TypeState::Deferred(_))
            || matches!(right, TypeState::Deferred(_))
        {
            self.defer_expression(expression, DeferredReason::FlowDependentType)
        } else {
            self.analysis.error(
                expression.span,
                "operator operand does not produce a value",
            )
        }
    }

    fn non_value_target(
        &mut self,
        target: &'ast AssignmentTarget,
        state: TypeState,
    ) -> TypeState {
        match state {
            TypeState::Error => TypeState::Error,
            TypeState::Never => TypeState::Never,
            TypeState::Deferred(deferred) => TypeState::Deferred(deferred),
            TypeState::NoValue => self.analysis.error(
                target.span,
                "assignment receiver does not produce a value",
            ),
            TypeState::Resolved(_) => unreachable!(),
        }
    }

    fn combine_target_states(
        &mut self,
        target: &'ast AssignmentTarget,
        value: TypeState,
        index: TypeState,
    ) -> TypeState {
        if value == TypeState::Error || index == TypeState::Error {
            TypeState::Error
        } else if value == TypeState::Never || index == TypeState::Never {
            TypeState::Never
        } else if let TypeState::Deferred(deferred) = value {
            TypeState::Deferred(deferred)
        } else if let TypeState::Deferred(deferred) = index {
            TypeState::Deferred(deferred)
        } else {
            self.analysis.error(
                target.span,
                "assignment index does not produce a value",
            )
        }
    }

    fn defer_target(
        &mut self,
        target: &'ast AssignmentTarget,
        reason: DeferredReason,
    ) -> TypeState {
        TypeState::Deferred(self.analysis.defer(AstNode::AssignmentTarget(target), reason))
    }

    fn defer_expression(
        &mut self,
        expression: &'ast Expression,
        reason: DeferredReason,
    ) -> TypeState {
        TypeState::Deferred(self.analysis.defer(AstNode::Expression(expression), reason))
    }

    fn primitive(&self, primitive: PrimitiveType) -> TypeState {
        TypeState::Resolved(self.analysis.types.primitive(primitive))
    }

    fn is_primitive(&self, ty: TypeId, primitive: PrimitiveType) -> bool {
        ty == self.analysis.types.primitive(primitive)
    }

    fn is_numeric(&self, ty: TypeId) -> bool {
        self.is_primitive(ty, PrimitiveType::Int)
            || self.is_primitive(ty, PrimitiveType::Float)
    }

    fn supports_ordering(&self, ty: TypeId) -> bool {
        matches!(
            self.analysis.types.get(ty),
            ResolvedType::Primitive(
                PrimitiveType::Int
                    | PrimitiveType::Float
                    | PrimitiveType::Str
                    | PrimitiveType::Char
            )
        )
    }

    fn supports_equality(&self, ty: TypeId) -> bool {
        self.supports_equality_inner(ty, &mut Vec::new())
    }

    fn supports_equality_inner(
        &self,
        ty: TypeId,
        visiting: &mut Vec<TypeDeclarationId>,
    ) -> bool {
        match self.analysis.types.get(ty) {
            ResolvedType::Primitive(_) | ResolvedType::List(_) | ResolvedType::Map { .. } => true,
            ResolvedType::Nominal(declaration) => {
                if visiting.contains(declaration) {
                    return false;
                }
                match &self.analysis.type_definition(*declaration).kind {
                    TypeDefinitionKind::Struct(_) => true,
                    TypeDefinitionKind::Tuple(members) => {
                        visiting.push(*declaration);
                        let comparable = members.iter().all(|member| {
                            matches!(
                                member.ty,
                                TypeState::Resolved(member_type)
                                    if self.supports_equality_inner(member_type, visiting)
                            )
                        });
                        visiting.pop();
                        comparable
                    }
                    TypeDefinitionKind::Union { .. } | TypeDefinitionKind::Invalid => false,
                }
            }
            ResolvedType::Union(_) => false,
        }
    }

    fn is_union_type(&self, ty: TypeId) -> bool {
        match self.analysis.types.get(ty) {
            ResolvedType::Union(_) => true,
            ResolvedType::Nominal(declaration) => matches!(
                &self.analysis.type_definition(*declaration).kind,
                TypeDefinitionKind::Union { .. }
            ),
            _ => false,
        }
    }

    fn membership_matches(&self, item: TypeId, container: TypeId) -> bool {
        match self.analysis.types.get(container) {
            ResolvedType::List(element) => item == *element,
            ResolvedType::Map { key, .. } => item == *key,
            _ => false,
        }
    }

    fn is_printable(&self, ty: TypeId, visiting: &mut Vec<TypeDeclarationId>) -> bool {
        match self.analysis.types.get(ty) {
            ResolvedType::Primitive(_) => true,
            ResolvedType::Nominal(declaration) => {
                if visiting.contains(declaration) {
                    return false;
                }
                let TypeDefinitionKind::Tuple(members) =
                    &self.analysis.type_definition(*declaration).kind
                else {
                    return false;
                };
                visiting.push(*declaration);
                let printable = members.iter().all(|member| {
                    matches!(member.ty, TypeState::Resolved(ty) if self.is_printable(ty, visiting))
                });
                visiting.pop();
                printable
            }
            ResolvedType::List(_) | ResolvedType::Map { .. } | ResolvedType::Union(_) => false,
        }
    }

    fn parse_tuple_index(&self, span: Span) -> Option<usize> {
        self.analysis
            .identifier_text(span)
            .replace('_', "")
            .parse::<usize>()
            .ok()
    }
}

struct ExpectedTypeResolver<'analysis, 'source, 'ast> {
    analysis: &'analysis mut Analysis<'source, 'ast>,
    function_result: TypeState,
}

impl<'analysis, 'source, 'ast> ExpectedTypeResolver<'analysis, 'source, 'ast> {
    fn new(analysis: &'analysis mut Analysis<'source, 'ast>, function_result: TypeState) -> Self {
        Self {
            analysis,
            function_result,
        }
    }

    fn resolve_block(&mut self, block: &'ast Block, expected: Option<TypeId>) -> TypeState {
        for statement in &block.statements {
            self.resolve_statement(statement);
        }
        block.value.as_ref().map_or(TypeState::NoValue, |value| {
            self.resolve_expression(value, expected)
        })
    }

    fn resolve_statement(&mut self, statement: &'ast Statement) {
        match &statement.kind {
            StatementKind::Local { initializer, .. } => {
                let state = self.resolve_expression(initializer, None);
                let binding = self.analysis.statement_binding(statement);
                self.analysis.binding_types[binding.0].state = state;
            }
            StatementKind::Assignment {
                target,
                operator,
                operator_span,
                value,
            } => {
                let target_state = self.resolve_assignment_target(target);
                let expected = match target_state {
                    TypeState::Resolved(ty) => Some(ty),
                    _ => None,
                };
                let value_state = self.resolve_expression(value, expected);
                self.validate_assignment(*operator, *operator_span, target_state, value_state);
            }
            StatementKind::Expression(expression) => {
                self.resolve_expression(expression, None);
            }
            StatementKind::Return(value) => {
                if let Some(value) = value {
                    let expected = match self.function_result {
                        TypeState::Resolved(ty) => Some(ty),
                        _ => None,
                    };
                    self.resolve_expression(value, expected);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
            StatementKind::If {
                branches,
                else_body,
            } => {
                for branch in branches {
                    self.resolve_condition(&branch.condition);
                    self.resolve_statement_body(&branch.body);
                }
                if let Some(body) = else_body {
                    self.resolve_statement_body(body);
                }
            }
            StatementKind::While { condition, body } => {
                self.resolve_condition(condition);
                self.resolve_statement_body(body);
            }
            StatementKind::For { iterable, body, .. } => {
                let state = self.resolve_expression(iterable, None);
                let binding = self.analysis.statement_binding(statement);
                if let TypeState::Resolved(ty) = state {
                    self.analysis.binding_types[binding.0].state =
                        match self.analysis.types.get(ty).clone() {
                            ResolvedType::List(element) => TypeState::Resolved(element),
                            ResolvedType::Map { key, .. } => TypeState::Resolved(key),
                            _ => TypeState::Error,
                        };
                }
                self.resolve_statement_body(body);
            }
            StatementKind::Switch {
                value,
                arms,
                else_body,
            } => {
                self.resolve_expression(value, None);
                for arm in arms {
                    self.resolve_statement_body(&arm.body);
                }
                if let Some(body) = else_body {
                    self.resolve_statement_body(body);
                }
            }
            StatementKind::Block(block) => {
                self.resolve_block(block, None);
            }
        }
    }

    fn resolve_statement_body(&mut self, body: &'ast StatementBody) {
        match &body.kind {
            StatementBodyKind::Block(block) => {
                self.resolve_block(block, None);
            }
            StatementBodyKind::Statement(statement) => self.resolve_statement(statement),
        }
    }

    fn resolve_condition(&mut self, expression: &'ast Expression) -> bool {
        let current = self.analysis.expression_annotation(expression)
            .map_or(TypeState::Error, |annotation| annotation.state);
        let state = self.resolve_expression(expression, None);
        if matches!(current, TypeState::Deferred(_))
            && let TypeState::Resolved(ty) = state
            && !self.is_primitive(ty, PrimitiveType::Bool)
        {
            self.analysis.error(expression.span, "condition must have type bool");
            false
        } else {
            true
        }
    }

    fn validate_assignment(
        &mut self,
        operator: AssignmentOperator,
        operator_span: Span,
        target: TypeState,
        value: TypeState,
    ) {
        if operator == AssignmentOperator::Assign
            || !matches!((target, value), (TypeState::Resolved(_), TypeState::Resolved(_)))
        {
            return;
        }
        let (TypeState::Resolved(target), TypeState::Resolved(value)) = (target, value) else {
            unreachable!()
        };
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let valid = match operator {
            AssignmentOperator::Add
            | AssignmentOperator::Subtract
            | AssignmentOperator::Multiply
            | AssignmentOperator::Divide => target == value && self.is_numeric(target),
            AssignmentOperator::Remainder
            | AssignmentOperator::BitwiseAnd
            | AssignmentOperator::BitwiseOr
            | AssignmentOperator::BitwiseXor
            | AssignmentOperator::ShiftLeft
            | AssignmentOperator::ShiftRight => target == int && value == int,
            AssignmentOperator::Assign => true,
        };
        if !valid {
            self.analysis.error(
                operator_span,
                "compound assignment is not defined for these operand types",
            );
        }
    }

    fn resolve_assignment_target(&mut self, target: &'ast AssignmentTarget) -> TypeState {
        let current = self.analysis.assignment_target(target)
            .map_or(TypeState::Error, |annotation| annotation.state);
        if !matches!(current, TypeState::Deferred(_)) {
            return current;
        }
        let mut state = match self.analysis.name_use(&target.root).map(|name| name.resolution) {
            Some(NameResolution::Binding(binding)) => self.analysis.binding_type(binding),
            _ => TypeState::Error,
        };
        for suffix in &target.suffixes {
            state = match &suffix.kind {
                AssignmentTargetSuffixKind::Index(index) => {
                    let index = self.resolve_expression(index, None);
                    self.resolve_target_index(target, state, index, suffix.span, current)
                }
                AssignmentTargetSuffixKind::Member(member) => {
                    self.resolve_target_member(target, state, *member, current)
                }
            };
        }
        if !matches!(state, TypeState::Deferred(_)) {
            for deferred in &mut self.analysis.deferred {
                if matches!(
                    deferred.node,
                    AstNode::AssignmentTarget(node) if std::ptr::eq(node, target)
                ) {
                    deferred.resolved = true;
                }
            }
        }
        let annotation = self.analysis.assignment_target_annotation(target, state);
        self.analysis.assignment_targets.push(annotation);
        state
    }

    fn resolve_target_index(
        &mut self,
        target: &'ast AssignmentTarget,
        value: TypeState,
        index: TypeState,
        span: Span,
        deferred: TypeState,
    ) -> TypeState {
        let (TypeState::Resolved(value), TypeState::Resolved(index)) = (value, index) else {
            return self.target_pair_state(target, value, index, deferred);
        };
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        match self.analysis.types.get(value).clone() {
            ResolvedType::List(element) if index == int => TypeState::Resolved(element),
            ResolvedType::Map { key, value } if index == key => TypeState::Resolved(value),
            _ => self.analysis.error(
                span,
                "index assignment is not defined for these operand types",
            ),
        }
    }

    fn resolve_target_member(
        &mut self,
        target: &'ast AssignmentTarget,
        value: TypeState,
        member: Member,
        deferred: TypeState,
    ) -> TypeState {
        let TypeState::Resolved(value) = value else {
            return self.target_state(target, value, deferred);
        };
        match self.analysis.types.get(value).clone() {
            ResolvedType::Nominal(declaration) => {
                match (self.analysis.type_definition(declaration).kind.clone(), member) {
                    (TypeDefinitionKind::Struct(members), Member::Named(name)) => {
                        let spelling = self.analysis.identifier_text(name.span).to_owned();
                        members.iter().find(|field| {
                            self.analysis.identifier_text(field.name.span) == spelling.as_str()
                        }).map_or_else(
                            || self.analysis.error(name.span, format!("struct has no member '{spelling}'")),
                            |field| field.ty,
                        )
                    }
                    (TypeDefinitionKind::Tuple(members), Member::TupleIndex(span)) => {
                        self.parse_tuple_index(span).and_then(|index| members.get(index)).map_or_else(
                            || self.analysis.error(span, "tuple member index is out of range"),
                            |field| field.ty,
                        )
                    }
                    (TypeDefinitionKind::Union { .. }, _) => deferred,
                    (TypeDefinitionKind::Invalid, _) => TypeState::Error,
                    (TypeDefinitionKind::Struct(_), Member::TupleIndex(span)) => self
                        .analysis
                        .error(span, "struct members must be accessed by name"),
                    (TypeDefinitionKind::Tuple(_), Member::Named(name)) => self
                        .analysis
                        .error(name.span, "tuple members must be accessed by position"),
                }
            }
            ResolvedType::Union(_) => deferred,
            _ => self.analysis.error(
                target.span,
                "assignment receiver has no assignable member",
            ),
        }
    }

    fn resolve_expression(
        &mut self,
        expression: &'ast Expression,
        expected: Option<TypeId>,
    ) -> TypeState {
        let current = self
            .analysis
            .expression_annotation(expression)
            .map_or(TypeState::Error, |annotation| annotation.state);
        let state = match &expression.kind {
            ExpressionKind::List(elements) => self.resolve_list(expression, elements, expected),
            ExpressionKind::Map(entries) => self.resolve_map(expression, entries, expected),
            ExpressionKind::TypedEmptyList(ty) | ExpressionKind::TypedEmptyMap(ty) => {
                self.resolve_typed_empty(expression, ty, expected)
            }
            ExpressionKind::Parenthesized(inner) => self.resolve_expression(inner, expected),
            ExpressionKind::Block(block) => self.resolve_block(block, expected),
            ExpressionKind::If {
                branches,
                else_branch,
            } => self.resolve_if(expression, branches, else_branch, expected),
            ExpressionKind::Call { arguments, .. } => {
                self.resolve_call(expression, arguments, expected)
            }
            ExpressionKind::Unary {
                operator,
                operator_span,
                operand,
            } => self.resolve_unary(
                expression,
                *operator,
                *operator_span,
                operand,
                current,
                expected,
            ),
            ExpressionKind::Binary {
                left,
                operator,
                operator_span,
                right,
            } => self.resolve_binary(
                expression,
                left,
                *operator,
                *operator_span,
                right,
                current,
                expected,
            ),
            ExpressionKind::Index { value, index } => {
                self.resolve_index(expression, value, index, current, expected)
            }
            ExpressionKind::Member { value, member } => {
                self.resolve_member(expression, value, *member, current, expected)
            }
            ExpressionKind::Try { value, .. } | ExpressionKind::Is { value, .. } => {
                self.resolve_expression(value, None);
                self.coerce(expression, current, expected)
            }
            ExpressionKind::Identifier(identifier) => {
                let state = match self
                    .analysis
                    .name_use(identifier)
                    .map(|name_use| name_use.resolution)
                {
                    Some(NameResolution::Binding(binding)) => {
                        self.analysis.binding_type(binding)
                    }
                    _ => current,
                };
                self.coerce(expression, state, expected)
            }
            ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_) => self.coerce(expression, current, expected),
        };
        for deferred in &mut self.analysis.deferred {
            if matches!(
                deferred.node,
                AstNode::Expression(node) if std::ptr::eq(node, expression)
            ) {
                if !matches!(state, TypeState::Deferred(_))
                    || deferred.reason != DeferredReason::FlowDependentType
                {
                    deferred.resolved = true;
                }
            }
        }
        self.analysis.annotate_expression(expression, state);
        state
    }

    fn resolve_unary(
        &mut self,
        expression: &'ast Expression,
        operator: UnaryOperator,
        operator_span: Span,
        operand: &'ast Expression,
        current: TypeState,
        expected: Option<TypeId>,
    ) -> TypeState {
        let operand = self.resolve_expression(operand, None);
        if !matches!(current, TypeState::Deferred(_)) {
            return self.coerce(expression, current, expected);
        }
        let TypeState::Resolved(operand) = operand else {
            let state = self.operand_state(expression, operand, current);
            return self.coerce(expression, state, expected);
        };
        let valid = match operator {
            UnaryOperator::LogicalNot => self.is_primitive(operand, PrimitiveType::Bool),
            UnaryOperator::BitwiseNot => self.is_primitive(operand, PrimitiveType::Int),
            UnaryOperator::Plus | UnaryOperator::Minus => self.is_numeric(operand),
        };
        let state = if valid {
            TypeState::Resolved(operand)
        } else {
            self.analysis.error(
                operator_span,
                "unary operator is not defined for this operand type",
            )
        };
        self.coerce(expression, state, expected)
    }

    fn resolve_binary(
        &mut self,
        expression: &'ast Expression,
        left: &'ast Expression,
        operator: BinaryOperator,
        operator_span: Span,
        right: &'ast Expression,
        current: TypeState,
        expected: Option<TypeId>,
    ) -> TypeState {
        let left = self.resolve_expression(left, None);
        let right = self.resolve_expression(right, None);
        if !matches!(current, TypeState::Deferred(_)) {
            return self.coerce(expression, current, expected);
        }
        let (TypeState::Resolved(left), TypeState::Resolved(right)) = (left, right) else {
            let state = self.operand_pair_state(expression, left, right, current);
            return self.coerce(expression, state, expected);
        };
        let bool_type = self.analysis.types.primitive(PrimitiveType::Bool);
        let int_type = self.analysis.types.primitive(PrimitiveType::Int);
        let result = match operator {
            BinaryOperator::LogicalOr | BinaryOperator::LogicalAnd
                if left == bool_type && right == bool_type => Some(bool_type),
            BinaryOperator::BitwiseOr
            | BinaryOperator::BitwiseXor
            | BinaryOperator::BitwiseAnd
            | BinaryOperator::ShiftLeft
            | BinaryOperator::ShiftRight
                if left == int_type && right == int_type => Some(int_type),
            BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
                if left == right && self.is_numeric(left) => Some(left),
            BinaryOperator::Remainder if left == int_type && right == int_type => Some(int_type),
            BinaryOperator::Equal | BinaryOperator::NotEqual
                if left == right && self.supports_equality(left) => Some(bool_type),
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual
                if left == right && self.supports_ordering(left) => Some(bool_type),
            BinaryOperator::In if self.membership_matches(left, right) => Some(bool_type),
            _ => None,
        };
        let state = result.map_or_else(
            || self.analysis.error(
                operator_span,
                "binary operator is not defined for these operand types",
            ),
            TypeState::Resolved,
        );
        self.coerce(expression, state, expected)
    }

    fn resolve_index(
        &mut self,
        expression: &'ast Expression,
        value: &'ast Expression,
        index: &'ast Expression,
        current: TypeState,
        expected: Option<TypeId>,
    ) -> TypeState {
        let value = self.resolve_expression(value, None);
        let index = self.resolve_expression(index, None);
        if !matches!(current, TypeState::Deferred(_)) {
            return self.coerce(expression, current, expected);
        }
        let (TypeState::Resolved(value), TypeState::Resolved(index)) = (value, index) else {
            let state = self.operand_pair_state(expression, value, index, current);
            return self.coerce(expression, state, expected);
        };
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let state = match self.analysis.types.get(value).clone() {
            ResolvedType::List(element) if index == int => TypeState::Resolved(element),
            ResolvedType::Map { key, value } if index == key => TypeState::Resolved(value),
            ResolvedType::Primitive(PrimitiveType::Str) if index == int => {
                TypeState::Resolved(self.analysis.types.primitive(PrimitiveType::Char))
            }
            _ => self.analysis.error(
                expression.span,
                "index operation is not defined for these operand types",
            ),
        };
        self.coerce(expression, state, expected)
    }

    fn resolve_member(
        &mut self,
        expression: &'ast Expression,
        value: &'ast Expression,
        member: Member,
        current: TypeState,
        expected: Option<TypeId>,
    ) -> TypeState {
        let value = self.resolve_expression(value, None);
        if !matches!(current, TypeState::Deferred(_)) {
            return self.coerce(expression, current, expected);
        }
        let TypeState::Resolved(value) = value else {
            let state = self.operand_state(expression, value, current);
            return self.coerce(expression, state, expected);
        };
        let state = match self.analysis.types.get(value).clone() {
            ResolvedType::Nominal(declaration) => {
                match (self.analysis.type_definition(declaration).kind.clone(), member) {
                    (TypeDefinitionKind::Struct(members), Member::Named(name)) => {
                        let spelling = self.analysis.identifier_text(name.span).to_owned();
                        members.iter().find(|field| {
                            self.analysis.identifier_text(field.name.span) == spelling.as_str()
                        }).map_or_else(
                            || self.analysis.error(name.span, format!("struct has no member '{spelling}'")),
                            |field| field.ty,
                        )
                    }
                    (TypeDefinitionKind::Tuple(members), Member::TupleIndex(span)) => {
                        self.parse_tuple_index(span).and_then(|index| members.get(index)).map_or_else(
                            || self.analysis.error(span, "tuple member index is out of range"),
                            |field| field.ty,
                        )
                    }
                    (TypeDefinitionKind::Union { .. }, _) => current,
                    (TypeDefinitionKind::Invalid, _) => TypeState::Error,
                    (TypeDefinitionKind::Struct(_), Member::TupleIndex(span)) => self
                        .analysis
                        .error(span, "struct members must be accessed by name"),
                    (TypeDefinitionKind::Tuple(_), Member::Named(name)) => self
                        .analysis
                        .error(name.span, "tuple members must be accessed by position"),
                }
            }
            ResolvedType::Union(_) => current,
            ResolvedType::List(_) => self.resolve_method_member(
                member,
                &["append", "removeIndex", "len"],
            ),
            ResolvedType::Map { .. } => {
                self.resolve_method_member(member, &["removeKey", "len"])
            }
            ResolvedType::Primitive(PrimitiveType::Str) => {
                self.resolve_method_member(member, &["len"])
            }
            ResolvedType::Primitive(_) => self.analysis.error(
                expression.span,
                "primitive value has no accessible member",
            ),
        };
        self.coerce(expression, state, expected)
    }

    fn resolve_method_member(&mut self, member: Member, methods: &[&str]) -> TypeState {
        match member {
            Member::Named(name) => {
                let spelling = self.analysis.identifier_text(name.span).to_owned();
                if methods.contains(&spelling.as_str()) {
                    self.analysis.error(
                        name.span,
                        "container methods must be used as direct call targets",
                    )
                } else {
                    self.analysis.error(
                        name.span,
                        format!("container has no member '{spelling}'"),
                    )
                }
            }
            Member::TupleIndex(span) => self
                .analysis
                .error(span, "container members must be accessed by name"),
        }
    }

    fn resolve_list(
        &mut self,
        expression: &'ast Expression,
        elements: &'ast [Expression],
        expected: Option<TypeId>,
    ) -> TypeState {
        let expected_list = expected.and_then(|ty| self.expected_container(ty, true));
        let expected_element = expected_list.and_then(|ty| match self.analysis.types.get(ty) {
            ResolvedType::List(element) => Some(*element),
            _ => None,
        });
        if elements.is_empty() && expected_element.is_none() {
            return self.analysis.error(
                expression.span,
                "empty list requires an expected list type or explicit ascription",
            );
        }
        let states = elements
            .iter()
            .map(|element| self.resolve_expression(element, expected_element))
            .collect::<Vec<_>>();
        if states.iter().any(|state| *state == TypeState::Error) {
            return TypeState::Error;
        }
        if states.iter().any(|state| *state == TypeState::Never) {
            return TypeState::Never;
        }
        if states.iter().any(|state| *state == TypeState::NoValue) {
            return TypeState::Error;
        }
        if let Some(deferred) = states.iter().find(|state| matches!(state, TypeState::Deferred(_))) {
            return *deferred;
        }
        if let (Some(list), Some(_)) = (expected_list, expected_element) {
            return self.coerce(expression, TypeState::Resolved(list), expected);
        }
        let Some(element) = states.iter().find_map(|state| match state {
            TypeState::Resolved(ty) => Some(*ty),
            _ => None,
        }) else {
            return states.first().copied().unwrap_or(TypeState::Error);
        };
        if !states
            .iter()
            .all(|state| matches!(state, TypeState::Resolved(ty) if *ty == element))
        {
            return self.analysis.error(
                expression.span,
                "list elements have incompatible types",
            );
        }
        let list = self.analysis.types.intern(ResolvedType::List(element));
        self.coerce(expression, TypeState::Resolved(list), expected)
    }

    fn resolve_map(
        &mut self,
        expression: &'ast Expression,
        entries: &'ast [crate::ast::MapEntry],
        expected: Option<TypeId>,
    ) -> TypeState {
        let expected_map = expected.and_then(|ty| self.expected_container(ty, false));
        let expected_parts = expected_map.and_then(|ty| match self.analysis.types.get(ty) {
            ResolvedType::Map { key, value } => Some((*key, *value)),
            _ => None,
        });
        if entries.is_empty() && expected_parts.is_none() {
            return self.analysis.error(
                expression.span,
                "empty map requires an expected map type or explicit ascription",
            );
        }
        let states = entries
            .iter()
            .map(|entry| {
                (
                    self.resolve_expression(&entry.key, expected_parts.map(|parts| parts.0)),
                    self.resolve_expression(&entry.value, expected_parts.map(|parts| parts.1)),
                )
            })
            .collect::<Vec<_>>();
        if states
            .iter()
            .any(|(key, value)| *key == TypeState::Error || *value == TypeState::Error)
        {
            return TypeState::Error;
        }
        if states
            .iter()
            .any(|(key, value)| *key == TypeState::Never || *value == TypeState::Never)
        {
            return TypeState::Never;
        }
        if states
            .iter()
            .any(|(key, value)| *key == TypeState::NoValue || *value == TypeState::NoValue)
        {
            return TypeState::Error;
        }
        if let Some(deferred) = states.iter().find_map(|(key, value)| {
            [*key, *value]
                .into_iter()
                .find(|state| matches!(state, TypeState::Deferred(_)))
        }) {
            return deferred;
        }
        if let (Some(map), Some(_)) = (expected_map, expected_parts) {
            return self.coerce(expression, TypeState::Resolved(map), expected);
        }
        let Some((TypeState::Resolved(key), TypeState::Resolved(value))) = states.first().copied()
        else {
            return TypeState::Error;
        };
        if !states.iter().all(|state| {
            state.0 == TypeState::Resolved(key) && state.1 == TypeState::Resolved(value)
        }) {
            return self.analysis.error(expression.span, "map entries have incompatible types");
        }
        if !self.analysis.is_valid_map_key(key, &mut Vec::new()) {
            return self.analysis.error(
                expression.span,
                "map key type must be int, str, bool, or an immutable tuple of valid map keys",
            );
        }
        let map = self.analysis.types.intern(ResolvedType::Map { key, value });
        self.coerce(expression, TypeState::Resolved(map), expected)
    }

    fn resolve_typed_empty(
        &mut self,
        expression: &'ast Expression,
        ty: &'ast Type,
        expected: Option<TypeId>,
    ) -> TypeState {
        let state = self.analysis.resolve_type(ty);
        let TypeState::Resolved(resolved) = state else {
            return TypeState::Error;
        };
        let shape_matches = matches!(
            (&expression.kind, self.analysis.types.get(resolved)),
            (ExpressionKind::TypedEmptyList(_), ResolvedType::List(_))
                | (ExpressionKind::TypedEmptyMap(_), ResolvedType::Map { .. })
        );
        if !shape_matches {
            return self.analysis.error(expression.span, "empty collection ascription has the wrong container type");
        }
        if let ResolvedType::Map { key, .. } = self.analysis.types.get(resolved).clone()
            && !self.analysis.is_valid_map_key(key, &mut Vec::new())
        {
            return self.analysis.error(
                ty.span,
                "map key type must be int, str, bool, or an immutable tuple of valid map keys",
            );
        }
        self.coerce(expression, TypeState::Resolved(resolved), expected)
    }

    fn resolve_if(
        &mut self,
        expression: &'ast Expression,
        branches: &'ast [crate::ast::ConditionalExpressionBranch],
        else_branch: &'ast ExpressionBody,
        expected: Option<TypeId>,
    ) -> TypeState {
        let mut states = Vec::new();
        let mut valid_conditions = true;
        for branch in branches {
            valid_conditions &= self.resolve_condition(&branch.condition);
            states.push(self.resolve_expression_body(&branch.body, expected));
        }
        states.push(self.resolve_expression_body(else_branch, expected));
        if !valid_conditions || states.iter().any(|state| *state == TypeState::Error) {
            return TypeState::Error;
        }
        if states.iter().any(|state| *state == TypeState::NoValue) {
            return TypeState::Error;
        }
        if let Some(expected) = expected {
            if states.iter().all(|state| {
                *state == TypeState::Resolved(expected) || *state == TypeState::Never
            }) {
                return TypeState::Resolved(expected);
            }
        }
        let resolved = states.iter().find_map(|state| match state {
            TypeState::Resolved(ty) => Some(*ty),
            _ => None,
        });
        match resolved {
            Some(ty) if states.iter().all(|state| {
                *state == TypeState::Resolved(ty) || *state == TypeState::Never
            }) => TypeState::Resolved(ty),
            None if states.iter().all(|state| *state == TypeState::Never) => TypeState::Never,
            _ => self.analysis.error(expression.span, "if-expression branches have incompatible types"),
        }
    }

    fn resolve_expression_body(
        &mut self,
        body: &'ast ExpressionBody,
        expected: Option<TypeId>,
    ) -> TypeState {
        match &body.kind {
            ExpressionBodyKind::Block(block) => self.resolve_block(block, expected),
            ExpressionBodyKind::Expression(expression) => {
                self.resolve_expression(expression, expected)
            }
        }
    }

    fn resolve_call(
        &mut self,
        expression: &'ast Expression,
        arguments: &'ast [crate::ast::Argument],
        expected: Option<TypeId>,
    ) -> TypeState {
        let target = self.analysis.call_resolution(expression).map(|call| call.target);
        let result = match target {
            Some(CallTarget::Callable(CallableId::Function(function))) => {
                let (parameters, result) = {
                    let signature = self.analysis.function_signature(function);
                    (
                        signature.parameters.iter().map(|parameter| parameter.ty).collect::<Vec<_>>(),
                        signature.result,
                    )
                };
                let mut states = Vec::new();
                for (index, argument) in arguments.iter().enumerate() {
                    let expected = parameters.get(index).and_then(|parameter| self.resolved(*parameter));
                    states.push(self.resolve_argument(argument, expected));
                }
                self.finish_call(expression, &states, result)
            }
            Some(CallTarget::Callable(CallableId::Intrinsic(intrinsic))) => {
                let signature = self.analysis.intrinsic_signature(intrinsic).clone();
                let mut failed = false;
                let mut states = Vec::new();
                match signature.arguments {
                    IntrinsicArguments::Exact(types) => {
                        for (index, argument) in arguments.iter().enumerate() {
                            let expected = types.get(index).copied();
                            let state = self.resolve_argument(argument, expected);
                            failed |= state == TypeState::Error;
                            states.push(state);
                        }
                    }
                    IntrinsicArguments::OnePrintable | IntrinsicArguments::ZeroOrOnePrintable => {
                        for argument in arguments {
                            let state = self.resolve_argument(argument, None);
                            match state {
                                TypeState::Resolved(ty) => {
                                    if !self.is_printable(ty, &mut Vec::new()) {
                                        self.analysis.error(
                                            argument.span,
                                            "print argument must be a primitive or printable tuple",
                                        );
                                        failed = true;
                                    }
                                }
                                TypeState::Error => failed = true,
                                TypeState::NoValue => {
                                    self.analysis.error(
                                        argument.span,
                                        "print argument must produce a value",
                                    );
                                    failed = true;
                                }
                                _ => {}
                            }
                            states.push(state);
                        }
                    }
                }
                if failed {
                    TypeState::Error
                } else {
                    self.finish_call(expression, &states, signature.result)
                }
            }
            Some(CallTarget::Builtin(method)) => {
                let expected_argument = self.builtin_argument_type(expression, method);
                let mut states = Vec::new();
                for argument in arguments {
                    states.push(self.resolve_argument(argument, expected_argument));
                }
                let result = match method {
                    BuiltinMethod::ListLen | BuiltinMethod::MapLen | BuiltinMethod::StrLen => {
                        TypeState::Resolved(self.analysis.types.primitive(PrimitiveType::Int))
                    }
                    BuiltinMethod::ListAppend
                    | BuiltinMethod::ListRemoveIndex
                    | BuiltinMethod::MapRemoveKey => TypeState::NoValue,
                };
                self.finish_call(expression, &states, result)
            }
            Some(CallTarget::Callable(CallableId::Constructor(declaration))) => {
                self.resolve_constructor(expression, declaration, arguments)
            }
            Some(CallTarget::QualifiedConstructor(declaration)) => {
                self.resolve_qualified_constructor(expression, declaration, arguments)
            }
            Some(CallTarget::ErrorConstructor) => {
                self.resolve_error_constructor(expression, arguments, expected)
            }
            Some(CallTarget::Expression) => {
                self.resolve_method_call(expression, arguments)
            }
            _ => {
                for argument in arguments {
                    self.resolve_argument(argument, None);
                }
                TypeState::Error
            }
        };
        self.coerce(expression, result, expected)
    }

    fn resolve_method_call(
        &mut self,
        expression: &'ast Expression,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let ExpressionKind::Call { callee, .. } = &expression.kind else { unreachable!() };
        let ExpressionKind::Member { value, member } = &callee.kind else {
            for argument in arguments {
                self.resolve_argument(argument, None);
            }
            return TypeState::Error;
        };
        let receiver = self.resolve_expression(value, None);
        let TypeState::Resolved(receiver) = receiver else {
            for argument in arguments {
                self.resolve_argument(argument, None);
            }
            let current = self.analysis.expression_annotation(expression)
                .map_or(TypeState::Error, |annotation| annotation.state);
            return self.operand_state(expression, receiver, current);
        };
        if !self.union_alternatives(receiver).is_empty() {
            for argument in arguments {
                self.resolve_argument(argument, None);
            }
            let current = self
                .analysis
                .expression_annotation(expression)
                .map_or(TypeState::Error, |annotation| annotation.state);
            assert!(
                matches!(current, TypeState::Deferred(_)),
                "flow-dependent method call lacks a deferred type annotation"
            );
            return current;
        }
        let Member::Named(name) = member else {
            for argument in arguments {
                self.resolve_argument(argument, None);
            }
            return self.analysis.error(expression.span, "tuple member is not callable");
        };
        let spelling = self.analysis.identifier_text(name.span).to_owned();
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let method = match self.analysis.types.get(receiver).clone() {
            ResolvedType::List(element) => match spelling.as_str() {
                "append" => Some((BuiltinMethod::ListAppend, Some(element), TypeState::NoValue)),
                "removeIndex" => Some((BuiltinMethod::ListRemoveIndex, Some(int), TypeState::NoValue)),
                "len" => Some((BuiltinMethod::ListLen, None, TypeState::Resolved(int))),
                _ => None,
            },
            ResolvedType::Map { key, .. } => match spelling.as_str() {
                "removeKey" => Some((BuiltinMethod::MapRemoveKey, Some(key), TypeState::NoValue)),
                "len" => Some((BuiltinMethod::MapLen, None, TypeState::Resolved(int))),
                _ => None,
            },
            ResolvedType::Primitive(PrimitiveType::Str) if spelling == "len" => {
                Some((BuiltinMethod::StrLen, None, TypeState::Resolved(int)))
            }
            _ => None,
        };
        let Some((method, expected, result)) = method else {
            for argument in arguments {
                self.resolve_argument(argument, None);
            }
            return self.analysis.error(
                name.span,
                format!("type has no built-in method '{spelling}'"),
            );
        };
        if let Some(call) = self
            .analysis
            .calls
            .iter_mut()
            .find(|call| std::ptr::eq(call.node, expression))
        {
            call.target = CallTarget::Builtin(method);
        }
        let expected_count = usize::from(expected.is_some());
        let mut failed = arguments.len() != expected_count;
        let mut states = Vec::new();
        if failed {
            self.analysis.error(
                expression.span,
                format!(
                    "method '{spelling}' expects {expected_count} argument(s), but {} were provided",
                    arguments.len()
                ),
            );
        }
        for argument in arguments {
            if matches!(&argument.kind, ArgumentKind::Named { .. }) {
                self.analysis.error(
                    argument.span,
                    "built-in methods do not accept named arguments",
                );
                failed = true;
            }
            let state = self.resolve_argument(argument, expected);
            failed |= state == TypeState::Error;
            states.push(state);
        }
        if failed {
            TypeState::Error
        } else {
            self.finish_call(expression, &states, result)
        }
    }

    fn finish_call(
        &self,
        expression: &'ast Expression,
        arguments: &[TypeState],
        result: TypeState,
    ) -> TypeState {
        if arguments.iter().any(|state| matches!(state, TypeState::Error | TypeState::NoValue)) {
            TypeState::Error
        } else if arguments.contains(&TypeState::Never) {
            TypeState::Never
        } else if let Some(deferred) = arguments.iter().find(|state| matches!(state, TypeState::Deferred(_))) {
            self.analysis.expression_annotation(expression)
                .map_or(*deferred, |annotation| match annotation.state {
                    TypeState::Deferred(_) => annotation.state,
                    _ => *deferred,
                })
        } else {
            result
        }
    }

    fn resolve_argument(&mut self, argument: &'ast crate::ast::Argument, expected: Option<TypeId>) -> TypeState {
        match &argument.kind {
            ArgumentKind::Positional(value) | ArgumentKind::Named { value, .. } => {
                self.resolve_expression(value, expected)
            }
        }
    }

    fn resolve_constructor(
        &mut self,
        expression: &'ast Expression,
        declaration: TypeDeclarationId,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let result_type = self.analysis.types.intern(ResolvedType::Nominal(declaration));
        match self.analysis.type_definition(declaration).kind.clone() {
            TypeDefinitionKind::Struct(members) => {
                let mut selected = Vec::new();
                let mut seen = Vec::new();
                let mut failed = false;
                let mut states = Vec::new();
                for argument in arguments {
                    let ArgumentKind::Named { name, value } = &argument.kind else {
                        states.push(self.resolve_argument(argument, None));
                        self.analysis.error(argument.span, "struct constructor arguments must be named");
                        failed = true;
                        continue;
                    };
                    let spelling = self.analysis.identifier_text(name.span).to_owned();
                    let member = members.iter().position(|member| {
                        self.analysis.identifier_text(member.name.span) == spelling.as_str()
                    });
                    let Some(member) = member else {
                        states.push(self.resolve_expression(value, None));
                        self.analysis.error(name.span, format!("unknown struct member '{spelling}'"));
                        failed = true;
                        continue;
                    };
                    if seen.contains(&member) {
                        self.analysis.error(name.span, format!("duplicate struct argument '{spelling}'"));
                        failed = true;
                    }
                    seen.push(member);
                    selected.push(member);
                    let expected = self.resolved(members[member].ty);
                    let state = self.resolve_expression(value, expected);
                    failed |= state == TypeState::Error;
                    states.push(state);
                }
                for (index, member) in members.iter().enumerate() {
                    if !seen.contains(&index) {
                        let name = self.analysis.identifier_text(member.name.span).to_owned();
                        self.analysis.error(expression.span, format!("missing struct argument '{name}'"));
                        failed = true;
                    }
                }
                let state = if failed {
                    TypeState::Error
                } else {
                    self.finish_call(expression, &states, TypeState::Resolved(result_type))
                };
                if state == TypeState::Resolved(result_type) {
                    self.analysis.constructors.push(ConstructorResolution {
                        node: expression,
                        kind: ConstructorKind::Struct {
                            declaration,
                            argument_members: selected.into_boxed_slice(),
                        },
                    });
                }
                state
            }
            TypeDefinitionKind::Tuple(members) => {
                let mut failed = arguments.len() != members.len();
                let mut states = Vec::new();
                if failed {
                    self.analysis.error(expression.span, format!("tuple constructor expects {} argument(s), but {} were provided", members.len(), arguments.len()));
                }
                for (index, argument) in arguments.iter().enumerate() {
                    if matches!(&argument.kind, ArgumentKind::Named { .. }) {
                        self.analysis.error(argument.span, "tuple constructor arguments must be positional");
                        failed = true;
                    }
                    let expected = members
                        .get(index)
                        .and_then(|member| self.resolved(member.ty));
                    let state = self.resolve_argument(argument, expected);
                    failed |= state == TypeState::Error;
                    states.push(state);
                }
                let state = if failed {
                    TypeState::Error
                } else {
                    self.finish_call(expression, &states, TypeState::Resolved(result_type))
                };
                if state == TypeState::Resolved(result_type) {
                    self.analysis.constructors.push(ConstructorResolution { node: expression, kind: ConstructorKind::Tuple(declaration) });
                }
                state
            }
            TypeDefinitionKind::Union { style: UnionStyle::Untagged, alternatives } => {
                self.resolve_union_constructor(expression, declaration, &alternatives, arguments, result_type)
            }
            TypeDefinitionKind::Union { style: UnionStyle::Tagged, .. } => {
                for argument in arguments { self.resolve_argument(argument, None); }
                self.analysis.error(expression.span, "tagged union constructor requires a qualified tag")
            }
            TypeDefinitionKind::Invalid => {
                for argument in arguments {
                    self.resolve_argument(argument, None);
                }
                TypeState::Error
            }
        }
    }

    fn resolve_union_constructor(
        &mut self,
        expression: &'ast Expression,
        declaration: TypeDeclarationId,
        alternatives: &[UnionAlternative],
        arguments: &'ast [crate::ast::Argument],
        result_type: TypeId,
    ) -> TypeState {
        let [argument] = arguments else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(expression.span, "union constructor expects exactly one argument");
        };
        if matches!(&argument.kind, ArgumentKind::Named { .. }) {
            self.resolve_argument(argument, None);
            return self.analysis.error(argument.span, "union constructor argument must be positional");
        }
        let value = match &argument.kind {
            ArgumentKind::Positional(value) => value,
            ArgumentKind::Named { .. } => unreachable!(),
        };
        if matches!(&value.kind, ExpressionKind::List(elements) if elements.is_empty())
            || matches!(&value.kind, ExpressionKind::Map(entries) if entries.is_empty())
        {
            let candidates = alternatives
                .iter()
                .filter_map(|alternative| match alternative {
                    UnionAlternative::Untagged(ty)
                        if self.collection_shape_matches(value, *ty) =>
                    {
                        Some(alternative.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let [alternative] = candidates.as_slice() else {
                self.resolve_argument(argument, None);
                return self.analysis.error(
                    argument.span,
                    "empty collection does not select exactly one union alternative",
                );
            };
            let payload = match alternative {
                UnionAlternative::Untagged(payload) => *payload,
                _ => unreachable!(),
            };
            let state = self.resolve_argument(argument, Some(payload));
            if !matches!(state, TypeState::Resolved(_)) {
                return state;
            }
            self.analysis.constructors.push(ConstructorResolution {
                node: expression,
                kind: ConstructorKind::Union {
                    declaration,
                    alternative: alternative.clone(),
                },
            });
            return TypeState::Resolved(result_type);
        }
        let state = self.resolve_argument(argument, None);
        let TypeState::Resolved(actual) = state else {
            return if state == TypeState::NoValue {
                self.analysis.error(argument.span, "union constructor argument must produce a value")
            } else {
                state
            };
        };
        let matches = alternatives.iter().filter(|alternative| {
            matches!(alternative, UnionAlternative::Untagged(ty) if *ty == actual)
        }).cloned().collect::<Vec<_>>();
        let [alternative] = matches.as_slice() else {
            return self.analysis.error(argument.span, "union constructor argument does not select exactly one alternative");
        };
        self.analysis.constructors.push(ConstructorResolution {
            node: expression,
            kind: ConstructorKind::Union { declaration, alternative: alternative.clone() },
        });
        TypeState::Resolved(result_type)
    }

    fn resolve_qualified_constructor(
        &mut self,
        expression: &'ast Expression,
        declaration: TypeDeclarationId,
        arguments: &'ast [crate::ast::Argument],
    ) -> TypeState {
        let ExpressionKind::Call { callee, .. } = &expression.kind else { unreachable!() };
        let ExpressionKind::Member { member: Member::Named(tag), .. } = &callee.kind else { unreachable!() };
        let spelling = self.analysis.identifier_text(tag.span).to_owned();
        let TypeDefinitionKind::Union { style: UnionStyle::Tagged, alternatives } = self.analysis.type_definition(declaration).kind.clone() else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(tag.span, "qualified constructor requires a tagged union");
        };
        let alternative = alternatives.iter().find(|alternative| {
            matches!(alternative, UnionAlternative::Tagged { tag, .. } if tag.as_ref() == spelling.as_str())
        }).cloned();
        let Some(alternative) = alternative else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(tag.span, format!("unknown union tag '{spelling}'"));
        };
        let [argument] = arguments else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(expression.span, "tagged union constructor expects exactly one argument");
        };
        if matches!(&argument.kind, ArgumentKind::Named { .. }) {
            self.resolve_argument(argument, None);
            return self.analysis.error(argument.span, "tagged union constructor argument must be positional");
        }
        let payload = match &alternative { UnionAlternative::Tagged { payload, .. } => *payload, _ => unreachable!() };
        let state = self.resolve_argument(argument, Some(payload));
        if !matches!(state, TypeState::Resolved(_)) { return state; }
        self.analysis.constructors.push(ConstructorResolution {
            node: expression,
            kind: ConstructorKind::TaggedUnion { declaration, alternative },
        });
        TypeState::Resolved(self.analysis.types.intern(ResolvedType::Nominal(declaration)))
    }

    fn resolve_error_constructor(
        &mut self,
        expression: &'ast Expression,
        arguments: &'ast [crate::ast::Argument],
        expected: Option<TypeId>,
    ) -> TypeState {
        let Some(union_type) = expected else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(expression.span, "Error constructor requires an expected union type");
        };
        let alternatives = self.union_alternatives(union_type);
        let errors = alternatives.iter().filter(|alternative| matches!(alternative, UnionAlternative::Error(_))).cloned().collect::<Vec<_>>();
        let [alternative] = errors.as_slice() else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(expression.span, "expected type does not contain one Error alternative");
        };
        let [argument] = arguments else {
            for argument in arguments { self.resolve_argument(argument, None); }
            return self.analysis.error(expression.span, "Error constructor expects exactly one argument");
        };
        if matches!(&argument.kind, ArgumentKind::Named { .. }) {
            self.resolve_argument(argument, None);
            return self.analysis.error(argument.span, "Error constructor argument must be positional");
        }
        let payload = match alternative { UnionAlternative::Error(payload) => *payload, _ => unreachable!() };
        let state = self.resolve_argument(argument, Some(payload));
        if !matches!(state, TypeState::Resolved(_)) { return state; }
        self.analysis.constructors.push(ConstructorResolution {
            node: expression,
            kind: ConstructorKind::Error { union_type, alternative: alternative.clone() },
        });
        TypeState::Resolved(union_type)
    }

    fn builtin_argument_type(&self, expression: &'ast Expression, method: BuiltinMethod) -> Option<TypeId> {
        let ExpressionKind::Call { callee, .. } = &expression.kind else { return None };
        let ExpressionKind::Member { value, .. } = &callee.kind else { return None };
        let receiver = self.analysis.expression_annotation(value).and_then(|annotation| self.resolved(annotation.state));
        match (method, receiver.map(|ty| self.analysis.types.get(ty))) {
            (BuiltinMethod::ListAppend, Some(ResolvedType::List(element))) => Some(*element),
            (BuiltinMethod::ListRemoveIndex, _) => Some(self.analysis.types.primitive(PrimitiveType::Int)),
            (BuiltinMethod::MapRemoveKey, Some(ResolvedType::Map { key, .. })) => Some(*key),
            _ => None,
        }
    }

    fn collection_shape_matches(&self, expression: &Expression, ty: TypeId) -> bool {
        matches!(
            (&expression.kind, self.analysis.types.get(ty)),
            (ExpressionKind::List(_), ResolvedType::List(_))
                | (ExpressionKind::Map(_), ResolvedType::Map { .. })
        )
    }

    fn expected_container(&self, expected: TypeId, list: bool) -> Option<TypeId> {
        let direct = matches!(
            (list, self.analysis.types.get(expected)),
            (true, ResolvedType::List(_)) | (false, ResolvedType::Map { .. })
        );
        if direct {
            return Some(expected);
        }
        let matches = self
            .union_alternatives(expected)
            .into_iter()
            .filter_map(|alternative| match alternative {
                UnionAlternative::Untagged(ty)
                    if matches!(
                        (list, self.analysis.types.get(ty)),
                        (true, ResolvedType::List(_))
                            | (false, ResolvedType::Map { .. })
                    ) => Some(ty),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [only] = matches.as_slice() else {
            return None;
        };
        Some(*only)
    }

    fn coerce(&mut self, expression: &'ast Expression, state: TypeState, expected: Option<TypeId>) -> TypeState {
        let Some(expected) = expected else { return state; };
        match state {
            TypeState::Resolved(actual) if actual == expected => state,
            TypeState::Resolved(actual) => {
                let matches = self.union_alternatives(expected).into_iter().filter(|alternative| {
                    matches!(alternative, UnionAlternative::Untagged(ty) if *ty == actual)
                }).collect::<Vec<_>>();
                let [alternative] = matches.as_slice() else {
                    return self.analysis.error(expression.span, "expression type does not match expected type");
                };
                self.analysis.union_injections.push(UnionInjection {
                    node: expression,
                    union_type: expected,
                    alternative: alternative.clone(),
                });
                TypeState::Resolved(expected)
            }
            TypeState::Never | TypeState::Error | TypeState::Deferred(_) => state,
            TypeState::NoValue => self.analysis.error(expression.span, "expression does not produce an expected value"),
        }
    }

    fn union_alternatives(&self, ty: TypeId) -> Vec<UnionAlternative> {
        match self.analysis.types.get(ty) {
            ResolvedType::Union(alternatives) => alternatives.to_vec(),
            ResolvedType::Nominal(declaration) => match &self.analysis.type_definition(*declaration).kind {
                TypeDefinitionKind::Union { alternatives, .. } => alternatives.to_vec(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn resolved(&self, state: TypeState) -> Option<TypeId> {
        match state { TypeState::Resolved(ty) => Some(ty), _ => None }
    }

    fn operand_state(
        &mut self,
        expression: &'ast Expression,
        state: TypeState,
        deferred: TypeState,
    ) -> TypeState {
        match state {
            TypeState::Error => TypeState::Error,
            TypeState::Never => TypeState::Never,
            TypeState::Deferred(_) => deferred,
            TypeState::NoValue => self.analysis.error(
                expression.span,
                "operand expression does not produce a value",
            ),
            TypeState::Resolved(_) => unreachable!(),
        }
    }

    fn operand_pair_state(
        &mut self,
        expression: &'ast Expression,
        left: TypeState,
        right: TypeState,
        deferred: TypeState,
    ) -> TypeState {
        if left == TypeState::Error || right == TypeState::Error {
            TypeState::Error
        } else if left == TypeState::Never || right == TypeState::Never {
            TypeState::Never
        } else if matches!(left, TypeState::Deferred(_))
            || matches!(right, TypeState::Deferred(_))
        {
            deferred
        } else {
            self.analysis.error(
                expression.span,
                "operator operand does not produce a value",
            )
        }
    }

    fn target_state(
        &mut self,
        target: &'ast AssignmentTarget,
        state: TypeState,
        deferred: TypeState,
    ) -> TypeState {
        match state {
            TypeState::Error => TypeState::Error,
            TypeState::Never => TypeState::Never,
            TypeState::Deferred(_) => deferred,
            TypeState::NoValue => self.analysis.error(
                target.span,
                "assignment receiver does not produce a value",
            ),
            TypeState::Resolved(_) => unreachable!(),
        }
    }

    fn target_pair_state(
        &mut self,
        target: &'ast AssignmentTarget,
        value: TypeState,
        index: TypeState,
        deferred: TypeState,
    ) -> TypeState {
        if value == TypeState::Error || index == TypeState::Error {
            TypeState::Error
        } else if value == TypeState::Never || index == TypeState::Never {
            TypeState::Never
        } else if matches!(value, TypeState::Deferred(_))
            || matches!(index, TypeState::Deferred(_))
        {
            deferred
        } else {
            self.analysis.error(
                target.span,
                "assignment index does not produce a value",
            )
        }
    }

    fn is_primitive(&self, ty: TypeId, primitive: PrimitiveType) -> bool {
        ty == self.analysis.types.primitive(primitive)
    }

    fn is_numeric(&self, ty: TypeId) -> bool {
        self.is_primitive(ty, PrimitiveType::Int)
            || self.is_primitive(ty, PrimitiveType::Float)
    }

    fn supports_ordering(&self, ty: TypeId) -> bool {
        matches!(
            self.analysis.types.get(ty),
            ResolvedType::Primitive(
                PrimitiveType::Int
                    | PrimitiveType::Float
                    | PrimitiveType::Str
                    | PrimitiveType::Char
            )
        )
    }

    fn supports_equality(&self, ty: TypeId) -> bool {
        self.supports_equality_inner(ty, &mut Vec::new())
    }

    fn supports_equality_inner(
        &self,
        ty: TypeId,
        visiting: &mut Vec<TypeDeclarationId>,
    ) -> bool {
        match self.analysis.types.get(ty) {
            ResolvedType::Primitive(_) | ResolvedType::List(_) | ResolvedType::Map { .. } => true,
            ResolvedType::Nominal(declaration) => {
                if visiting.contains(declaration) {
                    return false;
                }
                match &self.analysis.type_definition(*declaration).kind {
                    TypeDefinitionKind::Struct(_) => true,
                    TypeDefinitionKind::Tuple(members) => {
                        visiting.push(*declaration);
                        let comparable = members.iter().all(|member| {
                            matches!(
                                member.ty,
                                TypeState::Resolved(member_type)
                                    if self.supports_equality_inner(member_type, visiting)
                            )
                        });
                        visiting.pop();
                        comparable
                    }
                    TypeDefinitionKind::Union { .. } | TypeDefinitionKind::Invalid => false,
                }
            }
            ResolvedType::Union(_) => false,
        }
    }

    fn membership_matches(&self, item: TypeId, container: TypeId) -> bool {
        match self.analysis.types.get(container) {
            ResolvedType::List(element) => item == *element,
            ResolvedType::Map { key, .. } => item == *key,
            _ => false,
        }
    }

    fn is_printable(&self, ty: TypeId, visiting: &mut Vec<TypeDeclarationId>) -> bool {
        match self.analysis.types.get(ty) {
            ResolvedType::Primitive(_) => true,
            ResolvedType::Nominal(declaration) => {
                if visiting.contains(declaration) {
                    return false;
                }
                let TypeDefinitionKind::Tuple(members) =
                    &self.analysis.type_definition(*declaration).kind
                else {
                    return false;
                };
                visiting.push(*declaration);
                let printable = members.iter().all(|member| {
                    matches!(member.ty, TypeState::Resolved(ty) if self.is_printable(ty, visiting))
                });
                visiting.pop();
                printable
            }
            ResolvedType::List(_) | ResolvedType::Map { .. } | ResolvedType::Union(_) => false,
        }
    }

    fn parse_tuple_index(&self, span: Span) -> Option<usize> {
        self.analysis
            .identifier_text(span)
            .replace('_', "")
            .parse::<usize>()
            .ok()
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

/// Creates the analysis result, resolves declarations and lexical bindings, and
/// resolves names, types, constructors, and expected-type-dependent expressions,
/// leaving only later flow-sensitive analysis deferred.
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
    let binding_types = (0..bindings.len())
        .map(|index| BindingTypeAnnotation {
            binding: BindingId(index),
            state: TypeState::Error,
        })
        .collect();
    let mut analysis = Analysis {
        source,
        program,
        declarations,
        bindings,
        name_uses: Vec::new(),
        calls: Vec::new(),
        literals: Vec::new(),
        binding_types,
        assignment_targets: Vec::new(),
        constructors: Vec::new(),
        union_injections: Vec::new(),
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
        deferred: Vec::new(),
        diagnostics: Diagnostics::new(),
        pending_map_keys: Vec::new(),
    };
    analysis.reject_reserved_error_bindings();
    analysis.collect_top_level_names();
    analysis.resolve_type_definitions();
    analysis.resolve_function_signatures();
    analysis.validate_referenced_storage();
    analysis.validate_map_keys();
    analysis.validate_inline_layouts();
    analysis.resolve_function_bodies();
    analysis.infer_function_bodies();
    analysis.resolve_expected_types();
    analysis.finalize_call_targets();
    analysis
}

fn push_binding<'ast>(bindings: &mut Vec<BindingRecord<'ast>>, node: BindingNode<'ast>) {
    bindings.push(BindingRecord {
        id: BindingId(bindings.len()),
        span: node.span(),
        mutable: match node {
            BindingNode::Parameter(parameter) => parameter.mutable,
            BindingNode::Local(statement) => match &statement.kind {
                StatementKind::Local { mutable, .. } => *mutable,
                _ => unreachable!("local binding must point to a local statement"),
            },
            BindingNode::Loop(_) => false,
        },
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

    fn binding_named(analysis: &Analysis<'_, '_>, source: &SourceFile, name: &str) -> BindingId {
        analysis
            .bindings
            .iter()
            .find(|binding| {
                &source.text[binding.span.start..binding.span.end] == name
            })
            .map(|binding| binding.id)
            .unwrap()
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
    fn resolves_initializers_before_same_scope_shadowing() {
        let source = source(
            concat!(
                "fn main(seed int) { ",
                "value := seed; value := value; ",
                "{ value := value; value; } value; ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let uses = analysis
            .name_uses
            .iter()
            .map(|name_use| {
                (
                    &source.text[name_use.node.span.start..name_use.node.span.end],
                    name_use.resolution,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            uses,
            vec![
                ("seed", NameResolution::Binding(BindingId(0))),
                ("value", NameResolution::Binding(BindingId(1))),
                ("value", NameResolution::Binding(BindingId(2))),
                ("value", NameResolution::Binding(BindingId(3))),
                ("value", NameResolution::Binding(BindingId(2))),
            ]
        );
        assert!(analysis.diagnostics.is_empty());
    }

    #[test]
    fn respects_brace_colon_and_loop_binding_visibility() {
        let source = source(
            concat!(
                "fn main(items [int]) { ",
                "for item in items: seen := item; seen; item; ",
                "if true: leaked := seen; leaked; ",
                "if true { hidden := seen; } hidden; ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();
        let resolutions = analysis
            .name_uses
            .iter()
            .map(|name_use| {
                (
                    &source.text[name_use.node.span.start..name_use.node.span.end],
                    name_use.resolution,
                )
            })
            .collect::<Vec<_>>();

        assert!(resolutions.contains(&("items", NameResolution::Binding(BindingId(0)))));
        assert!(resolutions.contains(&("item", NameResolution::Binding(BindingId(1)))));
        assert!(resolutions.contains(&("seen", NameResolution::Binding(BindingId(2)))));
        assert!(resolutions.contains(&("leaked", NameResolution::Binding(BindingId(3)))));
        assert_eq!(diagnostics.matches("unknown value 'item'").count(), 1);
        assert_eq!(diagnostics.matches("unknown value 'hidden'").count(), 1);
    }

    #[test]
    fn resolves_assignment_accesses_call_targets_and_member_receivers() {
        let source = source(
            concat!(
                "type Build(int); fn helper() {} ",
                "fn main(var target int) { ",
                "target = target; target += target; helper(); Build(1); print(target); ",
                "target(); target.member; target.member = target; ",
                "missing = target; missing(); Build = target; ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let helper = analysis.function_by_name("helper").unwrap();
        let build = analysis.type_by_name("Build").unwrap();

        assert!(analysis.bindings[0].mutable);
        assert_eq!(
            analysis.calls.iter().map(|call| call.target).collect::<Vec<_>>(),
            vec![
                CallTarget::Callable(CallableId::Function(helper)),
                CallTarget::Callable(CallableId::Constructor(build)),
                CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Print)),
                CallTarget::Binding(BindingId(0)),
                CallTarget::Unknown,
            ]
        );
        assert!(analysis.name_uses.iter().any(|name_use| {
            name_use.access == BindingAccess::Write
                && name_use.resolution == NameResolution::Binding(BindingId(0))
        }));
        assert!(analysis.name_uses.iter().any(|name_use| {
            name_use.access == BindingAccess::ReadWrite
                && name_use.resolution == NameResolution::Binding(BindingId(0))
        }));
        assert!(analysis.name_uses.iter().any(|name_use| {
            name_use.access == BindingAccess::Mutate
                && name_use.resolution == NameResolution::Binding(BindingId(0))
        }));
        let int = analysis.types.primitive(PrimitiveType::Int);
        assert_eq!(analysis.assignment_targets[0].state, TypeState::Resolved(int));
        assert_eq!(analysis.assignment_targets[1].state, TypeState::Resolved(int));
        let diagnostics = analysis.diagnostics.to_string();
        assert_eq!(diagnostics.matches("unknown value 'missing'").count(), 2);
        assert_eq!(diagnostics.matches("unknown value 'Build'").count(), 1);
    }

    #[test]
    fn records_identity_based_typed_assignment_paths() {
        let source = source(concat!(
            "type Inner(value int); type Pair(Inner, int); ",
            "type Outer(inline Inner, shared &Inner, pair Pair, values [int], table {int: int}); ",
            "fn main() { var root := Outer(inline = Inner(value = 1), shared = Inner(value = 2), ",
            "pair = Pair(Inner(value = 3), 4), values = [5], table = {6: 7}); ",
            "root.inline.value = 8; root.shared.value = 9; root.pair.0.value = 10; ",
            "root.values[0] = 11; root.table[6] = 12; }"
        ));
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let root = binding_named(&analysis, &source, "root");
        let Declaration::Function(function) = &program.declarations[3] else {
            unreachable!()
        };
        let targets = function
            .body
            .statements
            .iter()
            .filter_map(|statement| match &statement.kind {
                StatementKind::Assignment { target, .. } => {
                    Some(analysis.assignment_target(target).unwrap())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(targets.len(), 5);
        assert!(targets.iter().all(|target| target.root == Some(root)));
        assert!(matches!(
            targets[0].steps.as_ref(),
            [AccessPathStep::StructMember { storage: MemberStorage::Inline, .. }, AccessPathStep::StructMember { .. }]
        ));
        assert!(matches!(
            targets[1].steps.as_ref(),
            [AccessPathStep::StructMember { storage: MemberStorage::Referenced, .. }, AccessPathStep::StructMember { .. }]
        ));
        assert!(matches!(
            targets[2].steps.as_ref(),
            [AccessPathStep::StructMember { .. }, AccessPathStep::TupleMember { position: 0, .. }, AccessPathStep::StructMember { .. }]
        ));
        assert!(matches!(
            targets[3].steps.last(),
            Some(AccessPathStep::ListIndex { .. })
        ));
        assert!(matches!(
            targets[4].steps.last(),
            Some(AccessPathStep::MapIndex { .. })
        ));
    }

    #[test]
    fn finalizes_call_targets_after_expected_type_resolution() {
        let source = source(concat!(
            "type Box(value int); fn touch(var value Box) {} ",
            "fn main() { value := Box(value = 1); values := [1]; ",
            "touch(value); values.append(2); values.len(); }"
        ));
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let touch = analysis.function_by_name("touch").unwrap();
        assert!(analysis.calls.iter().any(|call| {
            call.target == CallTarget::Callable(CallableId::Function(touch))
        }));
        assert!(analysis.calls.iter().any(|call| {
            call.target == CallTarget::Builtin(BuiltinMethod::ListAppend)
        }));
        assert!(analysis.calls.iter().any(|call| {
            call.target == CallTarget::Builtin(BuiltinMethod::ListLen)
        }));
    }

    #[test]
    fn diagnoses_ambiguous_calls_without_choosing_a_namespace() {
        let both_source = source(
            "type Both(int); fn Both() {} fn main() { Both(); }",
        );
        let program = parser::parse(&both_source).unwrap();
        let analysis = analyze(&both_source, &program);
        let function = analysis.function_by_name("Both").unwrap();
        let constructor = analysis.type_by_name("Both").unwrap();

        assert_eq!(
            analysis.calls[0].target,
            CallTarget::Ambiguous {
                value: CallableId::Function(function),
                constructor,
            }
        );
        assert!(
            analysis
                .diagnostics
                .to_string()
                .contains("call target 'Both' is ambiguous")
        );

        let error_source = source("type Error(int); fn main() { Error(1); }");
        let error_program = parser::parse(&error_source).unwrap();
        let error_analysis = analyze(&error_source, &error_program);
        assert!(
            error_analysis
                .diagnostics
                .to_string()
                .contains("'Error' is reserved")
        );
    }

    #[test]
    fn reserves_error_in_user_defined_identifier_positions() {
        let source = source(concat!(
            "type Error(int); type Record(Error int); ",
            "fn Error(Error int) { Error := 1; for Error in [1] {} } fn main() {}",
        ));
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        assert_eq!(
            analysis.diagnostics.to_string().matches("'Error' is reserved").count(),
            6,
            "{}",
            analysis.diagnostics,
        );
    }

    #[test]
    fn decodes_numeric_literals_and_checks_binary64_boundaries() {
        let source = source(
            concat!(
                "fn main() { ",
                "decimal := 1_000; hexadecimal := 0xff; binary := 0b1010; ",
                "fraction := 1.25e2; minimum := -9_223_372_036_854_775_808; ",
                "too_large := 9_223_372_036_854_775_808; infinite := 1e400; ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let values = analysis
            .literals
            .iter()
            .map(|literal| literal.value.clone())
            .collect::<Vec<_>>();

        for expected in [
            LiteralValue::Integer(1_000),
            LiteralValue::Integer(0xff),
            LiteralValue::Integer(0b1010),
            LiteralValue::Integer(1_u64 << 63),
            LiteralValue::Float(125.0),
        ] {
            assert!(values.contains(&expected), "{values:?}");
        }
        let diagnostics = analysis.diagnostics.to_string();
        assert!(diagnostics.contains("integer literal is outside the signed 64-bit range"));
        assert!(diagnostics.contains("floating-point literal is not a finite binary64 value"));
        assert!(matches!(
            analysis.binding_type(binding_named(&analysis, &source, "minimum")),
            TypeState::Resolved(ty)
                if ty == analysis.types.primitive(PrimitiveType::Int)
        ));
    }

    #[test]
    fn infers_operators_indexes_members_and_container_methods() {
        let source = source(
            concat!(
                "type Record(value int); type Pair(int, str); ",
                "fn inspect(a int, b int, f float, ok bool, text str, xs [int], ",
                "table {str: int}, record Record, pair Pair) { ",
                "sum := a + b; quotient := f / f; logic := ok && true; ",
                "comparison := text < \"z\"; bits := a << b; ",
                "item := xs[a]; mapped := table[text]; character := text[a]; ",
                "field := record.value; second := pair.1; ",
                "list_length := xs.len(); map_length := table.len(); string_length := text.len(); ",
                "xs.append(a); xs.removeIndex(a); table.removeKey(text); ",
                "} fn main() {}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let int = analysis.types.primitive(PrimitiveType::Int);
        let float = analysis.types.primitive(PrimitiveType::Float);
        let bool = analysis.types.primitive(PrimitiveType::Bool);
        let str_type = analysis.types.primitive(PrimitiveType::Str);
        let char_type = analysis.types.primitive(PrimitiveType::Char);

        for name in [
            "sum",
            "bits",
            "item",
            "mapped",
            "field",
            "list_length",
            "map_length",
            "string_length",
        ] {
            assert_eq!(
                analysis.binding_type(binding_named(&analysis, &source, name)),
                TypeState::Resolved(int),
                "{name}",
            );
        }
        assert_eq!(
            analysis.binding_type(binding_named(&analysis, &source, "quotient")),
            TypeState::Resolved(float)
        );
        for name in ["logic", "comparison"] {
            assert_eq!(
                analysis.binding_type(binding_named(&analysis, &source, name)),
                TypeState::Resolved(bool)
            );
        }
        assert_eq!(
            analysis.binding_type(binding_named(&analysis, &source, "character")),
            TypeState::Resolved(char_type)
        );
        assert_eq!(
            analysis.binding_type(binding_named(&analysis, &source, "second")),
            TypeState::Resolved(str_type)
        );
        assert_eq!(
            analysis
                .calls
                .iter()
                .filter(|call| matches!(call.target, CallTarget::Builtin(_)))
                .count(),
            6
        );
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
    }

    #[test]
    fn uses_declared_recursive_and_intrinsic_call_results() {
        let source = source(
            concat!(
                "fn recurse(value int) int { recurse(value) } ",
                "fn main() { result := recurse(1); print(result); panic(\"stop\"); }",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let int = analysis.types.primitive(PrimitiveType::Int);
        let states = analysis
            .calls
            .iter()
            .map(|call| {
                (
                    &source.text[call.callee.span.start..call.callee.span.end],
                    analysis.expression_annotation(call.node).unwrap().state,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            states,
            vec![
                ("recurse", TypeState::Resolved(int)),
                ("recurse", TypeState::Resolved(int)),
                ("print", TypeState::NoValue),
                ("panic", TypeState::Never),
            ]
        );
        assert_eq!(
            analysis.binding_type(binding_named(&analysis, &source, "result")),
            TypeState::Resolved(int)
        );
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
    }

    #[test]
    fn rejects_invalid_operand_argument_index_and_member_types() {
        let source = source(
            concat!(
                "fn takes(value int) bool { true } ",
                "fn main() { ",
                "mixed := 1 + 1.0; truthy := 1 && true; ordered := true < false; ",
                "bad_index := \"x\"[true]; takes(\"x\"); println(1, 2); ",
                "member := 1.missing; ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();

        for expected in [
            "binary operator is not defined for these operand types",
            "index operation is not defined for these operand types",
            "argument type does not match parameter type",
            "println expects zero or one argument",
            "primitive value has no accessible member",
        ] {
            assert!(diagnostics.contains(expected), "{diagnostics}");
        }
    }

    #[test]
    fn resolves_expected_types_and_retains_only_flow_dependent_work() {
        let source = source(
            concat!(
                "type Choice(A(int) | B(str)); type Any(int | str); ",
                "fn accept(input Any) {} ",
                "fn fail() int | Error(str) { Error(\"message\") } ",
                "fn inspect(flag bool, value int | str) { ",
                "same := if flag: 1 else: 2; block_value := { local := 1; local }; ",
                "tested := value is int; tried := value?; pending := [1, 2]; ",
                "tagged := Choice.A(1); ",
                "} fn main() { accept(1); }",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let int = analysis.types.primitive(PrimitiveType::Int);

        for name in ["same", "block_value"] {
            assert_eq!(
                analysis.binding_type(binding_named(&analysis, &source, name)),
                TypeState::Resolved(int)
            );
        }
        for name in ["tested", "tried"] {
            assert!(matches!(
                analysis.binding_type(binding_named(&analysis, &source, name)),
                TypeState::Deferred(_)
            ));
        }
        let TypeState::Resolved(list) =
            analysis.binding_type(binding_named(&analysis, &source, "pending"))
        else {
            panic!("nonempty list should infer its element type");
        };
        assert_eq!(analysis.types.get(list), &ResolvedType::List(int));
        assert!(matches!(
            analysis.binding_type(binding_named(&analysis, &source, "tagged")),
            TypeState::Resolved(_)
        ));
        assert!(analysis.deferred.iter().any(|deferred| {
            !deferred.resolved && deferred.reason == DeferredReason::FlowDependentType
        }));
        assert!(analysis.deferred.iter().all(|deferred| {
            deferred.resolved || deferred.reason == DeferredReason::FlowDependentType
        }));
        assert_eq!(analysis.constructors.len(), 2);
        assert_eq!(analysis.union_injections.len(), 1);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
    }

    #[test]
    fn resolves_struct_tuple_and_contextual_collection_construction() {
        let source = source(
            concat!(
                "type Point(x int, label str); type Pair(int, str); ",
                "fn consume(items [int], table {str: int}) {} ",
                "fn consumeNested(items [[int]]) {} ",
                "fn main() { ",
                "point := Point(label = \"p\", x = 1); pair := Pair(2, \"q\"); ",
                "consume([], {}); consumeNested([[]]); ",
                "explicit := [] : [int]; nested := [[1], [2]]; ",
                "indexed := [1][0]; equal := [1] == [1]; length := [1].len(); ",
                "var reassigned := [1]; reassigned = []; ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let int = analysis.types.primitive(PrimitiveType::Int);

        assert_eq!(analysis.constructors.len(), 2);
        assert!(matches!(
            &analysis.constructors[0].kind,
            ConstructorKind::Struct { argument_members, .. }
                if argument_members.as_ref() == [1, 0]
        ));
        assert!(matches!(
            analysis.constructors[1].kind,
            ConstructorKind::Tuple(_)
        ));
        let TypeState::Resolved(explicit) =
            analysis.binding_type(binding_named(&analysis, &source, "explicit"))
        else {
            panic!("typed empty list should resolve");
        };
        assert_eq!(analysis.types.get(explicit), &ResolvedType::List(int));
        let TypeState::Resolved(nested) =
            analysis.binding_type(binding_named(&analysis, &source, "nested"))
        else {
            panic!("nested nonempty list should resolve");
        };
        assert!(matches!(
            analysis.types.get(nested),
            ResolvedType::List(inner)
                if matches!(analysis.types.get(*inner), ResolvedType::List(element) if *element == int)
        ));
        assert_eq!(
            analysis.binding_type(binding_named(&analysis, &source, "indexed")),
            TypeState::Resolved(int)
        );
        for name in ["equal"] {
            assert_eq!(
                analysis.binding_type(binding_named(&analysis, &source, name)),
                TypeState::Resolved(analysis.types.primitive(PrimitiveType::Bool))
            );
        }
        assert_eq!(
            analysis.binding_type(binding_named(&analysis, &source, "length")),
            TypeState::Resolved(int)
        );
        let TypeState::Resolved(reassigned) =
            analysis.binding_type(binding_named(&analysis, &source, "reassigned"))
        else {
            panic!("assignment target should acquire the resolved local type");
        };
        assert_eq!(analysis.types.get(reassigned), &ResolvedType::List(int));
        assert!(analysis.calls.iter().any(|call| {
            matches!(call.target, CallTarget::Builtin(BuiltinMethod::ListLen))
        }));
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
    }

    #[test]
    fn diagnoses_invalid_constructors_and_uncontextualized_empty_collections() {
        let source = source(
            concat!(
                "type Point(x int, y str); type Pair(int, str); type Choice(int | str); ",
                "type Collections([int] | [str]); ",
                "fn main() { ",
                "bad_point := Point(x = 1, x = 2, z = 3); bad_pair := Pair(1); ",
                "bad_union := Choice(true); ambiguous := Collections([]); ",
                "empty_list := []; empty_map := {}; ",
                "bad_error := Error(\"message\"); ",
                "if [1]: println(); print([1]); ",
                "}",
            ),
        );
        let program = parser::parse(&source).unwrap();
        let analysis = analyze(&source, &program);
        let diagnostics = analysis.diagnostics.to_string();

        for expected in [
            "duplicate struct argument 'x'",
            "unknown struct member 'z'",
            "missing struct argument 'y'",
            "tuple constructor expects 2 argument(s), but 1 were provided",
            "union constructor argument does not select exactly one alternative",
            "empty collection does not select exactly one union alternative",
            "empty list requires an expected list type or explicit ascription",
            "empty map requires an expected map type or explicit ascription",
            "Error constructor requires an expected union type",
            "condition must have type bool",
            "print argument must be a primitive or printable tuple",
        ] {
            assert!(diagnostics.contains(expected), "{diagnostics}");
        }
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
        assert_eq!(
            analysis.expression_annotations.last().unwrap().state,
            TypeState::Deferred(deferred)
        );
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

}
