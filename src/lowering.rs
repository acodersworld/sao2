//! Straight-line lowering from the validated frontend handoff to owned IR.

use std::fmt;

use crate::analysis::{
    self, AccessPathStep, Analysis, BindingNode, BuiltinMethod, CallTarget, CallableId,
    ConstructorKind, IntrinsicId, LiteralValue, MemberStorage, ProjectionKind, ResolvedType,
    TypeDefinitionKind, TypeState, UnionAlternative,
};
use crate::ast::{
    self, Argument, ArgumentKind, AssignmentOperator, Expression, ExpressionKind, PrimitiveType,
    Statement, StatementKind,
};
use crate::ir;
use crate::semantic::{MutabilityOperation, MutabilityPath, MutabilitySubject, SemanticResult};
use crate::source::Span;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingStage { Stage3, Stage4 }

#[derive(Debug)]
pub(crate) enum LoweringError {
    Invariant(String),
    PendingStage { stage: PendingStage, construct: &'static str, span: Span },
}

impl fmt::Display for LoweringError {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invariant(message) => write!(output, "IR lowering invariant violated: {message}"),
            Self::PendingStage { stage, construct, span } => write!(
                output,
                "{construct} at bytes {}..{} awaits {:?} lowering",
                span.start, span.end, stage,
            ),
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
            let mut body = BodyLowerer { owner: self, function: &mut function, block: entry, bindings };
            body.block(&signature.node.body)?;
            let value = if let Some(value) = signature.node.body.value.as_deref() {
                body.expression(value)?
            } else {
                let frontend_unit = body.owner.analysis.types.unit();
                let unit = body.owner.map_type(frontend_unit)?;
                let value = ir::Operand::Constant(ir::Constant { ty: unit, value: ir::ConstantValue::Unit });
                let completion_injection = body.semantic_completion(signature.id, &signature.node.body).and_then(|item| item.unit_injection.clone());
                if let Some(alternative) = completion_injection.as_ref() {
                        let TypeState::Resolved(union) = signature.result else { unreachable!() };
                        body.inject(value, union, alternative, signature.node.body.span)?
                } else { value }
            };
            let location = body.location(signature.node.body.span);
            body.current_block().terminate(ir::TerminatorKind::Return(value), location);
        }
        self.program.functions[index] = function;
        Ok(())
    }
}

struct BodyLowerer<'a, 'b, 'source, 'ast> {
    owner: &'a mut Lowerer<'b, 'source, 'ast>,
    function: &'a mut ir::Function,
    block: ir::BlockId,
    bindings: Vec<Option<ir::LocalId>>,
}

impl<'a, 'b, 'source, 'ast> BodyLowerer<'a, 'b, 'source, 'ast> {
    fn current_block(&mut self) -> &mut ir::BasicBlock { &mut self.function.blocks[self.block.index()] }
    fn location(&mut self, span: Span) -> ir::LocationId { self.owner.program.intern_location(ir::ByteSpan::new(span.start, span.end)) }
    fn semantic_completion(&self, function: analysis::FunctionId, block: &ast::Block) -> Option<&crate::semantic::FunctionCompletion<'ast>> {
        self.owner.semantic.flow.completions.iter().find(|item| item.function == function && std::ptr::eq(item.body, block))
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
    fn push(&mut self, kind: ir::OperationKind, span: Span) {
        let location = self.location(span);
        self.current_block().push(kind, location);
    }

    fn block(&mut self, block: &'ast ast::Block) -> Result<(), LoweringError> {
        for statement in &block.statements { self.statement(statement)?; }
        Ok(())
    }

    fn statement(&mut self, statement: &'ast Statement) -> Result<(), LoweringError> {
        match &statement.kind {
            StatementKind::Local { name, initializer, .. } => {
                let binding = self.owner.analysis.bindings.iter().find_map(|record| match record.node {
                    BindingNode::Local(node) if std::ptr::eq(node, statement) => Some(record.id),
                    _ => None,
                }).ok_or_else(|| invariant("local declaration lacks a binding identity"))?;
                let TypeState::Resolved(ty) = self.owner.analysis.binding_type(binding) else { return Err(invariant("local binding has no resolved type")); };
                let mapped = self.ty(ty)?;
                let source_name = self.owner.analysis.identifier_text(name.span).to_owned();
                let local = self.function.add_local(mapped, Some(source_name), ir::LocalOrigin::Binding);
                self.bindings[binding.index()] = Some(local);
                let value = self.expression(initializer)?;
                self.push(ir::OperationKind::Assign { destination: ir::Place::local(local), value }, statement.span);
            }
            StatementKind::Assignment { target, operator, value, .. } => self.assignment(target, *operator, value, statement.span)?,
            StatementKind::Expression(expression) => {
                if self.is_panic(expression) {
                    return Err(pending(PendingStage::Stage3, "terminal panic", expression.span));
                }
                self.expression(expression)?;
            }
            StatementKind::Return(_) => return Err(pending(PendingStage::Stage3, "explicit return", statement.span)),
            StatementKind::If { .. } => return Err(pending(PendingStage::Stage3, "conditional statement", statement.span)),
            StatementKind::While { .. } | StatementKind::For { .. } => return Err(pending(PendingStage::Stage3, "loop", statement.span)),
            StatementKind::Switch { .. } => return Err(pending(PendingStage::Stage3, "switch", statement.span)),
            StatementKind::Break => return Err(pending(PendingStage::Stage3, "break", statement.span)),
            StatementKind::Continue => return Err(pending(PendingStage::Stage3, "continue", statement.span)),
            StatementKind::Block(_) => return Err(pending(PendingStage::Stage3, "statement block", statement.span)),
        }
        Ok(())
    }

    fn expression(&mut self, expression: &'ast Expression) -> Result<ir::Operand, LoweringError> {
        let value = self.expression_core(expression)?;
        if let Some(injection) = self.owner.analysis.union_injection(expression) {
            let union = injection.union_type;
            let alternative = injection.alternative.clone();
            self.inject(value, union, &alternative, expression.span)
        } else { Ok(value) }
    }

    fn expression_core(&mut self, expression: &'ast Expression) -> Result<ir::Operand, LoweringError> {
        match &expression.kind {
            ExpressionKind::Unit => {
                let ty = self.raw_type(expression)?;
                self.constant(ty, ir::ConstantValue::Unit)
            }
            ExpressionKind::Integer
            | ExpressionKind::Float
            | ExpressionKind::String(_)
            | ExpressionKind::Character(_)
            | ExpressionKind::Boolean(_) => self.literal_constant(expression),
            ExpressionKind::Identifier(identifier) => {
                let Some(analysis::NameResolution::Binding(binding)) = self.owner.analysis.name_use(identifier).map(|item| item.resolution) else { return Err(invariant("value identifier is not a binding")); };
                let local = self.bindings.get(binding.index()).and_then(|item| *item).ok_or_else(|| invariant("binding has not been allocated"))?;
                Ok(ir::Operand::Copy(ir::Place::local(local)))
            }
            ExpressionKind::Parenthesized(inner) => self.expression(inner),
            ExpressionKind::Conversion { operand, .. } => {
                let resolution = *self.owner.analysis.numeric_conversion(expression).ok_or_else(|| invariant("numeric conversion fact is missing"))?;
                if !self.owner.analysis.types.contains(resolution.source) || !self.owner.analysis.types.contains(resolution.destination) {
                    return Err(invariant("numeric conversion contains an invalid type identity"));
                }
                let conversion = if matches!(self.owner.analysis.types.get(resolution.source), ResolvedType::Primitive(PrimitiveType::Int)) { ir::NumericConversion::IntToFloat } else { ir::NumericConversion::FloatToInt };
                let operand = self.expression(operand)?;
                let destination = self.temporary(resolution.destination)?;
                self.push(ir::OperationKind::Convert { destination, conversion, operand }, expression.span);
                Ok(ir::Operand::Copy(ir::Place::local(destination)))
            }
            ExpressionKind::Unary { operator, operand, .. } => {
                let operand = self.expression(operand)?;
                let result = self.raw_type(expression)?;
                let destination = self.temporary(result)?;
                self.push(ir::OperationKind::Unary { destination, operator: map_unary(*operator), operand }, expression.span);
                Ok(ir::Operand::Copy(ir::Place::local(destination)))
            }
            ExpressionKind::Binary { left, operator, right, .. } => {
                if matches!(operator, ast::BinaryOperator::LogicalAnd | ast::BinaryOperator::LogicalOr) { return Err(pending(PendingStage::Stage3, "short-circuit operator", expression.span)); }
                let left_span = left.span;
                let left = self.expression(left)?;
                let left = self.stabilize(left, left_span)?;
                let right = self.expression(right)?;
                let result = self.raw_type(expression)?;
                let destination = self.temporary(result)?;
                self.push(ir::OperationKind::Binary { destination, operator: map_binary(*operator)?, left, right }, expression.span);
                Ok(ir::Operand::Copy(ir::Place::local(destination)))
            }
            ExpressionKind::List(elements) => self.list(expression, elements),
            ExpressionKind::Map(entries) => self.map(expression, entries),
            ExpressionKind::TypedEmptyList(_) => {
                let raw = self.raw_type(expression)?;
                let ty = self.ty(raw)?;
                self.aggregate(expression, ir::Aggregate::List { ty, elements: Vec::new() })
            }
            ExpressionKind::TypedEmptyMap(_) => {
                let raw = self.raw_type(expression)?;
                let ty = self.ty(raw)?;
                self.aggregate(expression, ir::Aggregate::Map { ty, entries: Vec::new() })
            }
            ExpressionKind::Call { arguments, .. } => self.call(expression, arguments),
            ExpressionKind::Member { value, .. } => self.member(expression, value),
            ExpressionKind::Index { value, index } => self.index(expression, value, index),
            ExpressionKind::Block(_) => Err(pending(PendingStage::Stage3, "block expression", expression.span)),
            ExpressionKind::If { .. } => Err(pending(PendingStage::Stage3, "if expression", expression.span)),
            ExpressionKind::Is { .. } => Err(pending(PendingStage::Stage4, "union test", expression.span)),
            ExpressionKind::Try { .. } => Err(pending(PendingStage::Stage4, "postfix try", expression.span)),
        }
    }

    fn constant(&mut self, ty: analysis::TypeId, value: ir::ConstantValue) -> Result<ir::Operand, LoweringError> { Ok(ir::Operand::Constant(ir::Constant { ty: self.ty(ty)?, value })) }
    fn literal_constant(&mut self, expression: &Expression) -> Result<ir::Operand, LoweringError> {
        let value = match self.owner.analysis.literal(expression).cloned() {
            Some(LiteralValue::Integer(value)) => ir::ConstantValue::Integer(value),
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
        self.push(ir::OperationKind::Aggregate { destination, aggregate }, expression.span);
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
        self.push(ir::OperationKind::Copy { destination, operand }, span);
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
    }
    fn materialize(&mut self, operand: ir::Operand, span: Span) -> Result<ir::Operand, LoweringError> {
        if matches!(&operand, ir::Operand::Copy(place) if place.projections.is_empty() && self.function.locals.get(place.local.index()).is_some_and(|local| local.origin == ir::LocalOrigin::Temporary)) {
            return Ok(operand);
        }
        let ty = self.operand_frontend_type(&operand)?;
        let destination = self.temporary(ty)?;
        self.push(ir::OperationKind::Copy { destination, operand }, span);
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
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

    fn list(&mut self, expression: &'ast Expression, elements: &'ast [Expression]) -> Result<ir::Operand, LoweringError> {
        let mut values = Vec::new();
        for element in elements { let value = self.expression(element)?; values.push(self.stabilize(value, element.span)?); }
        let raw = self.raw_type(expression)?;
        let ty = self.ty(raw)?;
        self.aggregate(expression, ir::Aggregate::List { ty, elements: values })
    }
    fn map(&mut self, expression: &'ast Expression, entries: &'ast [ast::MapEntry]) -> Result<ir::Operand, LoweringError> {
        let mut values = Vec::new();
        for entry in entries {
            let key = self.expression(&entry.key)?; let key = self.stabilize(key, entry.key.span)?;
            let value = self.expression(&entry.value)?; let value = self.stabilize(value, entry.value.span)?;
            values.push((key, value));
        }
        let raw = self.raw_type(expression)?;
        let ty = self.ty(raw)?;
        self.aggregate(expression, ir::Aggregate::Map { ty, entries: values })
    }

    fn call(&mut self, expression: &'ast Expression, arguments: &'ast [Argument]) -> Result<ir::Operand, LoweringError> {
        let resolution = *self.owner.analysis.call_resolution(expression).ok_or_else(|| invariant("call target fact is missing"))?;
        if matches!(resolution.target, CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Panic))) { return Err(pending(PendingStage::Stage3, "terminal panic", expression.span)); }
        if let Some(constructor) = self.owner.analysis.constructor(expression) {
            let kind = constructor.kind.clone();
            return self.constructor(expression, arguments, &kind);
        }
        let receiver = if let CallTarget::Builtin(_) = resolution.target {
            let ExpressionKind::Member { value, .. } = &resolution.callee.kind else { return Err(invariant("built-in call has no receiver")); };
            let receiver_span = value.span;
            let value = self.expression(value)?; Some(self.stabilize(value, receiver_span)?)
        } else { None };
        let mut lowered = Vec::new();
        for argument in arguments { let value = argument_value(argument); let operand = self.expression(value)?; lowered.push(self.stabilize(operand, value.span)?); }
        let result = self.raw_type(expression)?;
        let destination = self.temporary(result)?;
        let kind = match resolution.target {
            CallTarget::Callable(CallableId::Function(function)) => ir::OperationKind::Call { destination, function: self.owner.functions.get(function.index()).copied().ok_or_else(|| invariant("callee has no IR mapping"))?, arguments: lowered },
            CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Print)) => ir::OperationKind::Intrinsic { destination, intrinsic: ir::Intrinsic::Print, arguments: lowered },
            CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Println)) => ir::OperationKind::Intrinsic { destination, intrinsic: ir::Intrinsic::Println, arguments: lowered },
            CallTarget::Builtin(method) => ir::OperationKind::Builtin { destination, method: map_builtin(method), receiver: receiver.unwrap(), arguments: lowered },
            _ => return Err(invariant("call retained an unsupported target")),
        };
        self.push(kind, expression.span);
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
    }

    fn constructor(&mut self, expression: &'ast Expression, arguments: &'ast [Argument], kind: &ConstructorKind) -> Result<ir::Operand, LoweringError> {
        let mut values = Vec::new();
        for argument in arguments { let value = argument_value(argument); let operand = self.expression(value)?; values.push(self.stabilize(operand, value.span)?); }
        match kind {
            ConstructorKind::Struct { declaration, argument_members } => {
                if values.len() != argument_members.len() { return Err(invariant("struct argument mapping has the wrong length")); }
                let definition_fields = self.owner.fields.get(declaration.index()).ok_or_else(|| invariant("struct definition has no field map"))?;
                let mut fields = values.into_iter().zip(argument_members.iter().copied()).map(|(value, field)| {
                    definition_fields.get(field).copied().map(|field| (field, value)).ok_or_else(|| invariant("struct argument maps to an invalid field"))
                }).collect::<Result<Vec<_>, _>>()?;
                fields.sort_by_key(|(field, _)| field.index());
                let definition = self.definition(*declaration)?;
                self.aggregate(expression, ir::Aggregate::Struct { definition, fields })
            }
            ConstructorKind::Tuple(declaration) => {
                let definition = self.definition(*declaration)?;
                self.aggregate(expression, ir::Aggregate::Tuple { definition, elements: values })
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

    fn inject(&mut self, payload: ir::Operand, union: analysis::TypeId, alternative: &UnionAlternative, span: Span) -> Result<ir::Operand, LoweringError> {
        let destination = self.temporary(union)?;
        let alternative = self.alternative_id(union, alternative)?;
        let union_type = self.ty(union)?;
        self.push(ir::OperationKind::UnionInject { destination, union_type, alternative, payload }, span);
        Ok(ir::Operand::Copy(ir::Place::local(destination)))
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

    fn member(&mut self, expression: &'ast Expression, value: &'ast Expression) -> Result<ir::Operand, LoweringError> {
        let receiver = self.expression(value)?; let receiver = self.stabilize(receiver, value.span)?;
        let ir::Operand::Copy(mut place) = receiver else { return Err(invariant("member receiver did not materialize to a place")); };
        let projection = self.owner.analysis.projection(expression).ok_or_else(|| invariant("member projection fact is missing"))?.kind;
        match projection {
            ProjectionKind::Struct { declaration, field, storage, .. } => place.projections.push(ir::Projection::StructField { definition: self.definition(declaration)?, field: self.field(declaration, field)?, storage: map_storage(storage) }),
            ProjectionKind::Tuple { declaration, position, .. } => place.projections.push(ir::Projection::TupleField { definition: self.definition(declaration)?, field: self.field(declaration, position)? }),
            _ => return Err(invariant("member expression has a non-member projection")),
        }
        Ok(ir::Operand::Copy(place))
    }
    fn index(&mut self, expression: &'ast Expression, value: &'ast Expression, index: &'ast Expression) -> Result<ir::Operand, LoweringError> {
        let receiver = self.expression(value)?; let receiver = self.stabilize(receiver, value.span)?;
        let index_value = self.expression(index)?; let index_value = self.materialize(index_value, index.span)?;
        let projection = self.owner.analysis.projection(expression).ok_or_else(|| invariant("index projection fact is missing"))?.kind;
        if let ProjectionKind::String { .. } = projection {
            let result = self.raw_type(expression)?;
            let destination = self.temporary(result)?;
            self.push(ir::OperationKind::StringIndex { destination, string: receiver, index: index_value }, expression.span);
            return Ok(ir::Operand::Copy(ir::Place::local(destination)));
        }
        let ir::Operand::Copy(mut place) = receiver else { return Err(invariant("index receiver did not materialize to a place")); };
        let ir::Operand::Copy(index_place) = index_value else { return Err(invariant("dynamic index did not materialize to a local")); };
        if !index_place.projections.is_empty() { return Err(invariant("dynamic index retained projections")); }
        match projection {
            ProjectionKind::List { .. } => place.projections.push(ir::Projection::ListIndex { index: index_place.local }),
            ProjectionKind::Map { .. } => place.projections.push(ir::Projection::MapIndex { key: index_place.local }),
            _ => return Err(invariant("index expression has a non-index projection")),
        }
        Ok(ir::Operand::Copy(place))
    }

    fn assignment(&mut self, target: &'ast ast::AssignmentTarget, operator: AssignmentOperator, value: &'ast Expression, span: Span) -> Result<(), LoweringError> {
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
        let root_local = self.bindings.get(root.index()).and_then(|item| *item).ok_or_else(|| invariant("assignment root has not been allocated"))?;
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
                    let index_value = self.expression(index)?; let index_value = self.materialize(index_value, index.span)?;
                    let ir::Operand::Copy(index_place) = index_value else { return Err(invariant("assignment index did not materialize")); };
                    let projection = if matches!(step, AccessPathStep::ListIndex { .. }) { ir::Projection::ListIndex { index: index_place.local } } else { ir::Projection::MapIndex { key: index_place.local } };
                    place.projections.push(projection);
                }
            }
        }
        let assigned = if operator == AssignmentOperator::Assign { self.expression(value)? } else {
            let old_type = match annotation.state { TypeState::Resolved(ty) => ty, _ => return Err(invariant("compound assignment target type is unresolved")) };
            let old_local = self.temporary(old_type)?;
            self.push(ir::OperationKind::Copy { destination: old_local, operand: ir::Operand::Copy(place.clone()) }, target.span);
            let right = self.expression(value)?;
            let result = self.temporary(old_type)?;
            self.push(ir::OperationKind::Binary { destination: result, operator: map_assignment(operator)?, left: ir::Operand::Copy(ir::Place::local(old_local)), right }, span);
            ir::Operand::Copy(ir::Place::local(result))
        };
        self.push(ir::OperationKind::Assign { destination: place, value: assigned }, span);
        Ok(())
    }
    fn is_panic(&self, expression: &Expression) -> bool { matches!(self.owner.analysis.call_resolution(expression).map(|item| item.target), Some(CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Panic)))) }
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
fn alternative_payload(alternative: &UnionAlternative) -> analysis::TypeId { match alternative { UnionAlternative::Untagged(ty) | UnionAlternative::Tagged { payload: ty, .. } | UnionAlternative::Error(ty) => *ty } }
fn invariant(message: impl Into<String>) -> LoweringError { LoweringError::Invariant(message.into()) }
fn pending(stage: PendingStage, construct: &'static str, span: Span) -> LoweringError { LoweringError::PendingStage { stage, construct, span } }
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
    fn reports_each_deferred_stage_explicitly() {
        let control = lower_text("fn main() { if true: println(); }").unwrap_err();
        assert!(matches!(control, LoweringError::PendingStage { stage: PendingStage::Stage3, .. }));

        let error_flow = lower_text("type Result(int | Error(str)); fn main() { value := Result(1); value?; }").unwrap_err();
        assert!(matches!(error_flow, LoweringError::PendingStage { stage: PendingStage::Stage4, .. }));
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
