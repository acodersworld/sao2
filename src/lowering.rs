//! Control-flow lowering from the validated frontend handoff to owned IR.

use std::fmt;

use crate::analysis::{
    self, AccessPathStep, Analysis, BindingNode, BuiltinMethod, CallTarget, CallableId,
    ConstructorKind, IntrinsicId, LiteralValue, MemberStorage, ProjectionKind, ResolvedType,
    TypeDefinitionKind, TypeState, UnionAlternative,
};
use crate::ast::{
    self, Argument, ArgumentKind, AssignmentOperator, Expression, ExpressionKind, PrimitiveType,
    Statement, StatementBody, StatementBodyKind, StatementKind, ExpressionBody,
    ExpressionBodyKind,
};
use crate::ir;
use crate::semantic::{FallthroughKind, FlowSubject, MutabilityOperation, MutabilityPath, MutabilitySubject, SemanticResult, TryAction};
use crate::source::Span;

#[derive(Debug)]
pub(crate) enum LoweringError {
    Invariant(String),
}

impl fmt::Display for LoweringError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invariant(message) => write!(output, "IR lowering invariant violated: {message}"),
        }
    }
}

impl std::error::Error for LoweringError {}

pub(crate) fn lower<'ast>(
    analysis: &Analysis<'_, 'ast>,
    semantic: &SemanticResult<'ast>,
) -> Result<ir::Program, LoweringError> {
    Lowerer::new(analysis, semantic).lower()
}

struct Lowerer<'a, 'source, 'ast> {
    analysis: &'a Analysis<'source, 'ast>,
    semantic: &'a SemanticResult<'ast>,
    program: ir::Program,
    types: Vec<Option<ir::TypeId>>,
    definitions: Vec<ir::DefinitionId>,
    fields: Vec<Vec<ir::FieldId>>,
    alternatives: Vec<Vec<ir::AlternativeId>>,
    functions: Vec<ir::FunctionId>,
}

impl<'a, 'source, 'ast> Lowerer<'a, 'source, 'ast> {
    fn new(analysis: &'a Analysis<'source, 'ast>, semantic: &'a SemanticResult<'ast>) -> Self {
        Self {
            analysis,
            semantic,
            program: ir::Program::new(analysis.source.path.clone(), analysis.source.text.len()),
            types: analysis.types.iter().map(|_| None).collect(),
            definitions: Vec::new(),
            fields: analysis.type_definitions.iter().map(|_| Vec::new()).collect(),
            alternatives: analysis.type_definitions.iter().map(|_| Vec::new()).collect(),
            functions: Vec::new(),
        }
    }

    fn lower(mut self) -> Result<ir::Program, LoweringError> {
        self.reserve_definitions();
        let type_ids = self.analysis.types.iter().map(|(ty, _)| ty).collect::<Vec<_>>();
        for ty in type_ids { self.map_type(ty)?; }
        for definition in &self.definitions {
            self.program.intern_type(ir::Type::Nominal(*definition));
        }
        self.complete_definitions()?;
        self.reserve_functions()?;
        self.program.entry = self.semantic.entry_point.and_then(|entry| self.functions.get(entry.function_id().index()).copied());
        if self.program.entry.is_none() { return Err(invariant("validated entry function has no IR mapping")); }
        for index in 0..self.analysis.function_signatures.len() { self.lower_function(index)?; }
        self.program.validate().map_err(|error| LoweringError::Invariant(error.to_string()))?;
        Ok(self.program)
    }

    fn reserve_definitions(&mut self) {
        for definition in &self.analysis.type_definitions {
            let name = self.analysis.identifier_text(definition.node.name.span).to_owned();
            let shell = match &definition.kind {
                TypeDefinitionKind::Struct(_) => ir::NominalDefinition::structure(name),
                TypeDefinitionKind::Tuple(_) => ir::NominalDefinition::tuple(name),
                TypeDefinitionKind::Union { .. } => ir::NominalDefinition::union(name),
                TypeDefinitionKind::Invalid => ir::NominalDefinition::tuple(name),
            };
            self.definitions.push(self.program.add_definition(shell));
        }
    }

    fn map_type(&mut self, source: analysis::TypeId) -> Result<ir::TypeId, LoweringError> {
        if !self.analysis.types.contains(source) {
            return Err(invariant("frontend type identity is out of range"));
        }
        if let Some(mapped) = self.types.get(source.index()).and_then(|item| *item) { return Ok(mapped); }
        let mapped_type = match self.analysis.types.get(source).clone() {
            ResolvedType::Unit => ir::Type::Unit,
            ResolvedType::Primitive(value) => ir::Type::Primitive(map_primitive(value)),
            ResolvedType::List(element) => ir::Type::List(self.map_type(element)?),
            ResolvedType::Map { key, value } => ir::Type::Map { key: self.map_type(key)?, value: self.map_type(value)? },
            ResolvedType::Nominal(definition) => ir::Type::Nominal(*self.definitions.get(definition.index()).ok_or_else(|| invariant("invalid nominal definition identity"))?),
            ResolvedType::Union(alternatives) => {
                let mut mapped = Vec::with_capacity(alternatives.len());
                for alternative in alternatives.iter() { mapped.push(self.map_alternative(alternative)?); }
                ir::Type::Union(mapped)
            }
        };
        let mapped = self.program.intern_type(mapped_type);
        self.types[source.index()] = Some(mapped);
        Ok(mapped)
    }

    fn map_alternative(&mut self, alternative: &UnionAlternative) -> Result<ir::UnionAlternative, LoweringError> {
        Ok(match alternative {
            UnionAlternative::Untagged(payload) => ir::UnionAlternative::untagged(self.map_type(*payload)?),
            UnionAlternative::Tagged { tag, payload } => ir::UnionAlternative::tagged(tag.as_ref(), self.map_type(*payload)?),
            UnionAlternative::Error(payload) => ir::UnionAlternative::error(self.map_type(*payload)?),
        })
    }

    fn complete_definitions(&mut self) -> Result<(), LoweringError> {
        for index in 0..self.analysis.type_definitions.len() {
            let kind = self.analysis.type_definitions[index].kind.clone();
            match kind {
                TypeDefinitionKind::Struct(fields) => for field in fields.iter() {
                    let TypeState::Resolved(ty) = field.ty else { return Err(invariant("unresolved struct field type")); };
                    let name = self.analysis.identifier_text(field.name.span).to_owned();
                    let ty = self.map_type(ty)?;
                    let field = self.program.definitions[index].add_struct_field(name, ty, map_storage(field.storage));
                    self.fields[index].push(field);
                },
                TypeDefinitionKind::Tuple(fields) => for field in fields.iter() {
                    let TypeState::Resolved(ty) = field.ty else { return Err(invariant("unresolved tuple field type")); };
                    let ty = self.map_type(ty)?;
                    let field = self.program.definitions[index].add_tuple_field(ty);
                    self.fields[index].push(field);
                },
                TypeDefinitionKind::Union { alternatives, .. } => for alternative in alternatives.iter() {
                    let alternative = self.map_alternative(alternative)?;
                    let alternative = self.program.definitions[index].add_alternative(alternative);
                    self.alternatives[index].push(alternative);
                },
                TypeDefinitionKind::Invalid => return Err(invariant("invalid nominal definition reached lowering")),
            }
        }
        Ok(())
    }

    fn reserve_functions(&mut self) -> Result<(), LoweringError> {
        let signatures = self.analysis.function_signatures.clone();
        for signature in &signatures {
            let TypeState::Resolved(result) = signature.result else { return Err(invariant("unresolved function result")); };
            let name = self.analysis.identifier_text(signature.node.name.span).to_owned();
            let result = self.map_type(result)?;
            self.functions.push(self.program.add_function(ir::Function::new(name, result)));
        }
        Ok(())
    }

    fn lower_function(&mut self, index: usize) -> Result<(), LoweringError> {
        let signature = self.analysis.function_signatures[index].clone();
        let frontend_unit = self.analysis.types.unit();
        let unit = self.map_type(frontend_unit)?;
        let mut function = std::mem::replace(
            &mut self.program.functions[index],
            ir::Function::new("<lowering>", unit),
        );
        let mut bindings = vec![None; self.analysis.bindings.len()];
        for parameter in &signature.parameters {
            let TypeState::Resolved(ty) = parameter.ty else { return Err(invariant("unresolved parameter type")); };
            let mapped = self.map_type(ty)?;
            let local = function.add_local(
                mapped,
                Some(self.analysis.identifier_text(parameter.node.name.span).to_owned()),
                ir::LocalOrigin::Parameter,
            );
            bindings[parameter.binding.index()] = Some(local);
        }
        let entry = function.add_block();
        function.entry = Some(entry);
        {
            let mut body = BodyLowerer { owner: self, function: &mut function, function_id: signature.id, block: Some(entry), bindings, loops: Vec::new(), narrowings: Vec::new(), lowered_tests: Vec::new() };
            let expected_fallthrough = body.block_falls_through(&signature.node.body)?;
            body.block(&signature.node.body)?;
            let value = if body.block.is_some() {
                if let Some(value_expression) = signature.node.body.value.as_deref() {
                    body.expression(value_expression)?
                } else {
                    let completion_injection = body.semantic_completion(signature.id, &signature.node.body)?.unit_injection.clone();
                    let frontend_unit = body.owner.analysis.types.unit();
                    let unit = body.owner.map_type(frontend_unit)?;
                    let value = ir::Operand::Constant(ir::Constant { ty: unit, value: ir::ConstantValue::Unit });
                    if let Some(alternative) = completion_injection.as_ref() {
                        let TypeState::Resolved(union) = signature.result else { unreachable!() };
                        body.inject(value, union, alternative, signature.node.body.span)?
                    } else { Some(value) }
                }
            } else { None };
            if expected_fallthrough != body.block.is_some() { return Err(invariant("function-body lowering contradicts its flow summary")); }
            if let Some(value) = value {
                body.terminate(ir::TerminatorKind::Return(value), signature.node.body.span)?;
            }
        }
        self.program.functions[index] = function;
        Ok(())
    }
}

struct BodyLowerer<'a, 'b, 'source, 'ast> {
    owner: &'a mut Lowerer<'b, 'source, 'ast>,
    function: &'a mut ir::Function,
    function_id: analysis::FunctionId,
    block: Option<ir::BlockId>,
    bindings: Vec<Option<ir::LocalId>>,
    loops: Vec<LoopContext<'ast>>,
    narrowings: Vec<Narrowing>,
    lowered_tests: Vec<LoweredTest<'ast>>,
}

#[derive(Clone)]
struct Narrowing {
    binding: analysis::BindingId,
    union: ir::Operand,
    alternative: ir::AlternativeId,
    payload: analysis::TypeId,
    local: ir::LocalId,
}

#[derive(Clone)]
struct LoweredTest<'ast> {
    expression: &'ast Expression,
    binding: Option<analysis::BindingId>,
    union: ir::Operand,
    alternative: ir::AlternativeId,
    payload: analysis::TypeId,
}

#[derive(Clone)]
struct LoopContext<'ast> {
    source: &'ast Statement,
    continue_target: ir::BlockId,
    break_target: ir::BlockId,
    cleanup: Option<ir::Operand>,
}

impl<'a, 'b, 'source, 'ast> BodyLowerer<'a, 'b, 'source, 'ast> {
    fn current_block(&mut self) -> Result<&mut ir::BasicBlock, LoweringError> {
        let block = self.block.ok_or_else(|| invariant("operation emitted after path termination"))?;
        Ok(&mut self.function.blocks[block.index()])
    }
    fn new_block(&mut self) -> ir::BlockId { self.function.add_block() }
    fn enter(&mut self, block: ir::BlockId) { self.block = Some(block); }
    fn location(&mut self, span: Span) -> ir::LocationId { self.owner.program.intern_location(ir::ByteSpan::new(span.start, span.end)) }
    fn failure(&mut self, span: Span, operation: ir::FailureOperation) -> Result<ir::FailureSiteId, LoweringError> {
        let location = self.location(span);
        let function = self.owner.functions.get(self.function_id.index()).copied().ok_or_else(|| invariant("current function has no IR identity"))?;
        let (line, column) = self.owner.analysis.source.line_and_column(span.start);
        Ok(self.owner.program.intern_failure_site(ir::FailureSite { location, function, operation, line, column }))
    }
    fn semantic_completion(&self, function: analysis::FunctionId, block: &ast::Block) -> Result<&crate::semantic::FunctionCompletion<'ast>, LoweringError> {
        let matches = self.owner.semantic.flow.completions.iter().filter(|item| item.function == function && std::ptr::eq(item.body, block)).collect::<Vec<_>>();
        let [completion] = matches.as_slice() else { return Err(invariant("function fallthrough does not have exactly one completion record")); };
        Ok(*completion)
    }
    fn semantic_return(&self, statement: &Statement) -> Result<&crate::semantic::ExplicitReturn<'ast>, LoweringError> {
        let matches = self.owner.semantic.flow.returns.iter().filter(|item| std::ptr::eq(item.statement, statement)).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("explicit return does not have exactly one flow record")); };
        if record.function != self.function_id { return Err(invariant("explicit return is linked to the wrong function")); }
        Ok(*record)
    }
    fn semantic_loop_control(&self, statement: &Statement) -> Result<&crate::semantic::LoopControl<'ast>, LoweringError> {
        let matches = self.owner.semantic.flow.loop_controls.iter().filter(|item| std::ptr::eq(item.statement, statement)).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("loop control does not have exactly one target record")); };
        Ok(*record)
    }
    fn semantic_union_test(&self, expression: &Expression) -> Result<&crate::semantic::UnionTestResolution<'ast>, LoweringError> {
        let expression = strip_expression_parentheses(expression);
        let matches = self.owner.semantic.union_tests.iter().filter(|item| std::ptr::eq(item.expression, expression)).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("union test does not have exactly one resolution")); };
        Ok(*record)
    }
    fn semantic_switch(&self, statement: &Statement) -> Result<&crate::semantic::SwitchResolution<'ast>, LoweringError> {
        let matches = self.owner.semantic.switches.iter().filter(|item| std::ptr::eq(item.statement, statement)).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("switch does not have exactly one resolution")); };
        Ok(*record)
    }
    fn semantic_try(&self, expression: &Expression) -> Result<&crate::semantic::TryResolution<'ast>, LoweringError> {
        let matches = self.owner.semantic.tries.iter().filter(|item| std::ptr::eq(item.expression, expression)).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("postfix try does not have exactly one resolution")); };
        Ok(*record)
    }
    fn block_falls_through(&self, block: &ast::Block) -> Result<bool, LoweringError> {
        let matches = self.owner.semantic.flow.summaries.iter().filter(|record| matches!(record.subject, FlowSubject::Block(candidate) if std::ptr::eq(candidate, block))).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("block does not have exactly one flow summary")); };
        Ok(record.summary.fallthrough != FallthroughKind::None)
    }
    fn statement_falls_through(&self, statement: &Statement) -> Result<bool, LoweringError> {
        let matches = self.owner.semantic.flow.summaries.iter().filter(|record| matches!(record.subject, FlowSubject::Statement(candidate) if std::ptr::eq(candidate, statement))).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("statement does not have exactly one flow summary")); };
        Ok(record.summary.fallthrough != FallthroughKind::None)
    }
    fn expression_falls_through(&self, expression: &Expression) -> Result<bool, LoweringError> {
        let matches = self.owner.semantic.flow.summaries.iter().filter(|record| matches!(record.subject, FlowSubject::Expression(candidate) if std::ptr::eq(candidate, expression))).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("expression does not have exactly one flow summary")); };
        Ok(record.summary.fallthrough != FallthroughKind::None)
    }
    fn statement_body_falls_through(&self, body: &StatementBody) -> Result<bool, LoweringError> {
        let matches = self.owner.semantic.flow.summaries.iter().filter(|record| matches!(record.subject, FlowSubject::StatementBody(candidate) if std::ptr::eq(candidate, body))).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("statement body does not have exactly one flow summary")); };
        Ok(record.summary.fallthrough != FallthroughKind::None)
    }
    fn expression_body_falls_through(&self, body: &ExpressionBody) -> Result<bool, LoweringError> {
        let matches = self.owner.semantic.flow.summaries.iter().filter(|record| matches!(record.subject, FlowSubject::ExpressionBody(candidate) if std::ptr::eq(candidate, body))).collect::<Vec<_>>();
        let [record] = matches.as_slice() else { return Err(invariant("expression body does not have exactly one flow summary")); };
        Ok(record.summary.fallthrough != FallthroughKind::None)
    }
    fn ty(&mut self, ty: analysis::TypeId) -> Result<ir::TypeId, LoweringError> { self.owner.map_type(ty) }
    fn definition(&self, definition: analysis::TypeDeclarationId) -> Result<ir::DefinitionId, LoweringError> {
        self.owner.definitions.get(definition.index()).copied().ok_or_else(|| invariant("nominal definition has no IR mapping"))
    }
    fn field(&self, definition: analysis::TypeDeclarationId, field: usize) -> Result<ir::FieldId, LoweringError> {
        self.owner.fields.get(definition.index()).and_then(|fields| fields.get(field)).copied().ok_or_else(|| invariant("nominal field has no IR mapping"))
    }
    fn expression_type(&self, expression: &Expression) -> Result<analysis::TypeId, LoweringError> {
        match self.owner.analysis.expression_annotation(expression).map(|item| item.state) {
            Some(TypeState::Resolved(ty)) => Ok(ty),
            _ => Err(invariant("expression lacks a resolved value type")),
        }
    }
    fn raw_type(&self, expression: &Expression) -> Result<analysis::TypeId, LoweringError> {
        if let Some(injection) = self.owner.analysis.union_injection(expression) {
            Ok(alternative_payload(&injection.alternative))
        } else {
            self.expression_type(expression)
        }
    }
    fn temporary(&mut self, ty: analysis::TypeId) -> Result<ir::LocalId, LoweringError> {
        let ty = self.ty(ty)?;
        Ok(self.function.add_local(ty, None, ir::LocalOrigin::Temporary))
    }
    fn push(&mut self, kind: ir::OperationKind, span: Span) -> Result<(), LoweringError> {
        let location = self.location(span);
        self.current_block()?.push(kind, location);
        Ok(())
    }
    fn terminate(&mut self, kind: ir::TerminatorKind, span: Span) -> Result<(), LoweringError> {
        let location = self.location(span);
        self.current_block()?.terminate(kind, location);
        self.block = None;
        Ok(())
    }

    fn block(&mut self, block: &'ast ast::Block) -> Result<(), LoweringError> {
        self.block_falls_through(block)?;
        for statement in &block.statements {
            if self.block.is_none() { break; }
            self.statement(statement)?;
        }
        Ok(())
    }

    fn statement(&mut self, statement: &'ast Statement) -> Result<(), LoweringError> {
        let expected_fallthrough = self.statement_falls_through(statement)?;
        match &statement.kind {
            StatementKind::Local { name, initializer, .. } => {
                let binding = self.owner.analysis.bindings.iter().find_map(|record| match record.node {
                    BindingNode::Local(node) if std::ptr::eq(node, statement) => Some(record.id),
                    _ => None,
                }).ok_or_else(|| invariant("local declaration lacks a binding identity"))?;
                let binding_state = self.owner.analysis.binding_type(binding);
                if binding_state == TypeState::Never {
                    if self.expression(initializer)?.is_some() { return Err(invariant("never-typed local initializer did not diverge")); }
                } else {
                    let TypeState::Resolved(ty) = binding_state else { return Err(invariant("local binding has no resolved type")); };
                    let mapped = self.ty(ty)?;
                    let source_name = self.owner.analysis.identifier_text(name.span).to_owned();
                    let local = self.function.add_local(mapped, Some(source_name), ir::LocalOrigin::Binding);
                    self.bindings[binding.index()] = Some(local);
                    let Some(value) = self.expression(initializer)? else { return Err(invariant("value-typed local initializer diverged")); };
                    self.push(ir::OperationKind::Assign { destination: ir::Place::local(local), value }, statement.span)?;
                }
            }
            StatementKind::Assignment { target, operator, operator_span, value } => self.assignment(target, *operator, *operator_span, value, statement.span)?,
            StatementKind::Expression(expression) => { self.expression(expression)?; }
            StatementKind::Return(value) => self.return_statement(statement, value.as_ref())?,
            StatementKind::If { branches, else_body } => self.if_statement(branches, else_body.as_ref(), statement.span)?,
            StatementKind::While { condition, body } => self.while_loop(statement, condition, body)?,
            StatementKind::For { iterable, body, .. } => self.for_loop(statement, iterable, body)?,
            StatementKind::Switch { value, arms, else_body } => self.switch_statement(statement, value, arms, else_body.as_ref())?,
            StatementKind::Break | StatementKind::Continue => self.loop_control(statement)?,
            StatementKind::Block(block) => self.statement_block(block)?,
        }
        if expected_fallthrough != self.block.is_some() { return Err(invariant("statement lowering contradicts its flow summary")); }
        Ok(())
    }

    fn expression(&mut self, expression: &'ast Expression) -> Result<Option<ir::Operand>, LoweringError> {
        let expected_fallthrough = self.expression_falls_through(expression)?;
        let value = if let Some(value) = self.expression_core(expression)? {
            if let Some(injection) = self.owner.analysis.union_injection(expression) {
                let union = injection.union_type;
                let alternative = injection.alternative.clone();
                self.inject(value, union, &alternative, expression.span)?
            } else { Some(value) }
        } else { None };
        if expected_fallthrough != value.is_some() || expected_fallthrough != self.block.is_some() {
            return Err(invariant("expression lowering contradicts its flow summary"));
        }
        Ok(value)
    }

    fn expression_core(&mut self, expression: &'ast Expression) -> Result<Option<ir::Operand>, LoweringError> {
        let value = match &expression.kind {
            ExpressionKind::Unit => {
                let ty = self.raw_type(expression)?;
                self.constant(ty, ir::ConstantValue::Unit)?
            }
            ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_) => self.literal_constant(expression)?,
            ExpressionKind::Identifier(identifier) => {
                let Some(analysis::NameResolution::Binding(binding)) = self.owner.analysis.name_use(identifier).map(|item| item.resolution) else { return Err(invariant("value identifier is not a binding")); };
                let expression_type = self.expression_type(expression)?;
                let local = self.narrowings.iter().rev().find(|entry| entry.binding == binding && entry.payload == expression_type).map(|entry| entry.local)
                    .or_else(|| self.bindings.get(binding.index()).and_then(|item| *item)).ok_or_else(|| invariant("binding has not been allocated"))?;
                ir::Operand::Copy(ir::Place::local(local))
            }
            ExpressionKind::Parenthesized(inner) => return self.expression(inner),
            ExpressionKind::Conversion { operand, .. } => {
                let resolution = *self.owner.analysis.numeric_conversion(expression).ok_or_else(|| invariant("numeric conversion fact is missing"))?;
                if !self.owner.analysis.types.contains(resolution.source) || !self.owner.analysis.types.contains(resolution.destination) {
                    return Err(invariant("numeric conversion contains an invalid type identity"));
                }
                let conversion = if matches!(self.owner.analysis.types.get(resolution.source), ResolvedType::Primitive(PrimitiveType::Int)) { ir::NumericConversion::IntToFloat } else { ir::NumericConversion::FloatToInt };
                let Some(operand) = self.expression(operand)? else { return Ok(None); };
                let operand = if conversion == ir::NumericConversion::FloatToInt { self.stabilize(operand, expression.span)? } else { operand };
                if conversion == ir::NumericConversion::FloatToInt {
                    let failure = self.failure(expression.span, ir::FailureOperation::FloatToInt)?;
                    self.push(ir::OperationKind::Check(ir::RuntimeCheck::NumericConversion { operand: operand.clone(), conversion, failure }), expression.span)?;
                }
                let destination = self.temporary(resolution.destination)?;
                self.push(ir::OperationKind::Convert { destination, conversion, operand }, expression.span)?;
                ir::Operand::Copy(ir::Place::local(destination))
            }
            ExpressionKind::Unary { operator, operator_span, operand } => {
                if *operator == ast::UnaryOperator::Minus
                    && matches!(self.owner.analysis.literal(operand), Some(LiteralValue::Integer(value)) if *value == (i64::MAX as u64) + 1)
                {
                    let ty = self.raw_type(expression)?;
                    self.constant(ty, ir::ConstantValue::Integer(i64::MIN))?
                } else {
                let operand_span = operand.span;
                let Some(operand) = self.expression(operand)? else { return Ok(None); };
                let result = self.raw_type(expression)?;
                let operand = if *operator == ast::UnaryOperator::Minus && matches!(self.owner.analysis.types.get(result), ResolvedType::Primitive(PrimitiveType::Int)) { self.stabilize(operand, operand_span)? } else { operand };
                if *operator == ast::UnaryOperator::Minus && matches!(self.owner.analysis.types.get(result), ResolvedType::Primitive(PrimitiveType::Int)) {
                    let failure = self.failure(*operator_span, ir::FailureOperation::IntegerNegation)?;
                    self.push(ir::OperationKind::Check(ir::RuntimeCheck::IntegerNegation { operand: operand.clone(), failure }), *operator_span)?;
                }
                let destination = self.temporary(result)?;
                self.push(ir::OperationKind::Unary { destination, operator: map_unary(*operator), operand }, expression.span)?;
                ir::Operand::Copy(ir::Place::local(destination))
                }
            }
            ExpressionKind::Binary { left, operator, operator_span, right } => {
                if matches!(operator, ast::BinaryOperator::LogicalAnd | ast::BinaryOperator::LogicalOr) {
                    return self.short_circuit(expression, left, *operator, right);
                }
                let left_span = left.span;
                let Some(left) = self.expression(left)? else { return Ok(None); };
                let left = self.stabilize(left, left_span)?;
                let right_span = right.span;
                let Some(right) = self.expression(right)? else { return Ok(None); };
                let right = self.stabilize(right, right_span)?;
                let result = self.raw_type(expression)?;
                self.checked_binary(result, map_binary(*operator)?, left, right, *operator_span, expression.span)?
            }
            ExpressionKind::List(elements) => return self.list(expression, elements),
            ExpressionKind::Map(entries) => return self.map(expression, entries),
            ExpressionKind::TypedEmptyList(_) => {
                let raw = self.raw_type(expression)?;
                let ty = self.ty(raw)?;
                self.aggregate(expression, ir::Aggregate::List { ty, elements: Vec::new() })?
            }
            ExpressionKind::TypedEmptyMap(_) => {
                let raw = self.raw_type(expression)?;
                let ty = self.ty(raw)?;
                self.aggregate(expression, ir::Aggregate::Map { ty, entries: Vec::new() })?
            }
            ExpressionKind::Call { arguments, .. } => return self.call(expression, arguments),
            ExpressionKind::Member { value, .. } => return self.member(expression, value),
            ExpressionKind::Index { value, index } => return self.index(expression, value, index),
            ExpressionKind::Block(block) => return self.block_expression(expression, block),
            ExpressionKind::If { branches, else_branch } => return self.if_expression(expression, branches, else_branch),
            ExpressionKind::Is { value, .. } => return self.union_test(expression, value),
            ExpressionKind::Try { value, .. } => return self.try_expression(expression, value),
        };
        Ok(Some(value))
    }

    fn union_test(&mut self, expression: &'ast Expression, value: &'ast Expression) -> Result<Option<ir::Operand>, LoweringError> {
        let resolution = self.semantic_union_test(expression)?.clone();
        if resolution.operand_union != self.expression_type(value)? || alternative_payload(&resolution.alternative) != resolution.payload {
            return Err(invariant("union test resolution contradicts its operand"));
        }
        let Some(union) = self.expression(value)? else { return Ok(None); };
        let union = self.stabilize(union, value.span)?;
        let alternative = self.alternative_id(resolution.operand_union, &resolution.alternative)?;
        let destination = self.temporary(self.owner.analysis.types.primitive(PrimitiveType::Bool))?;
        self.push(ir::OperationKind::UnionTest { destination, union: union.clone(), alternative }, expression.span)?;
        self.lowered_tests.push(LoweredTest { expression, binding: resolution.binding, union, alternative, payload: resolution.payload });
        Ok(Some(ir::Operand::Copy(ir::Place::local(destination))))
    }

    fn activate_test_narrowing(&mut self, condition: &'ast Expression, span: Span) -> Result<(), LoweringError> {
        let condition = strip_expression_parentheses(condition);
        let Some(test) = self.lowered_tests.iter().rev().find(|item| std::ptr::eq(item.expression, condition)).cloned() else { return Ok(()); };
        let Some(binding) = test.binding else { return Ok(()); };
        let local = self.temporary(test.payload)?;
        self.push(ir::OperationKind::UnionPayload { destination: local, union: test.union.clone(), alternative: test.alternative }, span)?;
        self.narrowings.retain(|entry| entry.binding != binding);
        self.narrowings.push(Narrowing { binding, union: test.union, alternative: test.alternative, payload: test.payload, local });
        Ok(())
    }

    fn constant(&mut self, ty: analysis::TypeId, value: ir::ConstantValue) -> Result<ir::Operand, LoweringError> { Ok(ir::Operand::Constant(ir::Constant { ty: self.ty(ty)?, value })) }
    fn literal_constant(&mut self, expression: &Expression) -> Result<ir::Operand, LoweringError> {
        let value = match self.owner.analysis.literal(expression).cloned() {
            Some(LiteralValue::Integer(value)) => ir::ConstantValue::Integer(i64::try_from(value).map_err(|_| invariant("positive integer literal exceeds signed int range"))?),
            Some(LiteralValue::Float(value)) => ir::ConstantValue::Float(value.to_bits()),
            Some(LiteralValue::String(value)) => ir::ConstantValue::String(value.into_vec()),
            Some(LiteralValue::Character(value)) => ir::ConstantValue::Character(value),
            Some(LiteralValue::Boolean(value)) => ir::ConstantValue::Boolean(value),
            None => return Err(invariant("literal fact is missing")),
        };
        let ty = self.raw_type(expression)?;
        self.constant(ty, value)
    }
    fn aggregate(&mut self, expression: &Expression, aggregate: ir::Aggregate) -> Result<ir::Operand, LoweringError> {
        let result = self.raw_type(expression)?;
        let destination = self.temporary(result)?;
        self.push(ir::OperationKind::Aggregate { destination, aggregate }, expression.span)?;
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
    }
    fn stabilize(&mut self, operand: ir::Operand, span: Span) -> Result<ir::Operand, LoweringError> {
        let stable = match &operand {
            ir::Operand::Constant(_) => true,
            ir::Operand::Copy(place) if place.projections.is_empty() => self.function.locals.get(place.local.index()).is_some_and(|local| local.origin == ir::LocalOrigin::Temporary),
            _ => false,
        };
        if stable { return Ok(operand); }
        let ty = self.operand_frontend_type(&operand)?;
        let destination = self.temporary(ty)?;
        self.push(ir::OperationKind::Copy { destination, operand }, span)?;
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
    }
    fn materialize(&mut self, operand: ir::Operand, span: Span) -> Result<ir::Operand, LoweringError> {
        if matches!(&operand, ir::Operand::Copy(place) if place.projections.is_empty() && self.function.locals.get(place.local.index()).is_some_and(|local| local.origin == ir::LocalOrigin::Temporary)) {
            return Ok(operand);
        }
        let ty = self.operand_frontend_type(&operand)?;
        let destination = self.temporary(ty)?;
        self.push(ir::OperationKind::Copy { destination, operand }, span)?;
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
    }
    fn checked_binary(
        &mut self,
        result: analysis::TypeId,
        operator: ir::BinaryOperator,
        left: ir::Operand,
        right: ir::Operand,
        failure_span: Span,
        operation_span: Span,
    ) -> Result<ir::Operand, LoweringError> {
        let is_int = matches!(self.owner.analysis.types.get(result), ResolvedType::Primitive(PrimitiveType::Int));
        let is_float = matches!(self.owner.analysis.types.get(result), ResolvedType::Primitive(PrimitiveType::Float));
        let failure_operation = match (operator, is_int, is_float) {
            (ir::BinaryOperator::Add, true, _) => Some(ir::FailureOperation::IntegerAdd),
            (ir::BinaryOperator::Subtract, true, _) => Some(ir::FailureOperation::IntegerSubtract),
            (ir::BinaryOperator::Multiply, true, _) => Some(ir::FailureOperation::IntegerMultiply),
            (ir::BinaryOperator::Divide, true, _) => Some(ir::FailureOperation::IntegerDivision),
            (ir::BinaryOperator::Remainder, true, _) => Some(ir::FailureOperation::IntegerRemainder),
            (ir::BinaryOperator::ShiftLeft, true, _) => Some(ir::FailureOperation::ShiftLeft),
            (ir::BinaryOperator::ShiftRight, true, _) => Some(ir::FailureOperation::ShiftRight),
            (ir::BinaryOperator::Add, _, true) => Some(ir::FailureOperation::FloatAdd),
            (ir::BinaryOperator::Subtract, _, true) => Some(ir::FailureOperation::FloatSubtract),
            (ir::BinaryOperator::Multiply, _, true) => Some(ir::FailureOperation::FloatMultiply),
            (ir::BinaryOperator::Divide, _, true) => Some(ir::FailureOperation::FloatDivision),
            _ => None,
        };
        let failure = if let Some(operation) = failure_operation { Some(self.failure(failure_span, operation)?) } else { None };
        if let Some(failure) = failure {
            let check = match operator {
                ir::BinaryOperator::Add if is_int => Some(ir::RuntimeCheck::IntegerOverflow { operation: ir::IntegerOperation::Add, left: left.clone(), right: right.clone(), failure }),
                ir::BinaryOperator::Subtract if is_int => Some(ir::RuntimeCheck::IntegerOverflow { operation: ir::IntegerOperation::Subtract, left: left.clone(), right: right.clone(), failure }),
                ir::BinaryOperator::Multiply if is_int => Some(ir::RuntimeCheck::IntegerOverflow { operation: ir::IntegerOperation::Multiply, left: left.clone(), right: right.clone(), failure }),
                ir::BinaryOperator::Divide => Some(ir::RuntimeCheck::Division { left: left.clone(), right: right.clone(), failure }),
                ir::BinaryOperator::Remainder => Some(ir::RuntimeCheck::Remainder { left: left.clone(), right: right.clone(), failure }),
                ir::BinaryOperator::ShiftLeft | ir::BinaryOperator::ShiftRight => Some(ir::RuntimeCheck::ShiftRange { amount: right.clone(), failure }),
                _ => None,
            };
            if let Some(check) = check { self.push(ir::OperationKind::Check(check), failure_span)?; }
            if operator == ir::BinaryOperator::ShiftLeft && is_int {
                self.push(ir::OperationKind::Check(ir::RuntimeCheck::IntegerOverflow { operation: ir::IntegerOperation::ShiftLeft, left: left.clone(), right: right.clone(), failure }), failure_span)?;
            }
        }
        let destination = self.temporary(result)?;
        self.push(ir::OperationKind::Binary { destination, operator, left, right }, operation_span)?;
        let operand = ir::Operand::Copy(ir::Place::local(destination));
        if is_float && matches!(operator, ir::BinaryOperator::Add | ir::BinaryOperator::Subtract | ir::BinaryOperator::Multiply | ir::BinaryOperator::Divide) {
            self.push(ir::OperationKind::Check(ir::RuntimeCheck::FiniteFloat { operand: operand.clone(), failure: failure.expect("float arithmetic has a failure site") }), failure_span)?;
        }
        Ok(operand)
    }
    fn operand_frontend_type(&self, operand: &ir::Operand) -> Result<analysis::TypeId, LoweringError> {
        let ir_ty = match operand {
            ir::Operand::Constant(value) => value.ty,
            ir::Operand::Copy(place) => {
                let mut ty = self.function.locals.get(place.local.index()).ok_or_else(|| invariant("invalid local in operand"))?.ty;
                for projection in &place.projections { ty = projected_ir_type(&self.owner.program, ty, projection)?; }
                ty
            }
        };
        self.owner.types.iter().position(|item| *item == Some(ir_ty)).map(analysis::TypeId::from_index).ok_or_else(|| invariant("IR operand type has no frontend mapping"))
    }

    fn list(&mut self, expression: &'ast Expression, elements: &'ast [Expression]) -> Result<Option<ir::Operand>, LoweringError> {
        let mut values = Vec::new();
        for element in elements { let Some(value) = self.expression(element)? else { return Ok(None); }; values.push(self.stabilize(value, element.span)?); }
        let raw = self.raw_type(expression)?;
        let ty = self.ty(raw)?;
        Ok(Some(self.aggregate(expression, ir::Aggregate::List { ty, elements: values })?))
    }
    fn map(&mut self, expression: &'ast Expression, entries: &'ast [ast::MapEntry]) -> Result<Option<ir::Operand>, LoweringError> {
        let mut values = Vec::new();
        for entry in entries {
            let Some(key) = self.expression(&entry.key)? else { return Ok(None); }; let key = self.stabilize(key, entry.key.span)?;
            let Some(value) = self.expression(&entry.value)? else { return Ok(None); }; let value = self.stabilize(value, entry.value.span)?;
            values.push((key, value));
        }
        let raw = self.raw_type(expression)?;
        let ty = self.ty(raw)?;
        Ok(Some(self.aggregate(expression, ir::Aggregate::Map { ty, entries: values })?))
    }

    fn call(&mut self, expression: &'ast Expression, arguments: &'ast [Argument]) -> Result<Option<ir::Operand>, LoweringError> {
        let resolution = *self.owner.analysis.call_resolution(expression).ok_or_else(|| invariant("call target fact is missing"))?;
        if matches!(resolution.target, CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Panic))) {
            let [argument] = arguments else { return Err(invariant("panic call does not have exactly one argument")); };
            let Some(message) = self.expression(argument_value(argument))? else { return Ok(None); };
            let failure = self.failure(expression.span, ir::FailureOperation::ExplicitPanic)?;
            self.terminate(ir::TerminatorKind::Panic { message, failure }, expression.span)?;
            return Ok(None);
        }
        if let Some(constructor) = self.owner.analysis.constructor(expression) {
            let kind = constructor.kind.clone();
            return self.constructor(expression, arguments, &kind);
        }
        let receiver = if let CallTarget::Builtin(_) = resolution.target {
            let ExpressionKind::Member { value, .. } = &resolution.callee.kind else { return Err(invariant("built-in call has no receiver")); };
            let receiver_span = value.span;
            let Some(value) = self.expression(value)? else { return Ok(None); }; Some(self.stabilize(value, receiver_span)?)
        } else { None };
        let mut lowered = Vec::new();
        for argument in arguments { let value = argument_value(argument); let Some(operand) = self.expression(value)? else { return Ok(None); }; lowered.push(self.stabilize(operand, value.span)?); }
        let result = self.raw_type(expression)?;
        let destination = self.temporary(result)?;
        let kind = match resolution.target {
            CallTarget::Callable(CallableId::Function(function)) => ir::OperationKind::Call { destination, function: self.owner.functions.get(function.index()).copied().ok_or_else(|| invariant("callee has no IR mapping"))?, arguments: lowered },
            CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Print)) => ir::OperationKind::Intrinsic { destination, intrinsic: ir::Intrinsic::Print, arguments: lowered, failure: self.failure(expression.span, ir::FailureOperation::Output)? },
            CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Println)) => ir::OperationKind::Intrinsic { destination, intrinsic: ir::Intrinsic::Println, arguments: lowered, failure: self.failure(expression.span, ir::FailureOperation::Output)? },
            CallTarget::Builtin(method) => {
                let receiver = receiver.unwrap();
                let failure_operation = match method {
                    BuiltinMethod::ListAppend => Some(ir::FailureOperation::ListAppend),
                    BuiltinMethod::ListRemoveIndex => Some(ir::FailureOperation::ListRemoveIndex),
                    BuiltinMethod::MapRemoveKey => Some(ir::FailureOperation::MapRemoveKey),
                    _ => None,
                };
                let failure = failure_operation.map(|operation| self.failure(expression.span, operation)).transpose()?;
                if let Some(failure) = failure {
                    self.push(ir::OperationKind::Check(ir::RuntimeCheck::IterationUnlocked { receiver: receiver.clone(), failure }), expression.span)?;
                }
                ir::OperationKind::Builtin { destination, method: map_builtin(method), receiver, arguments: lowered, failure }
            }
            _ => return Err(invariant("call retained an unsupported target")),
        };
        self.push(kind, expression.span)?;
        Ok(Some(ir::Operand::Copy(ir::Place::local(destination))))
    }

    fn constructor(&mut self, expression: &'ast Expression, arguments: &'ast [Argument], kind: &ConstructorKind) -> Result<Option<ir::Operand>, LoweringError> {
        let mut values = Vec::new();
        for argument in arguments { let value = argument_value(argument); let Some(operand) = self.expression(value)? else { return Ok(None); }; values.push(self.stabilize(operand, value.span)?); }
        match kind {
            ConstructorKind::Struct { declaration, argument_members } => {
                if values.len() != argument_members.len() { return Err(invariant("struct argument mapping has the wrong length")); }
                let definition_fields = self.owner.fields.get(declaration.index()).ok_or_else(|| invariant("struct definition has no field map"))?;
                let mut fields = values.into_iter().zip(argument_members.iter().copied()).map(|(value, field)| {
                    definition_fields.get(field).copied().map(|field| (field, value)).ok_or_else(|| invariant("struct argument maps to an invalid field"))
                }).collect::<Result<Vec<_>, _>>()?;
                fields.sort_by_key(|(field, _)| field.index());
                let definition = self.definition(*declaration)?;
                Ok(Some(self.aggregate(expression, ir::Aggregate::Struct { definition, fields })?))
            }
            ConstructorKind::Tuple(declaration) => {
                let definition = self.definition(*declaration)?;
                Ok(Some(self.aggregate(expression, ir::Aggregate::Tuple { definition, elements: values })?))
            }
            ConstructorKind::Union { declaration, alternative } | ConstructorKind::TaggedUnion { declaration, alternative } => {
                let [payload] = values.as_slice() else { return Err(invariant("union constructor does not have one payload")); };
                let union = self.owner.analysis.types.iter().find_map(|(ty, value)| {
                    matches!(value, ResolvedType::Nominal(candidate) if candidate == declaration).then_some(ty)
                }).ok_or_else(|| invariant("union constructor declaration has no canonical nominal type"))?;
                self.inject(payload.clone(), union, alternative, expression.span)
            }
            ConstructorKind::Error { union_type, alternative } => {
                let [payload] = values.as_slice() else { return Err(invariant("Error constructor does not have one payload")); };
                self.inject(payload.clone(), *union_type, alternative, expression.span)
            }
        }
    }

    fn inject(&mut self, payload: ir::Operand, union: analysis::TypeId, alternative: &UnionAlternative, span: Span) -> Result<Option<ir::Operand>, LoweringError> {
        let destination = self.temporary(union)?;
        let alternative = self.alternative_id(union, alternative)?;
        let union_type = self.ty(union)?;
        self.push(ir::OperationKind::UnionInject { destination, union_type, alternative, payload }, span)?;
        Ok(Some(ir::Operand::Copy(ir::Place::local(destination))))
    }
    fn alternative_id(&self, union: analysis::TypeId, alternative: &UnionAlternative) -> Result<ir::AlternativeId, LoweringError> {
        let alternatives: &[UnionAlternative] = match self.owner.analysis.types.get(union) {
            ResolvedType::Union(items) => items,
            ResolvedType::Nominal(definition) => match &self.owner.analysis.type_definition(*definition).kind { TypeDefinitionKind::Union { alternatives, .. } => alternatives, _ => return Err(invariant("union operation names a non-union nominal")) },
            _ => return Err(invariant("union operation names a non-union type")),
        };
        let position = alternatives.iter().position(|item| item == alternative).ok_or_else(|| invariant("union alternative is absent"))?;
        match self.owner.analysis.types.get(union) {
            ResolvedType::Nominal(definition) => self.owner.alternatives[definition.index()].get(position).copied().ok_or_else(|| invariant("nominal union alternative map is incomplete")),
            ResolvedType::Union(_) => Ok(ir::AlternativeId::from_index(position)),
            _ => unreachable!(),
        }
    }

    fn member(&mut self, expression: &'ast Expression, value: &'ast Expression) -> Result<Option<ir::Operand>, LoweringError> {
        let Some(receiver) = self.expression(value)? else { return Ok(None); }; let receiver = self.stabilize(receiver, value.span)?;
        let ir::Operand::Copy(mut place) = receiver else { return Err(invariant("member receiver did not materialize to a place")); };
        let projection = self.owner.analysis.projection(expression).ok_or_else(|| invariant("member projection fact is missing"))?.kind;
        match projection {
            ProjectionKind::Struct { declaration, field, storage, .. } => place.projections.push(ir::Projection::StructField { definition: self.definition(declaration)?, field: self.field(declaration, field)?, storage: map_storage(storage) }),
            ProjectionKind::Tuple { declaration, position, .. } => place.projections.push(ir::Projection::TupleField { definition: self.definition(declaration)?, field: self.field(declaration, position)? }),
            _ => return Err(invariant("member expression has a non-member projection")),
        }
        Ok(Some(ir::Operand::Copy(place)))
    }
    fn index(&mut self, expression: &'ast Expression, value: &'ast Expression, index: &'ast Expression) -> Result<Option<ir::Operand>, LoweringError> {
        let Some(receiver) = self.expression(value)? else { return Ok(None); }; let receiver = self.stabilize(receiver, value.span)?;
        let Some(index_value) = self.expression(index)? else { return Ok(None); }; let index_value = self.materialize(index_value, index.span)?;
        let projection = self.owner.analysis.projection(expression).ok_or_else(|| invariant("index projection fact is missing"))?.kind;
        if let ProjectionKind::String { .. } = projection {
            let result = self.raw_type(expression)?;
            let destination = self.temporary(result)?;
            let failure = self.failure(index.span, ir::FailureOperation::StringIndex)?;
            self.push(ir::OperationKind::StringIndex { destination, string: receiver, index: index_value, failure }, expression.span)?;
            return Ok(Some(ir::Operand::Copy(ir::Place::local(destination))));
        }
        let ir::Operand::Copy(mut place) = receiver else { return Err(invariant("index receiver did not materialize to a place")); };
        let ir::Operand::Copy(index_place) = index_value else { return Err(invariant("dynamic index did not materialize to a local")); };
        if !index_place.projections.is_empty() { return Err(invariant("dynamic index retained projections")); }
        match projection {
            ProjectionKind::List { .. } => {
                let failure = self.failure(index.span, ir::FailureOperation::ListIndex)?;
                place.projections.push(ir::Projection::ListIndex { index: index_place.local, failure });
            }
            ProjectionKind::Map { .. } => {
                let failure = self.failure(index.span, ir::FailureOperation::MapIndex)?;
                place.projections.push(ir::Projection::MapIndex { key: index_place.local, failure });
            }
            _ => return Err(invariant("index expression has a non-index projection")),
        }
        Ok(Some(ir::Operand::Copy(place)))
    }

    fn statement_body(&mut self, body: &'ast StatementBody) -> Result<(), LoweringError> {
        let expected_fallthrough = self.statement_body_falls_through(body)?;
        match &body.kind {
            StatementBodyKind::Block(block) => self.statement_block(block),
            StatementBodyKind::Statement(statement) => self.statement(statement),
        }?;
        if expected_fallthrough != self.block.is_some() { return Err(invariant("statement-body lowering contradicts its flow summary")); }
        Ok(())
    }

    fn statement_block(&mut self, block: &'ast ast::Block) -> Result<(), LoweringError> {
        let expected_fallthrough = self.block_falls_through(block)?;
        self.block(block)?;
        if self.block.is_some() && let Some(value) = block.value.as_deref() { self.expression(value)?; }
        if expected_fallthrough != self.block.is_some() { return Err(invariant("statement-block lowering contradicts its flow summary")); }
        Ok(())
    }

    fn expression_body(&mut self, body: &'ast ExpressionBody) -> Result<Option<ir::Operand>, LoweringError> {
        let expected_fallthrough = self.expression_body_falls_through(body)?;
        let value = match &body.kind {
            ExpressionBodyKind::Expression(expression) => self.expression(expression),
            ExpressionBodyKind::Block(block) => self.block_value(block),
        }?;
        if expected_fallthrough != value.is_some() || expected_fallthrough != self.block.is_some() {
            return Err(invariant("expression-body lowering contradicts its flow summary"));
        }
        Ok(value)
    }

    fn block_value(&mut self, block: &'ast ast::Block) -> Result<Option<ir::Operand>, LoweringError> {
        let expected_fallthrough = self.block_falls_through(block)?;
        self.block(block)?;
        let value = if self.block.is_none() { None } else if let Some(value) = block.value.as_deref() {
            self.expression(value)?
        } else {
            let unit = self.owner.analysis.types.unit();
            Some(self.constant(unit, ir::ConstantValue::Unit)?)
        };
        if expected_fallthrough != value.is_some() || expected_fallthrough != self.block.is_some() {
            return Err(invariant("expression-block lowering contradicts its flow summary"));
        }
        Ok(value)
    }

    fn block_expression(&mut self, _expression: &'ast Expression, block: &'ast ast::Block) -> Result<Option<ir::Operand>, LoweringError> {
        self.block_value(block)
    }

    fn return_statement(&mut self, statement: &'ast Statement, value: Option<&'ast Expression>) -> Result<(), LoweringError> {
        let record = self.semantic_return(statement)?;
        if record.value.is_some() != value.is_some()
            || record.value.zip(value).is_some_and(|(recorded, actual)| !std::ptr::eq(recorded, actual))
        {
            return Err(invariant("explicit return record contradicts its syntax"));
        }
        let expected_state = value.map_or(TypeState::Resolved(self.owner.analysis.types.unit()), |expression| {
            self.owner.analysis.expression_annotation(expression).map_or(TypeState::Error, |annotation| annotation.state)
        });
        if record.value_state != expected_state { return Err(invariant("explicit return record has a contradictory value type")); }
        let injection = record.unit_injection.clone();
        let value = if let Some(expression) = value {
            let Some(value) = self.expression(expression)? else { return Ok(()); };
            value
        } else {
            let unit = self.owner.analysis.types.unit();
            let value = self.constant(unit, ir::ConstantValue::Unit)?;
            if let Some(alternative) = injection.as_ref() {
                let result = self.owner.analysis.function_signature(self.function_id).result;
                let TypeState::Resolved(union) = result else { return Err(invariant("return function result is unresolved")); };
                self.inject(value, union, alternative, statement.span)?.ok_or_else(|| invariant("unit return injection diverged"))?
            } else { value }
        };
        self.emit_active_cleanups(statement.span)?;
        self.terminate(ir::TerminatorKind::Return(value), statement.span)
    }

    fn emit_active_cleanups(&mut self, span: Span) -> Result<(), LoweringError> {
        let cleanups = self.loops.iter().rev().filter_map(|context| context.cleanup.clone()).collect::<Vec<_>>();
        for iterable in cleanups { self.push(ir::OperationKind::EndIteration { iterable }, span)?; }
        Ok(())
    }

    fn loop_control(&mut self, statement: &'ast Statement) -> Result<(), LoweringError> {
        let target = self.semantic_loop_control(statement)?.target;
        let context = self.loops.iter().rev().find(|context| std::ptr::eq(context.source, target)).ok_or_else(|| invariant("loop control target is not active"))?;
        let destination = match &statement.kind {
            StatementKind::Break => context.break_target,
            StatementKind::Continue => context.continue_target,
            _ => return Err(invariant("non-loop-control reached loop-control lowering")),
        };
        self.terminate(ir::TerminatorKind::Jump(destination), statement.span)
    }

    fn if_statement(
        &mut self,
        branches: &'ast [ast::ConditionalStatementBranch],
        else_body: Option<&'ast StatementBody>,
        span: Span,
    ) -> Result<(), LoweringError> {
        let enclosing_narrowings = self.narrowings.clone();
        let mut fallthrough = Vec::new();
        for branch in branches {
            let outer_narrowings = self.narrowings.clone();
            let Some(condition) = self.expression(&branch.condition)? else {
                self.merge_paths(fallthrough)?;
                self.narrowings = enclosing_narrowings;
                return Ok(());
            };
            let condition_block = self.block.ok_or_else(|| invariant("conditional lost its condition block"))?;
            let body_block = self.new_block();
            let false_block = self.new_block();
            let location = self.location(branch.condition.span);
            self.function.blocks[condition_block.index()].terminate(ir::TerminatorKind::Branch { condition, then_block: body_block, else_block: false_block }, location);
            self.enter(body_block);
            self.activate_test_narrowing(&branch.condition, branch.body.span)?;
            self.statement_body(&branch.body)?;
            if let Some(block) = self.block.take() { fallthrough.push((block, branch.body.span)); }
            self.narrowings = outer_narrowings;
            self.enter(false_block);
        }
        if let Some(body) = else_body {
            self.statement_body(body)?;
            if let Some(block) = self.block.take() { fallthrough.push((block, body.span)); }
        } else if let Some(block) = self.block.take() {
            fallthrough.push((block, span));
        }
        self.merge_paths(fallthrough)?;
        self.narrowings = enclosing_narrowings;
        Ok(())
    }

    fn if_expression(
        &mut self,
        expression: &'ast Expression,
        branches: &'ast [ast::ConditionalExpressionBranch],
        else_branch: &'ast ExpressionBody,
    ) -> Result<Option<ir::Operand>, LoweringError> {
        let enclosing_narrowings = self.narrowings.clone();
        let result = self.temporary(self.raw_type(expression)?)?;
        let mut fallthrough = Vec::new();
        for branch in branches {
            let outer_narrowings = self.narrowings.clone();
            let Some(condition) = self.expression(&branch.condition)? else {
                let merged = self.merge_paths(fallthrough)?;
                self.narrowings = enclosing_narrowings;
                return if merged {
                    Ok(Some(ir::Operand::Copy(ir::Place::local(result))))
                } else { Ok(None) };
            };
            let condition_block = self.block.ok_or_else(|| invariant("if expression lost its condition block"))?;
            let body_block = self.new_block();
            let false_block = self.new_block();
            let location = self.location(branch.condition.span);
            self.function.blocks[condition_block.index()].terminate(ir::TerminatorKind::Branch { condition, then_block: body_block, else_block: false_block }, location);
            self.enter(body_block);
            self.activate_test_narrowing(&branch.condition, branch.body.span)?;
            if let Some(value) = self.expression_body(&branch.body)? {
                self.push(ir::OperationKind::Assign { destination: ir::Place::local(result), value }, branch.body.span)?;
                fallthrough.push((self.block.take().unwrap(), branch.body.span));
            }
            self.narrowings = outer_narrowings;
            self.enter(false_block);
        }
        if let Some(value) = self.expression_body(else_branch)? {
            self.push(ir::OperationKind::Assign { destination: ir::Place::local(result), value }, else_branch.span)?;
            fallthrough.push((self.block.take().unwrap(), else_branch.span));
        }
        let merged = self.merge_paths(fallthrough)?;
        self.narrowings = enclosing_narrowings;
        if merged { Ok(Some(ir::Operand::Copy(ir::Place::local(result)))) } else { Ok(None) }
    }

    fn merge_paths(&mut self, paths: Vec<(ir::BlockId, Span)>) -> Result<bool, LoweringError> {
        if paths.is_empty() { self.block = None; return Ok(false); }
        let merge = self.new_block();
        for (block, edge_span) in paths {
            let location = self.location(edge_span);
            self.function.blocks[block.index()].terminate(ir::TerminatorKind::Jump(merge), location);
        }
        self.enter(merge);
        Ok(true)
    }

    fn short_circuit(
        &mut self,
        expression: &'ast Expression,
        left: &'ast Expression,
        operator: ast::BinaryOperator,
        right: &'ast Expression,
    ) -> Result<Option<ir::Operand>, LoweringError> {
        let Some(left_value) = self.expression(left)? else { return Ok(None); };
        let left_value = self.stabilize(left_value, left.span)?;
        let result = self.temporary(self.raw_type(expression)?)?;
        self.push(ir::OperationKind::Copy { destination: result, operand: left_value.clone() }, left.span)?;
        let condition_block = self.block.ok_or_else(|| invariant("short-circuit expression lost its condition block"))?;
        let right_block = self.new_block();
        self.enter(right_block);
        let right_value = self.expression(right)?;
        if let Some(value) = right_value {
            self.push(ir::OperationKind::Assign { destination: ir::Place::local(result), value }, right.span)?;
        }
        let right_fallthrough = self.block.take();
        let merge = self.new_block();
        let (then_block, else_block) = if operator == ast::BinaryOperator::LogicalAnd { (right_block, merge) } else { (merge, right_block) };
        let location = self.location(expression.span);
        self.function.blocks[condition_block.index()].terminate(ir::TerminatorKind::Branch { condition: left_value, then_block, else_block }, location);
        if let Some(block) = right_fallthrough {
            let location = self.location(right.span);
            self.function.blocks[block.index()].terminate(ir::TerminatorKind::Jump(merge), location);
        }
        self.enter(merge);
        Ok(Some(ir::Operand::Copy(ir::Place::local(result))))
    }

    fn while_loop(
        &mut self,
        statement: &'ast Statement,
        condition: &'ast Expression,
        body: &'ast StatementBody,
    ) -> Result<(), LoweringError> {
        let condition_block = self.new_block();
        self.terminate(ir::TerminatorKind::Jump(condition_block), statement.span)?;
        self.enter(condition_block);
        let Some(condition_value) = self.expression(condition)? else { return Ok(()); };
        let body_block = self.new_block();
        let exit_block = self.new_block();
        self.terminate(ir::TerminatorKind::Branch { condition: condition_value, then_block: body_block, else_block: exit_block }, condition.span)?;
        self.loops.push(LoopContext { source: statement, continue_target: condition_block, break_target: exit_block, cleanup: None });
        self.enter(body_block);
        let outer_narrowings = self.narrowings.clone();
        self.activate_test_narrowing(condition, body.span)?;
        self.statement_body(body)?;
        self.narrowings = outer_narrowings;
        self.loops.pop();
        if self.block.is_some() { self.terminate(ir::TerminatorKind::Jump(condition_block), body.span)?; }
        self.enter(exit_block);
        Ok(())
    }

    fn for_loop(
        &mut self,
        statement: &'ast Statement,
        iterable_expression: &'ast Expression,
        body: &'ast StatementBody,
    ) -> Result<(), LoweringError> {
        let binding = self.owner.analysis.bindings.iter().find_map(|record| match record.node {
            BindingNode::Loop(candidate) if std::ptr::eq(candidate, statement) => Some(record.id),
            _ => None,
        }).ok_or_else(|| invariant("for loop lacks a binding identity"))?;
        let TypeState::Resolved(binding_type) = self.owner.analysis.binding_type(binding) else { return Err(invariant("for-loop binding type is unresolved")); };
        let source_name = match &statement.kind {
            StatementKind::For { binding, .. } => self.owner.analysis.identifier_text(binding.span).to_owned(),
            _ => return Err(invariant("non-for statement reached for-loop lowering")),
        };
        let mapped_binding_type = self.ty(binding_type)?;
        let binding_local = self.function.add_local(mapped_binding_type, Some(source_name), ir::LocalOrigin::Binding);
        self.bindings[binding.index()] = Some(binding_local);
        let Some(iterable) = self.expression(iterable_expression)? else { return Ok(()); };
        let iterable = self.stabilize(iterable, iterable_expression.span)?;
        let iterable_type = self.operand_frontend_type(&iterable)?;
        let (element_type, length_method) = match self.owner.analysis.types.get(iterable_type) {
            ResolvedType::List(element) => (*element, ir::BuiltinMethod::ListLen),
            ResolvedType::Map { key, .. } => (*key, ir::BuiltinMethod::MapLen),
            _ => return Err(invariant("for-loop iterable is not a list or map")),
        };
        if binding_type != element_type { return Err(invariant("for-loop binding type contradicts its iterable")); }
        let int = self.owner.analysis.types.primitive(PrimitiveType::Int);
        let length = self.temporary(int)?;
        let index = self.temporary(int)?;
        self.push(ir::OperationKind::BeginIteration { iterable: iterable.clone() }, iterable_expression.span)?;
        self.push(ir::OperationKind::Builtin { destination: length, method: length_method, receiver: iterable.clone(), arguments: Vec::new(), failure: None }, iterable_expression.span)?;
        let zero = self.constant(int, ir::ConstantValue::Integer(0))?;
        self.push(ir::OperationKind::Assign { destination: ir::Place::local(index), value: zero }, statement.span)?;
        let header = self.new_block();
        let body_block = self.new_block();
        let advance = self.new_block();
        let cleanup = self.new_block();
        let exit = self.new_block();
        self.terminate(ir::TerminatorKind::Jump(header), statement.span)?;

        self.enter(header);
        let bool_type = self.owner.analysis.types.primitive(PrimitiveType::Bool);
        let condition = self.temporary(bool_type)?;
        self.push(ir::OperationKind::Binary {
            destination: condition,
            operator: ir::BinaryOperator::Less,
            left: ir::Operand::Copy(ir::Place::local(index)),
            right: ir::Operand::Copy(ir::Place::local(length)),
        }, statement.span)?;
        self.terminate(ir::TerminatorKind::Branch { condition: ir::Operand::Copy(ir::Place::local(condition)), then_block: body_block, else_block: cleanup }, statement.span)?;

        self.loops.push(LoopContext { source: statement, continue_target: advance, break_target: cleanup, cleanup: Some(iterable.clone()) });
        self.enter(body_block);
        let outer_narrowings = self.narrowings.clone();
        self.push(ir::OperationKind::IterationValue {
            destination: binding_local,
            iterable: iterable.clone(),
            index: ir::Operand::Copy(ir::Place::local(index)),
        }, statement.span)?;
        self.statement_body(body)?;
        self.narrowings = outer_narrowings;
        self.loops.pop();
        if self.block.is_some() { self.terminate(ir::TerminatorKind::Jump(advance), body.span)?; }

        self.enter(advance);
        let one = self.constant(int, ir::ConstantValue::Integer(1))?;
        let advance_failure = self.failure(statement.span, ir::FailureOperation::IntegerAdd)?;
        self.push(ir::OperationKind::Check(ir::RuntimeCheck::IntegerOverflow {
            operation: ir::IntegerOperation::Add,
            left: ir::Operand::Copy(ir::Place::local(index)),
            right: one.clone(),
            failure: advance_failure,
        }), statement.span)?;
        self.push(ir::OperationKind::Binary {
            destination: index,
            operator: ir::BinaryOperator::Add,
            left: ir::Operand::Copy(ir::Place::local(index)),
            right: one,
        }, statement.span)?;
        self.terminate(ir::TerminatorKind::Jump(header), statement.span)?;

        self.enter(cleanup);
        self.push(ir::OperationKind::EndIteration { iterable }, statement.span)?;
        self.terminate(ir::TerminatorKind::Jump(exit), statement.span)?;
        self.enter(exit);
        Ok(())
    }

    fn switch_statement(
        &mut self,
        statement: &'ast Statement,
        value: &'ast Expression,
        arms: &'ast [ast::SwitchArm],
        else_body: Option<&'ast StatementBody>,
    ) -> Result<(), LoweringError> {
        let resolution = self.semantic_switch(statement)?.clone();
        if resolution.operand_union != self.expression_type(value)? || resolution.arms.len() != arms.len() {
            return Err(invariant("switch resolution contradicts its syntax or operand"));
        }
        let Some(union) = self.expression(value)? else { return Ok(()); };
        let union = self.stabilize(union, value.span)?;
        let source_alternatives = self.frontend_union_alternatives(resolution.operand_union)?;
        let arm_blocks = arms.iter().map(|_| self.new_block()).collect::<Vec<_>>();
        let needs_else = resolution.covered.len() < source_alternatives.len();
        let else_block = needs_else.then(|| self.new_block());
        let mut targets = Vec::with_capacity(source_alternatives.len());
        for alternative in &source_alternatives {
            let alternative_id = self.alternative_id(resolution.operand_union, alternative)?;
            let target = resolution.arms.iter().position(|arm| arm.alternative == *alternative)
                .map(|index| arm_blocks[index])
                .or(else_block)
                .ok_or_else(|| invariant("exhaustive switch has no target for an alternative"))?;
            targets.push((alternative_id, target));
        }
        self.terminate(ir::TerminatorKind::Switch { union: union.clone(), targets }, value.span)?;

        let outer_narrowings = self.narrowings.clone();
        let mut fallthrough = Vec::new();
        for (index, arm) in arms.iter().enumerate() {
            self.narrowings = outer_narrowings.clone();
            self.enter(arm_blocks[index]);
            let arm_resolution = &resolution.arms[index];
            if !std::ptr::eq(arm_resolution.arm, arm) { return Err(invariant("switch arm resolution is out of source order")); }
            if let Some(binding) = resolution.binding {
                let alternative = self.alternative_id(resolution.operand_union, &arm_resolution.alternative)?;
                let local = self.temporary(arm_resolution.payload)?;
                self.push(ir::OperationKind::UnionPayload { destination: local, union: union.clone(), alternative }, arm.label.span)?;
                self.narrowings.retain(|entry| entry.binding != binding);
                self.narrowings.push(Narrowing { binding, union: union.clone(), alternative, payload: arm_resolution.payload, local });
            }
            self.statement_body(&arm.body)?;
            if let Some(block) = self.block.take() { fallthrough.push((block, arm.body.span)); }
        }
        if let Some(else_block) = else_block {
            let body = else_body.ok_or_else(|| invariant("switch needs an else target but has no else body"))?;
            self.narrowings = outer_narrowings.clone();
            self.enter(else_block);
            self.statement_body(body)?;
            if let Some(block) = self.block.take() { fallthrough.push((block, body.span)); }
        }
        self.narrowings = outer_narrowings;
        self.merge_paths(fallthrough)?;
        Ok(())
    }

    fn try_expression(&mut self, expression: &'ast Expression, value: &'ast Expression) -> Result<Option<ir::Operand>, LoweringError> {
        let Some(operand) = self.expression(value)? else { return Ok(None); };
        let operand = self.stabilize(operand, value.span)?;
        let resolution = self.semantic_try(expression)?.clone();
        if resolution.operand_union != self.expression_type(value)? {
            return Err(invariant("postfix try resolution contradicts its operand"));
        }
        let alternatives = self.frontend_union_alternatives(resolution.operand_union)?;
        let success_count = alternatives.iter().filter(|alternative| !matches!(alternative, UnionAlternative::Error(_))).count();
        let result_type = resolution.widening.unwrap_or(resolution.success);
        let result = self.temporary(result_type)?;
        let blocks = alternatives.iter().map(|_| self.new_block()).collect::<Vec<_>>();
        let targets = alternatives.iter().zip(&blocks).map(|(alternative, block)| {
            Ok((self.alternative_id(resolution.operand_union, alternative)?, *block))
        }).collect::<Result<Vec<_>, LoweringError>>()?;
        self.terminate(ir::TerminatorKind::Switch { union: operand.clone(), targets }, resolution.operator_span)?;
        let mut success_paths = Vec::new();
        for (alternative, block) in alternatives.iter().zip(blocks) {
            self.enter(block);
            let alternative_id = self.alternative_id(resolution.operand_union, alternative)?;
            let payload_type = alternative_payload(alternative);
            let payload_local = self.temporary(payload_type)?;
            self.push(ir::OperationKind::UnionPayload { destination: payload_local, union: operand.clone(), alternative: alternative_id }, resolution.operator_span)?;
            let payload = ir::Operand::Copy(ir::Place::local(payload_local));
            if matches!(alternative, UnionAlternative::Error(_)) {
                if *alternative != resolution.source_error { return Err(invariant("postfix try selects a contradictory Error alternative")); }
                match &resolution.action {
                    TryAction::Propagate { function, destination_union, destination_error } => {
                        if *function != self.function_id || alternative_payload(destination_error) != payload_type {
                            return Err(invariant("postfix try propagation action is contradictory"));
                        }
                        let returned = self.inject(payload, *destination_union, destination_error, resolution.operator_span)?.ok_or_else(|| invariant("Error reinjection diverged"))?;
                        self.emit_active_cleanups(resolution.operator_span)?;
                        self.terminate(ir::TerminatorKind::Return(returned), resolution.operator_span)?;
                    }
                    TryAction::Panic { function } => {
                        if *function != self.function_id { return Err(invariant("postfix try panic action names the wrong function")); }
                        let failure = self.failure(resolution.operator_span, ir::FailureOperation::UnhandledError)?;
                        self.terminate(ir::TerminatorKind::ErrorPanic { payload, failure }, resolution.operator_span)?;
                    }
                }
            } else {
                let success = if success_count == 1 && resolution.widening.is_none() {
                    if payload_type != resolution.success { return Err(invariant("single-success try has the wrong result type")); }
                    payload
                } else {
                    self.inject(payload, result_type, alternative, resolution.operator_span)?.ok_or_else(|| invariant("try success injection diverged"))?
                };
                self.push(ir::OperationKind::Assign { destination: ir::Place::local(result), value: success }, resolution.operator_span)?;
                success_paths.push((self.block.take().unwrap(), resolution.operator_span));
            }
        }
        if !self.merge_paths(success_paths)? { return Err(invariant("postfix try has no success path")); }
        Ok(Some(ir::Operand::Copy(ir::Place::local(result))))
    }

    fn frontend_union_alternatives(&self, union: analysis::TypeId) -> Result<Vec<UnionAlternative>, LoweringError> {
        match self.owner.analysis.types.get(union) {
            ResolvedType::Union(alternatives) => Ok(alternatives.to_vec()),
            ResolvedType::Nominal(definition) => match &self.owner.analysis.type_definition(*definition).kind {
                TypeDefinitionKind::Union { alternatives, .. } => Ok(alternatives.to_vec()),
                _ => Err(invariant("union identity names a non-union nominal")),
            },
            _ => Err(invariant("union identity names a non-union type")),
        }
    }

    fn assignment(&mut self, target: &'ast ast::AssignmentTarget, operator: AssignmentOperator, operator_span: Span, value: &'ast Expression, span: Span) -> Result<(), LoweringError> {
        let annotation = self.owner.analysis.assignment_target(target).ok_or_else(|| invariant("assignment target fact is missing"))?.clone();
        let root = annotation.root.ok_or_else(|| invariant("assignment target has no root"))?;
        let authorizations = self.owner.semantic.mutability.iter().filter(|resolution| {
            matches!(resolution.subject, MutabilitySubject::Assignment(subject) if std::ptr::eq(subject, target))
        }).collect::<Vec<_>>();
        let [authorization] = authorizations.as_slice() else {
            return Err(invariant("assignment does not have exactly one mutation authorization"));
        };
        if authorization.root != root
            || !matches!(&authorization.path, MutabilityPath::Assignment(path) if path.len() == annotation.steps.len())
        {
            return Err(invariant("assignment mutation authorization contradicts its typed target"));
        }
        let root_use = self.owner.analysis.name_use(&target.root).ok_or_else(|| invariant("assignment root has no name-use fact"))?;
        if root_use.resolution != analysis::NameResolution::Binding(root) || root_use.access != authorization.access {
            return Err(invariant("assignment mutation authorization contradicts its root access"));
        }
        let expected_operation = match annotation.steps.last() {
            None => MutabilityOperation::Rebind,
            Some(AccessPathStep::StructMember { .. }) => MutabilityOperation::ReplaceStructField,
            Some(AccessPathStep::ListIndex { .. }) => MutabilityOperation::ReplaceListElement,
            Some(AccessPathStep::MapIndex { .. }) => MutabilityOperation::ReplaceMapElement,
            Some(AccessPathStep::TupleMember { .. }) => return Err(invariant("tuple assignment reached lowering")),
        };
        if authorization.operation != expected_operation {
            return Err(invariant("assignment mutation authorization has the wrong operation"));
        }
        let root_local = if annotation.steps.is_empty() {
            self.bindings.get(root.index()).and_then(|item| *item)
        } else {
            self.narrowings.iter().rev().find(|entry| entry.binding == root).map(|entry| entry.local)
                .or_else(|| self.bindings.get(root.index()).and_then(|item| *item))
        }.ok_or_else(|| invariant("assignment root has not been allocated"))?;
        let mut place = ir::Place::local(root_local);
        if !annotation.steps.is_empty() {
            let root_operand = self.stabilize(ir::Operand::Copy(place), target.root.span)?;
            let ir::Operand::Copy(root_place) = root_operand else { unreachable!() };
            place = root_place;
        }
        for step in annotation.steps.iter() {
            match step {
                AccessPathStep::StructMember { declaration, field, storage, .. } => place.projections.push(ir::Projection::StructField { definition: self.definition(*declaration)?, field: self.field(*declaration, *field)?, storage: map_storage(*storage) }),
                AccessPathStep::TupleMember { declaration, position, .. } => place.projections.push(ir::Projection::TupleField { definition: self.definition(*declaration)?, field: self.field(*declaration, *position)? }),
                AccessPathStep::ListIndex { suffix, .. } | AccessPathStep::MapIndex { suffix, .. } => {
                    let ast::AssignmentTargetSuffixKind::Index(index) = &suffix.kind else { return Err(invariant("index path step has no index syntax")); };
                    let Some(index_value) = self.expression(index)? else { return Ok(()); }; let index_value = self.materialize(index_value, index.span)?;
                    let ir::Operand::Copy(index_place) = index_value else { return Err(invariant("assignment index did not materialize")); };
                    let projection = if matches!(step, AccessPathStep::ListIndex { .. }) {
                        ir::Projection::ListIndex { index: index_place.local, failure: self.failure(suffix.span, ir::FailureOperation::ListIndex)? }
                    } else {
                        ir::Projection::MapIndex { key: index_place.local, failure: self.failure(suffix.span, ir::FailureOperation::MapIndex)? }
                    };
                    place.projections.push(projection);
                }
            }
        }
        let assigned = if operator == AssignmentOperator::Assign {
            let Some(value) = self.expression(value)? else { return Ok(()); };
            value
        } else {
            let old_type = match annotation.state { TypeState::Resolved(ty) => ty, _ => return Err(invariant("compound assignment target type is unresolved")) };
            let old_local = self.temporary(old_type)?;
            self.push(ir::OperationKind::Copy { destination: old_local, operand: ir::Operand::Copy(place.clone()) }, target.span)?;
            let Some(right) = self.expression(value)? else { return Ok(()); };
            let right = self.stabilize(right, value.span)?;
            self.checked_binary(old_type, map_assignment(operator)?, ir::Operand::Copy(ir::Place::local(old_local)), right, operator_span, span)?
        };
        self.push(ir::OperationKind::Assign { destination: place, value: assigned }, span)?;
        if annotation.steps.is_empty() { self.narrowings.retain(|entry| entry.binding != root); }
        Ok(())
    }
}

fn projected_ir_type(program: &ir::Program, ty: ir::TypeId, projection: &ir::Projection) -> Result<ir::TypeId, LoweringError> {
    Ok(match projection {
        ir::Projection::StructField { definition, field, .. } => match &program.definitions[definition.index()].layout { ir::DefinitionLayout::Struct(fields) => fields.get(field.index()).ok_or_else(|| invariant("invalid projected struct field"))?.ty, _ => return Err(invariant("struct projection selects a non-struct")) },
        ir::Projection::TupleField { definition, field } => match &program.definitions[definition.index()].layout { ir::DefinitionLayout::Tuple(fields) => *fields.get(field.index()).ok_or_else(|| invariant("invalid projected tuple field"))?, _ => return Err(invariant("tuple projection selects a non-tuple")) },
        ir::Projection::ListIndex { .. } => match &program.types[ty.index()] { ir::Type::List(element) => *element, _ => return Err(invariant("list projection selects a non-list")) },
        ir::Projection::MapIndex { .. } => match &program.types[ty.index()] { ir::Type::Map { value, .. } => *value, _ => return Err(invariant("map projection selects a non-map")) },
    })
}

fn argument_value(argument: &Argument) -> &Expression { match &argument.kind { ArgumentKind::Positional(value) | ArgumentKind::Named { value, .. } => value } }
fn strip_expression_parentheses(mut expression: &Expression) -> &Expression { while let ExpressionKind::Parenthesized(inner) = &expression.kind { expression = inner; } expression }
fn alternative_payload(alternative: &UnionAlternative) -> analysis::TypeId { match alternative { UnionAlternative::Untagged(ty) | UnionAlternative::Tagged { payload: ty, .. } | UnionAlternative::Error(ty) => *ty } }
fn invariant(message: impl Into<String>) -> LoweringError { LoweringError::Invariant(message.into()) }
fn map_primitive(value: PrimitiveType) -> ir::PrimitiveType { match value { PrimitiveType::Int => ir::PrimitiveType::Int, PrimitiveType::Float => ir::PrimitiveType::Float, PrimitiveType::Str => ir::PrimitiveType::Str, PrimitiveType::Bool => ir::PrimitiveType::Bool, PrimitiveType::Char => ir::PrimitiveType::Char } }
fn map_storage(value: MemberStorage) -> ir::MemberStorage { match value { MemberStorage::Inline => ir::MemberStorage::Inline, MemberStorage::Referenced => ir::MemberStorage::Referenced } }
fn map_unary(value: ast::UnaryOperator) -> ir::UnaryOperator { match value { ast::UnaryOperator::LogicalNot => ir::UnaryOperator::LogicalNot, ast::UnaryOperator::BitwiseNot => ir::UnaryOperator::BitwiseNot, ast::UnaryOperator::Plus => ir::UnaryOperator::Plus, ast::UnaryOperator::Minus => ir::UnaryOperator::Minus } }
fn map_binary(value: ast::BinaryOperator) -> Result<ir::BinaryOperator, LoweringError> { Ok(match value { ast::BinaryOperator::LogicalOr | ast::BinaryOperator::LogicalAnd => return Err(invariant("short-circuit operator reached ordinary binary lowering")), ast::BinaryOperator::BitwiseOr => ir::BinaryOperator::BitwiseOr, ast::BinaryOperator::BitwiseXor => ir::BinaryOperator::BitwiseXor, ast::BinaryOperator::BitwiseAnd => ir::BinaryOperator::BitwiseAnd, ast::BinaryOperator::Equal => ir::BinaryOperator::Equal, ast::BinaryOperator::NotEqual => ir::BinaryOperator::NotEqual, ast::BinaryOperator::Less => ir::BinaryOperator::Less, ast::BinaryOperator::LessEqual => ir::BinaryOperator::LessEqual, ast::BinaryOperator::Greater => ir::BinaryOperator::Greater, ast::BinaryOperator::GreaterEqual => ir::BinaryOperator::GreaterEqual, ast::BinaryOperator::In => ir::BinaryOperator::In, ast::BinaryOperator::ShiftLeft => ir::BinaryOperator::ShiftLeft, ast::BinaryOperator::ShiftRight => ir::BinaryOperator::ShiftRight, ast::BinaryOperator::Add => ir::BinaryOperator::Add, ast::BinaryOperator::Subtract => ir::BinaryOperator::Subtract, ast::BinaryOperator::Multiply => ir::BinaryOperator::Multiply, ast::BinaryOperator::Divide => ir::BinaryOperator::Divide, ast::BinaryOperator::Remainder => ir::BinaryOperator::Remainder }) }
fn map_assignment(value: AssignmentOperator) -> Result<ir::BinaryOperator, LoweringError> { Ok(match value { AssignmentOperator::Assign => return Err(invariant("simple assignment reached compound lowering")), AssignmentOperator::Add => ir::BinaryOperator::Add, AssignmentOperator::Subtract => ir::BinaryOperator::Subtract, AssignmentOperator::Multiply => ir::BinaryOperator::Multiply, AssignmentOperator::Divide => ir::BinaryOperator::Divide, AssignmentOperator::Remainder => ir::BinaryOperator::Remainder, AssignmentOperator::BitwiseAnd => ir::BinaryOperator::BitwiseAnd, AssignmentOperator::BitwiseOr => ir::BinaryOperator::BitwiseOr, AssignmentOperator::BitwiseXor => ir::BinaryOperator::BitwiseXor, AssignmentOperator::ShiftLeft => ir::BinaryOperator::ShiftLeft, AssignmentOperator::ShiftRight => ir::BinaryOperator::ShiftRight }) }
fn map_builtin(value: BuiltinMethod) -> ir::BuiltinMethod { match value { BuiltinMethod::ListAppend => ir::BuiltinMethod::ListAppend, BuiltinMethod::ListRemoveIndex => ir::BuiltinMethod::ListRemoveIndex, BuiltinMethod::ListLen => ir::BuiltinMethod::ListLen, BuiltinMethod::MapRemoveKey => ir::BuiltinMethod::MapRemoveKey, BuiltinMethod::MapLen => ir::BuiltinMethod::MapLen, BuiltinMethod::StrLen => ir::BuiltinMethod::StrLen } }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceFile;
    use std::path::PathBuf;

    fn lower_text(text: &str) -> Result<ir::Program, LoweringError> {
        let source = SourceFile::new(PathBuf::from("lowering.sao2"), text.to_owned());
        let syntax = crate::parser::parse(&source).expect("test source must parse");
        let mut analysis = crate::analysis::analyze(&source, &syntax);
        assert!(analysis.diagnostics.is_empty(), "{}", analysis.diagnostics);
        let semantic = crate::semantic::analyze(&mut analysis);
        assert!(semantic.diagnostics.is_empty(), "{}", semantic.diagnostics);
        crate::semantic::validate_handoff(&analysis, &semantic).expect("valid frontend handoff");
        lower(&analysis, &semantic)
    }

    #[test]
    fn lowers_and_validates_straight_line_values_deterministically() {
        let text = concat!(
            "type Point(x int, y int); type Pair(int, float); ",
            "type Choice(int | float); type Tagged(A(int) | B(float)); ",
            "type Outer(Choice | str); type Node(value int, next &Node); ",
            "fn left() int { right() } fn right() int { left() } ",
            "fn failure() int | Error(str) { Error(\"failed\") } ",
            "fn nested() Outer { Outer(Choice(1)) } ",
            "fn sum(a int, b int) int { a + b } ",
            "fn main() int { ",
            "p := Point(y = 2, x = 1); pair := Pair(3, 4.0); ",
            "var items := [1, 2]; var table := {1: 3}; ",
            "items[0] += p.x; table[1] = pair.0; ",
            "empty := [] : [int]; empty_map := {} : {int: str}; ",
            "choice := Choice(1); tagged := Tagged.A(1); c := \"x\"[0]; got := table[1]; ",
            "f := float(items[0]); i := int(f); ",
            "items.append(i); items.removeIndex(0); table.removeKey(1); ",
            "println(items.len()); println(table.len()); println(\"x\".len()); sum(i, 1) }",
        );
        let first = lower_text(text).expect("straight-line lowering must succeed");
        let second = lower_text(text).expect("repeated lowering must succeed");
        assert!(first.validate().is_ok());
        assert_eq!(first.render(), second.render());
        assert!(first.render().contains("inject"));
        assert!(first.render().contains("int-to-float"));
    }

    #[test]
    fn lowers_an_unused_nominal_definition_with_its_canonical_type() {
        let program = lower_text("type Number(int); fn main() { print(\"x\"); }")
            .expect("unused nominal definitions must lower");
        assert!(
            program
                .types
                .contains(&ir::Type::Nominal(ir::DefinitionId::from_index(0)))
        );
        assert!(program.validate().is_ok());
    }

    #[test]
    fn lowers_postfix_try_without_a_pending_stage() {
        let program = lower_text("type Result(int | Error(str)); fn main() { value := Result(1); value?; }").expect("postfix try must lower");
        assert!(program.render().contains("error-panic"));
    }

    #[test]
    fn lowers_union_tests_narrowing_and_exhaustive_switches() {
        let text = concat!(
            "type Value(A(int) | B(str)); ",
            "fn inspect(value Value) int { ",
            "if value is A { narrowed := value; println(narrowed); } ",
            "switch value { A: return value; B: println(value); } ",
            "0 } fn main() int { inspect(Value.A(1)) }",
        );
        let first = lower_text(text).expect("union control flow must lower");
        let second = lower_text(text).expect("union control flow must be deterministic");
        let rendered = first.render();
        assert_eq!(rendered, second.render());
        assert!(rendered.contains("union-test"));
        assert!(rendered.contains("payload alt"));
        assert!(rendered.contains("switch"));
        assert!(first.validate().is_ok());
    }

    #[test]
    fn lowers_try_success_unions_propagation_and_iteration_cleanup() {
        let text = concat!(
            "type Many(A(int) | B(str) | Error(char)); ",
            "fn pass(value Many) A(int) | B(str) | Error(char) { value? } ",
            "fn in_loop(values [Many]) () | Error(char) { for value in values { value?; } } ",
            "fn main() { pass(Many.A(1)); }",
        );
        let program = lower_text(text).expect("postfix try propagation must lower");
        let rendered = program.render();
        assert!(rendered.matches("switch").count() >= 2);
        assert!(rendered.contains("inject"));
        assert!(rendered.contains("end-iteration"));
        assert!(program.validate().is_ok());
    }

    #[test]
    fn lowers_conditionals_short_circuit_returns_and_panic() {
        let text = concat!(
            "fn choose(flag bool) int { if flag: return 1; else: return 2; } ",
            "fn value(flag bool) int { (if flag: 3 else: 4) } ",
            "fn stops() bool { true || panic(\"not reached\") } ",
            "fn main() int { choose(true) + value(false) }",
        );
        let first = lower_text(text).expect("control flow must lower");
        let second = lower_text(text).expect("control flow must lower deterministically");
        let rendered = first.render();
        assert_eq!(rendered, second.render());
        assert!(rendered.contains("branch"));
        assert!(rendered.contains("panic const"));
        assert!(rendered.contains("return"));
        assert!(first.validate().is_ok());
    }

    #[test]
    fn lowers_while_and_indexed_iteration_with_cleanup() {
        let text = concat!(
            "fn first(values [int], table {int: int}) int { ",
            "while false: continue; ",
            "for value in values { for key in table { return value; } } ",
            "return 0; } ",
            "fn main() int { first([1, 2], {3: 4}) }",
        );
        let program = lower_text(text).expect("loops must lower");
        let rendered = program.render();
        assert!(rendered.contains("begin-iteration"));
        assert!(rendered.contains("iteration-value"));
        assert!(rendered.matches("end-iteration").count() >= 4);
        assert!(rendered.contains("less"));
        assert!(program.validate().is_ok());
    }

    #[test]
    fn lowers_runtime_failures_with_ordered_checks_and_compact_sites() {
        let text = concat!(
            "fn main() int { ",
            "a := 8; b := 2; x := 4.0; ",
            "sum := a + b; difference := a - b; product := a * b; ",
            "quotient := a / b; remainder := a % b; shifted := a << b; other := a >> b; ",
            "float_quotient := x / 2.0; converted := int(float_quotient); ",
            "var items := [sum]; var table := {a: quotient}; ",
            "items[0] = table[a]; items.append(converted); items.removeIndex(-1); table.removeKey(a); ",
            "println(\"index\"[-1]); shifted + other + difference + product + remainder }",
        );
        let program = lower_text(text).expect("runtime checks must lower");
        let rendered = program.render();
        assert!(rendered.contains("integer-add-overflow"));
        assert!(rendered.contains("check division"));
        assert!(rendered.contains("check shift-range"));
        assert!(rendered.contains("check finite-float"));
        assert!(rendered.contains("check numeric-float-to-int"));
        assert!(rendered.contains("iteration-unlocked"));
        assert!(rendered.contains("list-index"));
        assert!(rendered.contains("map-index"));
        assert!(rendered.contains("string-index"));
        assert!(rendered.contains(" output\n") || rendered.contains(" output\r\n") || rendered.contains(" output"));
        assert!(program.validate().is_ok());
    }

    #[test]
    fn lowers_signed_minimum_without_a_spurious_negation_check() {
        let program = lower_text("fn main() int { -9223372036854775808 }").expect("signed minimum must lower");
        let rendered = program.render();
        assert!(rendered.contains("const ty"));
        assert!(rendered.contains(" -9223372036854775808"));
        assert!(!rendered.contains("integer-negation"));
        assert!(program.validate().is_ok());
    }

    #[test]
    fn compound_indexing_checks_the_read_and_write_and_mutation_checks_the_lock() {
        let program = lower_text(concat!(
            "fn update(var items [int], delta int) int { items[0] += delta; 0 } ",
            "fn main() { var items := [1]; update(items, 2); ",
            "for item in items { items.append(item); break; } }",
        )).expect("checked mutations must lower");
        let rendered = program.render();
        let compound_site = program.failure_sites.iter().enumerate()
            .find(|(_, site)| site.operation == ir::FailureOperation::ListIndex)
            .map(|(index, _)| format!("! fail{index}"))
            .expect("compound list access must have a failure site");
        assert_eq!(rendered.matches(&compound_site).count(), 2);
        assert!(rendered.contains("iteration-unlocked"));
        assert!(rendered.contains("list-append"));
        assert!(program.validate().is_ok());
    }

    #[test]
    fn missing_flow_facts_are_lowering_invariants() {
        let source = SourceFile::new(PathBuf::from("flow.sao2"), "fn main() { if true: return; }".to_owned());
        let syntax = crate::parser::parse(&source).unwrap();
        let mut analysis = crate::analysis::analyze(&source, &syntax);
        let mut semantic = crate::semantic::analyze(&mut analysis);
        assert!(semantic.diagnostics.is_empty());
        semantic.flow.summaries.clear();
        assert!(matches!(lower(&analysis, &semantic), Err(LoweringError::Invariant(_))));
    }

    #[test]
    fn corrupted_return_completion_and_loop_targets_are_invariants() {
        {
            let source = SourceFile::new(PathBuf::from("return.sao2"), "fn main() { return; }".to_owned());
            let syntax = crate::parser::parse(&source).unwrap();
            let mut analysis = crate::analysis::analyze(&source, &syntax);
            let mut semantic = crate::semantic::analyze(&mut analysis);
            semantic.flow.returns.clear();
            assert!(matches!(lower(&analysis, &semantic), Err(LoweringError::Invariant(_))));
        }
        {
            let source = SourceFile::new(PathBuf::from("completion.sao2"), "fn main() {}".to_owned());
            let syntax = crate::parser::parse(&source).unwrap();
            let mut analysis = crate::analysis::analyze(&source, &syntax);
            let mut semantic = crate::semantic::analyze(&mut analysis);
            semantic.flow.completions.clear();
            assert!(matches!(lower(&analysis, &semantic), Err(LoweringError::Invariant(_))));
        }
        {
            let source = SourceFile::new(PathBuf::from("loop.sao2"), "fn main() { while true: break; }".to_owned());
            let syntax = crate::parser::parse(&source).unwrap();
            let mut analysis = crate::analysis::analyze(&source, &syntax);
            let mut semantic = crate::semantic::analyze(&mut analysis);
            let control = semantic.flow.loop_controls[0].statement;
            semantic.flow.loop_controls[0].target = control;
            assert!(matches!(lower(&analysis, &semantic), Err(LoweringError::Invariant(_))));
        }
    }

    #[test]
    fn missing_frontend_facts_are_lowering_invariants() {
        let source = SourceFile::new(
            PathBuf::from("corrupt.sao2"),
            "type Pair(int); fn main() int { pair := Pair(1); converted := float(pair.0); int(converted) }".to_owned(),
        );
        let syntax = crate::parser::parse(&source).unwrap();
        let mut analysis = crate::analysis::analyze(&source, &syntax);
        let semantic = crate::semantic::analyze(&mut analysis);
        assert!(semantic.diagnostics.is_empty());
        analysis.projections.clear();
        let error = lower(&analysis, &semantic).unwrap_err();
        assert!(matches!(error, LoweringError::Invariant(_)));
    }
}
