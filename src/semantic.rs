//! Flow-sensitive semantic validation over completed name-and-type analysis.
//!
//! The completed pass owns entry-point validation, mutation authorization,
//! structural control flow, returns and loop targets, unreachable warnings,
//! contextual union operations and narrowing, switch coverage, and postfix
//! error propagation. `validate_handoff` is the mandatory checked boundary for
//! downstream lowering.

use crate::analysis::{
    AccessPathStep, Analysis, BindingAccess, BindingId, BuiltinMethod, CallTarget,
    AstNode, BindingNode, CallableId, CallResolution, ConstructorKind, DeclarationId,
    DeclarationNode, DeferredId, FunctionId, FunctionSignature, NameResolution, ResolvedType,
    ProjectionKind, ProjectionResolution, TypeDefinitionKind, TypeId, TypeState,
    UnionAlternative, UnionStyle,
};
use crate::ast::{
    Argument, ArgumentKind, AssignmentOperator, AssignmentTarget, BinaryOperator, Block,
    Declaration, Expression, ExpressionBody, ExpressionBodyKind, ExpressionKind,
    FunctionDeclaration, Member, PrimitiveType, Program, Statement, StatementBody,
    StatementBodyKind, StatementKind, SwitchArm, Type, TypeKind, TypeMemberKind,
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
#[allow(dead_code)] // Retained until the handoff gate proves none remain.
pub(crate) struct DeferredMutability<'ast> {
    pub(crate) subject: MutabilitySubject<'ast>,
    pub(crate) root: Option<BindingId>,
    pub(crate) access: BindingAccess,
    pub(crate) operation: MutabilityOperation,
    pub(crate) deferred: DeferredId,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Consumed by typed-IR lowering.
pub(crate) struct UnionTestResolution<'ast> {
    pub(crate) expression: &'ast Expression,
    pub(crate) operand_union: TypeId,
    pub(crate) alternative: UnionAlternative,
    pub(crate) payload: TypeId,
    pub(crate) binding: Option<BindingId>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Consumed by typed-IR lowering.
pub(crate) struct SwitchArmResolution<'ast> {
    pub(crate) arm: &'ast SwitchArm,
    pub(crate) alternative: UnionAlternative,
    pub(crate) payload: TypeId,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Consumed by typed-IR lowering.
pub(crate) struct SwitchResolution<'ast> {
    pub(crate) statement: &'ast Statement,
    pub(crate) operand_union: TypeId,
    pub(crate) binding: Option<BindingId>,
    pub(crate) arms: Vec<SwitchArmResolution<'ast>>,
    pub(crate) covered: Box<[UnionAlternative]>,
    pub(crate) else_body: Option<&'ast StatementBody>,
    pub(crate) exhaustive: bool,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Consumed by typed-IR lowering.
pub(crate) enum TryAction {
    Propagate {
        function: FunctionId,
        destination_union: TypeId,
        destination_error: UnionAlternative,
    },
    Panic { function: FunctionId },
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Consumed by typed-IR lowering.
pub(crate) struct TryResolution<'ast> {
    pub(crate) expression: &'ast Expression,
    pub(crate) operator_span: Span,
    pub(crate) operand_union: TypeId,
    pub(crate) source_error: UnionAlternative,
    pub(crate) success: TypeId,
    pub(crate) action: TryAction,
}

#[derive(Debug)]
pub(crate) struct SemanticResult<'ast> {
    pub(crate) diagnostics: Diagnostics,
    pub(crate) warnings: Warnings,
    pub(crate) entry_point: Option<EntryPoint>,
    #[allow(dead_code)] // Consumed by typed-IR lowering.
    pub(crate) mutability: Vec<MutabilityResolution<'ast>>,
    /// Subjects classified as requiring authorization, retained so handoff
    /// coverage does not repeat the mutability decision.
    pub(crate) mutation_subjects: Vec<MutabilitySubject<'ast>>,
    #[allow(dead_code)] // Retained only for obligations whose input is postfix `?`.
    pub(crate) deferred_mutability: Vec<DeferredMutability<'ast>>,
    #[allow(dead_code)] // Consumed by typed-IR lowering.
    pub(crate) union_tests: Vec<UnionTestResolution<'ast>>,
    #[allow(dead_code)] // Consumed by typed-IR lowering.
    pub(crate) switches: Vec<SwitchResolution<'ast>>,
    #[allow(dead_code)] // Consumed by typed-IR lowering.
    pub(crate) tries: Vec<TryResolution<'ast>>,
    #[allow(dead_code)] // Consumed by typed-IR lowering.
    pub(crate) flow: FlowFacts<'ast>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FlowFlags(u8);

impl FlowFlags {
    pub(crate) const FALLTHROUGH: Self = Self(1 << 0);
    pub(crate) const RETURN: Self = Self(1 << 1);
    pub(crate) const BREAK: Self = Self(1 << 2);
    pub(crate) const CONTINUE: Self = Self(1 << 3);
    pub(crate) const DIVERGE: Self = Self(1 << 4);

    pub(crate) fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 != 0
    }

    fn insert(&mut self, flags: Self) {
        self.0 |= flags.0;
    }

    fn remove(&mut self, flags: Self) {
        self.0 &= !flags.0;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FallthroughKind {
    None,
    Definite,
    SwitchDependent,
}

#[derive(Clone, Debug)]
pub(crate) struct FlowSummary<'ast> {
    pub(crate) flags: FlowFlags,
    pub(crate) fallthrough: FallthroughKind,
    /// Non-`else` switches that may govern the remaining fallthrough paths.
    pub(crate) switch_dependencies: Box<[&'ast Statement]>,
}

impl<'ast> FlowSummary<'ast> {
    fn terminal(flags: FlowFlags) -> Self {
        Self {
            flags,
            fallthrough: FallthroughKind::None,
            switch_dependencies: Box::new([]),
        }
    }

    fn fallthrough() -> Self {
        Self {
            flags: FlowFlags::FALLTHROUGH,
            fallthrough: FallthroughKind::Definite,
            switch_dependencies: Box::new([]),
        }
    }

    fn dependent(switches: Vec<&'ast Statement>) -> Self {
        Self {
            flags: FlowFlags::FALLTHROUGH,
            fallthrough: FallthroughKind::SwitchDependent,
            switch_dependencies: switches.into_boxed_slice(),
        }
    }

    fn can_fallthrough(&self) -> bool {
        self.fallthrough != FallthroughKind::None
    }

    fn then(mut self, next: Self) -> Self {
        if !self.can_fallthrough() {
            return self;
        }
        let entry_kind = self.fallthrough;
        let mut dependencies = self.switch_dependencies.into_vec();
        self.flags.remove(FlowFlags::FALLTHROUGH);
        self.flags.insert(FlowFlags(next.flags.0 & !FlowFlags::FALLTHROUGH.0));
        self.fallthrough = match (entry_kind, next.fallthrough) {
            (_, FallthroughKind::None) => FallthroughKind::None,
            (FallthroughKind::Definite, kind) => kind,
            (FallthroughKind::SwitchDependent, _) => FallthroughKind::SwitchDependent,
            (FallthroughKind::None, _) => unreachable!(),
        };
        if self.fallthrough != FallthroughKind::None {
            if entry_kind == FallthroughKind::Definite {
                dependencies.clear();
            }
            dependencies.extend(next.switch_dependencies);
            deduplicate_switches(&mut dependencies);
            self.flags.insert(FlowFlags::FALLTHROUGH);
        } else {
            dependencies.clear();
        }
        self.switch_dependencies = dependencies.into_boxed_slice();
        self
    }

    fn alternatives(flows: impl IntoIterator<Item = Self>) -> Self {
        let mut flags = FlowFlags::default();
        let mut kind = FallthroughKind::None;
        let mut dependencies = Vec::new();
        for flow in flows {
            flags.insert(flow.flags);
            match flow.fallthrough {
                FallthroughKind::Definite => {
                    kind = FallthroughKind::Definite;
                    dependencies.clear();
                }
                FallthroughKind::SwitchDependent if kind != FallthroughKind::Definite => {
                    kind = FallthroughKind::SwitchDependent;
                    dependencies.extend(flow.switch_dependencies);
                }
                FallthroughKind::None | FallthroughKind::SwitchDependent => {}
            }
        }
        if kind == FallthroughKind::None {
            flags.remove(FlowFlags::FALLTHROUGH);
        } else {
            flags.insert(FlowFlags::FALLTHROUGH);
        }
        deduplicate_switches(&mut dependencies);
        Self {
            flags,
            fallthrough: kind,
            switch_dependencies: dependencies.into_boxed_slice(),
        }
    }
}

fn deduplicate_switches(switches: &mut Vec<&Statement>) {
    let mut index = 0;
    while index < switches.len() {
        if switches[..index]
            .iter()
            .any(|prior| std::ptr::eq(*prior, switches[index]))
        {
            switches.remove(index);
        } else {
            index += 1;
        }
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Retained for typed-IR lowering.
pub(crate) struct ExplicitReturn<'ast> {
    pub(crate) statement: &'ast Statement,
    pub(crate) function: FunctionId,
    pub(crate) value: Option<&'ast Expression>,
    pub(crate) value_state: TypeState,
    pub(crate) unit_injection: Option<UnionAlternative>,
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Consumed by typed-IR lowering.
pub(crate) struct FunctionCompletion<'ast> {
    pub(crate) function: FunctionId,
    pub(crate) body: &'ast Block,
    pub(crate) unit_injection: Option<UnionAlternative>,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Retained for typed-IR lowering.
pub(crate) struct LoopControl<'ast> {
    pub(crate) statement: &'ast Statement,
    pub(crate) target: &'ast Statement,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Retained to diagnose any incomplete flow recomposition.
pub(crate) enum DeferredFlowKind<'ast> {
    FunctionReturn {
        function: FunctionId,
        body: &'ast Block,
    },
    FunctionFinalValue {
        function: FunctionId,
        body: &'ast Block,
        value: &'ast Expression,
        declared_result: TypeState,
    },
    Reachability {
        construct: FlowSubject<'ast>,
    },
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // Retained to diagnose any incomplete flow recomposition.
pub(crate) struct DeferredFlow<'ast> {
    pub(crate) kind: DeferredFlowKind<'ast>,
    pub(crate) switches: Box<[&'ast Statement]>,
}

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)] // Variants are retained for completed flow facts and typed-IR lowering.
pub(crate) enum FlowSubject<'ast> {
    Block(&'ast Block),
    Statement(&'ast Statement),
    StatementBody(&'ast StatementBody),
    Expression(&'ast Expression),
    ExpressionBody(&'ast ExpressionBody),
    SwitchArm(&'ast SwitchArm),
}

#[derive(Clone, Debug)]
#[allow(dead_code)] // The complete side-table record is consumed by later milestones.
pub(crate) struct FlowRecord<'ast> {
    pub(crate) subject: FlowSubject<'ast>,
    pub(crate) summary: FlowSummary<'ast>,
}

#[derive(Debug, Default)]
pub(crate) struct FlowFacts<'ast> {
    pub(crate) summaries: Vec<FlowRecord<'ast>>,
    pub(crate) returns: Vec<ExplicitReturn<'ast>>,
    pub(crate) completions: Vec<FunctionCompletion<'ast>>,
    pub(crate) loop_controls: Vec<LoopControl<'ast>>,
    pub(crate) deferred: Vec<DeferredFlow<'ast>>,
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

    let mut warnings = Warnings::new();
    let (union_tests, switches, tries) =
        UnionValidator::new(analysis, &mut diagnostics, &mut warnings).validate();
    let (mutability, deferred_mutability, mutation_subjects) =
        MutabilityValidator::new(analysis, &mut diagnostics).validate();
    let mut flow = FlowValidator::new(analysis, &mut diagnostics, &mut warnings, &switches, &tries).validate();
    flow.deferred.retain(|obligation| {
        obligation.switches.is_empty() || obligation.switches.iter().any(|statement| {
            analysis.deferred.iter().any(|deferred| {
                !deferred.resolved
                    && matches!(deferred.node, crate::analysis::AstNode::Statement(node) if std::ptr::eq(node, *statement))
            })
        })
    });
    debug_assert!(entry_point.is_some() || !diagnostics.is_empty());
    SemanticResult {
        diagnostics,
        warnings,
        entry_point,
        mutability,
        mutation_subjects,
        deferred_mutability,
        union_tests,
        switches,
        tries,
        flow,
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
        matches!(signature.result, TypeState::Resolved(_)),
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
    let result_valid = signature.result == TypeState::Resolved(analysis.types.unit())
        || signature.result
            == TypeState::Resolved(analysis.types.primitive(PrimitiveType::Int));
    parameters_valid && result_valid
}

/// Checks the complete, diagnostic-free frontend result before any lowering
/// boundary is allowed to consume it.  This is deliberately recoverable: a
/// broken compiler invariant must not be reported as a source-language error
/// or reach backend capability validation.
pub(crate) fn validate_handoff<'ast>(
    analysis: &Analysis<'_, 'ast>,
    semantic: &SemanticResult<'ast>,
) -> Result<(), Diagnostic> {
    HandoffValidator::new(analysis, semantic).validate()
}

fn handoff_error(message: impl Into<String>) -> Diagnostic {
    Diagnostic::compiler(format!(
        "semantic handoff invariant violated: {}",
        message.into()
    ))
}

#[derive(Default)]
struct HandoffSubjects<'ast> {
    declarations: Vec<&'ast Declaration>,
    types: Vec<&'ast Type>,
    expressions: Vec<&'ast Expression>,
    assignments: Vec<&'ast AssignmentTarget>,
    calls: Vec<&'ast Expression>,
    call_callees: Vec<&'ast Expression>,
    name_identifiers: Vec<&'ast crate::ast::Identifier>,
    literals: Vec<&'ast Expression>,
    union_tests: Vec<&'ast Expression>,
    switches: Vec<&'ast Statement>,
    tries: Vec<&'ast Expression>,
    returns: Vec<&'ast Statement>,
    loop_controls: Vec<&'ast Statement>,
    loop_targets: Vec<(&'ast Statement, &'ast Statement)>,
    loops: Vec<&'ast Statement>,
    blocks: Vec<&'ast Block>,
    statements: Vec<&'ast Statement>,
    statement_bodies: Vec<&'ast StatementBody>,
    expression_bodies: Vec<&'ast ExpressionBody>,
    switch_arms: Vec<&'ast SwitchArm>,
    parameters: Vec<&'ast crate::ast::Parameter>,
    locals: Vec<&'ast Statement>,
    loop_bindings: Vec<&'ast Statement>,
    expression_functions: Vec<(&'ast Expression, usize)>,
    statement_functions: Vec<(&'ast Statement, usize)>,
    active_loops: Vec<&'ast Statement>,
    current_function: Option<usize>,
}

impl<'ast> HandoffSubjects<'ast> {
    fn collect(program: &'ast Program) -> Self {
        let mut subjects = Self::default();
        let mut function_index = 0;
        for declaration in &program.declarations {
            subjects.declarations.push(declaration);
            match declaration {
                Declaration::Type(declaration) => {
                    for member in &declaration.members {
                        match &member.kind {
                            TypeMemberKind::Named { ty, .. } | TypeMemberKind::Unnamed(ty) => {
                                subjects.ty(ty);
                            }
                        }
                    }
                }
                Declaration::Function(function) => {
                    subjects.current_function = Some(function_index);
                    function_index += 1;
                    for parameter in &function.parameters {
                        subjects.parameters.push(parameter);
                        subjects.ty(&parameter.ty);
                    }
                    if let Some(ty) = &function.return_type {
                        subjects.ty(ty);
                    }
                    subjects.block(&function.body);
                    subjects.current_function = None;
                }
            }
        }
        subjects
    }

    fn ty(&mut self, ty: &'ast Type) {
        self.types.push(ty);
        match &ty.kind {
            TypeKind::List(element) | TypeKind::Parenthesized(element) => self.ty(element),
            TypeKind::Map { key, value } => {
                self.ty(key);
                self.ty(value);
            }
            TypeKind::Union(alternatives) => {
                for alternative in alternatives.iter() {
                    self.ty(alternative);
                }
            }
            TypeKind::Tagged { payload, .. } => self.ty(payload),
            TypeKind::Unit | TypeKind::Primitive(_) | TypeKind::Named(_) => {}
        }
    }

    fn block(&mut self, block: &'ast Block) {
        self.blocks.push(block);
        for statement in &block.statements {
            self.statement(statement);
        }
        if let Some(value) = block.value.as_deref() {
            self.expression(value);
        }
    }

    fn statement_body(&mut self, body: &'ast StatementBody) {
        self.statement_bodies.push(body);
        match &body.kind {
            StatementBodyKind::Block(block) => self.block(block),
            StatementBodyKind::Statement(statement) => self.statement(statement),
        }
    }

    fn expression_body(&mut self, body: &'ast ExpressionBody) {
        self.expression_bodies.push(body);
        match &body.kind {
            ExpressionBodyKind::Block(block) => self.block(block),
            ExpressionBodyKind::Expression(expression) => self.expression(expression),
        }
    }

    fn statement(&mut self, statement: &'ast Statement) {
        self.statements.push(statement);
        if let Some(function) = self.current_function {
            self.statement_functions.push((statement, function));
        }
        match &statement.kind {
            StatementKind::Local { initializer, .. } => {
                self.locals.push(statement);
                self.expression(initializer);
            }
            StatementKind::Assignment { target, value, .. } => {
                self.assignments.push(target);
                self.name_identifiers.push(&target.root);
                for suffix in &target.suffixes {
                    if let crate::ast::AssignmentTargetSuffixKind::Index(index) = &suffix.kind {
                        self.expression(index);
                    }
                }
                self.expression(value);
            }
            StatementKind::Expression(expression) => self.expression(expression),
            StatementKind::Return(value) => {
                self.returns.push(statement);
                if let Some(value) = value {
                    self.expression(value);
                }
            }
            StatementKind::Break | StatementKind::Continue => {
                self.loop_controls.push(statement);
                if let Some(target) = self.active_loops.last().copied() {
                    self.loop_targets.push((statement, target));
                }
            }
            StatementKind::If { branches, else_body } => {
                for branch in branches {
                    self.expression(&branch.condition);
                    self.statement_body(&branch.body);
                }
                if let Some(body) = else_body {
                    self.statement_body(body);
                }
            }
            StatementKind::While { condition, body } => {
                self.loops.push(statement);
                self.expression(condition);
                self.active_loops.push(statement);
                self.statement_body(body);
                self.active_loops.pop();
            }
            StatementKind::For { iterable, body, .. } => {
                self.loops.push(statement);
                self.loop_bindings.push(statement);
                self.expression(iterable);
                self.active_loops.push(statement);
                self.statement_body(body);
                self.active_loops.pop();
            }
            StatementKind::Switch { value, arms, else_body } => {
                self.switches.push(statement);
                self.expression(value);
                for arm in arms {
                    self.switch_arms.push(arm);
                    self.ty(&arm.label);
                    self.statement_body(&arm.body);
                }
                if let Some(body) = else_body {
                    self.statement_body(body);
                }
            }
            StatementKind::Block(block) => self.block(block),
        }
    }

    fn expression(&mut self, expression: &'ast Expression) {
        self.expressions.push(expression);
        if let Some(function) = self.current_function {
            self.expression_functions.push((expression, function));
        }
        match &expression.kind {
            ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_) => self.literals.push(expression),
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Conversion { operand: inner, .. }
            | ExpressionKind::Unary { operand: inner, .. }
            | ExpressionKind::Member { value: inner, .. } => self.expression(inner),
            ExpressionKind::List(elements) => {
                for element in elements {
                    self.expression(element);
                }
            }
            ExpressionKind::Map(entries) => {
                for entry in entries {
                    self.expression(&entry.key);
                    self.expression(&entry.value);
                }
            }
            ExpressionKind::TypedEmptyList(ty) | ExpressionKind::TypedEmptyMap(ty) => self.ty(ty),
            ExpressionKind::Block(block) => self.block(block),
            ExpressionKind::If { branches, else_branch } => {
                for branch in branches {
                    self.expression(&branch.condition);
                    self.expression_body(&branch.body);
                }
                self.expression_body(else_branch);
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left);
                self.expression(right);
            }
            ExpressionKind::Is { value, ty, .. } => {
                self.union_tests.push(expression);
                self.expression(value);
                self.ty(ty);
            }
            ExpressionKind::Call { callee, arguments } => {
                self.calls.push(expression);
                self.call_callees.push(callee);
                self.expression(callee);
                for argument in arguments {
                    self.expression(argument_value(argument));
                }
            }
            ExpressionKind::Index { value, index } => {
                self.expression(value);
                self.expression(index);
            }
            ExpressionKind::Try { value, .. } => {
                self.tries.push(expression);
                self.expression(value);
            }
            ExpressionKind::Identifier(identifier) => self.name_identifiers.push(identifier),
            ExpressionKind::Unit => {}
        }
    }
}

struct HandoffValidator<'a, 'source, 'ast> {
    analysis: &'a Analysis<'source, 'ast>,
    semantic: &'a SemanticResult<'ast>,
    subjects: HandoffSubjects<'ast>,
}

impl<'a, 'source, 'ast> HandoffValidator<'a, 'source, 'ast> {
    fn new(
        analysis: &'a Analysis<'source, 'ast>,
        semantic: &'a SemanticResult<'ast>,
    ) -> Self {
        Self {
            analysis,
            semantic,
            subjects: HandoffSubjects::collect(analysis.program),
        }
    }

    fn validate(&self) -> Result<(), Diagnostic> {
        if !self.analysis.diagnostics.is_empty() || !self.semantic.diagnostics.is_empty() {
            return Err(handoff_error("validator requires diagnostic-free frontend input"));
        }
        self.validate_identities()?;
        self.validate_annotations()?;
        self.validate_entry_point()?;
        self.validate_contextual_records()?;
        self.validate_mutability()?;
        self.validate_flow()?;
        Ok(())
    }

    fn validate_identities(&self) -> Result<(), Diagnostic> {
        if self.analysis.declarations.len() != self.subjects.declarations.len() {
            return Err(handoff_error("declaration identity coverage is incomplete"));
        }
        let type_count = self.subjects.declarations.iter().filter(|declaration| matches!(declaration, Declaration::Type(_))).count();
        let function_count = self.subjects.declarations.len() - type_count;
        if self.analysis.type_definitions.len() != type_count
            || self.analysis.function_signatures.len() != function_count
            || self.analysis.type_names.len() != type_count
            || self.analysis.function_names.len() != function_count
        {
            return Err(handoff_error("declaration namespace or definition coverage is incomplete"));
        }
        if self.analysis.bindings.len()
            != self.subjects.parameters.len() + self.subjects.locals.len() + self.subjects.loop_bindings.len()
        {
            return Err(handoff_error("binding identity coverage is incomplete"));
        }
        if self.analysis.binding_types.len() != self.analysis.bindings.len() {
            return Err(handoff_error("binding type coverage is incomplete"));
        }
        for record in &self.analysis.declarations {
            let valid_id = match record.id {
                DeclarationId::Type(id) => id.index() < self.analysis.type_definitions.len(),
                DeclarationId::Function(id) => id.index() < self.analysis.function_signatures.len(),
            };
            if !valid_id || !self.declaration_node_belongs(record.node) {
                return Err(handoff_error("declaration record has an invalid identity or foreign AST subject"));
            }
            if self.analysis.declarations.iter().filter(|candidate| same_declaration_node(candidate.node, record.node)).count() != 1 {
                return Err(handoff_error("declaration identity subject is duplicated"));
            }
        }
        for (index, binding) in self.analysis.bindings.iter().enumerate() {
            if binding.id.index() != index || !self.binding_node_belongs(binding.node) {
                return Err(handoff_error("binding record has an invalid identity or foreign AST subject"));
            }
            if self.analysis.bindings.iter().filter(|candidate| same_binding_node(candidate.node, binding.node)).count() != 1 {
                return Err(handoff_error("binding identity subject is duplicated"));
            }
            let annotation = self.analysis.binding_types[index];
            if annotation.binding != binding.id || !self.final_state(annotation.state) {
                return Err(handoff_error("binding has a stale or unresolved final type"));
            }
        }
        for (index, definition) in self.analysis.type_definitions.iter().enumerate() {
            if definition.id.index() != index
                || !self.declaration_node_belongs(DeclarationNode::Type(definition.node))
            {
                return Err(handoff_error("type definition identity is invalid or cross-linked"));
            }
            let member_states_valid = match &definition.kind {
                TypeDefinitionKind::Struct(members) => members.iter().all(|member| matches!(member.ty, TypeState::Resolved(ty) if self.analysis.types.contains(ty))),
                TypeDefinitionKind::Tuple(members) => members.iter().all(|member| matches!(member.ty, TypeState::Resolved(ty) if self.analysis.types.contains(ty))),
                TypeDefinitionKind::Union { alternatives, .. } => alternatives.iter().all(|alternative| self.analysis.types.contains(alternative_payload(alternative))),
                TypeDefinitionKind::Invalid => false,
            };
            if !member_states_valid {
                return Err(handoff_error("type definition retains an invalid member or alternative type"));
            }
        }
        for entry in &self.analysis.type_names {
            if entry.id.index() >= self.analysis.type_definitions.len()
                || self.analysis.type_names.iter().filter(|candidate| candidate.name.as_ref() == entry.name.as_ref() || candidate.id == entry.id).count() != 1
            {
                return Err(handoff_error("resolved type namespace is invalid or duplicated"));
            }
        }
        for (index, signature) in self.analysis.function_signatures.iter().enumerate() {
            if signature.id.index() != index
                || !self.declaration_node_belongs(DeclarationNode::Function(signature.node))
                || !matches!(signature.result, TypeState::Resolved(ty) if self.analysis.types.contains(ty))
            {
                return Err(handoff_error("function signature identity or result type is invalid"));
            }
            for parameter in &signature.parameters {
                if parameter.binding.index() >= self.analysis.bindings.len()
                    || !self.subjects.parameters.iter().any(|node| std::ptr::eq(*node, parameter.node))
                    || !matches!(parameter.ty, TypeState::Resolved(ty) if self.analysis.types.contains(ty))
                {
                    return Err(handoff_error("function parameter signature is invalid or cross-linked"));
                }
            }
        }
        for entry in &self.analysis.function_names {
            if entry.id.index() >= self.analysis.function_signatures.len()
                || self.analysis.function_names.iter().filter(|candidate| candidate.name.as_ref() == entry.name.as_ref() || candidate.id == entry.id).count() != 1
            {
                return Err(handoff_error("resolved function namespace is invalid or duplicated"));
            }
        }
        for (index, deferred) in self.analysis.deferred.iter().enumerate() {
            if deferred.id.index() != index || !deferred.resolved || !self.ast_node_belongs(deferred.node) {
                return Err(handoff_error("unresolved or invalid deferred analysis record"));
            }
        }
        if !self.semantic.deferred_mutability.is_empty() || !self.semantic.flow.deferred.is_empty() {
            return Err(handoff_error("unresolved semantic obligation remains"));
        }
        Ok(())
    }

    fn validate_annotations(&self) -> Result<(), Diagnostic> {
        for annotation in &self.analysis.type_annotations {
            if !self.subjects.types.iter().any(|node| std::ptr::eq(*node, annotation.node))
                || !self.annotation_state_valid(annotation.state)
            {
                return Err(handoff_error("type annotation is invalid or has a foreign AST subject"));
            }
        }
        for annotation in &self.analysis.expression_annotations {
            if !self.contains_expression(annotation.node) || !self.annotation_state_valid(annotation.state) {
                return Err(handoff_error("expression annotation is invalid or has a foreign AST subject"));
            }
        }
        for annotation in &self.analysis.assignment_targets {
            if !self.subjects.assignments.iter().any(|node| std::ptr::eq(*node, annotation.node))
                || !self.annotation_state_valid(annotation.state)
                || annotation.root.is_some_and(|root| root.index() >= self.analysis.bindings.len())
                || annotation.steps.iter().any(|step| !self.assignment_step_valid(step, annotation.node))
            {
                return Err(handoff_error("assignment annotation is invalid or has a foreign AST subject"));
            }
        }
        for ty in &self.subjects.types {
            let Some(annotation) = self.analysis.type_annotations.iter().rev().find(|item| std::ptr::eq(item.node, *ty)) else {
                return Err(handoff_error("type node has no final annotation"));
            };
            if !matches!(annotation.state, TypeState::Resolved(ty) if self.analysis.types.contains(ty)) {
                return Err(handoff_error("type node has a stale or unresolved final annotation"));
            }
        }
        for expression in &self.subjects.expressions {
            if self.is_non_value_callee_syntax(expression) {
                continue;
            }
            let Some(annotation) = self.analysis.expression_annotation(expression) else {
                return Err(handoff_error("expression has no final annotation"));
            };
            if !self.final_state(annotation.state) {
                return Err(handoff_error("expression has a stale or unresolved final annotation"));
            }
        }
        for literal in &self.subjects.literals {
            if self.analysis.literals.iter().filter(|item| std::ptr::eq(item.node, *literal)).count() != 1 {
                return Err(handoff_error("literal annotation coverage is missing or duplicated"));
            }
        }
        if self.analysis.literals.iter().any(|item| !self.subjects.literals.iter().any(|node| std::ptr::eq(*node, item.node))) {
            return Err(handoff_error("literal annotation has a foreign AST subject"));
        }
        for expression in &self.subjects.expressions {
            if let ExpressionKind::Conversion { destination, operand } = &expression.kind {
                let records = self.analysis.numeric_conversions.iter().filter(|item| std::ptr::eq(item.node, *expression)).collect::<Vec<_>>();
                let expected = matches!(self.analysis.expression_annotation(expression).map(|item| item.state), Some(TypeState::Resolved(_)));
                if records.len() != usize::from(expected) {
                    return Err(handoff_error("numeric conversion coverage is missing or duplicated"));
                }
                if !expected { continue; }
                let record = records[0];
                let expected_destination = self.analysis.types.primitive(*destination);
                let source = self.analysis.expression_annotation(operand).map(|item| item.state);
                let conversion_result = self.analysis.union_injection(expression)
                    .map(|item| TypeState::Resolved(alternative_payload(&item.alternative)))
                    .or_else(|| self.analysis.expression_annotation(expression).map(|item| item.state));
                if record.destination != expected_destination
                    || source != Some(TypeState::Resolved(record.source))
                    || conversion_result != Some(TypeState::Resolved(record.destination))
                    || !matches!(
                        (self.analysis.types.get(record.source), self.analysis.types.get(record.destination)),
                        (ResolvedType::Primitive(PrimitiveType::Int), ResolvedType::Primitive(PrimitiveType::Float))
                            | (ResolvedType::Primitive(PrimitiveType::Float), ResolvedType::Primitive(PrimitiveType::Int))
                    )
                {
                    return Err(handoff_error("numeric conversion resolution is contradictory"));
                }
            }
            let applicable_projection = matches!(&expression.kind, ExpressionKind::Index { .. } | ExpressionKind::Member { .. })
                && !self.is_non_value_callee_syntax(expression)
                && matches!(self.analysis.expression_annotation(expression).map(|item| item.state), Some(TypeState::Resolved(_)));
            let count = self.analysis.projections.iter().filter(|item| std::ptr::eq(item.node, *expression)).count();
            if count != usize::from(applicable_projection) {
                return Err(handoff_error("value projection coverage is missing or duplicated"));
            }
        }
        if self.analysis.numeric_conversions.iter().any(|item| !self.contains_expression(item.node) || !matches!(&item.node.kind, ExpressionKind::Conversion { .. })) {
            return Err(handoff_error("numeric conversion resolution has a foreign AST subject"));
        }
        for projection in &self.analysis.projections {
            if !self.contains_expression(projection.node)
                || self.is_non_value_callee_syntax(projection.node)
                || !self.projection_valid(projection)
            {
                return Err(handoff_error("value projection resolution is contradictory or cross-linked"));
            }
        }
        for call in &self.subjects.calls {
            if self.analysis.calls.iter().filter(|item| std::ptr::eq(item.node, *call)).count() != 1 {
                return Err(handoff_error("call resolution coverage is missing or duplicated"));
            }
            let resolution = self.analysis.call_resolution(call).unwrap();
            if matches!(resolution.target, CallTarget::Unknown | CallTarget::Expression | CallTarget::Binding(_) | CallTarget::Ambiguous { .. } | CallTarget::AmbiguousErrorConstructor { .. }) {
                return Err(handoff_error("call retained an unresolved or non-callable target"));
            }
            if !self.call_target_valid(resolution.target) {
                return Err(handoff_error("call target contains an invalid stable identity"));
            }
            let needs_constructor = matches!(resolution.target, CallTarget::Callable(CallableId::Constructor(_)) | CallTarget::QualifiedConstructor(_) | CallTarget::ErrorConstructor);
            let constructor_count = self.analysis.constructors.iter().filter(|item| std::ptr::eq(item.node, *call)).count();
            if constructor_count != usize::from(needs_constructor) {
                return Err(handoff_error("constructor resolution coverage is missing or duplicated"));
            }
        }
        if self.analysis.calls.iter().any(|item| !self.subjects.calls.iter().any(|node| std::ptr::eq(*node, item.node)) || !self.contains_expression(item.callee)) {
            return Err(handoff_error("call resolution has a foreign AST subject"));
        }
        for identifier in &self.subjects.name_identifiers {
            if self.analysis.name_uses.iter().filter(|item| std::ptr::eq(item.node, *identifier)).count() != 1 {
                return Err(handoff_error("name-use resolution coverage is missing or duplicated"));
            }
        }
        for use_ in &self.analysis.name_uses {
            if !self.subjects.name_identifiers.iter().any(|node| std::ptr::eq(*node, use_.node))
                || matches!(use_.resolution, NameResolution::Unknown | NameResolution::AmbiguousCall { .. } | NameResolution::AmbiguousErrorConstructor { .. })
                || !self.name_resolution_valid(use_.resolution)
            {
                return Err(handoff_error("name-use resolution is unresolved or cross-linked"));
            }
        }
        for target in &self.subjects.assignments {
            let Some(annotation) = self.analysis.assignment_target(target) else {
                return Err(handoff_error("assignment target has no final annotation"));
            };
            if annotation.root.is_none()
                || annotation.steps.len() != target.suffixes.len()
                || !matches!(annotation.state, TypeState::Resolved(ty) if self.analysis.types.contains(ty))
            {
                return Err(handoff_error("executable assignment target has an incomplete typed path"));
            }
        }
        for constructor in &self.analysis.constructors {
            if !self.contains_expression(constructor.node)
                || !self.constructor_is_consistent(&constructor.kind)
            {
                return Err(handoff_error("constructor resolution is invalid or cross-linked"));
            }
        }
        for (index, subject) in self.analysis.union_injection_subjects.iter().enumerate() {
            if self.analysis.union_injection_subjects[..index].iter().any(|prior| same_union_injection(prior, subject))
                || !self.contains_expression(subject.node)
                || !self.union_has_alternative(subject.union_type, &subject.alternative)
                || self.analysis.union_injections.iter().filter(|injection| same_union_injection(injection, subject)).count() != 1
            {
                return Err(handoff_error("union injection coverage is invalid or incomplete"));
            }
        }
        for (index, injection) in self.analysis.union_injections.iter().enumerate() {
            if !self.contains_expression(injection.node)
                || !self.union_has_alternative(injection.union_type, &injection.alternative)
                || self.analysis.union_injections[..index].iter().any(|prior| std::ptr::eq(prior.node, injection.node) && prior.union_type == injection.union_type)
                || self.analysis.union_injection_subjects.iter().filter(|subject| same_union_injection(subject, injection)).count() != 1
            {
                return Err(handoff_error("union injection is invalid, duplicated, or cross-linked"));
            }
        }
        Ok(())
    }

    fn projection_valid(&self, resolution: &crate::analysis::ProjectionResolution<'ast>) -> bool {
        use crate::analysis::ProjectionKind;
        let result = self.analysis.union_injection(resolution.node)
            .map(|item| TypeState::Resolved(alternative_payload(&item.alternative)))
            .or_else(|| self.analysis.expression_annotation(resolution.node).map(|item| item.state));
        match (&resolution.node.kind, resolution.kind) {
            (ExpressionKind::Index { value, index }, ProjectionKind::List { element }) => {
                result == Some(TypeState::Resolved(element))
                    && matches!(self.analysis.expression_annotation(value).map(|item| item.state), Some(TypeState::Resolved(ty)) if matches!(self.analysis.types.get(ty), ResolvedType::List(actual) if *actual == element))
                    && self.analysis.expression_annotation(index).map(|item| item.state) == Some(TypeState::Resolved(self.analysis.types.primitive(PrimitiveType::Int)))
            }
            (ExpressionKind::Index { value, index }, ProjectionKind::Map { key, value: item }) => {
                result == Some(TypeState::Resolved(item))
                    && matches!(self.analysis.expression_annotation(value).map(|item| item.state), Some(TypeState::Resolved(ty)) if matches!(self.analysis.types.get(ty), ResolvedType::Map { key: actual_key, value: actual_value } if *actual_key == key && *actual_value == item))
                    && self.analysis.expression_annotation(index).map(|item| item.state) == Some(TypeState::Resolved(key))
            }
            (ExpressionKind::Index { value, index }, ProjectionKind::String { result: character }) => {
                result == Some(TypeState::Resolved(character))
                    && character == self.analysis.types.primitive(PrimitiveType::Char)
                    && self.analysis.expression_annotation(value).map(|item| item.state) == Some(TypeState::Resolved(self.analysis.types.primitive(PrimitiveType::Str)))
                    && self.analysis.expression_annotation(index).map(|item| item.state) == Some(TypeState::Resolved(self.analysis.types.primitive(PrimitiveType::Int)))
            }
            (ExpressionKind::Member { value, .. }, ProjectionKind::Struct { declaration, field, storage, result: member_type }) => {
                result == Some(TypeState::Resolved(member_type))
                    && matches!(self.analysis.expression_annotation(value).map(|item| item.state), Some(TypeState::Resolved(ty)) if matches!(self.analysis.types.get(ty), ResolvedType::Nominal(actual) if *actual == declaration))
                    && declaration.index() < self.analysis.type_definitions.len()
                    && matches!(&self.analysis.type_definition(declaration).kind, TypeDefinitionKind::Struct(fields) if fields.get(field).is_some_and(|item| item.storage == storage && item.ty == TypeState::Resolved(member_type)))
            }
            (ExpressionKind::Member { value, .. }, ProjectionKind::Tuple { declaration, position, result: member_type }) => {
                result == Some(TypeState::Resolved(member_type))
                    && matches!(self.analysis.expression_annotation(value).map(|item| item.state), Some(TypeState::Resolved(ty)) if matches!(self.analysis.types.get(ty), ResolvedType::Nominal(actual) if *actual == declaration))
                    && declaration.index() < self.analysis.type_definitions.len()
                    && matches!(&self.analysis.type_definition(declaration).kind, TypeDefinitionKind::Tuple(fields) if fields.get(position).is_some_and(|item| item.ty == TypeState::Resolved(member_type)))
            }
            _ => false,
        }
    }

    fn validate_entry_point(&self) -> Result<(), Diagnostic> {
        let Some(entry) = self.semantic.entry_point else {
            return Err(handoff_error("diagnostic-free result has no entry point"));
        };
        let id = entry.function_id();
        let Some(signature) = self.analysis.function_signatures.get(id.index()) else {
            return Err(handoff_error("entry point has an invalid function identity"));
        };
        if signature.id != id
            || self.analysis.function_by_name("main") != Some(id)
            || !valid_main_signature(self.analysis, signature)
        {
            return Err(handoff_error("entry point disagrees with the resolved main declaration"));
        }
        Ok(())
    }

    fn validate_contextual_records(&self) -> Result<(), Diagnostic> {
        for expression in &self.subjects.union_tests {
            if self.semantic.union_tests.iter().filter(|item| std::ptr::eq(item.expression, *expression)).count() != 1 {
                return Err(handoff_error("union test resolution coverage is missing or duplicated"));
            }
        }
        for resolution in &self.semantic.union_tests {
            if !self.contains_expression(resolution.expression)
                || !self.union_has_alternative(resolution.operand_union, &resolution.alternative)
                || alternative_payload(&resolution.alternative) != resolution.payload
                || resolution.binding.is_some_and(|id| id.index() >= self.analysis.bindings.len())
            {
                return Err(handoff_error("union test resolution is contradictory or cross-linked"));
            }
        }
        for statement in &self.subjects.switches {
            if self.semantic.switches.iter().filter(|item| std::ptr::eq(item.statement, *statement)).count() != 1 {
                return Err(handoff_error("switch resolution coverage is missing or duplicated"));
            }
        }
        for resolution in &self.semantic.switches {
            if !self.contains_statement(resolution.statement)
                || !resolution.exhaustive
                || resolution.binding.is_some_and(|id| id.index() >= self.analysis.bindings.len())
            {
                return Err(handoff_error("switch resolution is incomplete or cross-linked"));
            }
            let StatementKind::Switch { arms, else_body, .. } = &resolution.statement.kind else {
                return Err(handoff_error("switch resolution refers to a non-switch statement"));
            };
            if resolution.arms.len() != arms.len()
                || resolution.else_body.map(|body| body as *const _) != else_body.as_ref().map(|body| body as *const _)
            {
                return Err(handoff_error("switch arm or else coverage is incomplete"));
            }
            for arm in &resolution.arms {
                if !arms.iter().any(|node| std::ptr::eq(node, arm.arm))
                    || !self.union_has_alternative(resolution.operand_union, &arm.alternative)
                    || alternative_payload(&arm.alternative) != arm.payload
                {
                    return Err(handoff_error("switch alternative resolution is contradictory"));
                }
            }
            if resolution.arms.iter().any(|arm| !resolution.covered.contains(&arm.alternative))
                || resolution.covered.len() != resolution.arms.len()
            {
                return Err(handoff_error("switch covered set disagrees with its unique arms"));
            }
            let direct = self.union_alternatives(resolution.operand_union).ok_or_else(|| handoff_error("switch operand identity is not a union"))?;
            if resolution.covered.iter().any(|alternative| !direct.contains(alternative))
                || resolution.covered.iter().enumerate().any(|(index, alternative)| resolution.covered[..index].contains(alternative))
                || (resolution.else_body.is_none() && !direct.iter().all(|alternative| resolution.covered.contains(alternative)))
            {
                return Err(handoff_error("switch coverage contradicts its direct alternatives"));
            }
        }
        for expression in &self.subjects.tries {
            let ExpressionKind::Try { value, .. } = &expression.kind else { unreachable!() };
            let expected = !matches!(self.analysis.expression_annotation(value).map(|item| item.state), Some(TypeState::Never));
            let count = self.semantic.tries.iter().filter(|item| std::ptr::eq(item.expression, *expression)).count();
            if count != usize::from(expected) {
                return Err(handoff_error("postfix try resolution coverage is missing or duplicated"));
            }
        }
        for resolution in &self.semantic.tries {
            if !self.contains_expression(resolution.expression)
                || !self.union_has_alternative(resolution.operand_union, &resolution.source_error)
                || !matches!(resolution.source_error, UnionAlternative::Error(_))
                || !self.analysis.types.contains(resolution.success)
                || self.analysis.expression_annotation(resolution.expression).map(|item| item.state) != Some(TypeState::Resolved(resolution.success))
            {
                return Err(handoff_error("postfix try resolution is contradictory or cross-linked"));
            }
            let alternatives = self.union_alternatives(resolution.operand_union).unwrap();
            if alternatives.iter().filter(|alternative| matches!(alternative, UnionAlternative::Error(_))).count() != 1 {
                return Err(handoff_error("postfix try operand does not have one direct Error alternative"));
            }
            let successes = alternatives.iter().filter(|alternative| !matches!(alternative, UnionAlternative::Error(_))).cloned().collect::<Vec<_>>();
            let success_matches = match successes.as_slice() {
                [only] => resolution.success == alternative_payload(only),
                many => matches!(self.analysis.types.get(resolution.success), ResolvedType::Union(actual) if actual.as_ref() == many),
            };
            if !success_matches {
                return Err(handoff_error("postfix try success type contradicts its operand alternatives"));
            }
            let Some(function) = self.enclosing_function(resolution.expression) else {
                return Err(handoff_error("postfix try has no enclosing function"));
            };
            match &resolution.action {
                TryAction::Panic { function: action_function } => {
                    if *action_function != function || self.semantic.entry_point.map(EntryPoint::function_id) != Some(function) {
                        return Err(handoff_error("postfix try panic action has the wrong function"));
                    }
                }
                TryAction::Propagate { function: action_function, destination_union, destination_error } => {
                    let declared = self.analysis.function_signatures[function.index()].result;
                    if *action_function != function
                        || self.semantic.entry_point.map(EntryPoint::function_id) == Some(function)
                        || declared != TypeState::Resolved(*destination_union)
                        || !self.union_has_alternative(*destination_union, destination_error)
                        || alternative_payload(destination_error) != alternative_payload(&resolution.source_error)
                    {
                        return Err(handoff_error("postfix try propagation action is contradictory"));
                    }
                }
            }
            let Some(flow) = self.semantic.flow.summaries.iter().find_map(|record| match record.subject {
                FlowSubject::Expression(node) if std::ptr::eq(node, resolution.expression) => Some(&record.summary),
                _ => None,
            }) else {
                return Err(handoff_error("postfix try has no flow summary"));
            };
            let action_flag = match &resolution.action { TryAction::Propagate { .. } => FlowFlags::RETURN, TryAction::Panic { .. } => FlowFlags::DIVERGE };
            if !flow.flags.contains(action_flag) || flow.fallthrough == FallthroughKind::None {
                return Err(handoff_error("postfix try flow omits its success or action path"));
            }
        }
        Ok(())
    }

    fn validate_mutability(&self) -> Result<(), Diagnostic> {
        for target in &self.subjects.assignments {
            if self.semantic.mutation_subjects.iter().filter(|subject| matches!(subject, MutabilitySubject::Assignment(node) if std::ptr::eq(*node, *target))).count() != 1 {
                return Err(handoff_error("assignment mutation coverage is missing or duplicated"));
            }
        }
        for (index, subject) in self.semantic.mutation_subjects.iter().enumerate() {
            if self.semantic.mutation_subjects[..index].iter().any(|prior| same_mutability_subject(*prior, *subject))
                || !self.mutability_subject_belongs(*subject)
                || self.semantic.mutability.iter().filter(|resolution| same_mutability_subject(resolution.subject, *subject)).count() != 1
            {
                return Err(handoff_error("mutation coverage is invalid or lacks one authorization"));
            }
        }
        for (index, resolution) in self.semantic.mutability.iter().enumerate() {
            if resolution.root.index() >= self.analysis.bindings.len()
                || self.semantic.mutability[..index].iter().any(|prior| same_mutability_subject(prior.subject, resolution.subject))
                || self.semantic.mutation_subjects.iter().filter(|subject| same_mutability_subject(**subject, resolution.subject)).count() != 1
            {
                return Err(handoff_error("mutation authorization has an invalid root or duplicate subject"));
            }
            match (resolution.subject, &resolution.path) {
                (MutabilitySubject::Assignment(subject), MutabilityPath::Assignment(path)) => {
                    let Some(annotation) = self.analysis.assignment_target(subject) else {
                        return Err(handoff_error("mutation authorization refers to an unannotated assignment"));
                    };
                    if annotation.root != Some(resolution.root)
                        || !self.subjects.assignments.iter().any(|node| std::ptr::eq(*node, subject))
                        || path.len() != annotation.steps.len()
                        || !path.iter().zip(annotation.steps.iter()).all(|(left, right)| access_steps_agree(left, right))
                    {
                        return Err(handoff_error("assignment mutation path contradicts name/type analysis"));
                    }
                    let Some(use_) = self.analysis.name_use(&subject.root) else {
                        return Err(handoff_error("assignment mutation authorization has no root name use"));
                    };
                    if use_.resolution != NameResolution::Binding(resolution.root) || use_.access != resolution.access {
                        return Err(handoff_error("assignment mutation authorization contradicts root access"));
                    }
                }
                (MutabilitySubject::Receiver { call, receiver }, MutabilityPath::Expression(path)) => {
                    if !self.contains_expression(call) || !self.contains_expression(receiver) {
                        return Err(handoff_error("receiver mutation authorization has a foreign AST subject"));
                    }
                    if !matches!(self.analysis.call_resolution(call).map(|item| item.target), Some(CallTarget::Builtin(method)) if resolution.operation == MutabilityOperation::MutateReceiver(method)) {
                        return Err(handoff_error("receiver mutation authorization contradicts its call target"));
                    }
                    if path.iter().any(|step| !self.place_step_valid(step)) {
                        return Err(handoff_error("receiver mutation authorization has an invalid typed path"));
                    }
                }
                (MutabilitySubject::Argument { call, argument }, MutabilityPath::Expression(path)) => {
                    let ExpressionKind::Call { arguments, .. } = &call.kind else {
                        return Err(handoff_error("mutable argument authorization refers to a non-call"));
                    };
                    if !self.contains_expression(call)
                        || !arguments.iter().any(|node| std::ptr::eq(node, argument))
                        || path.iter().any(|step| !self.place_step_valid(step))
                    {
                        return Err(handoff_error("mutable argument authorization is invalid or cross-linked"));
                    }
                    let MutabilityOperation::MutableArgument { function, parameter } = resolution.operation else {
                        return Err(handoff_error("mutable argument authorization has the wrong operation"));
                    };
                    let Some(signature) = self.analysis.function_signatures.get(function.index()) else {
                        return Err(handoff_error("mutable argument authorization has an invalid function"));
                    };
                    if !signature.parameters.iter().any(|item| item.binding == parameter && item.node.mutable) {
                        return Err(handoff_error("mutable argument authorization has an invalid parameter"));
                    }
                }
                _ => return Err(handoff_error("mutation subject and typed path category disagree")),
            }
            if matches!(resolution.operation, MutabilityOperation::DeferredPath) {
                return Err(handoff_error("mutation authorization retained a deferred operation"));
            }
        }
        Ok(())
    }

    fn validate_flow(&self) -> Result<(), Diagnostic> {
        for block in &self.subjects.blocks {
            self.require_one_flow(FlowSubject::Block(block))?;
        }
        for statement in &self.subjects.statements {
            self.require_one_flow(FlowSubject::Statement(statement))?;
        }
        for body in &self.subjects.statement_bodies {
            self.require_one_flow(FlowSubject::StatementBody(body))?;
        }
        for expression in &self.subjects.expressions {
            self.require_one_flow(FlowSubject::Expression(expression))?;
        }
        for body in &self.subjects.expression_bodies {
            self.require_one_flow(FlowSubject::ExpressionBody(body))?;
        }
        for arm in &self.subjects.switch_arms {
            self.require_one_flow(FlowSubject::SwitchArm(arm))?;
        }
        for record in &self.semantic.flow.summaries {
            if !self.flow_subject_belongs(record.subject)
                || record.summary.fallthrough == FallthroughKind::SwitchDependent
                || !record.summary.switch_dependencies.is_empty()
                || record.summary.flags.contains(FlowFlags::FALLTHROUGH) != (record.summary.fallthrough != FallthroughKind::None)
            {
                return Err(handoff_error("flow summary is unresolved, contradictory, or cross-linked"));
            }
        }
        for statement in &self.subjects.returns {
            if self.semantic.flow.returns.iter().filter(|item| std::ptr::eq(item.statement, *statement)).count() != 1 {
                return Err(handoff_error("explicit return fact is missing or duplicated"));
            }
        }
        for returned in &self.semantic.flow.returns {
            if !self.subjects.returns.iter().any(|node| std::ptr::eq(*node, returned.statement))
                || returned.function.index() >= self.analysis.function_signatures.len()
                || self.enclosing_statement_function(returned.statement) != Some(returned.function)
                || returned.value.map(|value| self.analysis.expression_annotation(value).map(|item| item.state))
                    .unwrap_or(Some(TypeState::Resolved(self.analysis.types.unit()))) != Some(returned.value_state)
            {
                return Err(handoff_error("explicit return fact is stale or cross-linked"));
            }
            if let Some(alternative) = &returned.unit_injection {
                let result = self.analysis.function_signatures[returned.function.index()].result;
                let TypeState::Resolved(result) = result else { unreachable!() };
                if returned.value.is_some()
                    || alternative_payload(alternative) != self.analysis.types.unit()
                    || !self.union_has_alternative(result, alternative)
                {
                    return Err(handoff_error("explicit unit return injection is contradictory"));
                }
            }
        }
        for statement in &self.subjects.loop_controls {
            if self.semantic.flow.loop_controls.iter().filter(|item| std::ptr::eq(item.statement, *statement)).count() != 1 {
                return Err(handoff_error("loop-control target is missing or duplicated"));
            }
        }
        for control in &self.semantic.flow.loop_controls {
            let expected = self.subjects.loop_targets.iter().find_map(|(statement, target)| std::ptr::eq(*statement, control.statement).then_some(*target));
            if expected.is_none_or(|target| !std::ptr::eq(target, control.target)) {
                return Err(handoff_error("loop control does not target its nearest enclosing loop"));
            }
        }
        for completion in &self.semantic.flow.completions {
            let Some(signature) = self.analysis.function_signatures.get(completion.function.index()) else {
                return Err(handoff_error("function completion has an invalid function identity"));
            };
            if !std::ptr::eq(&signature.node.body, completion.body)
                || self.semantic.flow.completions.iter().filter(|item| std::ptr::eq(item.body, completion.body)).count() != 1
            {
                return Err(handoff_error("function completion is duplicated or cross-linked"));
            }
            let TypeState::Resolved(result) = signature.result else { unreachable!() };
            match &completion.unit_injection {
                None if result == self.analysis.types.unit() => {}
                Some(alternative) if alternative_payload(alternative) == self.analysis.types.unit()
                    && self.union_has_alternative(result, alternative) => {}
                _ => return Err(handoff_error("function completion disagrees with its unit result")),
            }
        }
        for signature in &self.analysis.function_signatures {
            let summary = self.semantic.flow.summaries.iter().find_map(|record| match record.subject {
                FlowSubject::Block(block) if std::ptr::eq(block, &signature.node.body) => Some(&record.summary),
                _ => None,
            }).ok_or_else(|| handoff_error("function body has no final flow summary"))?;
            let needs_completion = signature.node.body.value.is_none()
                && summary.fallthrough == FallthroughKind::Definite
                && matches!(signature.result, TypeState::Resolved(result) if result == self.analysis.types.unit() || self.unit_alternative_for(result).is_some());
            let count = self.semantic.flow.completions.iter().filter(|item| item.function == signature.id).count();
            if count != usize::from(needs_completion) {
                return Err(handoff_error("implicit function completion coverage is incomplete or duplicated"));
            }
        }
        Ok(())
    }

    fn require_one_flow(&self, subject: FlowSubject<'ast>) -> Result<(), Diagnostic> {
        if self.semantic.flow.summaries.iter().filter(|record| same_flow_subject(record.subject, subject)).count() == 1 {
            Ok(())
        } else {
            Err(handoff_error("flow summary coverage is missing or duplicated"))
        }
    }

    fn final_state(&self, state: TypeState) -> bool {
        match state {
            TypeState::Never => true,
            TypeState::Resolved(ty) => self.analysis.types.contains(ty),
            TypeState::Error | TypeState::Deferred(_) => false,
        }
    }

    fn annotation_state_valid(&self, state: TypeState) -> bool {
        match state {
            TypeState::Resolved(ty) => self.analysis.types.contains(ty),
            TypeState::Deferred(id) => id.index() < self.analysis.deferred.len(),
            TypeState::Never | TypeState::Error => true,
        }
    }

    fn declaration_node_belongs(&self, node: DeclarationNode<'ast>) -> bool {
        self.subjects.declarations.iter().any(|declaration| match node {
            DeclarationNode::Type(right) => match declaration {
                Declaration::Type(left) => std::ptr::eq(left, right),
                Declaration::Function(_) => false,
            },
            DeclarationNode::Function(right) => match declaration {
                Declaration::Function(left) => std::ptr::eq(left, right),
                Declaration::Type(_) => false,
            },
        })
    }

    fn binding_node_belongs(&self, node: BindingNode<'ast>) -> bool {
        match node {
            BindingNode::Parameter(node) => self.subjects.parameters.iter().any(|item| std::ptr::eq(*item, node)),
            BindingNode::Local(node) => self.subjects.locals.iter().any(|item| std::ptr::eq(*item, node)),
            BindingNode::Loop(node) => self.subjects.loop_bindings.iter().any(|item| std::ptr::eq(*item, node)),
        }
    }

    fn ast_node_belongs(&self, node: AstNode<'ast>) -> bool {
        match node {
            AstNode::Declaration(node) => self.declaration_node_belongs(node),
            AstNode::Binding(node) => self.binding_node_belongs(node),
            AstNode::Type(node) => self.subjects.types.iter().any(|item| std::ptr::eq(*item, node)),
            AstNode::Statement(node) => self.contains_statement(node),
            AstNode::Expression(node) => self.contains_expression(node),
            AstNode::AssignmentTarget(node) => self.subjects.assignments.iter().any(|item| std::ptr::eq(*item, node)),
        }
    }

    fn contains_expression(&self, node: &Expression) -> bool {
        self.subjects.expressions.iter().any(|item| std::ptr::eq(*item, node))
    }

    fn is_non_value_callee_syntax(&self, node: &Expression) -> bool {
        self.subjects
            .call_callees
            .iter()
            .any(|callee| std::ptr::eq(*callee, node))
            || self.subjects.calls.iter().any(|call| {
                if !matches!(
                    self.analysis.call_resolution(call).map(|resolution| resolution.target),
                    Some(CallTarget::QualifiedConstructor(_))
                ) {
                    return false;
                }
                let ExpressionKind::Call { callee, .. } = &call.kind else {
                    return false;
                };
                matches!(
                    &callee.kind,
                    ExpressionKind::Member { value, .. } if std::ptr::eq(value.as_ref(), node)
                )
            })
    }

    fn contains_statement(&self, node: &Statement) -> bool {
        self.subjects.statements.iter().any(|item| std::ptr::eq(*item, node))
    }

    fn flow_subject_belongs(&self, subject: FlowSubject<'ast>) -> bool {
        match subject {
            FlowSubject::Block(node) => self.subjects.blocks.iter().any(|item| std::ptr::eq(*item, node)),
            FlowSubject::Statement(node) => self.contains_statement(node),
            FlowSubject::StatementBody(node) => self.subjects.statement_bodies.iter().any(|item| std::ptr::eq(*item, node)),
            FlowSubject::Expression(node) => self.contains_expression(node),
            FlowSubject::ExpressionBody(node) => self.subjects.expression_bodies.iter().any(|item| std::ptr::eq(*item, node)),
            FlowSubject::SwitchArm(node) => self.subjects.switch_arms.iter().any(|item| std::ptr::eq(*item, node)),
        }
    }

    fn mutability_subject_belongs(&self, subject: MutabilitySubject<'ast>) -> bool {
        match subject {
            MutabilitySubject::Assignment(node) => self.subjects.assignments.iter().any(|item| std::ptr::eq(*item, node)),
            MutabilitySubject::Receiver { call, receiver } => self.contains_expression(call) && self.contains_expression(receiver),
            MutabilitySubject::Argument { call, argument } => matches!(&call.kind, ExpressionKind::Call { arguments, .. } if self.contains_expression(call) && arguments.iter().any(|item| std::ptr::eq(item, argument))),
        }
    }

    fn union_alternatives(&self, ty: TypeId) -> Option<&[UnionAlternative]> {
        if !self.analysis.types.contains(ty) {
            return None;
        }
        match self.analysis.types.get(ty) {
            ResolvedType::Union(alternatives) => Some(alternatives),
            ResolvedType::Nominal(declaration) if declaration.index() < self.analysis.type_definitions.len() => {
                match &self.analysis.type_definitions[declaration.index()].kind {
                    TypeDefinitionKind::Union { alternatives, .. } => Some(alternatives),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn union_has_alternative(&self, ty: TypeId, alternative: &UnionAlternative) -> bool {
        self.union_alternatives(ty).is_some_and(|alternatives| alternatives.contains(alternative))
            && self.analysis.types.contains(alternative_payload(alternative))
    }

    fn unit_alternative_for(&self, ty: TypeId) -> Option<&UnionAlternative> {
        let unit = self.analysis.types.unit();
        let mut matches = self.union_alternatives(ty)?.iter().filter(|alternative| matches!(alternative, UnionAlternative::Untagged(payload) if *payload == unit));
        let only = matches.next()?;
        matches.next().is_none().then_some(only)
    }

    fn constructor_is_consistent(&self, constructor: &ConstructorKind) -> bool {
        match constructor {
            ConstructorKind::Struct { declaration, argument_members } => declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Struct(members) if argument_members.len() == members.len() && argument_members.iter().all(|index| *index < members.len())),
            ConstructorKind::Tuple(declaration) => declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Tuple(_)),
            ConstructorKind::Union { declaration, alternative }
            | ConstructorKind::TaggedUnion { declaration, alternative } => declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Union { alternatives, .. } if alternatives.contains(alternative)),
            ConstructorKind::Error { union_type, alternative } => self.union_has_alternative(*union_type, alternative),
        }
    }

    fn call_target_valid(&self, target: CallTarget) -> bool {
        match target {
            CallTarget::Callable(CallableId::Function(id)) => id.index() < self.analysis.function_signatures.len(),
            CallTarget::Callable(CallableId::Constructor(id)) | CallTarget::QualifiedConstructor(id) => id.index() < self.analysis.type_definitions.len(),
            CallTarget::Callable(CallableId::Intrinsic(_)) | CallTarget::Builtin(_) | CallTarget::ErrorConstructor => true,
            CallTarget::Binding(_) | CallTarget::Expression | CallTarget::Unknown | CallTarget::Ambiguous { .. } | CallTarget::AmbiguousErrorConstructor { .. } => false,
        }
    }

    fn name_resolution_valid(&self, resolution: NameResolution) -> bool {
        match resolution {
            NameResolution::Binding(id) => id.index() < self.analysis.bindings.len(),
            NameResolution::Callable(CallableId::Function(id)) => id.index() < self.analysis.function_signatures.len(),
            NameResolution::Callable(CallableId::Constructor(id)) => id.index() < self.analysis.type_definitions.len(),
            NameResolution::Callable(CallableId::Intrinsic(_)) | NameResolution::ErrorConstructor => true,
            NameResolution::Unknown | NameResolution::AmbiguousCall { .. } | NameResolution::AmbiguousErrorConstructor { .. } => false,
        }
    }

    fn place_step_valid(&self, step: &PlaceStep<'ast>) -> bool {
        match step {
            PlaceStep::StructMember { node, declaration, member, storage, state } => self.contains_expression(node)
                && declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Struct(members) if members.iter().any(|item| std::ptr::eq(item.node, *member) && item.storage == *storage && item.ty == *state))
                && self.analysis.expression_annotation(node).map(|item| item.state) == Some(*state)
                && self.final_state(*state),
            PlaceStep::TupleMember { node, declaration, member, position, state } => self.contains_expression(node)
                && declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Tuple(members) if members.iter().any(|item| std::ptr::eq(item.node, *member) && item.position == *position && item.ty == *state))
                && self.analysis.expression_annotation(node).map(|item| item.state) == Some(*state)
                && self.final_state(*state),
            PlaceStep::ListIndex { node, element, state } => self.contains_expression(node) && self.analysis.types.contains(*element) && *state == TypeState::Resolved(*element) && self.analysis.expression_annotation(node).map(|item| item.state) == Some(*state),
            PlaceStep::MapIndex { node, key, value, state } => self.contains_expression(node) && self.analysis.types.contains(*key) && self.analysis.types.contains(*value) && *state == TypeState::Resolved(*value) && self.analysis.expression_annotation(node).map(|item| item.state) == Some(*state),
        }
    }

    fn assignment_step_valid(&self, step: &AccessPathStep<'ast>, target: &AssignmentTarget) -> bool {
        match step {
            AccessPathStep::StructMember { suffix, declaration, member, field, storage, state } => target.suffixes.iter().any(|item| std::ptr::eq(item, *suffix))
                && declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Struct(members) if members.get(*field).is_some_and(|item| std::ptr::eq(item.node, *member) && item.storage == *storage && item.ty == *state))
                && self.annotation_state_valid(*state),
            AccessPathStep::TupleMember { suffix, declaration, member, position, state } => target.suffixes.iter().any(|item| std::ptr::eq(item, *suffix))
                && declaration.index() < self.analysis.type_definitions.len()
                && matches!(&self.analysis.type_definitions[declaration.index()].kind, TypeDefinitionKind::Tuple(members) if members.iter().any(|item| std::ptr::eq(item.node, *member) && item.position == *position && item.ty == *state))
                && self.annotation_state_valid(*state),
            AccessPathStep::ListIndex { suffix, element, state } => target.suffixes.iter().any(|item| std::ptr::eq(item, *suffix)) && self.analysis.types.contains(*element) && *state == TypeState::Resolved(*element),
            AccessPathStep::MapIndex { suffix, key, value, state } => target.suffixes.iter().any(|item| std::ptr::eq(item, *suffix)) && self.analysis.types.contains(*key) && self.analysis.types.contains(*value) && *state == TypeState::Resolved(*value),
        }
    }

    fn enclosing_function(&self, expression: &Expression) -> Option<FunctionId> {
        let index = self.subjects.expression_functions.iter().find_map(|(node, function)| std::ptr::eq(*node, expression).then_some(*function))?;
        self.analysis.function_signatures.get(index).map(|signature| signature.id)
    }

    fn enclosing_statement_function(&self, statement: &Statement) -> Option<FunctionId> {
        let index = self.subjects.statement_functions.iter().find_map(|(node, function)| std::ptr::eq(*node, statement).then_some(*function))?;
        self.analysis.function_signatures.get(index).map(|signature| signature.id)
    }
}

fn access_steps_agree(left: &AccessPathStep<'_>, right: &AccessPathStep<'_>) -> bool {
    match (left, right) {
        (AccessPathStep::StructMember { suffix: left_suffix, declaration: left_declaration, member: left_member, field: left_field, storage: left_storage, state: left_state },
         AccessPathStep::StructMember { suffix: right_suffix, declaration: right_declaration, member: right_member, field: right_field, storage: right_storage, state: right_state }) => {
            std::ptr::eq(*left_suffix, *right_suffix) && left_declaration == right_declaration && std::ptr::eq(*left_member, *right_member) && left_field == right_field && left_storage == right_storage && left_state == right_state
        }
        (AccessPathStep::TupleMember { suffix: left_suffix, declaration: left_declaration, member: left_member, position: left_position, state: left_state },
         AccessPathStep::TupleMember { suffix: right_suffix, declaration: right_declaration, member: right_member, position: right_position, state: right_state }) => {
            std::ptr::eq(*left_suffix, *right_suffix) && left_declaration == right_declaration && std::ptr::eq(*left_member, *right_member) && left_position == right_position && left_state == right_state
        }
        (AccessPathStep::ListIndex { suffix: left_suffix, element: left_element, state: left_state },
         AccessPathStep::ListIndex { suffix: right_suffix, element: right_element, state: right_state }) => {
            std::ptr::eq(*left_suffix, *right_suffix) && left_element == right_element && left_state == right_state
        }
        (AccessPathStep::MapIndex { suffix: left_suffix, key: left_key, value: left_value, state: left_state },
         AccessPathStep::MapIndex { suffix: right_suffix, key: right_key, value: right_value, state: right_state }) => {
            std::ptr::eq(*left_suffix, *right_suffix) && left_key == right_key && left_value == right_value && left_state == right_state
        }
        _ => false,
    }
}

fn same_union_injection(
    left: &crate::analysis::UnionInjection<'_>,
    right: &crate::analysis::UnionInjection<'_>,
) -> bool {
    std::ptr::eq(left.node, right.node)
        && left.union_type == right.union_type
        && left.alternative == right.alternative
}

fn same_declaration_node(left: DeclarationNode<'_>, right: DeclarationNode<'_>) -> bool {
    match (left, right) {
        (DeclarationNode::Type(left), DeclarationNode::Type(right)) => std::ptr::eq(left, right),
        (DeclarationNode::Function(left), DeclarationNode::Function(right)) => std::ptr::eq(left, right),
        _ => false,
    }
}

fn same_binding_node(left: BindingNode<'_>, right: BindingNode<'_>) -> bool {
    match (left, right) {
        (BindingNode::Parameter(left), BindingNode::Parameter(right)) => std::ptr::eq(left, right),
        (BindingNode::Local(left), BindingNode::Local(right))
        | (BindingNode::Loop(left), BindingNode::Loop(right)) => std::ptr::eq(left, right),
        _ => false,
    }
}

fn same_mutability_subject(left: MutabilitySubject<'_>, right: MutabilitySubject<'_>) -> bool {
    match (left, right) {
        (MutabilitySubject::Assignment(left), MutabilitySubject::Assignment(right)) => std::ptr::eq(left, right),
        (MutabilitySubject::Receiver { call: left, .. }, MutabilitySubject::Receiver { call: right, .. }) => std::ptr::eq(left, right),
        (MutabilitySubject::Argument { call: left_call, argument: left }, MutabilitySubject::Argument { call: right_call, argument: right }) => std::ptr::eq(left_call, right_call) && std::ptr::eq(left, right),
        _ => false,
    }
}

fn same_flow_subject(left: FlowSubject<'_>, right: FlowSubject<'_>) -> bool {
    match (left, right) {
        (FlowSubject::Block(left), FlowSubject::Block(right)) => std::ptr::eq(left, right),
        (FlowSubject::Statement(left), FlowSubject::Statement(right)) => std::ptr::eq(left, right),
        (FlowSubject::StatementBody(left), FlowSubject::StatementBody(right)) => std::ptr::eq(left, right),
        (FlowSubject::Expression(left), FlowSubject::Expression(right)) => std::ptr::eq(left, right),
        (FlowSubject::ExpressionBody(left), FlowSubject::ExpressionBody(right)) => std::ptr::eq(left, right),
        (FlowSubject::SwitchArm(left), FlowSubject::SwitchArm(right)) => std::ptr::eq(left, right),
        _ => false,
    }
}

/// Resolves the contextual union syntax and applies branch-local binding types.
/// This deliberately runs after ordinary inference: it only revisits nodes whose
/// type can change as a consequence of a narrowing environment.
struct UnionValidator<'analysis, 'diagnostics, 'warnings, 'source, 'ast> {
    analysis: &'analysis mut Analysis<'source, 'ast>,
    diagnostics: &'diagnostics mut Diagnostics,
    warnings: &'warnings mut Warnings,
    tests: Vec<UnionTestResolution<'ast>>,
    switches: Vec<SwitchResolution<'ast>>,
    tries: Vec<TryResolution<'ast>>,
    narrowed: Vec<(BindingId, TypeId)>,
    function_result: TypeState,
    function: Option<FunctionId>,
}

impl<'analysis, 'diagnostics, 'warnings, 'source, 'ast>
    UnionValidator<'analysis, 'diagnostics, 'warnings, 'source, 'ast>
{
    fn new(
        analysis: &'analysis mut Analysis<'source, 'ast>,
        diagnostics: &'diagnostics mut Diagnostics,
        warnings: &'warnings mut Warnings,
    ) -> Self {
        let unit = analysis.types.unit();
        Self { analysis, diagnostics, warnings, tests: Vec::new(), switches: Vec::new(), tries: Vec::new(), narrowed: Vec::new(), function_result: TypeState::Resolved(unit), function: None }
    }

    fn validate(mut self) -> (Vec<UnionTestResolution<'ast>>, Vec<SwitchResolution<'ast>>, Vec<TryResolution<'ast>>) {
        let functions = self.analysis.function_signatures.iter().map(|signature| (signature.id, signature.node, signature.result)).collect::<Vec<_>>();
        for (id, function, result) in functions {
            self.narrowed.clear();
            self.function_result = result;
            self.function = Some(id);
            self.block(&function.body);
            if let Some(value) = function.body.value.as_deref() { self.validate_result_value(value); }
        }
        (self.tests, self.switches, self.tries)
    }

    fn block(&mut self, block: &'ast Block) {
        for statement in &block.statements { self.statement(statement); }
        if let Some(value) = block.value.as_deref() { self.expression(value); }
    }

    fn statement_body(&mut self, body: &'ast StatementBody) {
        match &body.kind {
            StatementBodyKind::Block(block) => self.block(block),
            StatementBodyKind::Statement(statement) => self.statement(statement),
        }
    }

    fn expression_body(&mut self, body: &'ast ExpressionBody) {
        match &body.kind {
            ExpressionBodyKind::Block(block) => self.block(block),
            ExpressionBodyKind::Expression(expression) => { self.expression(expression); }
        }
    }

    fn statement(&mut self, statement: &'ast Statement) {
        match &statement.kind {
            StatementKind::Local { initializer, .. } => {
                let state = self.expression(initializer);
                if let Some(binding) = self.analysis.local_binding(statement)
                    && matches!(self.analysis.binding_types[binding.index()].state, TypeState::Deferred(_))
                    && matches!(state, TypeState::Resolved(_))
                {
                    self.analysis.binding_types[binding.index()].state = state;
                }
            }
            StatementKind::Assignment { target, value, .. } => {
                for suffix in &target.suffixes {
                    if let crate::ast::AssignmentTargetSuffixKind::Index(index) = &suffix.kind { self.expression(index); }
                }
                self.retype_assignment_target(target);
                self.expression(value);
                if target.suffixes.is_empty()
                    && let Some(NameResolution::Binding(binding)) = self.analysis.name_use(&target.root).map(|use_| use_.resolution)
                {
                    self.remove_narrowing(binding);
                }
            }
            StatementKind::Expression(expression) => { self.expression(expression); }
            StatementKind::Return(value) => if let Some(value) = value { self.expression(value); self.validate_result_value(value); },
            StatementKind::Break | StatementKind::Continue => {}
            StatementKind::If { branches, else_body } => {
                let outer = self.narrowed.clone();
                for branch in branches {
                    self.narrowed.clone_from(&outer);
                    self.expression(&branch.condition);
                    if let Some((binding, payload)) = self.direct_test(&branch.condition) {
                        self.set_narrowing(binding, payload);
                    }
                    self.statement_body(&branch.body);
                }
                self.narrowed.clone_from(&outer);
                if let Some(body) = else_body { self.statement_body(body); }
                self.narrowed = outer;
                for branch in branches {
                    if statement_body_may_fallthrough(&branch.body) {
                        for binding in direct_assignments_in_body(self.analysis, &branch.body) { self.remove_narrowing(binding); }
                    }
                }
                if let Some(body) = else_body {
                    if statement_body_may_fallthrough(body) {
                        for binding in direct_assignments_in_body(self.analysis, body) { self.remove_narrowing(binding); }
                    }
                }
            }
            StatementKind::While { condition, body } => {
                self.expression(condition);
                let outer = self.narrowed.clone();
                if let Some((binding, payload)) = self.direct_test(condition) { self.set_narrowing(binding, payload); }
                self.statement_body(body);
                let assigned = direct_assignments_in_body(self.analysis, body);
                self.narrowed = outer;
                for binding in assigned { self.remove_narrowing(binding); }
            }
            StatementKind::For { iterable, body, .. } => {
                self.expression(iterable);
                let assigned = direct_assignments_in_body(self.analysis, body);
                self.statement_body(body);
                for binding in assigned { self.remove_narrowing(binding); }
            }
            StatementKind::Switch { value, arms, else_body } => self.switch(statement, value, arms, else_body.as_ref()),
            StatementKind::Block(block) => self.block(block),
        }
    }

    fn switch(
        &mut self,
        statement: &'ast Statement,
        value: &'ast Expression,
        arms: &'ast [SwitchArm],
        else_body: Option<&'ast StatementBody>,
    ) {
        self.expression(value);
        let state = self.state(value);
        let binding = exact_binding(self.analysis, value);
        let Some((union_type, style, alternatives)) = self.union_parts(state) else {
            if !matches!(state, TypeState::Error | TypeState::Never) {
                self.error(value.span, "switch operand must have a union type");
            }
            for arm in arms { self.statement_body(&arm.body); }
            if let Some(body) = else_body { self.statement_body(body); }
            self.resolve_statement_deferred(statement);
            return;
        };
        let outer = self.narrowed.clone();
        let mut covered = Vec::new();
        let mut resolved_arms = Vec::new();
        for arm in arms {
            let selected = self.resolve_label(&arm.label, style, &alternatives);
            self.narrowed.clone_from(&outer);
            if let Some(alternative) = selected {
                let payload = alternative_payload(&alternative);
                resolved_arms.push(SwitchArmResolution { arm, alternative: alternative.clone(), payload });
                if let Some(binding) = binding { self.set_narrowing(binding, payload); }
                if covered.contains(&alternative) {
                    self.error(arm.label.span, "switch alternative is covered more than once");
                } else {
                    covered.push(alternative.clone());
                }
            }
            self.statement_body(&arm.body);
        }
        self.narrowed.clone_from(&outer);
        if let Some(body) = else_body { self.statement_body(body); }
        let explicitly_exhaustive = alternatives.iter().all(|alternative| covered.contains(alternative));
        if else_body.is_none() && !explicitly_exhaustive {
            let missing = alternatives.iter().filter(|alternative| !covered.contains(*alternative))
                .map(|alternative| self.alternative_name(alternative)).collect::<Vec<_>>().join(", ");
            self.error(statement.span, format!("non-exhaustive switch; missing: {missing}"));
        }
        if else_body.is_some() && explicitly_exhaustive {
            self.warnings.push(Diagnostic::source_warning(self.analysis.source, else_body.unwrap().span, "unreachable source"));
        }
        self.switches.push(SwitchResolution {
            statement, operand_union: union_type, binding, arms: resolved_arms,
            covered: covered.into_boxed_slice(), else_body,
            exhaustive: else_body.is_some() || explicitly_exhaustive,
        });
        self.resolve_statement_deferred(statement);
        self.narrowed = outer;
        for arm in arms {
            if statement_body_may_fallthrough(&arm.body) {
                for binding in direct_assignments_in_body(self.analysis, &arm.body) { self.remove_narrowing(binding); }
            }
        }
        if let Some(body) = else_body {
            if statement_body_may_fallthrough(body) {
                for binding in direct_assignments_in_body(self.analysis, body) { self.remove_narrowing(binding); }
            }
        }
    }

    fn expression(&mut self, expression: &'ast Expression) -> TypeState {
        match &expression.kind {
            ExpressionKind::Identifier(identifier) => {
                if let Some(NameResolution::Binding(binding)) = self.analysis.name_use(identifier).map(|use_| use_.resolution)
                    && let Some(ty) = self.narrowed_type(binding)
                { self.replace_expression_state(expression, TypeState::Resolved(ty)); }
            }
            ExpressionKind::Parenthesized(inner) => {
                let state = self.expression(inner); self.replace_if_changed(expression, state);
            }
            ExpressionKind::Conversion { operand, .. } => {
                self.expression(operand);
            }
            ExpressionKind::Unary { operator, operand, .. } => {
                let operand = self.expression(operand);
                if let TypeState::Resolved(ty) = operand {
                    let int = self.analysis.types.primitive(PrimitiveType::Int);
                    let float = self.analysis.types.primitive(PrimitiveType::Float);
                    let bool_ty = self.analysis.types.primitive(PrimitiveType::Bool);
                    let valid = match operator {
                        crate::ast::UnaryOperator::LogicalNot => ty == bool_ty,
                        crate::ast::UnaryOperator::BitwiseNot => ty == int,
                        crate::ast::UnaryOperator::Plus | crate::ast::UnaryOperator::Minus => ty == int || ty == float,
                    };
                    if valid {
                        self.replace_if_changed(expression, TypeState::Resolved(ty));
                    } else if matches!(self.state(expression), TypeState::Deferred(_)) {
                        self.error(expression.span, "unary operator is not defined for the narrowed operand type");
                        self.replace_expression_state(expression, TypeState::Error);
                    }
                }
            }
            ExpressionKind::Try { value, operator_span } => {
                let operand = self.expression(value);
                self.resolve_try(expression, *operator_span, operand);
            }
            ExpressionKind::List(elements) => {
                for element in elements { self.expression(element); }
                if !elements.is_empty() {
                    let states = elements.iter().map(|element| self.state(element)).collect::<Vec<_>>();
                    if let Some(TypeState::Resolved(element)) = states.first().copied()
                        && states.iter().all(|state| *state == TypeState::Resolved(element))
                    {
                        let list = self.analysis.types.intern(ResolvedType::List(element));
                        self.replace_if_changed(expression, TypeState::Resolved(list));
                    }
                }
            }
            ExpressionKind::Map(entries) => {
                for entry in entries { self.expression(&entry.key); self.expression(&entry.value); }
                if let Some(first) = entries.first()
                    && let (TypeState::Resolved(key), TypeState::Resolved(value)) = (self.state(&first.key), self.state(&first.value))
                    && entries.iter().all(|entry| self.state(&entry.key) == TypeState::Resolved(key) && self.state(&entry.value) == TypeState::Resolved(value))
                {
                    let map = self.analysis.types.intern(ResolvedType::Map { key, value });
                    self.replace_if_changed(expression, TypeState::Resolved(map));
                }
            }
            ExpressionKind::Block(block) => {
                self.block(block);
                let state = block.value.as_deref().map_or(TypeState::Resolved(self.analysis.types.unit()), |value| self.state(value));
                self.replace_if_changed(expression, state);
            }
            ExpressionKind::If { branches, else_branch } => {
                let outer = self.narrowed.clone();
                let mut states = Vec::with_capacity(branches.len() + 1);
                for branch in branches {
                    self.narrowed.clone_from(&outer);
                    self.expression(&branch.condition);
                    if let Some((binding, payload)) = self.direct_test(&branch.condition) { self.set_narrowing(binding, payload); }
                    self.expression_body(&branch.body);
                    states.push(self.expression_body_state(&branch.body));
                }
                self.narrowed.clone_from(&outer); self.expression_body(else_branch);
                states.push(self.expression_body_state(else_branch));
                self.narrowed = outer;
                if let Some(TypeState::Resolved(expected)) = states.iter().find(|state| matches!(state, TypeState::Resolved(_))).copied()
                    && states.iter().all(|state| matches!(state, TypeState::Resolved(actual) if *actual == expected) || *state == TypeState::Never)
                {
                    self.replace_if_changed(expression, TypeState::Resolved(expected));
                }
            }
            ExpressionKind::Binary { left, operator, right, .. } => {
                let left_state = self.expression(left); let right_state = self.expression(right);
                if let (TypeState::Resolved(left), TypeState::Resolved(right)) = (left_state, right_state) {
                    let bool_ty = self.analysis.types.primitive(PrimitiveType::Bool);
                    let result = match operator {
                        BinaryOperator::LogicalOr | BinaryOperator::LogicalAnd if left == bool_ty && right == bool_ty => Some(bool_ty),
                        BinaryOperator::Equal | BinaryOperator::NotEqual
                        | BinaryOperator::Less | BinaryOperator::LessEqual
                        | BinaryOperator::Greater | BinaryOperator::GreaterEqual if left == right => Some(bool_ty),
                        BinaryOperator::In if matches!(self.analysis.types.get(right), ResolvedType::List(element) if *element == left)
                            || matches!(self.analysis.types.get(right), ResolvedType::Map { key, .. } if *key == left) => Some(bool_ty),
                        _ if left == right => Some(left),
                        _ => None,
                    };
                    if let Some(result) = result {
                        self.replace_if_changed(expression, TypeState::Resolved(result));
                    } else if matches!(self.state(expression), TypeState::Deferred(_)) {
                        self.error(expression.span, "operator is not defined for the narrowed operand types");
                        self.replace_expression_state(expression, TypeState::Error);
                    }
                }
            }
            ExpressionKind::Is { value, ty, .. } => {
                self.expression(value);
                let state = self.state(value);
                let binding = exact_binding(self.analysis, value);
                if let Some((union_type, style, alternatives)) = self.union_parts(state) {
                    if let Some(alternative) = self.resolve_label(ty, style, &alternatives) {
                        let payload = alternative_payload(&alternative);
                        self.tests.push(UnionTestResolution { expression, operand_union: union_type, alternative, payload, binding });
                        self.replace_expression_state(expression, TypeState::Resolved(self.analysis.types.primitive(PrimitiveType::Bool)));
                    } else { self.replace_expression_state(expression, TypeState::Error); }
                } else {
                    if !matches!(state, TypeState::Error | TypeState::Never) { self.error(value.span, "'is' operand must have a union type"); }
                    self.replace_expression_state(expression, TypeState::Error);
                }
            }
            ExpressionKind::Call { callee, arguments } => {
                let target = self.analysis.call_resolution(expression).map(|call| call.target);
                if !matches!(target, Some(CallTarget::QualifiedConstructor(_))) {
                    match &callee.kind {
                        ExpressionKind::Member { value, .. } => { self.expression(value); }
                        ExpressionKind::Identifier(_) => {}
                        _ if self.analysis.expression_annotation(callee).is_some() => { self.expression(callee); }
                        _ => {}
                    }
                }
                for argument in arguments { self.expression(argument_value(argument)); }
                self.retype_call(expression, callee, arguments);
            }
            ExpressionKind::Index { value, index } => {
                self.expression(value); self.expression(index); self.retype_index(expression, value, index);
            }
            ExpressionKind::Member { value, member } => {
                self.expression(value); self.retype_member(expression, value, *member);
            }
            ExpressionKind::Unit | ExpressionKind::Integer | ExpressionKind::Float | ExpressionKind::String(_)
            | ExpressionKind::Character(_) | ExpressionKind::Boolean(_)
            | ExpressionKind::TypedEmptyList(_) | ExpressionKind::TypedEmptyMap(_) => {}
        }
        self.state(expression)
    }

    fn union_parts(&self, state: TypeState) -> Option<(TypeId, UnionStyle, Vec<UnionAlternative>)> {
        let TypeState::Resolved(ty) = state else { return None; };
        match self.analysis.types.get(ty) {
            ResolvedType::Union(alternatives) => Some((ty, union_style_semantic(alternatives), alternatives.to_vec())),
            ResolvedType::Nominal(declaration) => match &self.analysis.type_definition(*declaration).kind {
                TypeDefinitionKind::Union { style, alternatives } => Some((ty, *style, alternatives.to_vec())),
                _ => None,
            },
            _ => None,
        }
    }

    fn resolve_try(&mut self, expression: &'ast Expression, operator_span: Span, operand: TypeState) {
        if operand == TypeState::Never {
            self.replace_if_changed(expression, TypeState::Never);
            return;
        }
        let Some((operand_union, _, alternatives)) = self.union_parts(operand) else {
            if operand != TypeState::Error {
                self.error(operator_span, "postfix '?' operand must be a union containing Error");
            }
            self.replace_expression_state(expression, TypeState::Error);
            return;
        };
        let errors = alternatives.iter().filter(|alternative| matches!(alternative, UnionAlternative::Error(_))).cloned().collect::<Vec<_>>();
        let [source_error] = errors.as_slice() else {
            self.error(operator_span, "postfix '?' operand union must contain exactly one direct Error alternative");
            self.replace_expression_state(expression, TypeState::Error);
            return;
        };
        let successes = alternatives.iter().filter(|alternative| !matches!(alternative, UnionAlternative::Error(_))).cloned().collect::<Vec<_>>();
        let success = if let [only] = successes.as_slice() {
            alternative_payload(only)
        } else {
            self.analysis.types.intern(ResolvedType::Union(successes.into_boxed_slice()))
        };
        let function = self.function.expect("try resolution requires an enclosing function");
        let function_node = self.analysis.function_signature(function).node;
        let action = if self.analysis.identifier_text(function_node.name.span) == "main" {
            TryAction::Panic { function }
        } else {
            let Some((destination_union, _, destination_alternatives)) = self.union_parts(self.function_result) else {
                self.error(operator_span, "enclosing function result must be a union with the same Error payload");
                self.replace_expression_state(expression, TypeState::Error);
                return;
            };
            let source_payload = alternative_payload(source_error);
            let matches = destination_alternatives.into_iter().filter(|alternative| matches!(alternative, UnionAlternative::Error(payload) if *payload == source_payload)).collect::<Vec<_>>();
            let [destination_error] = matches.as_slice() else {
                self.error(operator_span, "postfix '?' Error payload does not match the enclosing function result");
                self.replace_expression_state(expression, TypeState::Error);
                return;
            };
            TryAction::Propagate { function, destination_union, destination_error: destination_error.clone() }
        };
        self.replace_expression_state(expression, TypeState::Resolved(success));
        self.tries.push(TryResolution { expression, operator_span, operand_union, source_error: source_error.clone(), success, action });
    }

    fn resolve_label(&mut self, label: &'ast Type, style: UnionStyle, alternatives: &[UnionAlternative]) -> Option<UnionAlternative> {
        let bare = strip_type_parentheses(label);
        if let TypeKind::Named(identifier) = &bare.kind {
            let spelling = self.analysis.identifier_text(identifier.span).to_owned();
            if spelling == "Error" {
                let selected = alternatives.iter().find(|alternative| matches!(alternative, UnionAlternative::Error(_))).cloned();
                if let Some(alternative) = &selected {
                    self.annotate_contextual_label(label, alternative_payload(alternative));
                } else {
                    self.error(identifier.span, "union has no Error alternative");
                }
                return selected;
            }
            if style == UnionStyle::Tagged {
                if self.analysis.type_by_name(&spelling).is_some() {
                    self.error(identifier.span, "tagged union requires a tag label, not a type label");
                    return None;
                }
                let selected = alternatives.iter().find(|alternative| matches!(alternative, UnionAlternative::Tagged { tag, .. } if tag.as_ref() == spelling.as_str())).cloned();
                if let Some(alternative) = &selected {
                    self.annotate_contextual_label(label, alternative_payload(alternative));
                } else {
                    self.error(identifier.span, format!("unknown union tag '{spelling}'"));
                }
                return selected;
            } else if self.analysis.type_by_name(&spelling).is_none() {
                self.error(identifier.span, "untagged union requires a type label, not a tag label");
                return None;
            }
        }
        if style == UnionStyle::Tagged {
            self.error(label.span, "tagged union requires a bare tag label");
            return None;
        }
        let Some(label_type) = self.resolve_label_type(label) else { return None; };
        alternatives.iter().find(|alternative| matches!(alternative, UnionAlternative::Untagged(payload) if *payload == label_type)).cloned().or_else(|| {
            self.error(label.span, "type label is not an alternative of this union"); None
        })
    }

    fn annotate_contextual_label(&mut self, label: &'ast Type, payload: TypeId) {
        self.analysis.annotate_type(label, TypeState::Resolved(payload));
        if let TypeKind::Parenthesized(inner) = &label.kind {
            self.annotate_contextual_label(inner, payload);
        }
    }

    fn resolve_label_type(&mut self, ty: &'ast Type) -> Option<TypeId> {
        let resolved = match &ty.kind {
            TypeKind::Unit => Some(self.analysis.types.unit()),
            TypeKind::Primitive(primitive) => Some(self.analysis.types.primitive(*primitive)),
            TypeKind::Named(identifier) => {
                let name = self.analysis.identifier_text(identifier.span).to_owned();
                if let Some(declaration) = self.analysis.type_by_name(&name) {
                    Some(self.analysis.types.intern(ResolvedType::Nominal(declaration)))
                } else {
                    self.error(identifier.span, format!("unknown type '{name}'"));
                    None
                }
            }
            TypeKind::List(element) => {
                let element = self.resolve_label_type(element);
                element.map(|element| self.analysis.types.intern(ResolvedType::List(element)))
            }
            TypeKind::Map { key, value } => {
                let key = self.resolve_label_type(key); let value = self.resolve_label_type(value);
                key.zip(value).map(|(key, value)| self.analysis.types.intern(ResolvedType::Map { key, value }))
            }
            TypeKind::Union(alternatives) => {
                let mut resolved = Vec::new();
                for alternative in alternatives {
                    match &alternative.kind {
                        TypeKind::Tagged { tag, payload } if self.analysis.identifier_text(tag.span) == "Error" => {
                            if let Some(payload) = self.resolve_label_type(payload) { resolved.push(UnionAlternative::Error(payload)); }
                        }
                        TypeKind::Tagged { tag, payload } => {
                            if let Some(payload) = self.resolve_label_type(payload) { resolved.push(UnionAlternative::Tagged { tag: self.analysis.identifier_text(tag.span).into(), payload }); }
                        }
                        _ => if let Some(payload) = self.resolve_label_type(alternative) { resolved.push(UnionAlternative::Untagged(payload)); },
                    }
                }
                resolved.sort(); Some(self.analysis.types.intern(ResolvedType::Union(resolved.into_boxed_slice())))
            }
            TypeKind::Parenthesized(inner) => self.resolve_label_type(inner),
            TypeKind::Tagged { .. } => { self.error(ty.span, "tagged alternative is only valid within a union type"); None }
        };
        if let Some(resolved) = resolved { self.analysis.annotate_type(ty, TypeState::Resolved(resolved)); }
        resolved
    }

    fn retype_member(&mut self, expression: &'ast Expression, value: &'ast Expression, member: Member) {
        let TypeState::Resolved(receiver) = self.state(value) else { return; };
        let mut projection = None;
        let state = match (self.analysis.types.get(receiver).clone(), member) {
            (ResolvedType::Nominal(declaration), Member::Named(name)) => match self.analysis.type_definition(declaration).kind.clone() {
                TypeDefinitionKind::Struct(members) => members.iter().enumerate().find(|(_, field)| self.analysis.identifier_text(field.name.span) == self.analysis.identifier_text(name.span)).map(|(field, member)| {
                    if let TypeState::Resolved(result) = member.ty { projection = Some(ProjectionKind::Struct { declaration, field, storage: member.storage, result }); }
                    member.ty
                }),
                _ => None,
            },
            (ResolvedType::Nominal(declaration), Member::TupleIndex(span)) => match self.analysis.type_definition(declaration).kind.clone() {
                TypeDefinitionKind::Tuple(members) => self.analysis.identifier_text(span).replace('_', "").parse::<usize>().ok().and_then(|position| members.get(position).map(|member| (position, member))).map(|(position, member)| {
                    if let TypeState::Resolved(result) = member.ty { projection = Some(ProjectionKind::Tuple { declaration, position, result }); }
                    member.ty
                }),
                _ => None,
            },
            _ => None,
        };
        if let Some(state) = state {
            if self.analysis.projection(expression).is_none() && let Some(kind) = projection {
                self.analysis.projections.push(ProjectionResolution { node: expression, kind });
            }
            self.replace_if_changed(expression, state);
        } else if matches!(self.state(expression), TypeState::Deferred(_)) {
            self.error(expression.span, "member is not available on the narrowed alternative");
            self.replace_expression_state(expression, TypeState::Error);
        }
    }

    fn retype_index(&mut self, expression: &'ast Expression, value: &'ast Expression, index: &'ast Expression) {
        let (TypeState::Resolved(value), TypeState::Resolved(index)) = (self.state(value), self.state(index)) else { return; };
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let (result, projection) = match self.analysis.types.get(value).clone() {
            ResolvedType::List(element) if index == int => (Some(element), Some(ProjectionKind::List { element })),
            ResolvedType::Map { key, value } if index == key => (Some(value), Some(ProjectionKind::Map { key, value })),
            ResolvedType::Primitive(PrimitiveType::Str) if index == int => {
                let character = self.analysis.types.primitive(PrimitiveType::Char);
                (Some(character), Some(ProjectionKind::String { result: character }))
            }
            _ => (None, None),
        };
        if let Some(result) = result {
            if self.analysis.projection(expression).is_none() {
                self.analysis.projections.push(ProjectionResolution { node: expression, kind: projection.unwrap() });
            }
            self.replace_if_changed(expression, TypeState::Resolved(result));
        } else if matches!(self.state(expression), TypeState::Deferred(_)) {
            self.error(expression.span, "index operation is not defined for the narrowed alternative");
            self.replace_expression_state(expression, TypeState::Error);
        }
    }

    fn retype_call(&mut self, expression: &'ast Expression, callee: &'ast Expression, arguments: &'ast [Argument]) {
        if let Some(target) = self.analysis.call_resolution(expression).map(|call| call.target) {
            match target {
                CallTarget::Callable(CallableId::Function(function)) => {
                    let parameters = self.analysis.function_signature(function).parameters.iter().map(|parameter| (parameter.node.name.span, parameter.ty)).collect::<Vec<_>>();
                    let mut positional = 0;
                    for argument in arguments {
                        let expected = match &argument.kind {
                            ArgumentKind::Positional(_) => { let value = parameters.get(positional).map(|(_, ty)| *ty); positional += 1; value }
                            ArgumentKind::Named { name, .. } => parameters.iter().find(|(span, _)| self.analysis.identifier_text(*span) == self.analysis.identifier_text(name.span)).map(|(_, ty)| *ty),
                        };
                        if let Some(TypeState::Resolved(expected)) = expected { self.validate_expected_value(argument_value(argument), expected, "argument type does not match parameter type"); }
                    }
                    let result = self.analysis.function_signature(function).result;
                    self.replace_if_changed(expression, result);
                    return;
                }
                CallTarget::Callable(CallableId::Intrinsic(intrinsic)) => {
                    let result = self.analysis.intrinsic_signature(intrinsic).result;
                    self.replace_if_changed(expression, result);
                    return;
                }
                CallTarget::Callable(CallableId::Constructor(_))
                | CallTarget::QualifiedConstructor(_)
                | CallTarget::ErrorConstructor => return,
                CallTarget::Builtin(_)
                | CallTarget::Expression => {}
                CallTarget::Binding(_)
                | CallTarget::AmbiguousErrorConstructor { .. }
                | CallTarget::Ambiguous { .. }
                | CallTarget::Unknown => return,
            }
        }
        let ExpressionKind::Member { value: receiver, member: Member::Named(name) } = &callee.kind else { return; };
        let TypeState::Resolved(receiver_ty) = self.state(receiver) else { return; };
        let spelling = self.analysis.identifier_text(name.span).to_owned();
        let int = self.analysis.types.primitive(PrimitiveType::Int);
        let method = match self.analysis.types.get(receiver_ty) {
            ResolvedType::List(element) => match spelling.as_str() { "append" => Some((BuiltinMethod::ListAppend, Some(*element), TypeState::Resolved(self.analysis.types.unit()))), "removeIndex" => Some((BuiltinMethod::ListRemoveIndex, Some(int), TypeState::Resolved(self.analysis.types.unit()))), "len" => Some((BuiltinMethod::ListLen, None, TypeState::Resolved(int))), _ => None },
            ResolvedType::Map { key, .. } => match spelling.as_str() { "removeKey" => Some((BuiltinMethod::MapRemoveKey, Some(*key), TypeState::Resolved(self.analysis.types.unit()))), "len" => Some((BuiltinMethod::MapLen, None, TypeState::Resolved(int))), _ => None },
            ResolvedType::Primitive(PrimitiveType::Str) if spelling == "len" => Some((BuiltinMethod::StrLen, None, TypeState::Resolved(int))),
            _ => None,
        };
        if let Some((method, expected, result)) = method {
            if let (Some(expected), Some(argument)) = (expected, arguments.first()) {
                self.validate_expected_value(argument_value(argument), expected, "method argument type does not match the narrowed receiver type");
            }
            if let Some(call) = self.analysis.calls.iter_mut().find(|call| std::ptr::eq(call.node, expression)) { call.target = CallTarget::Builtin(method); }
            self.replace_if_changed(expression, result);
        } else if matches!(self.state(expression), TypeState::Deferred(_)) {
            self.error(expression.span, "method is not available on the narrowed alternative");
            self.replace_expression_state(expression, TypeState::Error);
        }
    }

    fn retype_assignment_target(&mut self, target: &'ast AssignmentTarget) {
        if target.suffixes.is_empty() { return; }
        let Some(NameResolution::Binding(root)) = self.analysis.name_use(&target.root).map(|use_| use_.resolution) else { return; };
        let root_type = self.narrowed_type(root).map(TypeState::Resolved).unwrap_or_else(|| self.analysis.binding_type(root));
        let original = self.analysis.assignment_target(target).cloned().expect("assignment target annotation");
        let mut state = root_type;
        let mut steps = Vec::new();
        for suffix in &target.suffixes {
            let TypeState::Resolved(receiver) = state else { break; };
            let step = match (self.analysis.types.get(receiver), &suffix.kind) {
                (ResolvedType::List(element), crate::ast::AssignmentTargetSuffixKind::Index(_)) => { state = TypeState::Resolved(*element); AccessPathStep::ListIndex { suffix, element: *element, state } }
                (ResolvedType::Map { key, value }, crate::ast::AssignmentTargetSuffixKind::Index(_)) => { state = TypeState::Resolved(*value); AccessPathStep::MapIndex { suffix, key: *key, value: *value, state } }
                (ResolvedType::Nominal(declaration), crate::ast::AssignmentTargetSuffixKind::Member(Member::Named(name))) => {
                    let TypeDefinitionKind::Struct(members) = &self.analysis.type_definition(*declaration).kind else { break; };
                    let Some((field, member)) = members.iter().enumerate().find(|(_, member)| self.analysis.identifier_text(member.name.span) == self.analysis.identifier_text(name.span)) else { break; };
                    state = member.ty; AccessPathStep::StructMember { suffix, declaration: *declaration, member: member.node, field, storage: member.storage, state }
                }
                (ResolvedType::Nominal(declaration), crate::ast::AssignmentTargetSuffixKind::Member(Member::TupleIndex(span))) => {
                    let TypeDefinitionKind::Tuple(members) = &self.analysis.type_definition(*declaration).kind else { break; };
                    let Some(position) = self.analysis.identifier_text(*span).replace('_', "").parse::<usize>().ok() else { break; };
                    let Some(member) = members.get(position) else { break; };
                    state = member.ty; AccessPathStep::TupleMember { suffix, declaration: *declaration, member: member.node, position, state }
                }
                _ => break,
            };
            steps.push(step);
        }
        if state != original.state || steps.len() != original.steps.len() {
            let ids = self.analysis.deferred.iter().filter(|deferred| {
                !deferred.resolved
                    && matches!(deferred.node, crate::analysis::AstNode::AssignmentTarget(node) if std::ptr::eq(node, target))
            }).map(|deferred| deferred.id).collect::<Vec<_>>();
            for id in ids { self.analysis.resolve_deferred(id); }
            self.analysis.assignment_targets.push(crate::analysis::AssignmentTargetAnnotation { node: target, root: Some(root), steps: steps.into_boxed_slice(), state });
        }
    }

    fn direct_test(&self, condition: &'ast Expression) -> Option<(BindingId, TypeId)> {
        let condition = strip_expression_parentheses(condition);
        self.tests.iter().rev().find(|test| std::ptr::eq(test.expression, condition)).and_then(|test| test.binding.map(|binding| (binding, test.payload)))
    }

    fn validate_result_value(&mut self, value: &'ast Expression) {
        if let (TypeState::Resolved(actual), TypeState::Resolved(expected)) = (self.state(value), self.function_result)
        {
            if actual == expected { return; }
            if self.try_success_widens_to(value, actual, expected) { return; }
            let alternatives = self.union_parts(TypeState::Resolved(expected)).map(|(_, _, alternatives)| {
                alternatives.into_iter().filter(|alternative| alternative_payload(alternative) == actual).collect::<Vec<_>>()
            }).unwrap_or_default();
            if let [alternative] = alternatives.as_slice() {
                self.analysis.record_union_injection(
                    value,
                    expected,
                    alternative.clone(),
                );
            } else {
                let message = if expected == self.analysis.types.unit() {
                    "unit-returning function cannot return a non-unit value"
                } else {
                    "return value type does not match function result type"
                };
                self.error(value.span, message);
            }
        }
    }

    fn validate_expected_value(&mut self, value: &'ast Expression, expected: TypeId, message: &'static str) {
        let TypeState::Resolved(actual) = self.state(value) else { return; };
        if actual == expected { return; }
        if self.try_success_widens_to(value, actual, expected) { return; }
        let alternatives = self.union_parts(TypeState::Resolved(expected)).map(|(_, _, alternatives)| {
            alternatives.into_iter().filter(|alternative| alternative_payload(alternative) == actual).collect::<Vec<_>>()
        }).unwrap_or_default();
        if let [alternative] = alternatives.as_slice() {
            self.analysis.record_union_injection(value, expected, alternative.clone());
        } else {
            self.error(value.span, message);
        }
    }

    fn state(&self, expression: &'ast Expression) -> TypeState {
        self.analysis.expression_annotation(expression).expect("every expression has a type annotation").state
    }

    fn try_success_widens_to(&self, expression: &'ast Expression, actual: TypeId, expected: TypeId) -> bool {
        let expression = strip_expression_parentheses(expression);
        if !matches!(&expression.kind, ExpressionKind::Try { .. }) {
            return false;
        }
        let ResolvedType::Union(actual_alternatives) = self.analysis.types.get(actual) else {
            return false;
        };
        let expected_alternatives = self.union_parts(TypeState::Resolved(expected))
            .map(|(_, _, alternatives)| alternatives)
            .unwrap_or_default();
        !actual_alternatives.is_empty()
            && actual_alternatives.iter().all(|alternative| expected_alternatives.contains(alternative))
    }

    fn expression_body_state(&self, body: &'ast ExpressionBody) -> TypeState {
        match &body.kind {
            ExpressionBodyKind::Expression(expression) => self.state(expression),
            ExpressionBodyKind::Block(block) => block.value.as_deref().map_or(TypeState::Resolved(self.analysis.types.unit()), |value| self.state(value)),
        }
    }

    fn replace_if_changed(&mut self, expression: &'ast Expression, state: TypeState) {
        if self.state(expression) != state { self.replace_expression_state(expression, state); }
    }

    fn replace_expression_state(&mut self, expression: &'ast Expression, state: TypeState) {
        let ids = self.analysis.deferred.iter().filter(|deferred| {
            !deferred.resolved
                && matches!(deferred.node, crate::analysis::AstNode::Expression(node) if std::ptr::eq(node, expression))
        }).map(|deferred| deferred.id).collect::<Vec<_>>();
        for id in ids { self.analysis.resolve_deferred(id); }
        self.analysis.annotate_expression(expression, state);
    }

    fn resolve_statement_deferred(&mut self, statement: &'ast Statement) {
        let ids = self.analysis.deferred.iter().filter(|deferred| !deferred.resolved && matches!(deferred.node, crate::analysis::AstNode::Statement(node) if std::ptr::eq(node, statement))).map(|deferred| deferred.id).collect::<Vec<_>>();
        for id in ids { self.analysis.resolve_deferred(id); }
    }

    fn narrowed_type(&self, binding: BindingId) -> Option<TypeId> { self.narrowed.iter().rev().find_map(|(candidate, ty)| (*candidate == binding).then_some(*ty)) }
    fn set_narrowing(&mut self, binding: BindingId, ty: TypeId) { self.remove_narrowing(binding); self.narrowed.push((binding, ty)); }
    fn remove_narrowing(&mut self, binding: BindingId) { self.narrowed.retain(|(candidate, _)| *candidate != binding); }
    fn alternative_name(&self, alternative: &UnionAlternative) -> String {
        match alternative {
            UnionAlternative::Tagged { tag, .. } => tag.to_string(),
            UnionAlternative::Error(_) => "Error".to_owned(),
            UnionAlternative::Untagged(ty) => self.type_name(*ty),
        }
    }
    fn type_name(&self, ty: TypeId) -> String {
        match self.analysis.types.get(ty) {
            ResolvedType::Unit => "()".to_owned(),
            ResolvedType::Primitive(primitive) => match primitive { PrimitiveType::Int => "int", PrimitiveType::Float => "float", PrimitiveType::Str => "str", PrimitiveType::Bool => "bool", PrimitiveType::Char => "char" }.to_owned(),
            ResolvedType::Nominal(declaration) => self.analysis.identifier_text(self.analysis.type_definition(*declaration).node.name.span).to_owned(),
            ResolvedType::List(element) => format!("[{}]", self.type_name(*element)),
            ResolvedType::Map { key, value } => format!("{{{}: {}}}", self.type_name(*key), self.type_name(*value)),
            ResolvedType::Union(_) => "union".to_owned(),
        }
    }
    fn error(&mut self, span: Span, message: impl Into<String>) { self.diagnostics.push(Diagnostic::source(self.analysis.source, span, message)); }
}

fn strip_expression_parentheses(mut expression: &Expression) -> &Expression {
    while let ExpressionKind::Parenthesized(inner) = &expression.kind { expression = inner; }
    expression
}

fn strip_type_parentheses(mut ty: &Type) -> &Type {
    while let TypeKind::Parenthesized(inner) = &ty.kind { ty = inner; }
    ty
}

fn exact_binding<'ast>(analysis: &Analysis<'_, 'ast>, expression: &'ast Expression) -> Option<BindingId> {
    let ExpressionKind::Identifier(identifier) = &strip_expression_parentheses(expression).kind else { return None; };
    match analysis.name_use(identifier).map(|use_| use_.resolution) { Some(NameResolution::Binding(binding)) => Some(binding), _ => None }
}

fn union_style_semantic(alternatives: &[UnionAlternative]) -> UnionStyle {
    if alternatives.iter().any(|alternative| matches!(alternative, UnionAlternative::Tagged { .. })) { UnionStyle::Tagged } else { UnionStyle::Untagged }
}

fn direct_assignments_in_body<'ast>(analysis: &Analysis<'_, 'ast>, body: &'ast StatementBody) -> Vec<BindingId> {
    fn block<'ast>(analysis: &Analysis<'_, 'ast>, block: &'ast Block, out: &mut Vec<BindingId>) {
        for statement in &block.statements { statement_assignments(analysis, statement, out); }
    }
    fn body_assignments<'ast>(analysis: &Analysis<'_, 'ast>, body: &'ast StatementBody, out: &mut Vec<BindingId>) {
        match &body.kind {
            StatementBodyKind::Block(value) => block(analysis, value, out),
            StatementBodyKind::Statement(value) => statement_assignments(analysis, value, out),
        }
    }
    fn statement_assignments<'ast>(analysis: &Analysis<'_, 'ast>, statement: &'ast Statement, out: &mut Vec<BindingId>) {
        match &statement.kind {
            StatementKind::Assignment { target, .. } if target.suffixes.is_empty() => {
                if let Some(NameResolution::Binding(binding)) = analysis.name_use(&target.root).map(|use_| use_.resolution)
                    && !out.contains(&binding) { out.push(binding); }
            }
            StatementKind::If { branches, else_body } => {
                for branch in branches { body_assignments(analysis, &branch.body, out); }
                if let Some(body) = else_body { body_assignments(analysis, body, out); }
            }
            StatementKind::While { body, .. } | StatementKind::For { body, .. } => body_assignments(analysis, body, out),
            StatementKind::Switch { arms, else_body, .. } => {
                for arm in arms { body_assignments(analysis, &arm.body, out); }
                if let Some(body) = else_body { body_assignments(analysis, body, out); }
            }
            StatementKind::Block(value) => block(analysis, value, out),
            _ => {}
        }
    }
    let mut out = Vec::new();
    body_assignments(analysis, body, &mut out);
    out
}

fn statement_body_may_fallthrough(body: &StatementBody) -> bool {
    fn block_may_fallthrough(block: &Block) -> bool {
        block.statements.iter().all(statement_may_fallthrough)
    }
    fn statement_may_fallthrough(statement: &Statement) -> bool {
        match &statement.kind {
            StatementKind::Return(_) | StatementKind::Break | StatementKind::Continue => false,
            StatementKind::Block(block) => block_may_fallthrough(block),
            StatementKind::If { branches, else_body: Some(else_body) } => {
                branches.iter().any(|branch| statement_body_may_fallthrough(&branch.body))
                    || statement_body_may_fallthrough(else_body)
            }
            _ => true,
        }
    }
    match &body.kind {
        StatementBodyKind::Block(block) => block_may_fallthrough(block),
        StatementBodyKind::Statement(statement) => statement_may_fallthrough(statement),
    }
}

struct FlowValidator<'analysis, 'diagnostics, 'warnings, 'source, 'ast> {
    analysis: &'analysis mut Analysis<'source, 'ast>,
    diagnostics: &'diagnostics mut Diagnostics,
    warnings: &'warnings mut Warnings,
    facts: FlowFacts<'ast>,
    function: Option<(FunctionId, TypeState)>,
    loops: Vec<&'ast Statement>,
    switches: &'analysis [SwitchResolution<'ast>],
    tries: &'analysis [TryResolution<'ast>],
}

impl<'analysis, 'diagnostics, 'warnings, 'source, 'ast>
    FlowValidator<'analysis, 'diagnostics, 'warnings, 'source, 'ast>
{
    fn new(
        analysis: &'analysis mut Analysis<'source, 'ast>,
        diagnostics: &'diagnostics mut Diagnostics,
        warnings: &'warnings mut Warnings,
        switches: &'analysis [SwitchResolution<'ast>],
        tries: &'analysis [TryResolution<'ast>],
    ) -> Self {
        Self {
            analysis,
            diagnostics,
            warnings,
            facts: FlowFacts::default(),
            function: None,
            loops: Vec::new(),
            switches,
            tries,
        }
    }

    fn validate(mut self) -> FlowFacts<'ast> {
        let functions = self
            .analysis
            .function_signatures
            .iter()
            .map(|signature| (signature.id, signature.node, signature.result))
            .collect::<Vec<_>>();
        for (id, function, result) in functions {
            self.validate_function(id, function, result);
        }
        self.facts
    }

    fn validate_function(
        &mut self,
        id: FunctionId,
        function: &'ast FunctionDeclaration,
        result: TypeState,
    ) {
        self.function = Some((id, result));
        debug_assert!(self.loops.is_empty());
        let body_flow = self.block(&function.body);
        let final_state = function
            .body
            .value
            .as_deref()
            .map_or(TypeState::Resolved(self.analysis.types.unit()), |value| self.expression_state(value));
        if let (Some(value), TypeState::Deferred(_)) =
            (function.body.value.as_deref(), final_state)
        {
            self.facts.deferred.push(DeferredFlow {
                kind: DeferredFlowKind::FunctionFinalValue {
                    function: id,
                    body: &function.body,
                    value,
                    declared_result: result,
                },
                switches: body_flow.switch_dependencies.clone(),
            });
        }

        match result {
            TypeState::Resolved(result_type) => {
                let implicitly_returns = function.body.value.is_some()
                    && matches!(final_state, TypeState::Resolved(_) | TypeState::Deferred(_) | TypeState::Error);
                if body_flow.can_fallthrough() && !implicitly_returns {
                    let unit_injection = self.unit_alternative(result_type);
                    if result_type == self.analysis.types.unit() || unit_injection.is_some() {
                        self.facts.completions.push(FunctionCompletion {
                            function: id,
                            body: &function.body,
                            unit_injection,
                        });
                        self.function = None;
                        return;
                    }
                    match body_flow.fallthrough {
                        FallthroughKind::Definite => self.error(
                            closing_brace_span(&function.body),
                            "value-returning function may fall through without returning a value",
                        ),
                        FallthroughKind::SwitchDependent => {
                            self.facts.deferred.push(DeferredFlow {
                                kind: DeferredFlowKind::FunctionReturn {
                                    function: id,
                                    body: &function.body,
                                },
                                switches: body_flow.switch_dependencies.clone(),
                            });
                        }
                        FallthroughKind::None => {}
                    }
                }
            }
            _ => panic!("semantic analysis received an invalid function result type"),
        }
        self.function = None;
    }

    fn block(&mut self, block: &'ast Block) -> FlowSummary<'ast> {
        let mut flow = FlowSummary::fallthrough();
        let mut unreachable_region = false;
        let mut deferred_dependencies: Vec<&'ast Statement> = Vec::new();
        for statement in &block.statements {
            let statement_flow = self.statement(statement);
            match flow.fallthrough {
                FallthroughKind::None => {
                    if !unreachable_region {
                        self.warn(statement.span);
                        unreachable_region = true;
                    }
                }
                FallthroughKind::SwitchDependent => {
                    if !same_switches(&deferred_dependencies, &flow.switch_dependencies) {
                        self.defer_reachability(
                            FlowSubject::Statement(statement),
                            &flow.switch_dependencies,
                        );
                        deferred_dependencies = flow.switch_dependencies.to_vec();
                    }
                    flow = flow.then(statement_flow);
                }
                FallthroughKind::Definite => {
                    deferred_dependencies.clear();
                    flow = flow.then(statement_flow);
                }
            }
        }
        if let Some(value) = block.value.as_deref() {
            let value_flow = self.expression(value);
            match flow.fallthrough {
                FallthroughKind::None => {
                    if !unreachable_region {
                        self.warn(value.span);
                    }
                }
                FallthroughKind::SwitchDependent => {
                    if !same_switches(&deferred_dependencies, &flow.switch_dependencies) {
                        self.defer_reachability(
                            FlowSubject::Expression(value),
                            &flow.switch_dependencies,
                        );
                    }
                    flow = flow.then(value_flow);
                }
                FallthroughKind::Definite => flow = flow.then(value_flow),
            }
        }
        self.record(FlowSubject::Block(block), flow.clone());
        flow
    }

    fn statement(&mut self, statement: &'ast Statement) -> FlowSummary<'ast> {
        let flow = match &statement.kind {
            StatementKind::Local { initializer, .. } => {
                let flow = self.expression(initializer);
                let state = self.expression_state(initializer);
                let binding = self
                    .analysis
                    .local_binding(statement)
                    .expect("local statement has a stable binding identity");
                if self.analysis.binding_types[binding.index()].state == TypeState::Never
                    && matches!(state, TypeState::Resolved(_))
                {
                    self.analysis.binding_types[binding.index()].state = state;
                }
                flow
            }
            StatementKind::Assignment { target, value, .. } => {
                let mut flow = FlowSummary::fallthrough();
                for suffix in &target.suffixes {
                    if let crate::ast::AssignmentTargetSuffixKind::Index(index) = &suffix.kind {
                        flow = flow.then(self.expression(index));
                    }
                }
                flow.then(self.expression(value))
            }
            StatementKind::Expression(expression) => self.expression(expression),
            StatementKind::Return(value) => self.return_statement(statement, value.as_ref()),
            StatementKind::Break => self.loop_control(statement, FlowFlags::BREAK),
            StatementKind::Continue => self.loop_control(statement, FlowFlags::CONTINUE),
            StatementKind::If { branches, else_body } => {
                let mut remaining = FlowSummary::fallthrough();
                let mut alternatives = Vec::new();
                for branch in branches {
                    remaining = remaining.then(self.expression(&branch.condition));
                    alternatives.push(remaining.clone().then(self.statement_body(&branch.body)));
                }
                if let Some(body) = else_body {
                    alternatives.push(remaining.then(self.statement_body(body)));
                } else {
                    alternatives.push(remaining);
                }
                FlowSummary::alternatives(alternatives)
            }
            StatementKind::While { condition, body } => {
                let condition = self.expression(condition);
                self.loops.push(statement);
                let body = self.statement_body(body);
                self.loops.pop();
                self.loop_flow(condition, body)
            }
            StatementKind::For { iterable, body, .. } => {
                let iterable = self.expression(iterable);
                self.loops.push(statement);
                let body = self.statement_body(body);
                self.loops.pop();
                self.loop_flow(iterable, body)
            }
            StatementKind::Switch { value, arms, else_body } => {
                let value = self.expression(value);
                let mut alternatives = Vec::new();
                for arm in arms {
                    let arm_flow = self.statement_body(&arm.body);
                    self.record(FlowSubject::SwitchArm(arm), arm_flow.clone());
                    alternatives.push(arm_flow);
                }
                if let Some(body) = else_body {
                    alternatives.push(self.statement_body(body));
                } else if !self.switch_is_exhaustive(statement) {
                    // Until Phase 4 proves coverage, the unmatched path remains
                    // conservatively reachable. It is switch-dependent rather
                    // than definite so nested unresolved switches do not cause
                    // premature function-fallthrough errors.
                    alternatives.push(FlowSummary::dependent(vec![statement]));
                }
                value.then(FlowSummary::alternatives(alternatives))
            }
            StatementKind::Block(block) => self.block(block),
        };
        self.record(FlowSubject::Statement(statement), flow.clone());
        flow
    }

    fn return_statement(
        &mut self,
        statement: &'ast Statement,
        value: Option<&'ast Expression>,
    ) -> FlowSummary<'ast> {
        let (function, result) = self
            .function
            .expect("return validation requires an enclosing function");
        let unit = self.analysis.types.unit();
        let value_state = value.map_or(TypeState::Resolved(unit), |value| self.expression_state(value));
        let unit_injection = match result {
            TypeState::Resolved(result_type) if value.is_none() && result_type == unit => None,
            TypeState::Resolved(result_type) if value.is_none() => self.unit_alternative(result_type),
            _ => None,
        };
        let valid = match (result, value) {
            (TypeState::Resolved(result_type), None) => {
                if result_type == unit || unit_injection.is_some() {
                    true
                } else {
                    self.error(statement.span, "value-returning function requires a return value");
                    false
                }
            }
            (TypeState::Resolved(_), Some(_)) => true,
            _ => panic!("semantic analysis received an invalid function result type"),
        };
        if valid {
            self.facts.returns.push(ExplicitReturn {
                statement,
                function,
                value,
                value_state,
                unit_injection,
            });
        }
        let evaluated = value.map_or_else(FlowSummary::fallthrough, |value| self.expression(value));
        if evaluated.can_fallthrough() {
            let mut returned = evaluated;
            returned.flags.remove(FlowFlags::FALLTHROUGH);
            returned.flags.insert(FlowFlags::RETURN);
            returned.fallthrough = FallthroughKind::None;
            returned.switch_dependencies = Box::new([]);
            returned
        } else {
            evaluated
        }
    }

    fn loop_control(
        &mut self,
        statement: &'ast Statement,
        exit: FlowFlags,
    ) -> FlowSummary<'ast> {
        if let Some(target) = self.loops.last().copied() {
            self.facts.loop_controls.push(LoopControl { statement, target });
            FlowSummary::terminal(exit)
        } else {
            let keyword = if exit == FlowFlags::BREAK { "break" } else { "continue" };
            self.error(
                statement.span,
                format!("'{keyword}' is only valid inside a loop"),
            );
            FlowSummary::fallthrough()
        }
    }

    fn loop_flow(
        &self,
        condition_or_iterable: FlowSummary<'ast>,
        body: FlowSummary<'ast>,
    ) -> FlowSummary<'ast> {
        if !condition_or_iterable.can_fallthrough() {
            return condition_or_iterable;
        }
        let mut flags = condition_or_iterable.flags;
        flags.remove(FlowFlags::FALLTHROUGH);
        if body.flags.contains(FlowFlags::RETURN) {
            flags.insert(FlowFlags::RETURN);
        }
        if body.flags.contains(FlowFlags::DIVERGE) {
            flags.insert(FlowFlags::DIVERGE);
        }
        flags.insert(FlowFlags::FALLTHROUGH);
        FlowSummary {
            flags,
            fallthrough: condition_or_iterable.fallthrough,
            switch_dependencies: condition_or_iterable.switch_dependencies,
        }
    }

    fn statement_body(&mut self, body: &'ast StatementBody) -> FlowSummary<'ast> {
        let flow = match &body.kind {
            StatementBodyKind::Block(block) => self.block(block),
            StatementBodyKind::Statement(statement) => self.statement(statement),
        };
        self.record(FlowSubject::StatementBody(body), flow.clone());
        flow
    }

    fn expression_body(&mut self, body: &'ast ExpressionBody) -> FlowSummary<'ast> {
        let flow = match &body.kind {
            ExpressionBodyKind::Block(block) => self.block(block),
            ExpressionBodyKind::Expression(expression) => self.expression(expression),
        };
        self.record(FlowSubject::ExpressionBody(body), flow.clone());
        flow
    }

    fn expression(&mut self, expression: &'ast Expression) -> FlowSummary<'ast> {
        let flow = match &expression.kind {
            ExpressionKind::Unit
            | ExpressionKind::Identifier(_)
            | ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_)
            | ExpressionKind::TypedEmptyList(_)
            | ExpressionKind::TypedEmptyMap(_) => FlowSummary::fallthrough(),
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Conversion { operand: inner, .. }
            | ExpressionKind::Unary { operand: inner, .. } => self.expression(inner),
            ExpressionKind::Try { value: inner, .. } => {
                let mut flow = self.expression(inner);
                if flow.can_fallthrough()
                    && let Some(resolution) = self.tries.iter().find(|resolution| std::ptr::eq(resolution.expression, expression))
                {
                    match &resolution.action {
                        TryAction::Propagate { .. } => flow.flags.insert(FlowFlags::RETURN),
                        TryAction::Panic { .. } => flow.flags.insert(FlowFlags::DIVERGE),
                    }
                }
                flow
            }
            ExpressionKind::List(elements) => self.expression_sequence(elements.iter()),
            ExpressionKind::Map(entries) => self.expression_sequence(
                entries.iter().flat_map(|entry| [&entry.key, &entry.value]),
            ),
            ExpressionKind::Block(block) => self.block(block),
            ExpressionKind::If { branches, else_branch } => {
                let mut remaining = FlowSummary::fallthrough();
                let mut alternatives = Vec::new();
                for branch in branches {
                    remaining = remaining.then(self.expression(&branch.condition));
                    alternatives.push(remaining.clone().then(self.expression_body(&branch.body)));
                }
                alternatives.push(remaining.then(self.expression_body(else_branch)));
                FlowSummary::alternatives(alternatives)
            }
            ExpressionKind::Binary { left, operator, right, .. }
                if matches!(*operator, BinaryOperator::LogicalAnd | BinaryOperator::LogicalOr) =>
            {
                let left_flow = self.expression(left);
                let right_flow = self.expression(right);
                let evaluated_right = left_flow.clone().then(right_flow);
                let flow = FlowSummary::alternatives([left_flow.clone(), evaluated_right]);
                let bool_type = self.analysis.types.primitive(PrimitiveType::Bool);
                if left_flow.can_fallthrough()
                    && flow.can_fallthrough()
                    && self.expression_state(left) == TypeState::Resolved(bool_type)
                    && self.expression_state(expression) == TypeState::Never
                {
                    self.analysis
                        .annotate_expression(expression, TypeState::Resolved(bool_type));
                }
                flow
            }
            ExpressionKind::Binary { left, right, .. } => {
                self.expression(left).then(self.expression(right))
            }
            ExpressionKind::Is { value, .. } => self.expression(value),
            ExpressionKind::Call { callee, arguments } => {
                let mut flow = self.expression(callee);
                for argument in arguments {
                    flow = flow.then(self.expression(argument_value(argument)));
                }
                if flow.can_fallthrough()
                    && matches!(
                        self.analysis.call_resolution(expression).map(|call| call.target),
                        Some(CallTarget::Callable(CallableId::Intrinsic(
                            crate::analysis::IntrinsicId::Panic
                        )))
                    )
                {
                    flow.flags.remove(FlowFlags::FALLTHROUGH);
                    flow.flags.insert(FlowFlags::DIVERGE);
                    flow.fallthrough = FallthroughKind::None;
                    flow.switch_dependencies = Box::new([]);
                }
                flow
            }
            ExpressionKind::Index { value, index } => {
                self.expression(value).then(self.expression(index))
            }
            ExpressionKind::Member { value, .. } => self.expression(value),
        };
        self.record(FlowSubject::Expression(expression), flow.clone());
        flow
    }

    fn expression_sequence(
        &mut self,
        expressions: impl Iterator<Item = &'ast Expression>,
    ) -> FlowSummary<'ast> {
        expressions.fold(FlowSummary::fallthrough(), |flow, expression| {
            flow.then(self.expression(expression))
        })
    }

    fn expression_state(&self, expression: &'ast Expression) -> TypeState {
        self.analysis
            .expression_annotation(expression)
            .expect("every expression has a name-and-type annotation")
            .state
    }

    fn switch_is_exhaustive(&self, statement: &Statement) -> bool {
        self.switches
            .iter()
            .find(|resolution| std::ptr::eq(resolution.statement, statement))
            .is_some_and(|resolution| resolution.exhaustive)
    }

    fn unit_alternative(&self, ty: TypeId) -> Option<UnionAlternative> {
        let unit = self.analysis.types.unit();
        let alternatives = match self.analysis.types.get(ty) {
            ResolvedType::Union(alternatives) => alternatives.as_ref(),
            ResolvedType::Nominal(declaration) => match &self.analysis.type_definition(*declaration).kind {
                TypeDefinitionKind::Union { alternatives, .. } => alternatives.as_ref(),
                _ => return None,
            },
            _ => return None,
        };
        let matches = alternatives.iter().filter(|alternative| matches!(alternative, UnionAlternative::Untagged(payload) if *payload == unit)).cloned().collect::<Vec<_>>();
        match matches.as_slice() { [only] => Some(only.clone()), _ => None }
    }

    fn record(&mut self, subject: FlowSubject<'ast>, summary: FlowSummary<'ast>) {
        self.facts.summaries.push(FlowRecord { subject, summary });
    }

    fn defer_reachability(
        &mut self,
        construct: FlowSubject<'ast>,
        switches: &[&'ast Statement],
    ) {
        self.facts.deferred.push(DeferredFlow {
            kind: DeferredFlowKind::Reachability { construct },
            switches: switches.into(),
        });
    }

    fn error(&mut self, span: Span, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::source(self.analysis.source, span, message));
    }

    fn warn(&mut self, span: Span) {
        self.warnings.push(Diagnostic::source_warning(
            self.analysis.source,
            span,
            "unreachable source",
        ));
    }
}

fn closing_brace_span(block: &Block) -> Span {
    Span::new(block.span.end.saturating_sub(1), block.span.end)
}

fn same_switches(left: &[&Statement], right: &[&Statement]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| std::ptr::eq(*left, *right))
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
    subjects: Vec<MutabilitySubject<'ast>>,
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
            subjects: Vec::new(),
        }
    }

    fn validate(mut self) -> (Vec<MutabilityResolution<'ast>>, Vec<DeferredMutability<'ast>>, Vec<MutabilitySubject<'ast>>) {
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
        self.subjects.sort_by_key(|subject| mutability_subject_span(*subject));
        (self.resolutions, self.deferred, self.subjects)
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
            ExpressionKind::Unit
            | ExpressionKind::Identifier(_)
            | ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_)
            | ExpressionKind::TypedEmptyList(_)
            | ExpressionKind::TypedEmptyMap(_) => {}
            ExpressionKind::Parenthesized(inner)
            | ExpressionKind::Conversion { operand: inner, .. }
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
        self.subjects.push(MutabilitySubject::Assignment(target));
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
                let subject = MutabilitySubject::Receiver { call, receiver };
                self.subjects.push(subject);
                self.authorize_place(
                    subject,
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
                        let subject = MutabilitySubject::Argument { call, argument };
                        self.subjects.push(subject);
                        self.authorize_place(
                            subject,
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
        let subject = MutabilitySubject::Receiver { call, receiver };
        self.subjects.push(subject);
        self.deferred.push(DeferredMutability {
            subject,
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
            ResolvedType::Unit | ResolvedType::Primitive(_) => false,
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
            "fn main() int { 0 }",
            "fn main(args [str]) {}",
            "fn main(args [str]) int { 0 }",
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
    fn resolves_flow_dependent_mutability_obligations() {
        with_semantic_result(
            "type Box(value int); type U(Box | int); fn main() { var value := U(Box(value = 1)); if value is Box: value.value = 2; }",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert!(result.deferred_mutability.is_empty());
                assert_eq!(result.mutability.len(), 1);
                assert!(matches!(
                    result.mutability[0].operation,
                    MutabilityOperation::ReplaceStructField
                ));
            },
        );
        with_semantic_result(
            "type Choice([int] | int); fn main() { var value := Choice([1]); if value is [int]: value.append(2); }",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert!(result.deferred_mutability.is_empty());
                assert_eq!(result.mutability.len(), 1);
                assert!(matches!(
                    result.mutability[0].operation,
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
    fn validates_explicit_implicit_branching_and_diverging_returns() {
        for text in [
            "fn answer() int { return 1; } fn main() {}",
            "fn answer() int { 1 } fn main() {}",
            "fn answer() int { if true: return 1; else: return 2; } fn main() {}",
            "fn answer() int { panic(\"stop\"); } fn main() {}",
            "fn main() { return; }",
        ] {
            with_semantic_result(text, |result| {
                assert!(result.diagnostics.is_empty(), "{text}: {}", result.diagnostics);
            });
        }

        for (text, expected) in [
            (
                "fn answer() int { return; } fn main() {}",
                "requires a return value",
            ),
            (
                "fn helper() { return 1; } fn main() {}",
                "cannot return a non-unit value",
            ),
            (
                "fn answer() int {} fn main() {}",
                "may fall through",
            ),
            (
                "fn helper() { 1 } fn main() {}",
                "cannot return a non-unit value",
            ),
        ] {
            with_semantic_result(text, |result| {
                assert!(
                    result.diagnostics.to_string().contains(expected),
                    "{text}: {}",
                    result.diagnostics
                );
            });
        }
    }

    #[test]
    fn incompatible_return_values_remain_name_and_type_errors() {
        let source = source("fn answer() int { return \"wrong\"; } fn main() {}");
        let program = parser::parse(&source).unwrap();
        let analysis = analysis::analyze(&source, &program);
        assert!(
            analysis
                .diagnostics
                .to_string()
                .contains("expression type does not match expected type")
        );
    }

    #[test]
    fn records_returns_and_resolves_loop_control_to_the_nearest_loop() {
        with_semantic_result(
            "fn helper() int { while true { while true { continue; } break; } return 1; } fn main() {}",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.flow.returns.len(), 1);
                assert_eq!(result.flow.loop_controls.len(), 2);
                assert!(!std::ptr::eq(
                    result.flow.loop_controls[0].target,
                    result.flow.loop_controls[1].target,
                ));
            },
        );

        for keyword in ["break", "continue"] {
            let text = format!("fn main() {{ {keyword}; }}");
            with_semantic_result(&text, |result| {
                assert!(
                    result
                        .diagnostics
                        .to_string()
                        .contains("only valid inside a loop"),
                    "{}",
                    result.diagnostics
                );
            });
        }

        with_semantic_result(
            "type Choice(int | str); fn main() { value := Choice(1); switch value { int: break; else: continue; } }",
            |result| {
                assert_eq!(
                    result
                        .diagnostics
                        .to_string()
                        .matches("only valid inside a loop")
                        .count(),
                    2
                );
            },
        );

        with_semantic_result(
            "type Choice(int | str); fn main() { value := Choice(1); while true { switch value { int: break; else: continue; } } }",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.flow.loop_controls.len(), 2);
                assert!(std::ptr::eq(
                    result.flow.loop_controls[0].target,
                    result.flow.loop_controls[1].target,
                ));
            },
        );
    }

    #[test]
    fn warns_once_for_each_contiguous_unreachable_region_and_keeps_validating() {
        with_semantic_result(
            concat!(
                "fn helper() int { return 1; first := 1; second := 2; } ",
                "fn looped() { while true { break; after_break := 1; } ",
                "while true { continue; after_continue := 1; } } ",
                "fn main() { panic(\"stop\"); after_panic := 1; return 2; }"
            ),
            |result| {
                assert_eq!(result.warnings.len(), 4, "{}", result.warnings);
                assert!(
                    result.diagnostics.to_string().contains("cannot return a non-unit value"),
                    "{}",
                    result.diagnostics
                );
            },
        );

        with_semantic_result(
            "fn answer() int { return 1; 2 } fn main() {}",
            |result| {
                assert_eq!(result.warnings.len(), 1);
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
            },
        );

        with_semantic_result(
            "fn main() { return; { entered := 1; return; inner_unreachable := 2; } }",
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.warnings.len(), 2, "{}", result.warnings);
            },
        );
    }

    #[test]
    fn caps_real_unreachable_warnings_independently() {
        let mut text = String::new();
        for index in 0..25 {
            text.push_str(&format!(
                "fn helper{index}() {{ return; value := {index}; }} "
            ));
        }
        text.push_str("fn main() {}");
        with_semantic_result(&text, |result| {
            assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
            assert_eq!(result.warnings.len(), crate::diagnostic::MAX_SOURCE_WARNINGS);
        });
    }

    #[test]
    fn recomposes_switch_dependent_return_and_reachability_obligations() {
        with_semantic_result(
            concat!(
                "type Choice(int | str); ",
                "fn answer(value Choice) int { switch value { int: return 1; str: return 2; } } ",
                "fn main() {}"
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert!(result.flow.deferred.is_empty());
            },
        );

        with_semantic_result(
            concat!(
                "type Choice(int | str); ",
                "fn helper(value Choice) { switch value { int: return; str: return; } after := 1; } ",
                "fn main() {}"
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.warnings.len(), 1, "{}", result.warnings);
                assert!(result.flow.deferred.is_empty());
            },
        );

        with_semantic_result(
            concat!(
                "type Choice(int | str); ",
                "fn answer(value Choice) int { switch value { int: return 1; else: return 2; } } ",
                "fn main() {}"
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert!(result.flow.deferred.is_empty());
            },
        );

        with_semantic_result(
            concat!(
                "type Choice(int | str); ",
                "fn answer(first Choice, second Choice) int { ",
                "switch first { int: switch second { int: return 1; str: return 2; } str: return 3; } } ",
                "fn main() {}"
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert!(result.flow.deferred.is_empty());
            },
        );

        with_semantic_result(
            concat!(
                "type Choice(int | str); ",
                "fn helper(first Choice, second Choice) { ",
                "switch first { int: return; str: return; } ",
                "switch second { int: return; str: return; } after := 1; } ",
                "fn main() {}"
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.warnings.len(), 1, "{}", result.warnings);
                assert!(result.flow.deferred.is_empty());
            },
        );
    }

    #[test]
    fn resolves_outer_function_values_after_union_analysis() {
        with_semantic_result(
            concat!(
                "type Choice(int | str); ",
                "fn predicate(value Choice) bool { value is int } ",
                "fn invalid_after_resolution(value Choice) { value is int } ",
                "fn main() {}"
            ),
            |result| {
                assert!(result.diagnostics.to_string().contains("unit-returning function cannot return a non-unit value"));
                assert_eq!(
                    result
                        .flow
                        .deferred
                        .iter()
                        .filter(|obligation| matches!(
                            obligation.kind,
                            DeferredFlowKind::FunctionFinalValue { .. }
                        ))
                        .count(),
                    0
                );
            },
        );
    }

    #[test]
    fn short_circuit_flow_keeps_a_boolean_result_when_only_the_right_diverges() {
        let source = source("fn main() { value := true && panic(\"stop\"); }");
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let result = analyze(&mut analysis);
        assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
        let crate::ast::Declaration::Function(main) = &program.declarations[0] else {
            unreachable!()
        };
        let StatementKind::Local { initializer, .. } = &main.body.statements[0].kind else {
            unreachable!()
        };
        let bool_type = analysis.types.primitive(PrimitiveType::Bool);
        assert_eq!(
            analysis.expression_annotation(initializer).unwrap().state,
            TypeState::Resolved(bool_type)
        );
        assert!(result.flow.summaries.iter().any(|record| {
            matches!(record.subject, FlowSubject::Expression(node) if std::ptr::eq(node, initializer))
                && record.summary.flags.contains(FlowFlags::FALLTHROUGH)
                && record.summary.flags.contains(FlowFlags::DIVERGE)
        }));
    }

    #[test]
    fn resolves_union_tests_and_exhaustive_switches_with_narrowed_reads() {
        with_semantic_result(
            concat!(
                "type Box(value int); type Choice(Box | str); ",
                "fn read(value Choice) int { switch value { Box: return value.value; str: return 0; } } ",
                "fn main() {}",
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.switches.len(), 1);
                assert!(result.switches[0].exhaustive);
                assert_eq!(result.switches[0].arms.len(), 2);
                assert!(result.flow.deferred.is_empty());
            },
        );

        with_semantic_result(
            concat!(
                "type Result(int | Error(str)); ",
                "fn failed(value Result) bool { value is Error } fn main() {}",
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.union_tests.len(), 1);
                assert!(matches!(result.union_tests[0].alternative, UnionAlternative::Error(_)));
            },
        );
    }

    #[test]
    fn diagnoses_contextual_switch_labels_and_coverage() {
        with_semantic_result(
            concat!(
                "type Choice(A(int) | B(str)); ",
                "fn inspect(value Choice) { switch value { A: return; A: return; Missing: return; } } ",
                "fn main() {}",
            ),
            |result| {
                let diagnostics = result.diagnostics.to_string();
                assert!(diagnostics.contains("covered more than once"), "{diagnostics}");
                assert!(diagnostics.contains("unknown union tag 'Missing'"), "{diagnostics}");
                assert!(diagnostics.contains("non-exhaustive switch; missing: B"), "{diagnostics}");
            },
        );
    }

    #[test]
    fn warns_when_else_follows_complete_explicit_coverage() {
        with_semantic_result(
            concat!(
                "type Choice(int | str); fn inspect(value Choice) { ",
                "switch value { int: return; str: return; else: return; } } fn main() {}",
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.warnings.len(), 1, "{}", result.warnings);
                assert!(result.switches[0].exhaustive);
            },
        );
    }

    #[test]
    fn expression_flow_is_ordered_but_later_children_are_still_validated() {
        with_semantic_result(
            concat!(
                "fn main() { values := [panic(\"stop\"), { return; 1 }]; ",
                "after := 2; }"
            ),
            |result| {
                assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
                assert_eq!(result.flow.returns.len(), 1);
                assert_eq!(result.warnings.len(), 2, "{}", result.warnings);
            },
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

    #[test]
    fn resolves_postfix_try_success_propagation_panic_and_chaining() {
        let source = source(concat!(
            "type Inner(int | Error(str)); type Outer(Inner | Error(str)); ",
            "type Many(int | str | Error(bool)); ",
            "fn propagate(value Inner) int | Error(str) { value? } ",
            "fn main() () { inner := Inner(1); first := inner?; ",
            "outer := Outer(Inner(2)); second := outer??; ",
            "many := Many(3); choice := many?; }",
        ));
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let result = analyze(&mut analysis);
        assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
        assert_eq!(result.tries.len(), 5);
        assert!(matches!(&result.tries[0].action, TryAction::Propagate { .. }));
        assert!(result.tries[1..].iter().all(|resolution| matches!(&resolution.action, TryAction::Panic { .. })));
        assert!(matches!(analysis.types.get(result.tries.last().unwrap().success), ResolvedType::Union(alternatives) if alternatives.len() == 2));
        assert!(analysis.deferred.iter().all(|deferred| deferred.resolved));
    }

    #[test]
    fn diagnoses_invalid_try_operands_and_error_payload_mismatches_once() {
        with_semantic_result(
            concat!(
                "type Failed(int | Error(str)); ",
                "fn mismatch(value Failed) int | Error(bool) { value? } ",
                "fn main() { plain := 1?; }",
            ),
            |result| {
                let diagnostics = result.diagnostics.to_string();
                assert!(diagnostics.contains("Error payload does not match"), "{diagnostics}");
                assert!(diagnostics.contains("operand must be a union containing Error"), "{diagnostics}");
            },
        );
    }

    #[test]
    fn completed_semantic_results_pass_the_handoff_gate() {
        for text in [
            "fn main() {}",
            "fn main() int { var value := 1; value = 2; return value; }",
            concat!(
                "type Choice(int | str); fn inspect(value Choice) { ",
                "if value is int: println(value); ",
                "switch value { int: return; str: return; } } fn main() {}"
            ),
            concat!(
                "type Result(int | Error(str)); ",
                "fn pass(value Result) int | Error(str) { value? } ",
                "fn main() { value := Result(1); println(value?); }"
            ),
        ] {
            let source = source(text);
            let program = parser::parse(&source).unwrap();
            let mut analysis = analysis::analyze(&source, &program);
            assert!(analysis.diagnostics.is_empty(), "{text}: {}", analysis.diagnostics);
            let result = analyze(&mut analysis);
            assert!(result.diagnostics.is_empty(), "{text}: {}", result.diagnostics);
            validate_handoff(&analysis, &result).unwrap_or_else(|error| panic!("{text}: {error}"));
        }
    }

    #[test]
    fn handoff_gate_reports_corruptions_without_panicking() {
        let source = source("fn main() { var value := 1; value = 2; }");
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        let mut result = analyze(&mut analysis);
        assert!(result.diagnostics.is_empty());

        result.entry_point = None;
        assert!(validate_handoff(&analysis, &result).unwrap_err().to_string().contains("compiler error"));

        result.entry_point = Some(EntryPoint { function: analysis.function_by_name("main").unwrap() });
        result.flow.summaries.push(result.flow.summaries[0].clone());
        assert!(validate_handoff(&analysis, &result).unwrap_err().to_string().contains("flow summary"));
        result.flow.summaries.pop();

        let expression = program.declarations.iter().find_map(|declaration| match declaration {
                Declaration::Function(function) => function.body.statements.first().and_then(|statement| match &statement.kind {
                    StatementKind::Local { initializer, .. } => Some(initializer),
                    _ => None,
                }),
                Declaration::Type(_) => None,
            }).unwrap();
        analysis.defer(
            AstNode::Expression(expression),
            crate::analysis::DeferredReason::FlowDependentType,
        );
        assert!(validate_handoff(&analysis, &result).unwrap_err().to_string().contains("deferred"));
    }

    #[test]
    fn handoff_gate_requires_contextual_mutation_loop_and_flow_facts() {
        fn corrupted(
            text: &str,
            mutate: impl FnOnce(&mut SemanticResult<'_>),
        ) -> String {
            let source = source(text);
            let program = parser::parse(&source).unwrap();
            let mut analysis = analysis::analyze(&source, &program);
            let mut result = analyze(&mut analysis);
            assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
            mutate(&mut result);
            validate_handoff(&analysis, &result).unwrap_err().to_string()
        }

        let union_source = concat!(
            "type Choice(int | str); fn inspect(value Choice) { ",
            "if value is int: println(value); ",
            "switch value { int: return; str: return; } } fn main() {}"
        );
        assert!(corrupted(union_source, |result| result.union_tests.clear()).contains("union test"));
        assert!(corrupted(union_source, |result| result.switches.clear()).contains("switch"));
        assert!(corrupted(union_source, |result| result.switches[0].exhaustive = false).contains("switch"));
        assert!(corrupted(union_source, |result| {
            result.union_tests[0].alternative = result.switches[0].arms[1].alternative.clone();
        }).contains("union test"));

        let try_source = concat!(
            "type Result(int | Error(str)); ",
            "fn pass(value Result) int | Error(str) { value? } fn main() {}"
        );
        assert!(corrupted(try_source, |result| result.tries.clear()).contains("try"));
        assert!(corrupted(try_source, |result| {
            let function = match &result.tries[0].action {
                TryAction::Propagate { function, .. } => *function,
                TryAction::Panic { .. } => unreachable!(),
            };
            result.tries[0].action = TryAction::Panic { function };
        }).contains("try"));

        assert!(corrupted(
            "fn main() { var value := 1; value = 2; }",
            |result| result.mutability.clear(),
        ).contains("mutation"));
        assert!(corrupted(
            "fn main() { var values := [1]; values.append(2); }",
            |result| result.mutability.clear(),
        ).contains("mutation"));
        assert!(corrupted(
            "type Box(value int); fn touch(var value Box) {} fn main() { var value := Box(value = 1); touch(value); }",
            |result| result.mutability.clear(),
        ).contains("mutation"));
        assert!(corrupted(
            "fn main() { while true { break; } }",
            |result| result.flow.loop_controls.clear(),
        ).contains("loop-control"));
        assert!(corrupted(
            "fn main() { while true { break; } }",
            |result| {
                let statement = result.flow.loop_controls[0].statement;
                result.flow.loop_controls[0].target = statement;
            },
        ).contains("loop control"));
        assert!(corrupted(
            "fn main() { value := 1; }",
            |result| { result.flow.summaries.pop(); },
        ).contains("flow summary"));
    }

    #[test]
    fn handoff_gate_rejects_a_foreign_ast_subject() {
        let primary_source = source("fn main() {}");
        let program = parser::parse(&primary_source).unwrap();
        let foreign_source = source("fn main() {}");
        let foreign_program = parser::parse(&foreign_source).unwrap();
        let mut analysis = analysis::analyze(&primary_source, &program);
        let mut result = analyze(&mut analysis);

        let foreign_body = match &foreign_program.declarations[0] {
            Declaration::Function(function) => &function.body,
            Declaration::Type(_) => unreachable!(),
        };
        result.flow.summaries[0].subject = FlowSubject::Block(foreign_body);

        assert!(validate_handoff(&analysis, &result)
            .unwrap_err()
            .to_string()
            .contains("flow summary"));
    }

    #[test]
    fn handoff_gate_requires_union_injection_coverage() {
        let source = source(
            "type Choice(int | str); fn take(value Choice) {} fn main() { take(1); }",
        );
        let program = parser::parse(&source).unwrap();
        let mut analysis = analysis::analyze(&source, &program);
        let result = analyze(&mut analysis);
        assert!(result.diagnostics.is_empty(), "{}", result.diagnostics);
        assert_eq!(analysis.union_injections.len(), 1);
        analysis.union_injections.clear();

        assert!(validate_handoff(&analysis, &result)
            .unwrap_err()
            .to_string()
            .contains("union injection"));
    }
}
