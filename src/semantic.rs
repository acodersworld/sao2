//! Flow-sensitive semantic validation over completed name-and-type analysis.
//!
//! Phase 1 establishes this boundary and gives it ownership of executable entry
//! point validation. Phase 2 adds mutability authorization and explicit
//! obligations for paths that remain flow-dependent until union narrowing.

use crate::analysis::{
    AccessPathStep, Analysis, BindingAccess, BindingId, BuiltinMethod, CallTarget,
    CallableId, CallResolution, DeferredId, FunctionId, FunctionSignature, NameResolution,
    ResolvedType, TypeDefinitionKind, TypeId, TypeState, UnionAlternative,
};
use crate::ast::{
    Argument, ArgumentKind, AssignmentOperator, AssignmentTarget, Block, Expression,
    ExpressionBody, ExpressionBodyKind, ExpressionKind, Member, PrimitiveType, Statement,
    StatementBody, StatementBodyKind, StatementKind,
};
use crate::diagnostic::{Diagnostic, Diagnostics, Warnings};
use crate::source::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EntryPoint {
    function: FunctionId,
}

impl EntryPoint {
    pub(crate) fn function_id(self) -> FunctionId {
        self.function
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MutabilityOperation {
    DeferredPath,
    Rebind,
    ReplaceStructField,
    ReplaceListElement,
    ReplaceMapElement,
    MutateReceiver(BuiltinMethod),
    MutableArgument { function: FunctionId, parameter: BindingId },
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Retained for typed-IR lowering and deferred-obligation resolution.
pub(crate) enum MutabilitySubject<'ast> {
    Assignment(&'ast AssignmentTarget),
    Receiver {
        call: &'ast Expression,
        receiver: &'ast Expression,
    },
    Argument {
        call: &'ast Expression,
        argument: &'ast Argument,
    },
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Retained for typed-IR lowering.
pub(crate) enum PlaceStep<'ast> {
    StructMember {
        node: &'ast Expression,
        declaration: crate::analysis::TypeDeclarationId,
        member: &'ast crate::ast::TypeMember,
        storage: crate::analysis::MemberStorage,
        state: TypeState,
    },
    TupleMember {
        node: &'ast Expression,
        declaration: crate::analysis::TypeDeclarationId,
        member: &'ast crate::ast::TypeMember,
        position: usize,
        state: TypeState,
    },
    ListIndex {
        node: &'ast Expression,
        element: TypeId,
        state: TypeState,
    },
    MapIndex {
        node: &'ast Expression,
        key: TypeId,
        value: TypeId,
        state: TypeState,
    },
}

#[derive(Debug)]
#[allow(dead_code)] // Retained for typed-IR lowering.
pub(crate) enum MutabilityPath<'ast> {
    Assignment(Box<[AccessPathStep<'ast>]>),
    Expression(Box<[PlaceStep<'ast>]>),
}

#[derive(Debug)]
#[allow(dead_code)] // The complete authorization record is consumed by later milestones.
pub(crate) struct MutabilityResolution<'ast> {
    pub(crate) subject: MutabilitySubject<'ast>,
    pub(crate) root: BindingId,
    pub(crate) access: BindingAccess,
    pub(crate) operation: MutabilityOperation,
    pub(crate) path: MutabilityPath<'ast>,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Phase 4 consumes and resolves these obligations.
pub(crate) struct DeferredMutability<'ast> {
    pub(crate) subject: MutabilitySubject<'ast>,
    pub(crate) root: Option<BindingId>,
    pub(crate) access: BindingAccess,
    pub(crate) operation: MutabilityOperation,
    pub(crate) deferred: DeferredId,
}

#[derive(Debug)]
pub(crate) struct SemanticResult<'ast> {
    pub(crate) diagnostics: Diagnostics,
    pub(crate) warnings: Warnings,
    pub(crate) entry_point: Option<EntryPoint>,
    #[allow(dead_code)] // Consumed by typed-IR lowering.
    pub(crate) mutability: Vec<MutabilityResolution<'ast>>,
    #[allow(dead_code)] // Consumed by Phase 4 narrowing.
    pub(crate) deferred_mutability: Vec<DeferredMutability<'ast>>,
}

pub(crate) fn analyze<'ast>(analysis: &mut Analysis<'_, 'ast>) -> SemanticResult<'ast> {
    assert!(
        analysis.diagnostics.is_empty(),
        "semantic analysis requires diagnostic-free name-and-type analysis"
    );

    let mut diagnostics = Diagnostics::new();
    let entry_point = match analysis.function_by_name("main") {
        None => {
            diagnostics.push(Diagnostic::source(
                analysis.source,
                Span::empty(analysis.source.text.len()),
                "executable program requires one 'main' function",
            ));
            None
        }
        Some(function) => {
            let signature = analysis.function_signature(function);
            assert_entry_point_prerequisites(signature);
            if valid_main_signature(analysis, signature) {
                Some(EntryPoint { function })
            } else {
                diagnostics.push(Diagnostic::source(
                    analysis.source,
                    signature.node.name.span,
                    "invalid 'main' signature; expected main(), main() int, main(args [str]), or main(args [str]) int",
                ));
                None
            }
        }
    };

    let (mutability, deferred_mutability) =
        MutabilityValidator::new(analysis, &mut diagnostics).validate();
    debug_assert!(entry_point.is_some() || !diagnostics.is_empty());
    SemanticResult {
        diagnostics,
        warnings: Warnings::new(),
        entry_point,
        mutability,
        deferred_mutability,
    }
}

fn assert_entry_point_prerequisites(signature: &FunctionSignature<'_>) {
    assert!(
        signature
            .parameters
            .iter()
            .all(|parameter| matches!(parameter.ty, TypeState::Resolved(_))),
        "semantic analysis received an unresolved entry-point parameter type"
    );
    assert!(
        matches!(signature.result, TypeState::NoValue | TypeState::Resolved(_)),
        "semantic analysis received an unresolved entry-point result type"
    );
}

fn valid_main_signature(
    analysis: &Analysis<'_, '_>,
    signature: &FunctionSignature<'_>,
) -> bool {
    let parameters_valid = match signature.parameters.as_slice() {
        [] => true,
        [parameter] => {
            !parameter.node.mutable
                && analysis.identifier_text(parameter.node.name.span) == "args"
                && matches!(
                    parameter.ty,
                    TypeState::Resolved(ty)
                        if matches!(
                            analysis.types.get(ty),
                            ResolvedType::List(element)
                                if *element == analysis.types.primitive(PrimitiveType::Str)
                        )
                )
        }
        _ => false,
    };
    let result_valid = signature.result == TypeState::NoValue
        || signature.result
            == TypeState::Resolved(analysis.types.primitive(PrimitiveType::Int));
    parameters_valid && result_valid
}

enum Place<'ast> {
    Rooted {
        root: BindingId,
        steps: Vec<PlaceStep<'ast>>,
    },
    Deferred {
        root: Option<BindingId>,
        deferred: DeferredId,
    },
    Unrooted,
}

struct MutabilityValidator<'analysis, 'diagnostics, 'source, 'ast> {
    analysis: &'analysis Analysis<'source, 'ast>,
    diagnostics: &'diagnostics mut Diagnostics,
    resolutions: Vec<MutabilityResolution<'ast>>,
    deferred: Vec<DeferredMutability<'ast>>,
}

impl<'analysis, 'diagnostics, 'source, 'ast>
    MutabilityValidator<'analysis, 'diagnostics, 'source, 'ast>
{
    fn new(
        analysis: &'analysis Analysis<'source, 'ast>,
        diagnostics: &'diagnostics mut Diagnostics,
    ) -> Self {
        Self {
            analysis,
            diagnostics,
            resolutions: Vec::new(),
            deferred: Vec::new(),
        }
    }

    fn validate(mut self) -> (Vec<MutabilityResolution<'ast>>, Vec<DeferredMutability<'ast>>) {
        for declaration in &self.analysis.program.declarations {
            if let crate::ast::Declaration::Function(function) = declaration {
                self.visit_block(&function.body);
            }
        }
        for call in self.analysis.calls.clone() {
            self.validate_call(call);
        }
        self.resolutions
            .sort_by_key(|resolution| mutability_subject_span(resolution.subject));
        self.deferred
            .sort_by_key(|obligation| mutability_subject_span(obligation.subject));
        (self.resolutions, self.deferred)
    }

    fn visit_block(&mut self, block: &'ast Block) {
        for statement in &block.statements {
            self.visit_statement(statement);
        }
        if let Some(value) = &block.value {
            self.visit_expression(value);
        }
    }

    fn visit_statement(&mut self, statement: &'ast Statement) {
        match &statement.kind {
            StatementKind::Local { initializer, .. } => self.visit_expression(initializer),
            StatementKind::Assignment {
                target,
                operator,
                value,
                ..
            } => {
                self.validate_assignment(target, *operator);
                for suffix in &target.suffixes {
                    if let crate::ast::AssignmentTargetSuffixKind::Index(index) = &suffix.kind {
                        self.visit_expression(index);
                    }
                }
                self.visit_expression(value);
            }
            StatementKind::Expression(expression) => self.visit_expression(expression),
            StatementKind::Return(value) => {
                if let Some(value) = value {
                    self.visit_expression(value);
                }
            }
            StatementKind::Break | StatementKind::Continue => {}
            StatementKind::If {
                branches,
                else_body,
            } => {
                for branch in branches {
                    self.visit_expression(&branch.condition);
                    self.visit_statement_body(&branch.body);
                }
                if let Some(body) = else_body {
                    self.visit_statement_body(body);
                }
            }
            StatementKind::While { condition, body } => {
                self.visit_expression(condition);
                self.visit_statement_body(body);
            }
            StatementKind::For { iterable, body, .. } => {
                self.visit_expression(iterable);
                self.visit_statement_body(body);
            }
            StatementKind::Switch {
                value,
                arms,
                else_body,
            } => {
                self.visit_expression(value);
                for arm in arms {
                    self.visit_statement_body(&arm.body);
                }
                if let Some(body) = else_body {
                    self.visit_statement_body(body);
                }
            }
            StatementKind::Block(block) => self.visit_block(block),
        }
    }

    fn visit_statement_body(&mut self, body: &'ast StatementBody) {
        match &body.kind {
            StatementBodyKind::Block(block) => self.visit_block(block),
            StatementBodyKind::Statement(statement) => self.visit_statement(statement),
        }
    }

    fn visit_expression_body(&mut self, body: &'ast ExpressionBody) {
        match &body.kind {
            ExpressionBodyKind::Block(block) => self.visit_block(block),
            ExpressionBodyKind::Expression(expression) => self.visit_expression(expression),
        }
    }

    fn visit_expression(&mut self, expression: &'ast Expression) {
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
            | ExpressionKind::Try { value: inner, .. } => self.visit_expression(inner),
            ExpressionKind::List(elements) => {
                for element in elements {
                    self.visit_expression(element);
                }
            }
            ExpressionKind::Map(entries) => {
                for entry in entries {
                    self.visit_expression(&entry.key);
                    self.visit_expression(&entry.value);
                }
            }
            ExpressionKind::Block(block) => self.visit_block(block),
            ExpressionKind::If {
                branches,
                else_branch,
            } => {
                for branch in branches {
                    self.visit_expression(&branch.condition);
                    self.visit_expression_body(&branch.body);
                }
                self.visit_expression_body(else_branch);
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.visit_expression(left);
                self.visit_expression(right);
            }
            ExpressionKind::Is { value, .. } => self.visit_expression(value),
            ExpressionKind::Call { callee, arguments } => {
                self.visit_expression(callee);
                for argument in arguments {
                    self.visit_expression(argument_value(argument));
                }
            }
            ExpressionKind::Index { value, index } => {
                self.visit_expression(value);
                self.visit_expression(index);
            }
            ExpressionKind::Member { value, .. } => self.visit_expression(value),
        }
    }

    fn validate_assignment(
        &mut self,
        target: &'ast AssignmentTarget,
        operator: AssignmentOperator,
    ) {
        let annotation = self
            .analysis
            .assignment_target(target)
            .cloned()
            .expect("every assignment target has a name-and-type annotation");
        let root = annotation
            .root
            .expect("diagnostic-free assignment target has a resolved binding root");
        let access = self
            .analysis
            .name_use(&target.root)
            .expect("assignment root has a name-use record")
            .access;
        debug_assert_eq!(
            access,
            match (operator, target.suffixes.is_empty()) {
                (AssignmentOperator::Assign, true) => BindingAccess::Write,
                (AssignmentOperator::Assign, false) => BindingAccess::Mutate,
                (_, true) => BindingAccess::ReadWrite,
                (_, false) => BindingAccess::ReadMutate,
            }
        );
        let operation = if matches!(annotation.state, TypeState::Deferred(_))
            && annotation.steps.len() != target.suffixes.len()
        {
            MutabilityOperation::DeferredPath
        } else if let Some(step) = annotation.steps.last() {
            match step {
                AccessPathStep::StructMember { .. } => MutabilityOperation::ReplaceStructField,
                AccessPathStep::TupleMember { .. } => {
                    let suffix = match step {
                        AccessPathStep::TupleMember { suffix, .. } => *suffix,
                        _ => unreachable!(),
                    };
                    self.error(suffix.span, "tuple members are immutable and cannot be assigned");
                    return;
                }
                AccessPathStep::ListIndex { .. } => MutabilityOperation::ReplaceListElement,
                AccessPathStep::MapIndex { .. } => MutabilityOperation::ReplaceMapElement,
            }
        } else {
            MutabilityOperation::Rebind
        };
        if let TypeState::Deferred(deferred) = annotation.state {
            self.deferred.push(DeferredMutability {
                subject: MutabilitySubject::Assignment(target),
                root: Some(root),
                access,
                operation,
                deferred,
            });
            return;
        }
        assert!(
            matches!(annotation.state, TypeState::Resolved(_)),
            "semantic analysis received an invalid assignment-target state"
        );
        let needs_var = operation != MutabilityOperation::Rebind
            || !self.is_directly_rebindable_object(self.analysis.binding_type(root));
        if needs_var && !self.analysis.binding(root).mutable {
            self.error(target.root.span, "binding must be declared 'var' for this assignment");
            return;
        }
        self.resolutions.push(MutabilityResolution {
            subject: MutabilitySubject::Assignment(target),
            root,
            access,
            operation,
            path: MutabilityPath::Assignment(annotation.steps.clone()),
        });
    }

    fn validate_call(&mut self, resolution: CallResolution<'ast>) {
        let call = resolution.node;
        let callee = resolution.callee;
        let ExpressionKind::Call { arguments, .. } = &call.kind else {
            panic!("call resolution must point to a call expression")
        };
        let target = resolution.target;
        match target {
            CallTarget::Builtin(method) if is_mutating_method(method) => {
                let ExpressionKind::Member { value: receiver, .. } = &callee.kind else {
                    panic!("built-in method target must have a member callee")
                };
                self.authorize_place(
                    MutabilitySubject::Receiver { call, receiver },
                    receiver,
                    BindingAccess::Mutate,
                    MutabilityOperation::MutateReceiver(method),
                );
            }
            CallTarget::Callable(CallableId::Function(function)) => {
                let parameters = self
                    .analysis
                    .function_signature(function)
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.binding, parameter.node.mutable, parameter.ty))
                    .collect::<Vec<_>>();
                for (argument, (parameter, mutable, ty)) in
                    arguments.iter().zip(parameters)
                {
                    if mutable
                        && matches!(ty, TypeState::Resolved(ty) if self.can_reach_mutable_object(ty, &mut Vec::new()))
                    {
                        self.authorize_place(
                            MutabilitySubject::Argument { call, argument },
                            argument_value(argument),
                            BindingAccess::Mutate,
                            MutabilityOperation::MutableArgument {
                                function,
                                parameter,
                            },
                        );
                    }
                }
            }
            CallTarget::Expression => self.defer_unknown_mutating_method(call, callee),
            _ => {}
        }
    }

    fn defer_unknown_mutating_method(&mut self, call: &'ast Expression, callee: &'ast Expression) {
        let ExpressionKind::Member {
            value: receiver,
            member: Member::Named(name),
        } = &callee.kind
        else {
            return;
        };
        let spelling = self.analysis.identifier_text(name.span);
        let method = match spelling {
            "append" => BuiltinMethod::ListAppend,
            "removeIndex" => BuiltinMethod::ListRemoveIndex,
            "removeKey" => BuiltinMethod::MapRemoveKey,
            _ => return,
        };
        let deferred = match self.expression_state(callee).or_else(|| self.expression_state(call)) {
            Some(TypeState::Deferred(deferred)) => deferred,
            _ => return,
        };
        let root = self.place_root(receiver);
        self.deferred.push(DeferredMutability {
            subject: MutabilitySubject::Receiver { call, receiver },
            root,
            access: BindingAccess::Mutate,
            operation: MutabilityOperation::MutateReceiver(method),
            deferred,
        });
    }

    fn authorize_place(
        &mut self,
        subject: MutabilitySubject<'ast>,
        expression: &'ast Expression,
        access: BindingAccess,
        operation: MutabilityOperation,
    ) {
        match self.classify_place(expression) {
            Place::Rooted { root, steps } => {
                if !self.analysis.binding(root).mutable {
                    self.error(self.root_use_span(expression), "binding must be declared 'var' for this mutation");
                } else {
                    self.resolutions.push(MutabilityResolution {
                        subject,
                        root,
                        access,
                        operation,
                        path: MutabilityPath::Expression(steps.into_boxed_slice()),
                    });
                }
            }
            Place::Deferred { root, deferred } => self.deferred.push(DeferredMutability {
                subject,
                root,
                access,
                operation,
                deferred,
            }),
            Place::Unrooted => self.error(expression.span, "mutation requires a binding-rooted place"),
        }
    }

    fn classify_place(&self, expression: &'ast Expression) -> Place<'ast> {
        if let Some(TypeState::Deferred(deferred)) = self.expression_state(expression) {
            return Place::Deferred {
                root: self.place_root(expression),
                deferred,
            };
        }
        match &expression.kind {
            ExpressionKind::Parenthesized(inner) => self.classify_place(inner),
            ExpressionKind::Identifier(identifier) => match self
                .analysis
                .name_use(identifier)
                .map(|name_use| name_use.resolution)
            {
                Some(NameResolution::Binding(root)) => match self.analysis.binding_type(root) {
                    TypeState::Deferred(deferred) => Place::Deferred {
                        root: Some(root),
                        deferred,
                    },
                    TypeState::Resolved(_) => Place::Rooted {
                        root,
                        steps: Vec::new(),
                    },
                    _ => panic!("semantic analysis received an invalid place-root type"),
                },
                _ => Place::Unrooted,
            },
            ExpressionKind::Member { value, .. } => {
                self.extend_place(value, self.member_step(expression, value))
            }
            ExpressionKind::Index { value, .. } => {
                self.extend_place(value, self.index_step(expression, value))
            }
            _ => Place::Unrooted,
        }
    }

    fn extend_place(
        &self,
        receiver: &'ast Expression,
        step: Option<PlaceStep<'ast>>,
    ) -> Place<'ast> {
        let Some(step) = step else {
            if let Some(TypeState::Deferred(deferred)) = self.expression_state(receiver) {
                return Place::Deferred {
                    root: self.place_root(receiver),
                    deferred,
                };
            }
            panic!("resolved place access lacks a resolved typed path step");
        };
        match self.classify_place(receiver) {
            Place::Rooted { root, mut steps } => {
                steps.push(step);
                Place::Rooted { root, steps }
            }
            other => other,
        }
    }

    fn member_step(
        &self,
        node: &'ast Expression,
        receiver: &'ast Expression,
    ) -> Option<PlaceStep<'ast>> {
        let TypeState::Resolved(receiver) = self.expression_state(receiver)? else {
            return None;
        };
        let ExpressionKind::Member { member, .. } = &node.kind else {
            return None;
        };
        let ResolvedType::Nominal(declaration) = self.analysis.types.get(receiver) else {
            return None;
        };
        let state = self.expression_state(node)?;
        match (&self.analysis.type_definition(*declaration).kind, member) {
            (TypeDefinitionKind::Struct(members), Member::Named(name)) => {
                let spelling = self.analysis.identifier_text(name.span);
                let member = members
                    .iter()
                    .find(|member| self.analysis.identifier_text(member.name.span) == spelling)?;
                Some(PlaceStep::StructMember {
                    node,
                    declaration: *declaration,
                    member: member.node,
                    storage: member.storage,
                    state,
                })
            }
            (TypeDefinitionKind::Tuple(members), Member::TupleIndex(span)) => {
                let position = self
                    .analysis
                    .identifier_text(*span)
                    .replace('_', "")
                    .parse::<usize>()
                    .ok()?;
                let member = members.get(position)?;
                Some(PlaceStep::TupleMember {
                    node,
                    declaration: *declaration,
                    member: member.node,
                    position,
                    state,
                })
            }
            _ => None,
        }
    }

    fn index_step(
        &self,
        node: &'ast Expression,
        receiver: &'ast Expression,
    ) -> Option<PlaceStep<'ast>> {
        let TypeState::Resolved(receiver) = self.expression_state(receiver)? else {
            return None;
        };
        let state = self.expression_state(node)?;
        match self.analysis.types.get(receiver) {
            ResolvedType::List(element) => Some(PlaceStep::ListIndex {
                node,
                element: *element,
                state,
            }),
            ResolvedType::Map { key, value } => Some(PlaceStep::MapIndex {
                node,
                key: *key,
                value: *value,
                state,
            }),
            _ => None,
        }
    }

    fn expression_state(&self, expression: &'ast Expression) -> Option<TypeState> {
        self.analysis.expression_annotation(expression).map(|annotation| annotation.state)
    }

    fn place_root(&self, expression: &'ast Expression) -> Option<BindingId> {
        match &expression.kind {
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Member { value: inner, .. }
            | ExpressionKind::Index { value: inner, .. } => self.place_root(inner),
            ExpressionKind::Identifier(identifier) => self.analysis.name_use(identifier).and_then(
                |name_use| match name_use.resolution {
                    NameResolution::Binding(binding) => Some(binding),
                    _ => None,
                },
            ),
            _ => None,
        }
    }

    fn root_use_span(&self, expression: &'ast Expression) -> Span {
        match &expression.kind {
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Member { value: inner, .. }
            | ExpressionKind::Index { value: inner, .. } => self.root_use_span(inner),
            ExpressionKind::Identifier(identifier) => identifier.span,
            _ => expression.span,
        }
    }

    fn is_directly_rebindable_object(&self, state: TypeState) -> bool {
        let TypeState::Resolved(ty) = state else {
            return false;
        };
        match self.analysis.types.get(ty) {
            ResolvedType::List(_) | ResolvedType::Map { .. } => true,
            ResolvedType::Nominal(declaration) => {
                matches!(&self.analysis.type_definition(*declaration).kind, TypeDefinitionKind::Struct(_))
            }
            _ => false,
        }
    }

    fn can_reach_mutable_object(&self, ty: TypeId, visiting: &mut Vec<TypeId>) -> bool {
        if visiting.contains(&ty) {
            return false;
        }
        match self.analysis.types.get(ty) {
            ResolvedType::List(_) | ResolvedType::Map { .. } => true,
            ResolvedType::Primitive(_) => false,
            ResolvedType::Union(alternatives) => {
                visiting.push(ty);
                let result = alternatives.iter().any(|alternative| {
                    self.can_reach_mutable_object(alternative_payload(alternative), visiting)
                });
                visiting.pop();
                result
            }
            ResolvedType::Nominal(declaration) => match &self.analysis.type_definition(*declaration).kind {
                TypeDefinitionKind::Struct(_) => true,
                TypeDefinitionKind::Tuple(members) => {
                    visiting.push(ty);
                    let result = members.iter().any(|member| {
                        matches!(member.ty, TypeState::Resolved(member_ty) if self.can_reach_mutable_object(member_ty, visiting))
                    });
                    visiting.pop();
                    result
                }
                TypeDefinitionKind::Union { alternatives, .. } => {
                    visiting.push(ty);
                    let result = alternatives.iter().any(|alternative| {
                        self.can_reach_mutable_object(alternative_payload(alternative), visiting)
                    });
                    visiting.pop();
                    result
                }
                TypeDefinitionKind::Invalid => panic!("semantic analysis received an invalid type definition"),
            },
        }
    }

    fn error(&mut self, span: Span, message: &'static str) {
        self.diagnostics
            .push(Diagnostic::source(self.analysis.source, span, message));
    }
}

fn argument_value(argument: &Argument) -> &Expression {
    match &argument.kind {
        ArgumentKind::Positional(value) | ArgumentKind::Named { value, .. } => value,
    }
}

fn is_mutating_method(method: BuiltinMethod) -> bool {
    matches!(
        method,
        BuiltinMethod::ListAppend | BuiltinMethod::ListRemoveIndex | BuiltinMethod::MapRemoveKey
    )
}

fn alternative_payload(alternative: &UnionAlternative) -> TypeId {
    match alternative {
        UnionAlternative::Untagged(payload)
        | UnionAlternative::Tagged { payload, .. }
        | UnionAlternative::Error(payload) => *payload,
    }
}

fn mutability_subject_span(subject: MutabilitySubject<'_>) -> Span {
    match subject {
        MutabilitySubject::Assignment(target) => target.span,
        MutabilitySubject::Receiver { receiver, .. } => receiver.span,
        MutabilitySubject::Argument { argument, .. } => argument.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{analysis, parser};
    use crate::source::SourceFile;
    use std::path::PathBuf;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }

    fn with_semantic_result(text: &str, inspect: impl FnOnce(&SemanticResult<'_>)) {
        let source = source(text);
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let result = analyze(&mut analysis);
        inspect(&result);
    }

    #[test]
    fn accepts_all_four_entry_point_signatures() {
        for text in [
            "fn main() {}",
            "fn main() int {}",
            "fn main(args [str]) {}",
            "fn main(args [str]) int {}",
        ] {
            with_semantic_result(text, |result| {
                assert!(
                    result.diagnostics.is_empty(),
                    "{text}: {}",
                    result.diagnostics
                );
                assert!(result.entry_point.is_some(), "{text}");
            });
        }
    }

    #[test]
    fn rejects_missing_or_invalid_entry_points() {
        for (text, expected) in [
            ("fn helper() {}", "requires one 'main'"),
            ("fn main(value str) {}", "invalid 'main' signature"),
            ("fn main(var args [str]) {}", "invalid 'main' signature"),
            ("fn main(args [int]) {}", "invalid 'main' signature"),
            ("fn main(first str, second str) {}", "invalid 'main' signature"),
            ("fn main() str {}", "invalid 'main' signature"),
        ] {
            with_semantic_result(text, |result| {
                assert!(result.entry_point.is_none(), "{text}");
                assert!(result.diagnostics.to_string().contains(expected), "{text}");
            });
        }
    }

    #[test]
    fn validates_direct_and_indirect_assignment_permissions() {
        for (text, expected) in [
            ("fn main() { value := 1; value = 2; }", Some("must be declared 'var'")),
            ("fn main() { var value := 1; value = 2; }", None),
            ("fn set(value int) { value = 2; } fn main() {}", Some("must be declared 'var'")),
            ("fn set(var value int) { value = 2; } fn main() {}", None),
            ("fn main() { text := \"a\"; text = \"b\"; }", Some("must be declared 'var'")),
            ("type Choice(int | str); fn main() { value := Choice(1); value = Choice(\"x\"); }", Some("must be declared 'var'")),
            ("type Choice(int | str); fn main() { var value := Choice(1); value = Choice(\"x\"); }", None),
            ("type Box(value int); fn main() { box := Box(value = 1); box = Box(value = 2); }", None),
            ("fn main() { values := [1]; values = [2]; }", None),
            ("fn main() { values := [1]; values[0] = 2; }", Some("must be declared 'var'")),
            ("fn main() { var values := [1]; values[0] = 2; }", None),
            ("fn main() { var table := {1: 2}; table[1] = 3; }", None),
            ("type Box(value int); fn main() { box := Box(value = 1); box.value = 2; }", Some("must be declared 'var'")),
            ("type Box(value int); fn main() { var box := Box(value = 1); box.value = 2; }", None),
            ("type Box(value int); type Outer(inner &Box); fn main() { var outer := Outer(inner = Box(value = 1)); outer.inner.value = 2; }", None),
            ("type Pair(int, int); fn main() { var pair := Pair(1, 2); pair.0 = 3; }", Some("tuple members are immutable")),
            ("type Box(value int); type Holder(Box); fn main() { var holder := Holder(Box(value = 1)); holder.0.value = 2; }", None),
            ("type Box(value int); type Holder(Box); fn main() { holder := Holder(Box(value = 1)); holder.0.value = 2; }", Some("must be declared 'var'")),
        ] {
            with_semantic_result(text, |result| match expected {
                Some(expected) => assert!(result.diagnostics.to_string().contains(expected), "{text}: {}", result.diagnostics),
                None => assert!(result.diagnostics.is_empty(), "{text}: {}", result.diagnostics),
            });
        }
    }

    #[test]
    fn validates_mutating_receivers_and_leaves_len_read_only() {
        for (text, expected) in [
            ("fn main() { values := [1]; values.append(2); }", Some("must be declared 'var'")),
            ("fn main() { var values := [1]; (values).removeIndex(0); }", None),
            ("type Bag(items [int]); type Holder(Bag); fn main() { var holder := Holder(Bag(items = [1])); (holder.0.items).append(2); }", None),
            ("fn main() { var table := {1: [2]}; table[1].append(3); }", None),
            ("fn main() { [1].append(2); }", Some("binding-rooted place")),
            ("fn main() { values := [1]; value := values.len(); }", None),
            ("fn main() { table := {1: 2}; table.removeKey(1); }", Some("must be declared 'var'")),
        ] {
            with_semantic_result(text, |result| match expected {
                Some(expected) => assert!(result.diagnostics.to_string().contains(expected), "{text}: {}", result.diagnostics),
                None => assert!(result.diagnostics.is_empty(), "{text}: {}", result.diagnostics),
            });
        }
    }

    #[test]
    fn var_arguments_require_places_only_when_objects_are_reachable() {
        for (text, expected) in [
            ("fn take(var value int) {} fn main() { take(1); }", None),
            ("type Pair(int, str); fn take(var value Pair) {} fn main() { take(Pair(1, \"x\")); }", None),
            ("type Choice(int | str); fn take(var value Choice) {} fn main() { take(Choice(1)); }", None),
            ("type Box(value int); fn touch(var value Box) {} fn main() { value := Box(value = 1); touch(value); }", Some("must be declared 'var'")),
            ("type Box(value int); fn touch(var value Box) {} fn main() { var value := Box(value = 1); touch(value); }", None),
            ("type Box(value int); fn touch(var value Box) {} fn main() { touch(Box(value = 1)); }", Some("binding-rooted place")),
            ("type Box(value int); type Choice(int | Box); fn touch(var value Choice) {} fn main() { value := Choice(1); touch(value); }", Some("must be declared 'var'")),
        ] {
            with_semantic_result(text, |result| match expected {
                Some(expected) => assert!(result.diagnostics.to_string().contains(expected), "{text}: {}", result.diagnostics),
                None => assert!(result.diagnostics.is_empty(), "{text}: {}", result.diagnostics),
            });
        }
    }

    #[test]
    fn retains_successes_and_flow_dependent_obligations() {
        with_semantic_result(
            "type Box(value int); type U(Box | int); fn main() { var value := U(Box(value = 1)); if value is Box: value.value = 2; }",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.deferred_mutability.len(), 1);
                assert!(matches!(result.deferred_mutability[0].operation, MutabilityOperation::DeferredPath));
            },
        );
        with_semantic_result(
            "type Choice([int] | int); fn main() { var value := Choice([1]); if value is [int]: value.append(2); }",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.deferred_mutability.len(), 1);
                assert!(matches!(
                    result.deferred_mutability[0].operation,
                    MutabilityOperation::MutateReceiver(BuiltinMethod::ListAppend)
                ));
            },
        );
        with_semantic_result("fn main() { var values := [1]; values.append(2); }", |result| {
            assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
            assert_eq!(result.mutability.len(), 1);
            assert!(matches!(result.mutability[0].operation, MutabilityOperation::MutateReceiver(BuiltinMethod::ListAppend)));
        });
    }

    #[test]
    fn mutability_diagnostics_use_focused_source_spans_in_source_order() {
        let text = concat!(
            "type Pair(int, int); fn main() { first := 1; values := [1]; ",
            "values.append(2); first = 2; var pair := Pair(1, 2); pair.0 = 3; }"
        );
        let source = source(text);
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let result = analyze(&mut analysis);
        let spans = result
            .diagnostics
            .into_sorted()
            .into_iter()
            .map(|diagnostic| diagnostic.primary_span().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].start, text.find("values.append").unwrap());
        assert_eq!(spans[1].start, text.find("first = 2").unwrap());
        assert_eq!(spans[2].start, text.find(".0 = 3").unwrap());
    }

    #[test]
    fn duplicate_main_remains_a_name_analysis_error() {
        let source = source("fn main() {} fn main() {}");
        let program = parser::parse(&source).unwrap();
        let analysis = analysis::analyze(&source, &program);
        assert!(
            analysis
                .diagnostics
                .to_string()
                .contains("duplicate function declaration 'main'")
        );
    }

    #[test]
    #[should_panic(expected = "unresolved entry-point result type")]
    fn missing_prerequisite_annotation_is_a_compiler_invariant() {
        let source = source("fn main() {}");
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty());
        analysis.function_signatures[0].result = TypeState::Error;
        let _ = analyze(&mut analysis);
    }
}
