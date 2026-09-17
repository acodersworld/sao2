//! C11 core value operations and host entry code generated solely from the validated owned IR.
//!
//! This is the production backend. It accepts only validated, owned IR and
//! retains unsupported value operations as explicit capability boundaries.

use std::fmt::{self, Write as _};

use crate::ir::{
    self, AlternativeConstructor, BinaryOperator, ConstantValue, DefinitionId, DefinitionLayout, FailureOperation,
    FailureSiteId, Function, FunctionId, IntegerOperation, Intrinsic, LocalId, MemberStorage,
    NumericConversion, OperationKind, OperationSite, Operand, Place, PrimitiveType, Projection,
    RuntimeCheck, TerminatorKind, Type, TypeId, UnaryOperator, ValidationError,
};
use crate::escape::{self, AllocationClass, AllocationId, AllocationPlan};

#[allow(dead_code)] // Retained as the validated-program convenience boundary for direct backend tests.
pub(crate) fn emit(program: &ir::Program) -> Result<String, CEmissionError> {
    // Keep the longstanding direct-backend contract: malformed IR is reported
    // as InvalidIr before any optional analysis boundary is entered.
    program.validate().map_err(CEmissionError::InvalidIr)?;
    let plan = escape::analyze(program).map_err(|error| CEmissionError::Invariant(BackendInvariant {
        definition: None, ty: None, message: error.to_string(),
    }))?;
    emit_with_plan(program, &plan)
}

/// The validated escape plan is the backend's sole authority for choosing a
/// struct allocation arena.
pub(crate) fn emit_with_plan(program: &ir::Program, plan: &AllocationPlan) -> Result<String, CEmissionError> {
    program.validate().map_err(CEmissionError::InvalidIr)?;
    plan.validate(program).map_err(|error| CEmissionError::Invariant(BackendInvariant {
        definition: None, ty: None, message: error.to_string(),
    }))?;
    CapabilityValidator::new(program).validate()?;
    let layouts = LayoutPlanner::new(program).plan()?;
    let traces = TracePlanner::new(program, &layouts).plan()?;
    let roots = RootPlanner::new(program, &traces).plan()?;
    Ok(Renderer::new(program, plan, layouts, traces, roots).render())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CEmissionError {
    InvalidIr(ValidationError),
    Unsupported(UnsupportedFeature),
    Invariant(BackendInvariant),
}

impl fmt::Display for CEmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIr(error) => write!(formatter, "C backend received {error}"),
            Self::Unsupported(error) => error.fmt(formatter),
            Self::Invariant(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CEmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self { Self::InvalidIr(error) => Some(error), _ => None }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnsupportedFeature {
    pub(crate) definition: Option<DefinitionId>,
    pub(crate) ty: Option<TypeId>,
    pub(crate) function: Option<FunctionId>,
    pub(crate) block: Option<ir::BlockId>,
    pub(crate) site: Option<OperationSite>,
    pub(crate) message: String,
}

impl fmt::Display for UnsupportedFeature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unsupported by the current C backend")?;
        if let Some(definition) = self.definition { write!(formatter, " in {definition}")?; }
        if let Some(ty) = self.ty { write!(formatter, " in {ty}")?; }
        write_function_context(formatter, self.function, self.block, self.site)?;
        write!(formatter, ": {}", self.message)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BackendInvariant {
    pub(crate) definition: Option<DefinitionId>,
    pub(crate) ty: Option<TypeId>,
    pub(crate) message: String,
}

impl fmt::Display for BackendInvariant {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "C backend invariant failed")?;
        if let Some(definition) = self.definition { write!(formatter, " in {definition}")?; }
        if let Some(ty) = self.ty { write!(formatter, " in {ty}")?; }
        write!(formatter, ": {}", self.message)
    }
}

fn write_function_context(
    formatter: &mut fmt::Formatter<'_>,
    function: Option<FunctionId>,
    block: Option<ir::BlockId>,
    site: Option<OperationSite>,
) -> fmt::Result {
    if let Some(function) = function { write!(formatter, " in {function}")?; }
    if let Some(block) = block { write!(formatter, " {block}")?; }
    match site {
        Some(OperationSite::Operation(index)) => write!(formatter, " op{index}"),
        Some(OperationSite::Terminator) => write!(formatter, " terminator"),
        None => Ok(()),
    }
}

struct CapabilityValidator<'a> {
    program: &'a ir::Program,
    definition: Option<DefinitionId>,
    ty: Option<TypeId>,
    function: Option<FunctionId>,
    block: Option<ir::BlockId>,
    site: Option<OperationSite>,
    entry_args: Option<(FunctionId, LocalId)>,
}

impl<'a> CapabilityValidator<'a> {
    fn new(program: &'a ir::Program) -> Self {
        Self {
            program, definition: None, ty: None, function: None, block: None,
            site: None, entry_args: None,
        }
    }

    fn validate(mut self) -> Result<(), CEmissionError> {
        for (index, definition) in self.program.definitions.iter().enumerate() {
            let id = DefinitionId::from_index(index);
            self.definition = Some(id);
            match &definition.layout {
                DefinitionLayout::Struct(fields) => for field in fields {
                    self.struct_field_type(field.ty, field.storage, "struct field")?;
                },
                DefinitionLayout::Tuple(fields) => for field in fields {
                    self.storage_type(*field, "tuple field")?;
                },
                DefinitionLayout::Union(alternatives) => for alternative in alternatives {
                    self.storage_type(alternative.payload, "union payload")?;
                },
            }
        }
        self.definition = None;

        for (index, ty) in self.program.types.iter().enumerate() {
            self.ty = Some(TypeId::from_index(index));
            if let Type::Union(alternatives) = ty {
                for alternative in alternatives {
                    self.storage_type(alternative.payload, "anonymous union payload")?;
                }
            }
        }
        self.ty = None;

        for index in 0..self.program.functions.len() {
            let function = self.program.functions[index].clone();
            let function_id = FunctionId::from_index(index);
            self.function = Some(function_id);
            if Some(function_id) == self.program.entry {
                self.validate_entry_shape(&function)?;
            }
            self.signature_type(function.result, false, "function result")?;
            for (position, parameter) in function.parameters.iter().enumerate() {
                let ty = function.locals[parameter.index()].ty;
                let entry_args = Some(function_id) == self.program.entry
                    && position == 0
                    && function.parameters.len() == 1
                    && self.is_string_list(ty);
                self.signature_type(ty, entry_args, "function parameter")?;
                if entry_args { self.entry_args = Some((function_id, *parameter)); }
            }
            for (index, local) in function.locals.iter().enumerate() {
                let entry_args = self.entry_args == Some((function_id, LocalId::from_index(index)));
                self.signature_type(local.ty, entry_args, "local storage")?;
            }
            self.validate_body(&function)?;
        }
        Ok(())
    }

    fn validate_entry_shape(&self, function: &Function) -> Result<(), CEmissionError> {
        if !matches!(&self.program.types[function.result.index()],
            Type::Unit | Type::Primitive(PrimitiveType::Int))
        {
            return self.unsupported("entry function result is not unit or int");
        }
        match function.parameters.as_slice() {
            [] => Ok(()),
            [parameter] if self.is_string_list(function.locals[parameter.index()].ty) => Ok(()),
            _ => self.unsupported("entry function parameters are not empty or a single [str] argument"),
        }
    }

    fn validate_body(&mut self, function: &Function) -> Result<(), CEmissionError> {
        for (block_index, block) in function.blocks.iter().enumerate() {
            self.block = Some(ir::BlockId::from_index(block_index));
            for (operation_index, operation) in block.operations.iter().enumerate() {
                self.site = Some(OperationSite::Operation(operation_index));
                self.operation(function, &operation.kind)?;
            }
            self.site = Some(OperationSite::Terminator);
            self.terminator(function, &block.terminator.as_ref().expect("validated block").kind)?;
        }
        self.block = None;
        self.site = None;
        Ok(())
    }

    fn operation(&self, function: &Function, operation: &OperationKind) -> Result<(), CEmissionError> {
        if self.operation_uses_entry_args(operation) {
            return self.unsupported("use of the entry args parameter");
        }
        if matches!(operation, OperationKind::Assign { destination, .. } if place_has_unsupported_read_projection(destination))
            || operation_operands_have_unsupported_projection(operation)
        {
            return self.unsupported("place projection");
        }
        match operation {
            OperationKind::Copy { destination, operand } => {
                self.executable_union_use(function.locals[destination.index()].ty)?;
                self.executable_union_use(self.operand_type(function, operand))
            }
            OperationKind::Call { destination, arguments, .. } => {
                self.executable_union_use(function.locals[destination.index()].ty)?;
                for argument in arguments {
                    self.executable_union_use(self.operand_type(function, argument))?;
                }
                Ok(())
            }
            OperationKind::UnionInject { union_type, .. } => self.require_executable_union(*union_type),
            OperationKind::UnionTest { union, .. } | OperationKind::UnionPayload { union, .. } => {
                self.require_executable_union(self.operand_type(function, union))
            }
            OperationKind::Unary { operand, .. } => {
                self.require_scalar_operand(function, operand, "non-scalar unary operation")
            }
            OperationKind::Binary { operator, left, .. } => {
                let ty = self.operand_type(function, left);
                if self.is_scalar(ty)
                    || matches!(operator, BinaryOperator::Equal | BinaryOperator::NotEqual)
                        && (self.is_struct(ty) || self.is_equatable_tuple(ty, &mut Vec::new()))
                {
                    Ok(())
                } else {
                    self.unsupported("non-scalar binary operation")
                }
            }
            OperationKind::Convert { .. } => Ok(()),
            OperationKind::Assign { destination, value } => {
                self.executable_union_use(self.place_type(function, destination))?;
                self.executable_union_use(self.operand_type(function, value))
            }
            OperationKind::Aggregate { aggregate: ir::Aggregate::Tuple { .. }, .. } => Ok(()),
            OperationKind::Aggregate { aggregate: ir::Aggregate::Struct { .. }, .. } => Ok(()),
            OperationKind::Aggregate { .. } => self.unsupported("aggregate construction"),
            OperationKind::StringIndex { .. } => Ok(()),
            OperationKind::Intrinsic { intrinsic, arguments, .. } => match intrinsic {
                Intrinsic::Print if arguments.len() == 1 && self.output_operand(function, &arguments[0]) => Ok(()),
                Intrinsic::Println if arguments.is_empty()
                    || arguments.len() == 1 && self.output_operand(function, &arguments[0]) => Ok(()),
                Intrinsic::Print | Intrinsic::Println => self.unsupported("printing this value type"),
            },
            OperationKind::Builtin { .. } => self.unsupported("built-in container or string operation"),
            OperationKind::BeginIteration { .. } | OperationKind::EndIteration { .. }
            | OperationKind::IterationValue { .. } => self.unsupported("container iteration"),
            OperationKind::Check(RuntimeCheck::IterationUnlocked { .. }) => {
                self.unsupported("container mutation check")
            }
            OperationKind::Check(_) => Ok(()),
        }
    }

    fn terminator(&self, function: &Function, terminator: &TerminatorKind) -> Result<(), CEmissionError> {
        if self.terminator_uses_entry_args(terminator) {
            return self.unsupported("use of the entry args parameter");
        }
        if terminator_operands_have_unsupported_projection(terminator) {
            return self.unsupported("place projection");
        }
        match terminator {
            TerminatorKind::Jump(_) | TerminatorKind::Branch { .. }
            | TerminatorKind::Panic { .. } | TerminatorKind::Unreachable => Ok(()),
            TerminatorKind::Return(value) => {
                self.executable_union_use(self.operand_type(function, value))
            }
            TerminatorKind::Switch { union, .. } => {
                self.require_executable_union(self.operand_type(function, union))
            }
            TerminatorKind::ErrorPanic { payload, .. } => match &self.program.types[self.operand_type(function, payload).index()] {
                Type::Primitive(_) => Ok(()),
                _ => self.unsupported("Error panic payload formatting"),
            },
        }
    }

    fn storage_type(&self, ty: TypeId, description: &str) -> Result<(), CEmissionError> {
        match &self.program.types[ty.index()] {
            Type::Unit | Type::Primitive(_) => Ok(()),
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(_) | DefinitionLayout::Union(_) => Ok(()),
                DefinitionLayout::Struct(_) => Ok(()),
            },
            Type::Union(_) => Ok(()),
            Type::List(_) | Type::Map { .. } => self.unsupported(format!("{description} uses container storage")),
        }
    }

    fn struct_field_type(&self, ty: TypeId, storage: MemberStorage, description: &str) -> Result<(), CEmissionError> {
        if matches!(&self.program.types[ty.index()], Type::Nominal(definition)
            if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)))
        { return Ok(()); }
        let _ = storage;
        self.storage_type(ty, description)
    }

    fn signature_type(&self, ty: TypeId, entry_args: bool, description: &str) -> Result<(), CEmissionError> {
        if entry_args { return Ok(()); }
        self.storage_type(ty, description)
    }

    fn is_string_list(&self, ty: TypeId) -> bool {
        matches!(&self.program.types[ty.index()], Type::List(element)
            if matches!(&self.program.types[element.index()], Type::Primitive(PrimitiveType::Str)))
    }

    fn is_scalar(&self, ty: TypeId) -> bool {
        matches!(&self.program.types[ty.index()], Type::Unit | Type::Primitive(_))
    }

    fn is_struct(&self, ty: TypeId) -> bool {
        matches!(&self.program.types[ty.index()], Type::Nominal(definition)
            if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)))
    }

    fn executable_union_use(&self, ty: TypeId) -> Result<(), CEmissionError> {
        if self.union_alternatives(ty).is_some() { self.require_executable_union(ty) } else { Ok(()) }
    }

    fn require_executable_union(&self, ty: TypeId) -> Result<(), CEmissionError> {
        if self.union_alternatives(ty).is_none() {
            return self.unsupported("union operation on a non-union value");
        }
        if self.supports_union_value(ty, &mut Vec::new()) { Ok(()) }
        else { self.unsupported("union operation with a non-scalar reachable payload") }
    }

    fn supports_union_value(&self, ty: TypeId, visiting: &mut Vec<TypeId>) -> bool {
        if visiting.contains(&ty) { return true; }
        match &self.program.types[ty.index()] {
            Type::Unit | Type::Primitive(_) => true,
            Type::Union(alternatives) => {
                visiting.push(ty);
                let supported = alternatives.iter()
                    .all(|alternative| self.supports_union_value(alternative.payload, visiting));
                visiting.pop();
                supported
            }
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(fields) => {
                    visiting.push(ty);
                    let supported = fields.iter()
                        .all(|field| self.supports_union_value(*field, visiting));
                    visiting.pop();
                    supported
                }
                DefinitionLayout::Union(alternatives) => {
                    visiting.push(ty);
                    let supported = alternatives.iter()
                        .all(|alternative| self.supports_union_value(alternative.payload, visiting));
                    visiting.pop();
                    supported
                }
                DefinitionLayout::Struct(_) => true,
            },
            Type::List(_) | Type::Map { .. } => false,
        }
    }

    fn is_equatable_tuple(&self, ty: TypeId, visiting: &mut Vec<TypeId>) -> bool {
        if visiting.contains(&ty) { return true; }
        let Type::Nominal(definition) = &self.program.types[ty.index()] else { return false; };
        let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout
            else { return false; };
        visiting.push(ty);
        let equatable = fields.iter().all(|field| match &self.program.types[field.index()] {
            Type::Unit | Type::Primitive(_) => true,
            Type::Nominal(definition) => matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_))
                || self.is_equatable_tuple(*field, visiting),
            Type::Union(_) | Type::List(_) | Type::Map { .. } => false,
        });
        visiting.pop();
        equatable
    }

    fn union_alternatives(&self, ty: TypeId) -> Option<&[ir::UnionAlternative]> {
        match &self.program.types[ty.index()] {
            Type::Union(alternatives) => Some(alternatives),
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Union(alternatives) => Some(alternatives),
                DefinitionLayout::Tuple(_) | DefinitionLayout::Struct(_) => None,
            },
            _ => None,
        }
    }

    fn output_operand(&self, function: &Function, operand: &Operand) -> bool {
        self.is_printable(self.operand_type(function, operand), &mut Vec::new())
    }

    fn is_printable(&self, ty: TypeId, visiting: &mut Vec<TypeId>) -> bool {
        if visiting.contains(&ty) { return true; }
        match &self.program.types[ty.index()] {
            Type::Unit | Type::Primitive(_) => true,
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(fields) => {
                    visiting.push(ty);
                    let printable = fields.iter().all(|field| self.is_printable(*field, visiting));
                    visiting.pop();
                    printable
                }
                DefinitionLayout::Union(alternatives) => {
                    visiting.push(ty);
                    let printable = alternatives.iter().all(|item| self.is_printable(item.payload, visiting));
                    visiting.pop();
                    printable
                }
                DefinitionLayout::Struct(_) => false,
            },
            Type::Union(alternatives) => {
                visiting.push(ty);
                let printable = alternatives.iter().all(|item| self.is_printable(item.payload, visiting));
                visiting.pop();
                printable
            }
            Type::List(_) | Type::Map { .. } => false,
        }
    }

    fn require_scalar_operand(&self, function: &Function, operand: &Operand, message: &str) -> Result<(), CEmissionError> {
        if self.is_scalar(self.operand_type(function, operand)) { Ok(()) }
        else { self.unsupported(message) }
    }

    fn operand_type(&self, function: &Function, operand: &Operand) -> TypeId {
        match operand {
            Operand::Constant(constant) => constant.ty,
            Operand::Copy(place) => self.place_type(function, place),
        }
    }

    fn place_type(&self, function: &Function, place: &Place) -> TypeId {
        let mut ty = function.locals[place.local.index()].ty;
        for projection in &place.projections {
            ty = match projection {
                Projection::StructField { definition, field, .. } => {
                    let DefinitionLayout::Struct(fields) = &self.program.definitions[definition.index()].layout else { unreachable!() };
                    fields[field.index()].ty
                }
                Projection::TupleField { definition, field } => {
                    let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout else { unreachable!() };
                    fields[field.index()]
                }
                Projection::ListIndex { .. } => {
                    let Type::List(element) = &self.program.types[ty.index()] else { unreachable!() };
                    *element
                }
                Projection::MapIndex { .. } => {
                    let Type::Map { value, .. } = &self.program.types[ty.index()] else { unreachable!() };
                    *value
                }
            };
        }
        ty
    }

    fn operation_uses_entry_args(&self, operation: &OperationKind) -> bool {
        let mut found = false;
        visit_operation_places(operation, &mut |place| found |= self.is_entry_args(place.local));
        found
    }

    fn terminator_uses_entry_args(&self, terminator: &TerminatorKind) -> bool {
        let mut found = false;
        visit_terminator_places(terminator, &mut |place| found |= self.is_entry_args(place.local));
        found
    }

    fn is_entry_args(&self, local: LocalId) -> bool {
        self.entry_args.is_some_and(|(function, args)| self.function == Some(function) && local == args)
    }

    fn unsupported<T>(&self, message: impl Into<String>) -> Result<T, CEmissionError> {
        Err(CEmissionError::Unsupported(UnsupportedFeature {
            definition: self.definition, ty: self.ty, function: self.function,
            block: self.block, site: self.site, message: message.into(),
        }))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum AggregateId { Definition(DefinitionId), AnonymousUnion(TypeId) }

#[derive(Clone, Debug)]
struct FieldLayout { id: ir::FieldId, ty: TypeId, storage: ir::MemberStorage, offset: u64, size: u64, align: u64, nested_layout: Option<u64> }
#[derive(Clone, Debug)]
struct StructLayout { definition: DefinitionId, identity: u64, size: u64, align: u64, fields: Vec<FieldLayout> }
#[derive(Clone, Debug)]
struct LayoutPlan { aggregates: Vec<AggregateId>, structs: Vec<StructLayout> }

#[derive(Clone, Debug)]
struct TracePlan {
    type_contains_reference: Vec<bool>,
    struct_callbacks: Vec<DefinitionId>,
    aggregate_callbacks: Vec<AggregateId>,
}

/// Backend-owned, deterministic storage plan for the precise shadow stack.
/// IR locals remain the identities: this plan only chooses their C storage.
#[derive(Clone, Debug)]
struct FunctionRootPlan { function: FunctionId, locals: Vec<(LocalId, TypeId)> }

#[derive(Clone, Debug)]
struct RootPlan { functions: Vec<FunctionRootPlan> }

impl RootPlan {
    fn function(&self, id: FunctionId) -> &FunctionRootPlan {
        self.functions.get(id.index()).expect("validated root plan")
    }
    fn contains(&self, function: FunctionId, local: LocalId) -> bool {
        self.function(function).locals.binary_search_by_key(&local, |(id, _)| *id).is_ok()
    }
}

struct RootPlanner<'a> { program: &'a ir::Program, traces: &'a TracePlan }

impl<'a> RootPlanner<'a> {
    fn new(program: &'a ir::Program, traces: &'a TracePlan) -> Self { Self { program, traces } }
    fn plan(&self) -> Result<RootPlan, CEmissionError> {
        let functions = self.program.functions.iter().enumerate().map(|(index, function)| {
            let locals = function.locals.iter().enumerate().filter_map(|(local, item)| {
                self.traces.type_contains_reference[item.ty.index()]
                    .then_some((LocalId::from_index(local), item.ty))
            }).collect();
            FunctionRootPlan { function: FunctionId::from_index(index), locals }
        }).collect();
        let plan = RootPlan { functions };
        self.validate(&plan)?;
        Ok(plan)
    }
    fn validate(&self, plan: &RootPlan) -> Result<(), CEmissionError> {
        if plan.functions.len() != self.program.functions.len() { return self.fail("root plan function count mismatch"); }
        for (index, function) in self.program.functions.iter().enumerate() {
            let entry = plan.functions.get(index).ok_or_else(|| CEmissionError::Invariant(BackendInvariant { definition: None, ty: None, message: "root plan function missing".into() }))?;
            if entry.function != FunctionId::from_index(index) { return self.fail("root plan functions are unordered"); }
            for (position, (local, ty)) in entry.locals.iter().enumerate() {
                if local.index() >= function.locals.len() || function.locals[local.index()].ty != *ty
                    || !self.traces.type_contains_reference[ty.index()] || !self.trace_expression_available(*ty) { return self.fail("invalid selected root local"); }
                if position > 0 && entry.locals[position - 1].0 >= *local { return self.fail("duplicate or unordered root local"); }
            }
            for (local, item) in function.locals.iter().enumerate() {
                let selected = entry.locals.binary_search_by_key(&LocalId::from_index(local), |(id, _)| *id).is_ok();
                if selected != self.traces.type_contains_reference[item.ty.index()] { return self.fail("root plan omits or includes an invalid local"); }
            }
        }
        Ok(())
    }
    fn fail<T>(&self, message: &str) -> Result<T, CEmissionError> {
        Err(CEmissionError::Invariant(BackendInvariant { definition: None, ty: None, message: message.into() }))
    }
    fn trace_expression_available(&self, ty: TypeId) -> bool {
        match &self.program.types[ty.index()] {
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                // A struct root always has a trace expression: enqueueing its
                // packed reference and exact layout.  A body callback is only
                // needed when that body itself has outgoing references.
                DefinitionLayout::Struct(_) => true,
                DefinitionLayout::Tuple(_) | DefinitionLayout::Union(_) => self.traces.aggregate_callbacks.contains(&AggregateId::Definition(*definition)),
            },
            Type::Union(_) => self.traces.aggregate_callbacks.contains(&AggregateId::AnonymousUnion(ty)),
            Type::Unit | Type::Primitive(_) | Type::List(_) | Type::Map { .. } => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TraceVisitState { Visiting, Complete(bool) }

struct TracePlanner<'a> {
    program: &'a ir::Program,
    layouts: &'a LayoutPlan,
    states: Vec<Option<TraceVisitState>>,
}

impl<'a> TracePlanner<'a> {
    fn new(program: &'a ir::Program, layouts: &'a LayoutPlan) -> Self {
        Self { program, layouts, states: vec![None; program.types.len()] }
    }

    fn plan(mut self) -> Result<TracePlan, CEmissionError> {
        for index in 0..self.program.types.len() {
            self.type_contains_reference(TypeId::from_index(index))?;
        }
        let type_contains_reference = self.states.iter().map(|state| match state {
            Some(TraceVisitState::Complete(value)) => *value,
            _ => false,
        }).collect::<Vec<_>>();

        let mut aggregate_callbacks = Vec::new();
        for aggregate in &self.layouts.aggregates {
            let values = self.aggregate_values(*aggregate);
            if values.into_iter().any(|ty| type_contains_reference[ty.index()]) {
                aggregate_callbacks.push(*aggregate);
            }
        }
        aggregate_callbacks.sort();
        aggregate_callbacks.dedup();

        let mut struct_callbacks = Vec::new();
        for layout in &self.layouts.structs {
            let carries = self.struct_contains_reference(layout.definition, &type_contains_reference, &mut Vec::new())?;
            if carries { struct_callbacks.push(layout.definition); }
        }
        struct_callbacks.sort();
        struct_callbacks.dedup();

        let plan = TracePlan { type_contains_reference, struct_callbacks, aggregate_callbacks };
        self.validate(&plan)?;
        Ok(plan)
    }

    fn type_contains_reference(&mut self, ty: TypeId) -> Result<bool, CEmissionError> {
        match self.states[ty.index()] {
            Some(TraceVisitState::Complete(value)) => return Ok(value),
            Some(TraceVisitState::Visiting) => return Err(CEmissionError::Invariant(BackendInvariant {
                definition: None, ty: Some(ty), message: "cyclic by-value trace shape".into(),
            })),
            None => {}
        }
        self.states[ty.index()] = Some(TraceVisitState::Visiting);
        let value = match self.program.types[ty.index()].clone() {
            Type::Unit | Type::Primitive(_) => false,
            Type::Nominal(definition) => match self.program.definitions[definition.index()].layout.clone() {
                DefinitionLayout::Struct(_) => true,
                DefinitionLayout::Tuple(fields) => {
                    let mut found = false;
                    for field in fields { found |= self.type_contains_reference(field)?; }
                    found
                }
                DefinitionLayout::Union(alternatives) => {
                    let mut found = false;
                    for alternative in alternatives { found |= self.type_contains_reference(alternative.payload)?; }
                    found
                }
            },
            Type::Union(alternatives) => {
                let mut found = false;
                for alternative in alternatives { found |= self.type_contains_reference(alternative.payload)?; }
                found
            }
            Type::List(element) => {
                if self.type_contains_reference(element)? {
                    return Err(CEmissionError::Invariant(BackendInvariant {
                        definition: None, ty: Some(ty),
                        message: "reference-bearing list traversal is unavailable before milestone 11".into(),
                    }));
                }
                false
            }
            Type::Map { key, value } => {
                if self.type_contains_reference(key)? || self.type_contains_reference(value)? {
                    return Err(CEmissionError::Invariant(BackendInvariant {
                        definition: None, ty: Some(ty),
                        message: "reference-bearing map traversal is unavailable before milestone 11".into(),
                    }));
                }
                false
            }
        };
        self.states[ty.index()] = Some(TraceVisitState::Complete(value));
        Ok(value)
    }

    fn aggregate_values(&self, aggregate: AggregateId) -> Vec<TypeId> {
        match aggregate {
            AggregateId::Definition(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(fields) => fields.clone(),
                DefinitionLayout::Union(alternatives) => alternatives.iter().map(|item| item.payload).collect(),
                DefinitionLayout::Struct(_) => unreachable!(),
            },
            AggregateId::AnonymousUnion(ty) => match &self.program.types[ty.index()] {
                Type::Union(alternatives) => alternatives.iter().map(|item| item.payload).collect(),
                _ => unreachable!(),
            },
        }
    }

    fn struct_contains_reference(&self, definition: DefinitionId, types: &[bool], visiting: &mut Vec<DefinitionId>) -> Result<bool, CEmissionError> {
        if visiting.contains(&definition) {
            return Err(CEmissionError::Invariant(BackendInvariant {
                definition: Some(definition), ty: None, message: "cyclic inline struct trace shape".into(),
            }));
        }
        let layout = self.layouts.structs.iter().find(|layout| layout.definition == definition)
            .ok_or_else(|| CEmissionError::Invariant(BackendInvariant {
                definition: Some(definition), ty: None, message: "struct trace layout is missing".into(),
            }))?;
        visiting.push(definition);
        for field in &layout.fields {
            let carries = if field.storage == MemberStorage::Inline {
                if let Type::Nominal(child) = &self.program.types[field.ty.index()]
                    && matches!(&self.program.definitions[child.index()].layout, DefinitionLayout::Struct(_))
                {
                    self.struct_contains_reference(*child, types, visiting)?
                } else {
                    types[field.ty.index()]
                }
            } else {
                types[field.ty.index()]
            };
            if carries { visiting.pop(); return Ok(true); }
        }
        visiting.pop();
        Ok(false)
    }

    fn validate(&self, plan: &TracePlan) -> Result<(), CEmissionError> {
        for definition in &plan.struct_callbacks {
            if !self.layouts.structs.iter().any(|layout| layout.definition == *definition) {
                return Err(CEmissionError::Invariant(BackendInvariant {
                    definition: Some(*definition), ty: None, message: "trace callback has no struct layout".into(),
                }));
            }
        }
        for window in plan.struct_callbacks.windows(2) {
            if window[0] >= window[1] {
                return Err(CEmissionError::Invariant(BackendInvariant {
                    definition: Some(window[1]), ty: None, message: "duplicate or unordered struct trace callback".into(),
                }));
            }
        }
        Ok(())
    }
}

struct LayoutPlanner<'a> {
    program: &'a ir::Program,
    states: Vec<(AggregateId, VisitState)>,
    ordered: Vec<AggregateId>,
    struct_states: Vec<(DefinitionId, VisitState)>,
    structs: Vec<StructLayout>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VisitState { Visiting, Complete }

impl<'a> LayoutPlanner<'a> {
    fn new(program: &'a ir::Program) -> Self {
        Self { program, states: Vec::new(), ordered: Vec::new(), struct_states: Vec::new(), structs: Vec::new() }
    }

    fn plan(mut self) -> Result<LayoutPlan, CEmissionError> {
        let mut roots = self.program.definitions.iter().enumerate()
            .filter(|(_, definition)| !matches!(&definition.layout, DefinitionLayout::Struct(_)))
            .map(|(index, _)| AggregateId::Definition(DefinitionId::from_index(index)))
            .chain(self.program.types.iter().enumerate().filter_map(|(index, ty)| {
                matches!(ty, Type::Union(_)).then_some(AggregateId::AnonymousUnion(TypeId::from_index(index)))
            }))
            .collect::<Vec<_>>();
        roots.sort();
        for root in roots { self.visit(root)?; }
        for index in 0..self.program.definitions.len() {
            if matches!(self.program.definitions[index].layout, DefinitionLayout::Struct(_)) {
                self.visit_struct(DefinitionId::from_index(index))?;
            }
        }
        Ok(LayoutPlan { aggregates: self.ordered, structs: self.structs })
    }

    fn visit(&mut self, aggregate: AggregateId) -> Result<(), CEmissionError> {
        if let Some((_, state)) = self.states.iter().find(|(id, _)| *id == aggregate) {
            return match state {
                VisitState::Complete => Ok(()),
                VisitState::Visiting => Err(CEmissionError::Invariant(self.invariant(
                    aggregate, "cyclic by-value aggregate layout",
                ))),
            };
        }
        self.states.push((aggregate, VisitState::Visiting));
        let mut dependencies = self.dependencies(aggregate);
        dependencies.sort();
        dependencies.dedup();
        for dependency in dependencies { self.visit(dependency)?; }
        self.states.iter_mut().find(|(id, _)| *id == aggregate).expect("visit state").1 = VisitState::Complete;
        self.ordered.push(aggregate);
        Ok(())
    }

    fn dependencies(&self, aggregate: AggregateId) -> Vec<AggregateId> {
        let types: Vec<TypeId> = match aggregate {
            AggregateId::Definition(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(fields) => fields.clone(),
                DefinitionLayout::Union(alternatives) => alternatives.iter().map(|item| item.payload).collect(),
                DefinitionLayout::Struct(_) => unreachable!(),
            },
            AggregateId::AnonymousUnion(ty) => {
                let Type::Union(alternatives) = &self.program.types[ty.index()] else { unreachable!() };
                alternatives.iter().map(|item| item.payload).collect()
            }
        };
        types.into_iter().filter_map(|ty| match &self.program.types[ty.index()] {
            Type::Nominal(definition) if !matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)) => Some(AggregateId::Definition(*definition)),
            Type::Union(_) => Some(AggregateId::AnonymousUnion(ty)),
            _ => None,
        }).collect()
    }

    fn invariant(&self, aggregate: AggregateId, message: impl Into<String>) -> BackendInvariant {
        match aggregate {
            AggregateId::Definition(definition) => BackendInvariant { definition: Some(definition), ty: None, message: message.into() },
            AggregateId::AnonymousUnion(ty) => BackendInvariant { definition: None, ty: Some(ty), message: message.into() },
        }
    }

    fn visit_struct(&mut self, definition: DefinitionId) -> Result<(), CEmissionError> {
        if let Some((_, state)) = self.struct_states.iter().find(|(id, _)| *id == definition) {
            return match state { VisitState::Complete => Ok(()), VisitState::Visiting => Err(CEmissionError::Invariant(self.invariant(AggregateId::Definition(definition), "cyclic inline struct body layout"))) };
        }
        self.struct_states.push((definition, VisitState::Visiting));
        let DefinitionLayout::Struct(fields) = &self.program.definitions[definition.index()].layout else { unreachable!() };
        let fields = fields.clone();
        for field in &fields {
            if field.storage == MemberStorage::Inline {
                if let Type::Nominal(inner) = &self.program.types[field.ty.index()] {
                    if matches!(self.program.definitions[inner.index()].layout, DefinitionLayout::Struct(_)) { self.visit_struct(*inner)?; }
                }
            }
        }
        let mut offset = 0_u64;
        let mut alignment = 1_u64;
        let mut planned = Vec::new();
        for (index, field) in fields.iter().enumerate() {
            let (size, align, nested_layout) = self.body_field_physical(field.ty, field.storage)?;
            offset = align_up(offset, align, definition)?;
            planned.push(FieldLayout { id: ir::FieldId::from_index(index), ty: field.ty, storage: field.storage, offset, size, align, nested_layout });
            offset = offset.checked_add(size).ok_or_else(|| CEmissionError::Invariant(BackendInvariant { definition: Some(definition), ty: None, message: "struct body size overflow".into() }))?;
            alignment = alignment.max(align);
        }
        let size = align_up(offset, alignment, definition)?;
        if size > u64::from(u32::MAX) { return Err(CEmissionError::Invariant(BackendInvariant { definition: Some(definition), ty: None, message: "struct body does not fit packed arena offset ABI".into() })); }
        let layout = StructLayout { definition, identity: layout_identity(definition)?, size, align: alignment, fields: planned };
        self.struct_states.iter_mut().find(|(id, _)| *id == definition).expect("struct visit state").1 = VisitState::Complete;
        self.structs.push(layout);
        Ok(())
    }

    fn body_field_physical(&self, ty: TypeId, storage: ir::MemberStorage) -> Result<(u64, u64, Option<u64>), CEmissionError> {
        if storage == MemberStorage::Inline && let Type::Nominal(definition) = self.program.types[ty.index()] {
            if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)) {
                let layout = self.structs.iter().find(|layout| layout.definition == definition).expect("inline dependency planned");
                return Ok((layout.size, layout.align, Some(layout.identity)));
            }
        }
        Ok(match &self.program.types[ty.index()] {
            Type::Unit | Type::Primitive(PrimitiveType::Bool | PrimitiveType::Char) => (1, 1, None),
            Type::Primitive(PrimitiveType::Int | PrimitiveType::Float) | Type::Primitive(PrimitiveType::Str) => (8, 8, None),
            Type::Nominal(definition) if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)) => (8, 4, Some(layout_identity(*definition)?)),
            Type::Nominal(definition) => self.aggregate_physical(AggregateId::Definition(*definition))?,
            Type::Union(_) => self.aggregate_physical(AggregateId::AnonymousUnion(ty))?,
            Type::List(_) | Type::Map { .. } => return Err(CEmissionError::Invariant(BackendInvariant { definition: None, ty: Some(ty), message: "container has no Stage 2 physical carrier".into() })),
        })
    }

    fn aggregate_physical(&self, aggregate: AggregateId) -> Result<(u64, u64, Option<u64>), CEmissionError> {
        let values: Vec<TypeId> = match aggregate { AggregateId::Definition(definition) => match &self.program.definitions[definition.index()].layout { DefinitionLayout::Tuple(fields) => fields.clone(), DefinitionLayout::Union(items) => items.iter().map(|item| item.payload).collect(), DefinitionLayout::Struct(_) => unreachable!() }, AggregateId::AnonymousUnion(ty) => match &self.program.types[ty.index()] { Type::Union(items) => items.iter().map(|item| item.payload).collect(), _ => unreachable!() } };
        let is_union = matches!(aggregate, AggregateId::AnonymousUnion(_)) || matches!(aggregate, AggregateId::Definition(definition) if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Union(_)));
        if is_union {
            let mut payload_size = 0; let mut payload_align = 1;
            for value in values { let (size, align, _) = self.body_field_physical(value, ir::MemberStorage::Referenced)?; payload_size = payload_size.max(size); payload_align = payload_align.max(align); }
            let payload_offset = align_up(4, payload_align, match aggregate { AggregateId::Definition(id) => id, AggregateId::AnonymousUnion(_) => DefinitionId::from_index(0) })?;
            let align = 4_u64.max(payload_align); return Ok((align_up(payload_offset.checked_add(payload_size).ok_or_else(|| CEmissionError::Invariant(BackendInvariant { definition: None, ty: None, message: "union size overflow".into() }))?, align, DefinitionId::from_index(0))?, align, None));
        }
        let mut size = 0; let mut align = 1;
        for value in values { let (field_size, field_align, _) = self.body_field_physical(value, ir::MemberStorage::Referenced)?; size = align_up(size, field_align, DefinitionId::from_index(0))?.checked_add(field_size).ok_or_else(|| CEmissionError::Invariant(BackendInvariant { definition: None, ty: None, message: "tuple size overflow".into() }))?; align = align.max(field_align); }
        Ok((align_up(size, align, DefinitionId::from_index(0))?, align, None))
    }
}

fn align_up(value: u64, align: u64, definition: DefinitionId) -> Result<u64, CEmissionError> {
    if align == 0 { return Err(CEmissionError::Invariant(BackendInvariant { definition: Some(definition), ty: None, message: "zero layout alignment".into() })); }
    let remainder = value % align;
    if remainder == 0 { Ok(value) } else { value.checked_add(align - remainder).ok_or_else(|| CEmissionError::Invariant(BackendInvariant { definition: Some(definition), ty: None, message: "layout alignment overflow".into() })) }
}

fn layout_identity(definition: DefinitionId) -> Result<u64, CEmissionError> {
    u64::try_from(definition.index()).map_err(|_| CEmissionError::Invariant(BackendInvariant {
        definition: Some(definition), ty: None,
        message: "struct layout identity cannot be represented as uint64_t".into(),
    }))
}

struct Renderer<'a> {
    program: &'a ir::Program,
    allocation_plan: &'a AllocationPlan,
    definitions: Vec<AggregateId>,
    struct_layouts: Vec<StructLayout>,
    trace_plan: TracePlan,
    root_plan: RootPlan,
    current_function: Option<FunctionId>,
    strings: Vec<StringLiteral>,
    labels: Vec<Vec<u8>>,
    output: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StringLiteral {
    bytes: Vec<u8>,
    hash: u64,
}

impl<'a> Renderer<'a> {
    fn new(program: &'a ir::Program, allocation_plan: &'a AllocationPlan, layouts: LayoutPlan, trace_plan: TracePlan, root_plan: RootPlan) -> Self {
        let mut renderer = Self { program, allocation_plan, definitions: layouts.aggregates, struct_layouts: layouts.structs, trace_plan, root_plan, current_function: None, strings: collect_strings(program), labels: Vec::new(), output: String::new() };
        renderer.plan_format_labels();
        renderer
    }

    fn render(mut self) -> String {
        self.output.push_str("/* Generated by sao2. */\n\n");
        self.output.push_str(concat!(
            "#ifndef _WIN32\n",
            "#ifndef _POSIX_C_SOURCE\n#define _POSIX_C_SOURCE 200809L\n#endif\n",
            "#ifndef _DEFAULT_SOURCE\n#define _DEFAULT_SOURCE\n#endif\n",
            "#endif\n",
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n",
            "#include <float.h>\n#include <inttypes.h>\n#include <limits.h>\n#include <math.h>\n",
            "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n",
            "#ifdef _WIN32\n#include <windows.h>\n#include <fcntl.h>\n#include <io.h>\n",
            "#else\n#include <sys/mman.h>\n#include <unistd.h>\n#endif\n\n",
        ));
        self.output.push_str("typedef struct { uint8_t value; } sao2_unit;\n");
        self.output.push_str(concat!(
            "typedef struct {\n",
            "    uint32_t owner_ptr;\n",
            "    uint32_t member_ptr;\n",
            "} sao2_ref;\n",
            "_Static_assert(sizeof(sao2_ref) == 8, \"SAO2 references must be eight bytes\");\n",
            "_Static_assert(sizeof(uint32_t) == 4, \"SAO2 requires four-byte uint32_t\");\n",
            "_Static_assert(SIZE_MAX >= UINT64_C(4294967296), \"SAO2 requires size_t to represent 4 GiB\");\n",
            "_Static_assert(sizeof(void *) >= 8, \"SAO2 requires 64-bit native pointers\");\n",
            "\n",
        ));
        self.output.push_str(concat!(
            "_Static_assert(sizeof(sao2_unit) == 1 && _Alignof(sao2_unit) == 1, \"SAO2 unit carrier ABI\");\n",
            "_Static_assert(sizeof(bool) == 1 && _Alignof(bool) == 1, \"SAO2 bool carrier ABI\");\n",
            "_Static_assert(sizeof(uint8_t) == 1 && _Alignof(uint8_t) == 1, \"SAO2 char carrier ABI\");\n",
            "_Static_assert(sizeof(int64_t) == 8 && _Alignof(int64_t) == 8, \"SAO2 int carrier ABI\");\n",
            "_Static_assert(sizeof(double) == 8 && _Alignof(double) == 8, \"SAO2 float carrier ABI\");\n",
            "_Static_assert(sizeof(sao2_ref) == 8 && _Alignof(sao2_ref) == _Alignof(uint32_t), \"SAO2 reference carrier ABI\");\n\n",
        ));
        self.output.push_str(concat!(
            "typedef struct sao2_interned_string {\n",
            "    const unsigned char *bytes;\n",
            "    size_t length;\n",
            "    uint64_t hash;\n",
            "} sao2_interned_string;\n",
            "typedef const sao2_interned_string *sao2_string;\n",
        ));
        self.output.push_str("_Static_assert(sizeof(sao2_string) == 8 && _Alignof(sao2_string) == 8, \"SAO2 string carrier ABI\");\n");
        self.output.push_str("typedef struct { int count; char **values; } sao2_args;\n");

        let aggregates = self.aggregate_roots();
        if !aggregates.is_empty() {
            self.output.push('\n');
            for aggregate in &aggregates {
                let name = aggregate_name(*aggregate);
                let _ = writeln!(self.output, "typedef struct {name} {name};");
            }
        }
        if !self.definitions.is_empty() {
            self.output.push('\n');
            for index in 0..self.definitions.len() {
                self.render_definition(self.definitions[index]);
                if index + 1 != self.definitions.len() { self.output.push('\n'); }
            }
        }
        if !self.struct_layouts.is_empty() {
            self.output.push('\n');
            for layout in &self.struct_layouts {
                let _ = writeln!(self.output, "typedef struct sao2_body_def_{} sao2_body_def_{};", layout.definition.index(), layout.definition.index());
            }
            self.output.push('\n');
            let layouts = self.struct_layouts.clone();
            for layout in &layouts { self.render_struct_body(layout); self.output.push('\n'); }
            self.render_trace_declarations();
            self.render_struct_descriptors(&layouts);
        }
        if self.root_plan.functions.iter().any(|item| !item.locals.is_empty()) {
            self.render_shadow_declarations();
            self.render_shadow_frame_definitions();
        }
        self.render_runtime_metadata();
        self.render_writer_declarations();
        self.render_scalar_helpers();
        self.render_arena_runtime();
        if !self.struct_layouts.is_empty() {
            self.render_trace_runtime();
            self.render_trace_callbacks();
            if self.root_plan.functions.iter().any(|item| !item.locals.is_empty()) {
                self.render_shadow_runtime();
                self.render_shadow_callbacks();
            }
        }
        self.render_struct_allocation_helpers();
        self.render_struct_copy_helpers();
        self.render_primitive_formatters();
        self.render_string_data();
        self.render_tuple_helpers();
        self.render_format_labels();
        self.render_aggregate_formatters();
        if !self.program.functions.is_empty() {
            self.output.push('\n');
            for index in 0..self.program.functions.len() {
                let function = self.program.functions[index].clone();
                self.render_prototype(FunctionId::from_index(index), &function);
            }
            self.output.push('\n');
            for index in 0..self.program.functions.len() {
                let function = self.program.functions[index].clone();
                self.render_function(FunctionId::from_index(index), &function);
                if index + 1 != self.program.functions.len() { self.output.push('\n'); }
            }
            self.output.push('\n');
            self.render_entry_adapter();
        }
        self.output
    }

    fn render_runtime_metadata(&mut self) {
        self.output.push_str(concat!(
            "\ntypedef struct { const unsigned char *bytes; size_t length; } sao2_bytes;\n",
            "typedef struct { uint32_t operation; size_t function; size_t line; size_t column; } sao2_failure_site;\n\n",
        ));
        for operation in ALL_FAILURE_OPERATIONS {
            let _ = writeln!(
                self.output,
                "#define {} UINT32_C({})",
                failure_operation_macro(*operation),
                failure_operation_code(*operation),
            );
        }
        self.output.push('\n');

        let filename = self.program.source.filename.to_string_lossy().into_owned();
        self.render_byte_array("sao2_filename_data", filename.as_bytes());
        let _ = writeln!(
            self.output,
            "static const sao2_bytes sao2_filename = {{ sao2_filename_data, {} }};",
            filename.len(),
        );
        let function_names = self.program.functions.iter()
            .map(|function| function.name.as_bytes().to_vec())
            .collect::<Vec<_>>();
        for (index, name) in function_names.iter().enumerate() {
            self.render_byte_array(&format!("sao2_function_name_{index}"), name);
        }
        self.output.push_str("static const sao2_bytes sao2_function_names[] = {\n");
        if self.program.functions.is_empty() {
            self.output.push_str("    { NULL, 0 },\n");
        } else {
            for (index, name) in function_names.iter().enumerate() {
                let _ = writeln!(self.output, "    {{ sao2_function_name_{index}, {} }},", name.len());
            }
        }
        self.output.push_str("};\n");
        let _ = writeln!(self.output, "static const size_t sao2_function_count = {};", self.program.functions.len());
        self.output.push_str("static const sao2_failure_site sao2_failures[] = {\n");
        if self.program.failure_sites.is_empty() {
            self.output.push_str("    { UINT32_C(0), 0, 0, 0 },\n");
        } else {
            for failure in &self.program.failure_sites {
                let _ = writeln!(
                    self.output,
                    "    {{ {}, {}, {}, {} }},",
                    failure_operation_macro(failure.operation),
                    failure.function.index(),
                    failure.line,
                    failure.column,
                );
            }
        }
        self.output.push_str("};\n");
        let _ = writeln!(self.output, "static const size_t sao2_failure_count = {};", self.program.failure_sites.len());
    }

    fn render_byte_array(&mut self, name: &str, bytes: &[u8]) {
        let _ = write!(self.output, "static const unsigned char {name}[] = {{");
        if bytes.is_empty() {
            self.output.push_str(" UINT8_C(0) ");
        } else {
            for (index, byte) in bytes.iter().enumerate() {
                if index == 0 { self.output.push(' '); } else { self.output.push_str(", "); }
                let _ = write!(self.output, "UINT8_C({byte})");
            }
            self.output.push(' ');
        }
        self.output.push_str("};\n");
    }

    fn render_scalar_helpers(&mut self) {
        self.output.push_str(SCALAR_RUNTIME);
    }

    fn render_arena_runtime(&mut self) {
        self.output.push_str(ARENA_RUNTIME);
    }

    fn render_trace_runtime(&mut self) {
        self.output.push_str(TRACE_RUNTIME);
    }

    fn render_struct_allocation_helpers(&mut self) {
        if !self.struct_layouts.is_empty() {
            self.output.push_str(STRUCT_ALLOCATION_RUNTIME);
        }
    }

    fn render_struct_copy_helpers(&mut self) {
        if self.struct_layouts.is_empty() { return; }
        self.output.push('\n');
        let layouts = self.struct_layouts.clone();
        for layout in &layouts {
            let definition = layout.definition.index();
            let _ = writeln!(self.output, "static void sao2_copy_body_def_{definition}(sao2_body_def_{definition} *destination, const sao2_body_def_{definition} *source) {{");
            self.output.push_str("    if (destination == source) return;\n");
            for field in &layout.fields {
                if field.storage == MemberStorage::Inline && field.nested_layout.is_some()
                    && matches!(&self.program.types[field.ty.index()], Type::Nominal(inner) if matches!(self.program.definitions[inner.index()].layout, DefinitionLayout::Struct(_))) {
                    let Type::Nominal(inner) = &self.program.types[field.ty.index()] else { unreachable!() };
                    let _ = writeln!(self.output, "    sao2_copy_body_def_{}(&destination->field_{}, &source->field_{});", inner.index(), field.id.index(), field.id.index());
                } else {
                    let _ = writeln!(self.output, "    destination->field_{} = source->field_{};", field.id.index(), field.id.index());
                }
            }
            self.output.push_str("}\n\n");
        }
    }

    fn render_writer_declarations(&mut self) {
        self.output.push_str(concat!(
            "\ntypedef struct { FILE *stream; size_t site; bool checked; bool failed; } sao2_writer;\n",
            "static _Noreturn void sao2_output_failure(size_t site);\n",
            "static void sao2_writer_bytes(sao2_writer *writer, const unsigned char *bytes, size_t length);\n",
            "static void sao2_writer_char(sao2_writer *writer, unsigned char value);\n",
            "static void sao2_format_unit(sao2_writer *writer, sao2_unit value);\n",
            "static void sao2_format_int(sao2_writer *writer, int64_t value);\n",
            "static void sao2_format_float(sao2_writer *writer, double value);\n",
            "static void sao2_format_string(sao2_writer *writer, sao2_string value);\n",
            "static void sao2_format_bool(sao2_writer *writer, bool value);\n",
            "static void sao2_format_char(sao2_writer *writer, uint8_t value);\n",
        ));
    }

    fn render_primitive_formatters(&mut self) {
        self.output.push_str(FORMAT_RUNTIME);
    }

    fn plan_label(&mut self, bytes: &[u8]) {
        if !self.labels.iter().any(|item| item.as_slice() == bytes) { self.labels.push(bytes.to_vec()); }
    }

    fn plan_format_labels(&mut self) {
        let definitions = self.definitions.clone();
        for aggregate in definitions {
            match aggregate {
                AggregateId::Definition(definition) => match self.program.definitions[definition.index()].layout.clone() {
                    DefinitionLayout::Tuple(_) => {
                        let name = self.program.definitions[definition.index()].name.clone();
                        self.plan_label(name.as_bytes());
                        self.plan_label(b"("); self.plan_label(b", "); self.plan_label(b")");
                    }
                    DefinitionLayout::Union(alternatives) => self.plan_union_labels(Some(definition), &alternatives),
                    DefinitionLayout::Struct(_) => {}
                },
                AggregateId::AnonymousUnion(ty) => {
                    let Type::Union(alternatives) = self.program.types[ty.index()].clone() else { unreachable!() };
                    self.plan_union_labels(None, &alternatives);
                }
            }
        }
    }

    fn plan_union_labels(&mut self, definition: Option<DefinitionId>, alternatives: &[ir::UnionAlternative]) {
        for alternative in alternatives {
            match &alternative.constructor {
                AlternativeConstructor::Error => self.plan_label(b"Error("),
                AlternativeConstructor::Tagged(tag) => {
                    if let Some(definition) = definition {
                        let name = self.program.definitions[definition.index()].name.clone();
                        self.plan_label(name.as_bytes());
                        self.plan_label(b".");
                    }
                    self.plan_label(tag.as_bytes()); self.plan_label(b"(");
                }
                AlternativeConstructor::Untagged => if let Some(definition) = definition {
                    let name = self.program.definitions[definition.index()].name.clone();
                    self.plan_label(name.as_bytes());
                    self.plan_label(b"(");
                },
            }
            if !matches!(alternative.constructor, AlternativeConstructor::Untagged) || definition.is_some() {
                self.plan_label(b")");
            }
        }
    }

    fn render_format_labels(&mut self) {
        if self.labels.is_empty() { return; }
        self.output.push('\n');
        let labels = self.labels.clone();
        for (index, label) in labels.iter().enumerate() {
            self.render_byte_array(&format!("sao2_format_label_{index}"), label);
        }
    }

    fn label(&self, bytes: &[u8]) -> String {
        let index = self.labels.iter().position(|label| label.as_slice() == bytes).expect("planned formatting label");
        format!("sao2_format_label_{index}")
    }

    fn render_string_data(&mut self) {
        if self.strings.is_empty() { return; }
        self.output.push('\n');
        for index in 0..self.strings.len() {
            let literal = &self.strings[index];
            let bytes = &literal.bytes;
            let _ = write!(self.output, "static const unsigned char sao2_string_data_{index}[] = {{");
            if bytes.is_empty() {
                self.output.push_str(" UINT8_C(0) ");
            } else {
                for (position, byte) in bytes.iter().enumerate() {
                    if position == 0 { self.output.push(' '); }
                    else { self.output.push_str(", "); }
                    let _ = write!(self.output, "UINT8_C({byte})");
                }
                self.output.push(' ');
            }
            self.output.push_str("};\n");
            let _ = writeln!(
                self.output,
                "static const sao2_interned_string sao2_string_descriptor_{index} = {{ sao2_string_data_{index}, {}, UINT64_C({}) }};",
                bytes.len(),
                literal.hash,
            );
        }
    }

    fn render_tuple_helpers(&mut self) {
        let tuples = self.definitions.iter().filter_map(|aggregate| {
            let AggregateId::Definition(definition) = aggregate else { return None; };
            matches!(&self.program.definitions[definition.index()].layout, DefinitionLayout::Tuple(_))
                .then_some(*definition)
        }).collect::<Vec<_>>();
        let equal = tuples.iter().copied().filter(|definition| {
            self.is_equatable_tuple_definition(*definition, &mut Vec::new())
        }).collect::<Vec<_>>();
        let hashed = tuples.iter().copied().filter(|definition| {
            self.is_hashable_tuple_definition(*definition, &mut Vec::new())
        }).collect::<Vec<_>>();
        if equal.is_empty() && hashed.is_empty() { return; }

        self.output.push('\n');
        for definition in equal {
            self.render_tuple_equal_helper(definition);
        }
        if !hashed.is_empty() {
            self.output.push_str(concat!(
                "static inline uint64_t sao2_hash_combine(uint64_t state, uint64_t value) {\n",
                "    for (unsigned int byte = 0; byte < 8; ++byte) {\n",
                "        state ^= value & UINT64_C(255);\n",
                "        state *= UINT64_C(1099511628211);\n",
                "        value >>= 8;\n",
                "    }\n",
                "    return state;\n",
                "}\n\n",
            ));
            for definition in hashed {
                self.render_tuple_hash_helper(definition);
            }
        }
    }

    fn render_aggregate_formatters(&mut self) {
        let aggregates = self.definitions.clone();
        for aggregate in aggregates {
            if !self.is_printable_aggregate(aggregate, &mut Vec::new()) { continue; }
            self.output.push('\n');
            match aggregate {
                AggregateId::Definition(definition) => match self.program.definitions[definition.index()].layout.clone() {
                    DefinitionLayout::Tuple(fields) => self.render_tuple_formatter(definition, &fields),
                    DefinitionLayout::Union(alternatives) => self.render_union_formatter(aggregate, &alternatives),
                    DefinitionLayout::Struct(_) => {}
                },
                AggregateId::AnonymousUnion(ty) => {
                    let Type::Union(alternatives) = self.program.types[ty.index()].clone() else { unreachable!() };
                    self.render_union_formatter(aggregate, &alternatives);
                }
            }
        }
    }

    fn is_printable_aggregate(&self, aggregate: AggregateId, visiting: &mut Vec<AggregateId>) -> bool {
        if visiting.contains(&aggregate) { return true; }
        visiting.push(aggregate);
        let values = match aggregate {
            AggregateId::Definition(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(fields) => fields.clone(),
                DefinitionLayout::Union(alternatives) => alternatives.iter().map(|item| item.payload).collect(),
                DefinitionLayout::Struct(_) => Vec::new(),
            },
            AggregateId::AnonymousUnion(ty) => match &self.program.types[ty.index()] {
                Type::Union(alternatives) => alternatives.iter().map(|item| item.payload).collect(),
                _ => unreachable!(),
            },
        };
        let printable = !values.is_empty() && values.into_iter().all(|ty| self.is_printable_type(ty, visiting));
        visiting.pop();
        printable
    }

    fn is_printable_type(&self, ty: TypeId, visiting: &mut Vec<AggregateId>) -> bool {
        match &self.program.types[ty.index()] {
            Type::Unit | Type::Primitive(_) => true,
            Type::Nominal(definition) => self.is_printable_aggregate(AggregateId::Definition(*definition), visiting),
            Type::Union(_) => self.is_printable_aggregate(AggregateId::AnonymousUnion(ty), visiting),
            Type::List(_) | Type::Map { .. } => false,
        }
    }

    fn render_tuple_formatter(&mut self, definition: DefinitionId, fields: &[TypeId]) {
        let name = format!("sao2_def_{}", definition.index());
        let _ = writeln!(self.output, "static inline void sao2_format_tuple_def_{}(sao2_writer *writer, {name} value) {{", definition.index());
        let prefix = self.label(self.program.definitions[definition.index()].name.as_bytes());
        let _ = writeln!(self.output, "    sao2_writer_bytes(writer, {prefix}, {});", self.program.definitions[definition.index()].name.len());
        self.write_label_call(b"(");
        for (index, ty) in fields.iter().enumerate() {
            if index != 0 { self.write_label_call(b", "); }
            let call = self.format_call(*ty, &format!("value.field_{index}"));
            let _ = writeln!(self.output, "    {call};");
        }
        self.write_label_call(b")");
        self.output.push_str("}\n");
    }

    fn render_union_formatter(&mut self, aggregate: AggregateId, alternatives: &[ir::UnionAlternative]) {
        let name = aggregate_name(aggregate);
        let _ = writeln!(self.output, "static inline void sao2_format_union_{}(sao2_writer *writer, {name} value) {{", match aggregate { AggregateId::Definition(id) => format!("def_{}", id.index()), AggregateId::AnonymousUnion(id) => format!("ty_{}", id.index()) });
        self.output.push_str("    switch (value.tag) {\n");
        for (index, alternative) in alternatives.iter().enumerate() {
            let _ = writeln!(self.output, "        case UINT32_C({}):", index + 1);
            self.render_union_wrapper(aggregate, alternative, index);
            let call = self.format_call(alternative.payload, &format!("value.payload.alternative_{index}"));
            let _ = writeln!(self.output, "            {call};");
            if self.union_has_wrapper(aggregate, alternative) { self.output.push_str("            "); self.write_label_call(b")"); }
            self.output.push_str("            return;\n");
        }
        self.output.push_str("        default: sao2_compiler_invariant();\n    }\n}\n");
    }

    fn render_union_wrapper(&mut self, aggregate: AggregateId, alternative: &ir::UnionAlternative, _index: usize) {
        match &alternative.constructor {
            AlternativeConstructor::Error => { self.output.push_str("            "); self.write_label_call(b"Error("); }
            AlternativeConstructor::Tagged(tag) => {
                if let AggregateId::Definition(definition) = aggregate {
                    let name = self.label(self.program.definitions[definition.index()].name.as_bytes());
                    let _ = writeln!(self.output, "            sao2_writer_bytes(writer, {name}, {});", self.program.definitions[definition.index()].name.len());
                    self.output.push_str("            "); self.write_label_call(b".");
                }
                let tag_label = self.label(tag.as_bytes());
                let _ = writeln!(self.output, "            sao2_writer_bytes(writer, {tag_label}, {});", tag.len());
                self.output.push_str("            "); self.write_label_call(b"(");
            }
            AlternativeConstructor::Untagged => if let AggregateId::Definition(definition) = aggregate {
                let name = self.label(self.program.definitions[definition.index()].name.as_bytes());
                let _ = writeln!(self.output, "            sao2_writer_bytes(writer, {name}, {});", self.program.definitions[definition.index()].name.len());
                self.output.push_str("            "); self.write_label_call(b"(");
            },
        }
    }

    fn union_has_wrapper(&self, aggregate: AggregateId, alternative: &ir::UnionAlternative) -> bool {
        !matches!(alternative.constructor, AlternativeConstructor::Untagged) || matches!(aggregate, AggregateId::Definition(_))
    }

    fn write_label_call(&mut self, bytes: &[u8]) {
        let label = self.label(bytes);
        let _ = writeln!(self.output, "sao2_writer_bytes(writer, {label}, {});", bytes.len());
    }

    fn format_call(&self, ty: TypeId, value: &str) -> String {
        match &self.program.types[ty.index()] {
            Type::Unit => format!("sao2_format_unit(writer, {value})"),
            Type::Primitive(PrimitiveType::Int) => format!("sao2_format_int(writer, {value})"),
            Type::Primitive(PrimitiveType::Float) => format!("sao2_format_float(writer, {value})"),
            Type::Primitive(PrimitiveType::Str) => format!("sao2_format_string(writer, {value})"),
            Type::Primitive(PrimitiveType::Bool) => format!("sao2_format_bool(writer, {value})"),
            Type::Primitive(PrimitiveType::Char) => format!("sao2_format_char(writer, {value})"),
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(_) => format!("sao2_format_tuple_def_{}(writer, {value})", definition.index()),
                DefinitionLayout::Union(_) => format!("sao2_format_union_def_{}(writer, {value})", definition.index()),
                DefinitionLayout::Struct(_) => unreachable!(),
            },
            Type::Union(_) => format!("sao2_format_union_ty_{}(writer, {value})", ty.index()),
            Type::List(_) | Type::Map { .. } => unreachable!(),
        }
    }

    fn render_tuple_equal_helper(&mut self, definition: DefinitionId) {
        let name = format!("sao2_def_{}", definition.index());
        let _ = writeln!(self.output, "static inline bool sao2_tuple_equal_def_{}({name} left, {name} right) {{", definition.index());
        let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout else { unreachable!() };
        let fields = fields.clone();
        self.output.push_str("    return ");
        for (index, field) in fields.iter().enumerate() {
            if index != 0 { self.output.push_str("\n        && "); }
            self.output.push_str(&self.tuple_equal_expression(*field, index));
        }
        self.output.push_str(";\n}\n\n");
    }

    fn tuple_equal_expression(&self, ty: TypeId, field: usize) -> String {
        let left = format!("left.field_{field}");
        let right = format!("right.field_{field}");
        match &self.program.types[ty.index()] {
            Type::Unit => "true".to_owned(),
            Type::Primitive(PrimitiveType::Str) => format!("{left} == {right}"),
            Type::Primitive(_) => format!("{left} == {right}"),
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Struct(_) => format!("sao2_ref_equal({left}, {right})"),
                DefinitionLayout::Tuple(_) => format!("sao2_tuple_equal_def_{}({left}, {right})", definition.index()),
                DefinitionLayout::Union(_) => unreachable!(),
            },
            Type::Union(_) | Type::List(_) | Type::Map { .. } => unreachable!(),
        }
    }

    fn render_tuple_hash_helper(&mut self, definition: DefinitionId) {
        let name = format!("sao2_def_{}", definition.index());
        let _ = writeln!(self.output, "static inline uint64_t sao2_tuple_hash_def_{}({name} value) {{", definition.index());
        self.output.push_str("    uint64_t hash = UINT64_C(14695981039346656037);\n");
        let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout else { unreachable!() };
        let fields = fields.clone();
        for (index, field) in fields.iter().enumerate() {
            let component = self.tuple_hash_component(*field, index);
            let _ = writeln!(self.output, "    hash = sao2_hash_combine(hash, {component});");
        }
        self.output.push_str("    return hash;\n}\n\n");
    }

    fn tuple_hash_component(&self, ty: TypeId, field: usize) -> String {
        let value = format!("value.field_{field}");
        match &self.program.types[ty.index()] {
            Type::Unit => "UINT64_C(0)".to_owned(),
            Type::Primitive(PrimitiveType::Int) => format!("sao2_int_to_bits({value})"),
            Type::Primitive(PrimitiveType::Str) => format!("{value}->hash"),
            Type::Primitive(PrimitiveType::Bool) => format!("({value} ? UINT64_C(1) : UINT64_C(0))"),
            Type::Nominal(definition) => format!("sao2_tuple_hash_def_{}({value})", definition.index()),
            Type::Primitive(PrimitiveType::Float | PrimitiveType::Char) | Type::Union(_)
            | Type::List(_) | Type::Map { .. } => unreachable!(),
        }
    }

    fn is_equatable_tuple_definition(&self, definition: DefinitionId, visiting: &mut Vec<DefinitionId>) -> bool {
        if visiting.contains(&definition) { return true; }
        let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout else { return false; };
        visiting.push(definition);
        let equatable = fields.iter().all(|field| match &self.program.types[field.index()] {
            Type::Unit | Type::Primitive(_) => true,
            Type::Nominal(inner) => matches!(self.program.definitions[inner.index()].layout, DefinitionLayout::Struct(_))
                || self.is_equatable_tuple_definition(*inner, visiting),
            Type::Union(_) | Type::List(_) | Type::Map { .. } => false,
        });
        visiting.pop();
        equatable
    }

    fn is_hashable_tuple_definition(&self, definition: DefinitionId, visiting: &mut Vec<DefinitionId>) -> bool {
        if visiting.contains(&definition) { return true; }
        let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout else { return false; };
        visiting.push(definition);
        let hashable = fields.iter().all(|field| match &self.program.types[field.index()] {
            Type::Unit | Type::Primitive(PrimitiveType::Int | PrimitiveType::Str | PrimitiveType::Bool) => true,
            Type::Nominal(inner) => self.is_hashable_tuple_definition(*inner, visiting),
            Type::Primitive(PrimitiveType::Float | PrimitiveType::Char) | Type::Union(_)
            | Type::List(_) | Type::Map { .. } => false,
        });
        visiting.pop();
        hashable
    }

    fn aggregate_roots(&self) -> Vec<AggregateId> {
        let mut aggregates = self.program.definitions.iter().enumerate()
            .filter(|(_, definition)| !matches!(&definition.layout, DefinitionLayout::Struct(_)))
            .map(|(index, _)| AggregateId::Definition(DefinitionId::from_index(index)))
            .chain(self.program.types.iter().enumerate().filter_map(|(index, ty)| {
                matches!(ty, Type::Union(_)).then_some(AggregateId::AnonymousUnion(TypeId::from_index(index)))
            }))
            .collect::<Vec<_>>();
        aggregates.sort();
        aggregates
    }

    fn render_definition(&mut self, aggregate: AggregateId) {
        let name = aggregate_name(aggregate);
        let _ = writeln!(self.output, "struct {name} {{");
        match aggregate {
            AggregateId::Definition(definition) => match self.program.definitions[definition.index()].layout.clone() {
                DefinitionLayout::Tuple(fields) => for (index, ty) in fields.into_iter().enumerate() {
                    let c_ty = self.c_type(ty);
                    let _ = writeln!(self.output, "    {c_ty} field_{index};");
                },
                DefinitionLayout::Union(alternatives) => self.render_union(&alternatives),
                DefinitionLayout::Struct(_) => unreachable!(),
            },
            AggregateId::AnonymousUnion(ty) => {
                let Type::Union(alternatives) = self.program.types[ty.index()].clone() else { unreachable!() };
                self.render_union(&alternatives);
            }
        }
        self.output.push_str("};\n");
        let (size, align, _) = LayoutPlanner::new(self.program).aggregate_physical(aggregate)
            .expect("validated aggregate has a physical layout");
        let _ = writeln!(self.output, "_Static_assert(sizeof({name}) == UINT64_C({size}), \"SAO2 planned aggregate size\");");
        let _ = writeln!(self.output, "_Static_assert(_Alignof({name}) == UINT64_C({align}), \"SAO2 planned aggregate alignment\");");
        match aggregate {
            AggregateId::Definition(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(fields) => {
                    let mut offset = 0_u64;
                    for (index, field) in fields.iter().enumerate() {
                        let (_, field_align, _) = LayoutPlanner::new(self.program).body_field_physical(*field, MemberStorage::Referenced).expect("validated tuple field");
                        offset = align_up(offset, field_align, definition).expect("validated tuple offset");
                        let _ = writeln!(self.output, "_Static_assert(offsetof({name}, field_{index}) == UINT64_C({offset}), \"SAO2 planned tuple field offset\");");
                        let (field_size, _, _) = LayoutPlanner::new(self.program).body_field_physical(*field, MemberStorage::Referenced).expect("validated tuple field");
                        offset += field_size;
                    }
                }
                DefinitionLayout::Union(_) => self.render_union_assertions(&name),
                DefinitionLayout::Struct(_) => unreachable!(),
            },
            AggregateId::AnonymousUnion(_) => self.render_union_assertions(&name),
        }
    }

    fn render_union_assertions(&mut self, name: &str) {
        self.output.push_str("_Static_assert(offsetof(");
        self.output.push_str(name);
        self.output.push_str(", tag) == UINT64_C(0), \"SAO2 planned union tag offset\");\n");
        self.output.push_str("_Static_assert(offsetof(");
        self.output.push_str(name);
        self.output.push_str(", payload) >= UINT64_C(4), \"SAO2 planned union payload offset\");\n");
    }

    fn render_struct_body(&mut self, layout: &StructLayout) {
        let _ = writeln!(self.output, "struct sao2_body_def_{} {{", layout.definition.index());
        for field in &layout.fields {
            let c_ty = self.body_field_c_type(field);
            let _ = writeln!(self.output, "    {c_ty} field_{};", field.id.index());
        }
        self.output.push_str("};\n");
        let name = format!("sao2_body_def_{}", layout.definition.index());
        let _ = writeln!(self.output, "_Static_assert(sizeof({name}) == UINT64_C({}), \"SAO2 planned struct body size\");", layout.size);
        let _ = writeln!(self.output, "_Static_assert(_Alignof({name}) == UINT64_C({}), \"SAO2 planned struct body alignment\");", layout.align);
        for field in &layout.fields {
            let _ = writeln!(self.output, "_Static_assert(offsetof({name}, field_{}) == UINT64_C({}), \"SAO2 planned struct field offset\");", field.id.index(), field.offset);
        }
    }

    fn body_field_c_type(&self, field: &FieldLayout) -> String {
        if field.storage == MemberStorage::Inline && field.nested_layout.is_some()
            && matches!(&self.program.types[field.ty.index()], Type::Nominal(definition) if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)))
        { return format!("sao2_body_def_{}", match &self.program.types[field.ty.index()] { Type::Nominal(definition) => definition.index(), _ => unreachable!() }); }
        self.c_type(field.ty)
    }

    fn render_struct_descriptors(&mut self, layouts: &[StructLayout]) {
        self.output.push_str(concat!(
            "typedef struct { uint64_t offset; uint64_t size; uint64_t alignment; uint64_t inline_layout; uint32_t storage; } sao2_layout_field_descriptor;\n",
            "typedef struct sao2_layout_descriptor { uint64_t identity; uint64_t size; uint64_t alignment; size_t field_count; const sao2_layout_field_descriptor *fields; sao2_trace_body_fn trace_body; } sao2_layout_descriptor;\n",
            "#define SAO2_LAYOUT_FIELD_INLINE UINT32_C(0)\n#define SAO2_LAYOUT_FIELD_REFERENCED UINT32_C(1)\n\n",
        ));
        let mut ordered = layouts.to_vec();
        ordered.sort_by_key(|layout| layout.identity);
        for layout in &ordered {
            let _ = writeln!(self.output, "static const sao2_layout_field_descriptor sao2_layout_fields_def_{}[] = {{", layout.definition.index());
            if layout.fields.is_empty() { self.output.push_str("    { 0, 0, 0, 0, 0 },\n"); }
            for field in &layout.fields {
                let storage = match field.storage { MemberStorage::Inline => "SAO2_LAYOUT_FIELD_INLINE", MemberStorage::Referenced => "SAO2_LAYOUT_FIELD_REFERENCED" };
                let nested = field.nested_layout.unwrap_or(u64::MAX);
                let _ = writeln!(self.output, "    {{ UINT64_C({}), UINT64_C({}), UINT64_C({}), UINT64_C({}), {} }},", field.offset, field.size, field.align, nested, storage);
            }
            self.output.push_str("};\n");
            let callback = if self.trace_plan.struct_callbacks.contains(&layout.definition) {
                format!("sao2_trace_body_def_{}", layout.definition.index())
            } else { "NULL".into() };
            let _ = writeln!(self.output, "static const sao2_layout_descriptor sao2_layout_def_{} = {{ UINT64_C({}), UINT64_C({}), UINT64_C({}), {}, sao2_layout_fields_def_{}, {} }};", layout.definition.index(), layout.identity, layout.size, layout.align, layout.fields.len(), layout.definition.index(), callback);
        }
        self.output.push_str("\nstatic const sao2_layout_descriptor *const sao2_layout_registry[] = {\n");
        for layout in &ordered {
            let _ = writeln!(self.output, "    &sao2_layout_def_{},", layout.definition.index());
        }
        self.output.push_str("};\n");
        let _ = writeln!(self.output, "static const size_t sao2_layout_registry_count = {};", ordered.len());
        self.output.push_str(concat!(
            "static const sao2_layout_descriptor *sao2_layout_find(uint64_t identity) {\n",
            "    size_t low = 0, high = sao2_layout_registry_count;\n",
            "    while (low < high) {\n",
            "        size_t middle = low + (high - low) / 2;\n",
            "        const sao2_layout_descriptor *layout = sao2_layout_registry[middle];\n",
            "        if (layout->identity < identity) low = middle + 1;\n",
            "        else if (layout->identity > identity) high = middle;\n",
            "        else return layout;\n",
            "    }\n",
            "    return NULL;\n",
            "}\n",
            "static bool sao2_layout_registered(const sao2_layout_descriptor *layout) {\n",
            "    return layout != NULL && sao2_layout_find(layout->identity) == layout;\n",
            "}\n",
        ));
    }

    fn render_trace_declarations(&mut self) {
        self.output.push_str(concat!(
            "typedef struct sao2_trace_context sao2_trace_context;\n",
            "typedef void (*sao2_trace_body_fn)(sao2_trace_context *context, const unsigned char *body);\n",
            "struct sao2_layout_descriptor;\n",
            "static void sao2_trace_enqueue(sao2_trace_context *context, sao2_ref reference, const struct sao2_layout_descriptor *layout);\n",
        ));
        for definition in &self.trace_plan.struct_callbacks {
            let _ = writeln!(self.output, "static void sao2_trace_body_def_{}(sao2_trace_context *context, const unsigned char *body);", definition.index());
        }
        for aggregate in &self.trace_plan.aggregate_callbacks {
            let name = aggregate_name(*aggregate);
            let suffix = trace_aggregate_suffix(*aggregate);
            let _ = writeln!(self.output, "static void sao2_trace_value_{suffix}(sao2_trace_context *context, const {name} *value);");
        }
        self.output.push('\n');
    }

    fn render_trace_callbacks(&mut self) {
        self.output.push('\n');
        let aggregates = self.trace_plan.aggregate_callbacks.clone();
        for aggregate in aggregates { self.render_trace_aggregate_callback(aggregate); }
        let definitions = self.trace_plan.struct_callbacks.clone();
        for definition in definitions { self.render_trace_struct_callback(definition); }
    }

    fn render_trace_struct_callback(&mut self, definition: DefinitionId) {
        let layout = self.struct_layouts.iter().find(|layout| layout.definition == definition).expect("planned trace layout").clone();
        let _ = writeln!(self.output, "static void sao2_trace_body_def_{}(sao2_trace_context *context, const unsigned char *body) {{", definition.index());
        let _ = writeln!(self.output, "    const sao2_body_def_{} *value = (const sao2_body_def_{} *)body;", definition.index(), definition.index());
        self.output.push_str("    if (context->result != SAO2_TRACE_OK) return;\n");
        for field in &layout.fields {
            let expression = format!("value->field_{}", field.id.index());
            self.render_trace_value(field.ty, field.storage, &expression, 1);
        }
        self.output.push_str("}\n\n");
    }

    fn render_trace_aggregate_callback(&mut self, aggregate: AggregateId) {
        let name = aggregate_name(aggregate);
        let suffix = trace_aggregate_suffix(aggregate);
        let _ = writeln!(self.output, "static void sao2_trace_value_{suffix}(sao2_trace_context *context, const {name} *value) {{");
        self.output.push_str("    if (context->result != SAO2_TRACE_OK) return;\n");
        match aggregate {
            AggregateId::Definition(definition) => match self.program.definitions[definition.index()].layout.clone() {
                DefinitionLayout::Tuple(fields) => for (index, ty) in fields.into_iter().enumerate() {
                    self.render_trace_value(ty, MemberStorage::Referenced, &format!("value->field_{index}"), 1);
                },
                DefinitionLayout::Union(alternatives) => self.render_trace_union_cases(&alternatives),
                DefinitionLayout::Struct(_) => unreachable!(),
            },
            AggregateId::AnonymousUnion(ty) => {
                let Type::Union(alternatives) = self.program.types[ty.index()].clone() else { unreachable!() };
                self.render_trace_union_cases(&alternatives);
            }
        }
        self.output.push_str("}\n\n");
    }

    fn render_trace_union_cases(&mut self, alternatives: &[ir::UnionAlternative]) {
        self.output.push_str("    switch (value->tag) {\n    case 0: break;\n");
        for (index, alternative) in alternatives.iter().enumerate() {
            let _ = writeln!(self.output, "    case {}:", index + 1);
            self.render_trace_value(alternative.payload, MemberStorage::Referenced, &format!("value->payload.alternative_{index}"), 2);
            self.output.push_str("        break;\n");
        }
        self.output.push_str("    default: context->result = SAO2_TRACE_INVALID; break;\n    }\n");
    }

    fn render_trace_value(&mut self, ty: TypeId, storage: MemberStorage, expression: &str, indent: usize) {
        if !self.trace_plan.type_contains_reference[ty.index()] { return; }
        let padding = "    ".repeat(indent);
        match self.program.types[ty.index()].clone() {
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Struct(_) if storage == MemberStorage::Inline => {
                    if self.trace_plan.struct_callbacks.contains(&definition) {
                        let _ = writeln!(self.output, "{padding}sao2_trace_body_def_{}(context, (const unsigned char *)&({expression}));", definition.index());
                    }
                }
                DefinitionLayout::Struct(_) => {
                    let _ = writeln!(self.output, "{padding}sao2_trace_enqueue(context, {expression}, &sao2_layout_def_{});", definition.index());
                }
                DefinitionLayout::Tuple(_) | DefinitionLayout::Union(_) => {
                    let _ = writeln!(self.output, "{padding}sao2_trace_value_def_{}(context, &({expression}));", definition.index());
                }
            },
            Type::Union(_) => {
                let _ = writeln!(self.output, "{padding}sao2_trace_value_ty_{}(context, &({expression}));", ty.index());
            }
            Type::Unit | Type::Primitive(_) | Type::List(_) | Type::Map { .. } => {}
        }
    }

    fn render_shadow_declarations(&mut self) {
        self.output.push_str(concat!(
            "\ntypedef struct sao2_shadow_frame sao2_shadow_frame;\n",
            "typedef void (*sao2_trace_frame_fn)(sao2_trace_context *context, const sao2_shadow_frame *frame);\n",
            "struct sao2_shadow_frame { sao2_shadow_frame *previous; sao2_trace_frame_fn trace; };\n",
            "static sao2_shadow_frame *sao2_shadow_top = NULL;\n\n",
        ));
    }

    fn render_shadow_frame_definitions(&mut self) {
        for entry in self.root_plan.functions.clone().into_iter().filter(|item| !item.locals.is_empty()) {
            let id = entry.function.index();
            let _ = writeln!(self.output, "typedef struct sao2_shadow_frame_fn_{id} {{");
            self.output.push_str("    sao2_shadow_frame header;\n");
            for (local, ty) in &entry.locals {
                let _ = writeln!(self.output, "    {} local_{};", self.c_type(*ty), local.index());
            }
            let _ = writeln!(self.output, "}} sao2_shadow_frame_fn_{id};");
            let _ = writeln!(self.output, "_Static_assert(offsetof(sao2_shadow_frame_fn_{id}, header) == 0, \"SAO2 shadow header offset\");");
            for (local, ty) in &entry.locals {
                let c_ty = self.c_type(*ty);
                let _ = writeln!(self.output, "_Static_assert(sizeof(((sao2_shadow_frame_fn_{id} *)0)->local_{}) == sizeof({c_ty}), \"SAO2 shadow field size\");", local.index());
                let _ = writeln!(self.output, "_Static_assert(_Alignof(((sao2_shadow_frame_fn_{id} *)0)->local_{}) == _Alignof({c_ty}), \"SAO2 shadow field alignment\");", local.index());
            }
            let _ = writeln!(self.output, "static void sao2_trace_frame_fn_{id}(sao2_trace_context *context, const sao2_shadow_frame *frame);\n");
        }
    }

    fn render_shadow_runtime(&mut self) {
        self.output.push_str(concat!(
            "static void sao2_shadow_link(sao2_shadow_frame *frame) {\n",
            "    if (frame == NULL || frame->trace == NULL) sao2_compiler_invariant();\n",
            "    frame->previous = sao2_shadow_top; sao2_shadow_top = frame;\n}\n",
            "static void sao2_shadow_unlink(sao2_shadow_frame *frame) {\n",
            "    if (frame == NULL || sao2_shadow_top != frame) sao2_compiler_invariant();\n",
            "    sao2_shadow_top = frame->previous; frame->previous = NULL;\n}\n",
            "static void sao2_trace_global_roots(sao2_trace_context *context) { (void)context; }\n",
            "static void sao2_shadow_assert_empty(void) { if (sao2_shadow_top != NULL) sao2_compiler_invariant(); }\n",
            "static void sao2_trace_shadow_roots(sao2_trace_context *context) {\n",
            "    sao2_shadow_frame *slow = sao2_shadow_top, *fast = sao2_shadow_top, *frame;\n",
            "    while (fast != NULL && fast->previous != NULL) { slow = slow->previous; fast = fast->previous->previous; if (slow == fast) { context->result = SAO2_TRACE_INVALID; return; } }\n",
            "    for (frame = sao2_shadow_top; frame != NULL && context->result == SAO2_TRACE_OK; frame = frame->previous) {\n",
            "        if (frame->trace == NULL) { context->result = SAO2_TRACE_INVALID; return; } frame->trace(context, frame);\n    }\n}\n\n",
        ));
    }

    fn render_shadow_callbacks(&mut self) {
        for entry in self.root_plan.functions.clone().into_iter().filter(|item| !item.locals.is_empty()) {
            let id = entry.function.index();
            let _ = writeln!(self.output, "static void sao2_trace_frame_fn_{id}(sao2_trace_context *context, const sao2_shadow_frame *frame) {{");
            let _ = writeln!(self.output, "    const sao2_shadow_frame_fn_{id} *typed = (const sao2_shadow_frame_fn_{id} *)frame;");
            self.output.push_str("    if (context->result != SAO2_TRACE_OK) return;\n");
            for (local, ty) in &entry.locals {
                self.render_trace_value(*ty, MemberStorage::Referenced, &format!("typed->local_{}", local.index()), 1);
            }
            self.output.push_str("}\n\n");
        }
    }

    fn render_union(&mut self, alternatives: &[ir::UnionAlternative]) {
        self.output.push_str("    uint32_t tag;\n    union {\n");
        for (index, alternative) in alternatives.iter().enumerate() {
            let c_ty = self.c_type(alternative.payload);
            let _ = writeln!(self.output, "        {c_ty} alternative_{index}; /* tag {} */", index + 1);
        }
        self.output.push_str("    } payload;\n");
    }

    fn render_prototype(&mut self, id: FunctionId, function: &Function) {
        self.render_signature(id, function);
        self.output.push_str(";\n");
    }

    fn render_signature(&mut self, id: FunctionId, function: &Function) {
        let result = self.c_type(function.result);
        let _ = write!(self.output, "{result} sao2_fn_{}(", id.index());
        if function.parameters.is_empty() {
            self.output.push_str("void");
        } else {
            for (position, local) in function.parameters.iter().enumerate() {
                if position != 0 { self.output.push_str(", "); }
                let ty = function.locals[local.index()].ty;
                let c_ty = if self.is_entry_args(id, ty) { "sao2_args".to_owned() }
                    else { self.c_type(ty) };
                let _ = write!(self.output, "{c_ty} sao2_arg_{position}");
            }
        }
        self.output.push(')');
    }

    fn render_function(&mut self, id: FunctionId, function: &Function) {
        self.current_function = Some(id);
        let framed = !self.root_plan.function(id).locals.is_empty();
        self.render_signature(id, function);
        self.output.push_str(" {\n");
        if framed {
            let _ = writeln!(self.output, "    sao2_shadow_frame_fn_{} sao2_frame = {{0}};", id.index());
            let _ = writeln!(self.output, "    sao2_frame.header.trace = sao2_trace_frame_fn_{};", id.index());
            self.output.push_str("    sao2_shadow_link(&sao2_frame.header);\n");
        }
        for (index, local) in function.locals.iter().enumerate() {
            if self.root_plan.contains(id, LocalId::from_index(index)) { continue; }
            let ty = local.ty;
            let c_ty = if self.is_entry_args(id, ty) { "sao2_args".to_owned() }
                else { self.c_type(ty) };
            let _ = writeln!(self.output, "    {c_ty} sao2_local_{index} = {{0}};");
        }
        for (position, local) in function.parameters.iter().enumerate() {
            let _ = writeln!(self.output, "    {} = sao2_arg_{position};", self.local(*local));
        }
        let has_scoped = self.allocation_plan.function_has_scoped(id);
        if has_scoped {
            self.output.push_str("    sao2_scoped_mark sao2_function_mark = sao2_scoped_mark_current();\n");
        }
        let entry = function.entry.expect("validated function entry");
        let _ = writeln!(self.output, "    goto sao2_block_{};", entry.index());
        for (block_index, block) in function.blocks.iter().enumerate() {
            let _ = writeln!(self.output, "sao2_block_{block_index}:");
            for (operation_index, operation) in block.operations.iter().enumerate() {
                self.render_operation(id, ir::BlockId::from_index(block_index), operation_index, function, &operation.kind);
            }
            self.render_terminator(function, framed, has_scoped, &block.terminator.as_ref().expect("validated block").kind);
        }
        self.output.push_str("}\n");
        self.current_function = None;
    }

    fn render_operation(&mut self, function_id: FunctionId, block_id: ir::BlockId,
        operation_index: usize, function: &Function, operation: &OperationKind) {
        match operation {
            OperationKind::Copy { destination, operand } => {
                let value = self.operand(operand);
                let _ = writeln!(self.output, "    {} = {value};", self.local(*destination));
            }
            OperationKind::Unary { destination, operator, operand } => {
                let value = self.operand(operand);
                let expression = match operator {
                    UnaryOperator::LogicalNot => format!("!{value}"),
                    UnaryOperator::BitwiseNot => format!("sao2_int_from_bits(~sao2_int_to_bits({value}))"),
                    UnaryOperator::Plus => format!("+{value}"),
                    UnaryOperator::Minus => format!("-{value}"),
                };
                let _ = writeln!(self.output, "    {} = {expression};", self.local(*destination));
            }
            OperationKind::Binary { destination, operator, left, right } => {
                let left_value = self.operand(left);
                let right_value = self.operand(right);
                let left_ty = self.operand_type(function, left);
                let expression = self.binary_expression(*operator, left_ty, &left_value, &right_value);
                let _ = writeln!(self.output, "    {} = {expression};", self.local(*destination));
            }
            OperationKind::Convert { destination, conversion, operand } => {
                let value = self.operand(operand);
                let cast = match conversion { NumericConversion::IntToFloat => "double", NumericConversion::FloatToInt => "int64_t" };
                let _ = writeln!(self.output, "    {} = ({cast})({value});", self.local(*destination));
            }
            OperationKind::Assign { destination, value } => {
                let value = self.operand(value);
                if let Some((definition, field)) = self.inline_assignment(destination) {
                    let destination_ref = self.inline_destination_reference(destination, definition, field);
                    let _ = writeln!(self.output, "    sao2_copy_body_def_{}((sao2_body_def_{0} *)sao2_resolve_body({destination_ref}, &sao2_layout_def_{0}), (const sao2_body_def_{0} *)sao2_resolve_body({value}, &sao2_layout_def_{0}));", definition.index());
                } else {
                    let destination = self.place(destination);
                    let _ = writeln!(self.output, "    {destination} = {value};");
                }
            }
            OperationKind::Call { destination, function: callee, arguments } => {
                let arguments = arguments.iter().map(|argument| self.operand(argument)).collect::<Vec<_>>().join(", ");
                let _ = writeln!(self.output, "    {} = sao2_fn_{}({arguments});", self.local(*destination), callee.index());
            }
            OperationKind::UnionInject { destination, union_type, alternative, payload } => {
                let c_ty = self.c_type(*union_type);
                let payload = self.operand(payload);
                let destination = self.local(*destination);
                let alternative = alternative.index();
                let _ = writeln!(self.output, "    {destination} = ({c_ty}){{0}};");
                let _ = writeln!(self.output, "    {destination}.payload.alternative_{alternative} = {payload};");
                let _ = writeln!(self.output, "    {destination}.tag = UINT32_C({});", alternative + 1);
            }
            OperationKind::UnionTest { destination, union, alternative } => {
                let union = self.operand(union);
                let tag = alternative.index() + 1;
                let _ = writeln!(self.output, "    {} = ({union}).tag == UINT32_C({tag});", self.local(*destination));
            }
            OperationKind::UnionPayload { destination, union, alternative } => {
                let union = self.operand(union);
                let alternative = alternative.index();
                let tag = alternative + 1;
                let _ = writeln!(self.output, "    if (({union}).tag != UINT32_C({tag})) sao2_compiler_invariant();");
                let _ = writeln!(self.output, "    {} = ({union}).payload.alternative_{alternative};", self.local(*destination));
            }
            OperationKind::Intrinsic { destination, intrinsic, arguments, failure } => {
                self.render_intrinsic(function, *destination, *intrinsic, arguments, *failure);
            }
            OperationKind::StringIndex { destination, string, index, failure } => {
                let string = self.operand(string);
                let index = self.operand(index);
                let _ = writeln!(
                    self.output,
                    "    {} = sao2_string_index({string}, {index}, {});",
                    self.local(*destination),
                    failure.index(),
                );
            }
            OperationKind::Aggregate { destination, aggregate: ir::Aggregate::Tuple { definition: _, elements } } => {
                let c_ty = self.c_type(function.locals[destination.index()].ty);
                let _ = writeln!(self.output, "    {} = ({c_ty}){{0}};", self.local(*destination));
                for (index, element) in elements.iter().enumerate() {
                    let value = self.operand(element);
                    let _ = writeln!(self.output, "    {}.field_{index} = {value};", self.local(*destination));
                }
            }
            OperationKind::Aggregate { destination, aggregate: ir::Aggregate::Struct { definition, fields, failure } } => {
                let allocation = AllocationId { function: function_id, block: block_id, operation: operation_index };
                let class = self.allocation_plan.allocation_class(allocation)
                    .expect("validated allocation plan covers every struct aggregate");
                self.render_struct_aggregate(*destination, *definition, fields, *failure, class);
            }
            OperationKind::Check(check) => self.render_check(function, check),
            OperationKind::Aggregate { .. } | OperationKind::Builtin { .. }
            | OperationKind::BeginIteration { .. } | OperationKind::EndIteration { .. }
            | OperationKind::IterationValue { .. } => unreachable!("capability validation rejected operation"),
        }
    }

    fn render_struct_aggregate(&mut self, destination: LocalId, definition: DefinitionId,
        operands: &[(ir::FieldId, Operand)], failure: FailureSiteId, class: AllocationClass) {
        let layout = self.struct_layouts.iter().find(|item| item.definition == definition)
            .expect("validated struct layout").clone();
        let id = destination.index();
        let destination_name = self.local(destination);
        self.output.push_str("    {\n");
        let _ = writeln!(self.output, "    sao2_ref sao2_struct_ref_{id};");
        let _ = writeln!(self.output, "    unsigned char *sao2_struct_bytes_{id};");
        let _ = writeln!(self.output, "    sao2_body_def_{} *sao2_struct_body_{id};", definition.index());
        let allocation_helper = match class {
            AllocationClass::Scoped => "sao2_allocate_scoped_struct",
            AllocationClass::Heap => "sao2_allocate_struct",
        };
        let _ = writeln!(self.output, "    {allocation_helper}(&sao2_layout_def_{}, {}, &sao2_struct_ref_{id}, &sao2_struct_bytes_{id});", definition.index(), failure.index());
        let _ = writeln!(self.output, "    sao2_struct_body_{id} = (sao2_body_def_{0} *)sao2_struct_bytes_{id};", definition.index());
        for field in &layout.fields {
            let (_, operand) = operands.iter().find(|(field_id, _)| *field_id == field.id)
                .expect("validated struct field operand");
            let value = self.operand(operand);
            if field.storage == MemberStorage::Inline && field.nested_layout.is_some()
                && matches!(&self.program.types[field.ty.index()], Type::Nominal(inner) if matches!(self.program.definitions[inner.index()].layout, DefinitionLayout::Struct(_))) {
                let Type::Nominal(inner) = &self.program.types[field.ty.index()] else { unreachable!() };
                let _ = writeln!(self.output, "    sao2_copy_body_def_{}(&sao2_struct_body_{id}->field_{}, (const sao2_body_def_{0} *)sao2_resolve_body({value}, &sao2_layout_def_{0}));", inner.index(), field.id.index());
            } else {
                let _ = writeln!(self.output, "    sao2_struct_body_{id}->field_{} = {value};", field.id.index());
            }
        }
        let _ = writeln!(self.output, "    {destination_name} = sao2_struct_ref_{id};");
        self.output.push_str("    }\n");
    }

    fn render_terminator(&mut self, function: &Function, framed: bool, has_scoped: bool, terminator: &TerminatorKind) {
        match terminator {
            TerminatorKind::Jump(target) => {
                let _ = writeln!(self.output, "    goto sao2_block_{};", target.index());
            }
            TerminatorKind::Branch { condition, then_block, else_block } => {
                let condition = self.operand(condition);
                let _ = writeln!(self.output, "    if ({condition}) {{");
                let _ = writeln!(self.output, "        goto sao2_block_{};", then_block.index());
                self.output.push_str("    } else {\n");
                let _ = writeln!(self.output, "        goto sao2_block_{};", else_block.index());
                self.output.push_str("    }\n");
            }
            TerminatorKind::Return(value) => {
                let value = self.operand(value);
                if framed || has_scoped {
                    let result = self.c_type(function.result);
                    self.output.push_str("    {\n");
                    let _ = writeln!(self.output, "        {result} sao2_return_value = {value};");
                    if framed { self.output.push_str("        sao2_shadow_unlink(&sao2_frame.header);\n"); }
                    if has_scoped { self.output.push_str("        if (!sao2_scoped_restore(sao2_function_mark)) sao2_compiler_invariant();\n"); }
                    self.output.push_str("        return sao2_return_value;\n    }\n");
                } else {
                    let _ = writeln!(self.output, "    return {value};");
                }
            }
            TerminatorKind::Panic { message, failure } => {
                let message = self.operand(message);
                let _ = writeln!(self.output, "    sao2_panic({message}, {});", failure.index());
            }
            TerminatorKind::ErrorPanic { payload, failure } => {
                let payload_value = self.operand(payload);
                let helper = match &self.program.types[self.operand_type(function, payload).index()] {
                    Type::Primitive(PrimitiveType::Int) => "sao2_error_panic_int",
                    Type::Primitive(PrimitiveType::Float) => "sao2_error_panic_float",
                    Type::Primitive(PrimitiveType::Str) => "sao2_error_panic_string",
                    Type::Primitive(PrimitiveType::Bool) => "sao2_error_panic_bool",
                    Type::Primitive(PrimitiveType::Char) => "sao2_error_panic_char",
                    _ => unreachable!("capability validation rejected Error payload"),
                };
                let _ = writeln!(self.output, "    {helper}({payload_value}, {});", failure.index());
            }
            TerminatorKind::Unreachable => self.output.push_str("    sao2_compiler_invariant();\n"),
            TerminatorKind::Switch { union, targets } => {
                let union = self.operand(union);
                let mut targets = targets.clone();
                targets.sort_by_key(|(alternative, _)| *alternative);
                let _ = writeln!(self.output, "    switch (({union}).tag) {{");
                for (alternative, target) in targets {
                    let _ = writeln!(
                        self.output,
                        "        case UINT32_C({}): goto sao2_block_{};",
                        alternative.index() + 1,
                        target.index(),
                    );
                }
                self.output.push_str("        default: sao2_compiler_invariant();\n    }\n");
            }
        }
    }

    fn render_entry_adapter(&mut self) {
        let entry = self.program.entry.expect("validated program entry");
        let function = &self.program.functions[entry.index()];
        let has_args = !function.parameters.is_empty();
        if has_args {
            self.output.push_str(concat!(
                "int main(int argc, char **argv) {\n",
                "    int sao2_argument;\n",
                "    for (sao2_argument = 1; sao2_argument < argc; ++sao2_argument) {\n",
                "        const unsigned char *sao2_byte = (const unsigned char *)argv[sao2_argument];\n",
                "        while (*sao2_byte != UINT8_C(0)) {\n",
                "            if (*sao2_byte > UINT8_C(127))\n",
                "                sao2_pre_entry_panic_argument((size_t)(sao2_argument - 1));\n",
                "            ++sao2_byte;\n",
                "        }\n",
                "    }\n",
                "    sao2_args sao2_entry_args = { argc > 0 ? argc - 1 : 0, argc > 0 ? argv + 1 : argv };\n",
            ));
        } else {
            self.output.push_str("int main(void) {\n");
        }

        let arguments = if has_args { "sao2_entry_args" } else { "" };
        if self.root_plan.functions.iter().any(|item| !item.locals.is_empty()) {
            self.output.push_str("    sao2_shadow_assert_empty();\n");
        }
        self.output.push_str(concat!(
            "    if (!sao2_arena_runtime_init())\n",
            "        sao2_pre_entry_panic_arena();\n",
        ));
        match &self.program.types[function.result.index()] {
            Type::Unit => {
                let _ = writeln!(self.output, "    sao2_unit sao2_result = sao2_fn_{}({arguments});", entry.index());
                self.output.push_str("    (void)sao2_result;\n");
                if self.root_plan.functions.iter().any(|item| !item.locals.is_empty()) { self.output.push_str("    sao2_shadow_assert_empty();\n"); }
                self.output.push_str("    sao2_arena_runtime_release();\n");
                self.output.push_str("    return EXIT_SUCCESS;\n");
            }
            Type::Primitive(PrimitiveType::Int) => {
                let _ = writeln!(self.output, "    int64_t sao2_result = sao2_fn_{}({arguments});", entry.index());
                if self.root_plan.functions.iter().any(|item| !item.locals.is_empty()) { self.output.push_str("    sao2_shadow_assert_empty();\n"); }
                self.output.push_str(concat!(
                    "    if (sao2_result < INT_MIN || sao2_result > INT_MAX)\n",
                    "        { sao2_arena_runtime_release(); sao2_pre_entry_panic_exit_status(); }\n",
                    "    sao2_arena_runtime_release();\n",
                    "    return (int)sao2_result;\n",
                ));
            }
            _ => unreachable!("capability validation accepted an invalid entry result"),
        }
        self.output.push_str("}\n");
    }

    fn render_check(&mut self, function: &Function, check: &RuntimeCheck) {
        let (helper, operands, failure) = match check {
            RuntimeCheck::IntegerOverflow { operation, left, right, failure } => {
                let helper = match operation {
                    IntegerOperation::Add => "sao2_check_integer_add",
                    IntegerOperation::Subtract => "sao2_check_integer_subtract",
                    IntegerOperation::Multiply => "sao2_check_integer_multiply",
                    IntegerOperation::ShiftLeft => "sao2_check_shift_left",
                };
                (helper, vec![self.operand(left), self.operand(right)], *failure)
            }
            RuntimeCheck::IntegerNegation { operand, failure } =>
                ("sao2_check_integer_negation", vec![self.operand(operand)], *failure),
            RuntimeCheck::Division { left, right, failure } => {
                let helper = match &self.program.types[self.operand_type(function, left).index()] {
                    Type::Primitive(PrimitiveType::Int) => "sao2_check_division_int",
                    Type::Primitive(PrimitiveType::Float) => "sao2_check_division_float",
                    _ => unreachable!(),
                };
                (helper, vec![self.operand(left), self.operand(right)], *failure)
            }
            RuntimeCheck::Remainder { left, right, failure } =>
                ("sao2_check_remainder", vec![self.operand(left), self.operand(right)], *failure),
            RuntimeCheck::ShiftRange { amount, failure } =>
                ("sao2_check_shift_range", vec![self.operand(amount)], *failure),
            RuntimeCheck::FiniteFloat { operand, failure } =>
                ("sao2_check_finite_float", vec![self.operand(operand)], *failure),
            RuntimeCheck::NumericConversion { operand, failure, .. } =>
                ("sao2_check_float_to_int", vec![self.operand(operand)], *failure),
            RuntimeCheck::IterationUnlocked { .. } => unreachable!("capability validation rejected check"),
        };
        let mut arguments = operands.join(", ");
        if !arguments.is_empty() { arguments.push_str(", "); }
        let _ = writeln!(self.output, "    {helper}({arguments}{});", failure.index());
    }

    fn render_intrinsic(
        &mut self,
        function: &Function,
        destination: LocalId,
        intrinsic: Intrinsic,
        arguments: &[Operand],
        failure: FailureSiteId,
    ) {
        let site = failure.index();
        let writer = format!("sao2_output_writer_{site}");
        self.output.push_str("    {\n");
        let _ = writeln!(self.output, "    sao2_prepare_output({site});");
        let _ = writeln!(self.output, "    sao2_writer {writer} = {{ stdout, {site}, true, false }};");
        if let Some(argument) = arguments.first() {
            let value = self.operand(argument);
            let call = self.format_call(self.operand_type(function, argument), &value)
                .replace("writer", &format!("&{writer}"));
            let _ = writeln!(self.output, "    {call};");
        }
        if intrinsic == Intrinsic::Println {
            let _ = writeln!(self.output, "    sao2_writer_char(&{writer}, UINT8_C(10));");
        }
        let _ = writeln!(self.output, "    {} = (sao2_unit){{0}};", self.local(destination));
        self.output.push_str("    }\n");
    }

    fn binary_expression(&self, operator: BinaryOperator, ty: TypeId, left: &str, right: &str) -> String {
        if matches!(&self.program.types[ty.index()], Type::Primitive(PrimitiveType::Str)) {
            return match operator {
                BinaryOperator::Equal => format!("{left} == {right}"),
                BinaryOperator::NotEqual => format!("{left} != {right}"),
                BinaryOperator::Less => format!("sao2_string_compare({left}, {right}) < 0"),
                BinaryOperator::LessEqual => format!("sao2_string_compare({left}, {right}) <= 0"),
                BinaryOperator::Greater => format!("sao2_string_compare({left}, {right}) > 0"),
                BinaryOperator::GreaterEqual => format!("sao2_string_compare({left}, {right}) >= 0"),
                _ => unreachable!("validated string binary operation"),
            };
        }
        if let Type::Nominal(definition) = &self.program.types[ty.index()]
            && matches!(&self.program.definitions[definition.index()].layout, DefinitionLayout::Tuple(_))
        {
            return match operator {
                BinaryOperator::Equal => format!("sao2_tuple_equal_def_{}({left}, {right})", definition.index()),
                BinaryOperator::NotEqual => format!("!sao2_tuple_equal_def_{}({left}, {right})", definition.index()),
                _ => unreachable!("capability validation rejected tuple binary operation"),
            };
        }
        if self.is_struct_type(ty) {
            return match operator {
                BinaryOperator::Equal => format!("sao2_ref_equal({left}, {right})"),
                BinaryOperator::NotEqual => format!("!sao2_ref_equal({left}, {right})"),
                _ => unreachable!("capability validation rejected struct binary operation"),
            };
        }
        match operator {
            BinaryOperator::BitwiseOr => format!("sao2_int_from_bits(sao2_int_to_bits({left}) | sao2_int_to_bits({right}))"),
            BinaryOperator::BitwiseXor => format!("sao2_int_from_bits(sao2_int_to_bits({left}) ^ sao2_int_to_bits({right}))"),
            BinaryOperator::BitwiseAnd => format!("sao2_int_from_bits(sao2_int_to_bits({left}) & sao2_int_to_bits({right}))"),
            BinaryOperator::ShiftLeft => format!("sao2_int_from_bits(sao2_int_to_bits({left}) << (uint64_t)({right}))"),
            BinaryOperator::ShiftRight => format!("sao2_shift_right({left}, {right})"),
            BinaryOperator::Equal if matches!(&self.program.types[ty.index()], Type::Unit) => "true".to_owned(),
            BinaryOperator::NotEqual if matches!(&self.program.types[ty.index()], Type::Unit) => "false".to_owned(),
            BinaryOperator::Equal => format!("{left} == {right}"),
            BinaryOperator::NotEqual => format!("{left} != {right}"),
            BinaryOperator::Less => format!("{left} < {right}"),
            BinaryOperator::LessEqual => format!("{left} <= {right}"),
            BinaryOperator::Greater => format!("{left} > {right}"),
            BinaryOperator::GreaterEqual => format!("{left} >= {right}"),
            BinaryOperator::Add => format!("{left} + {right}"),
            BinaryOperator::Subtract => format!("{left} - {right}"),
            BinaryOperator::Multiply => format!("{left} * {right}"),
            BinaryOperator::Divide => format!("{left} / {right}"),
            BinaryOperator::Remainder => format!("{left} % {right}"),
            BinaryOperator::In => unreachable!("capability validation rejected membership"),
        }
    }

    fn operand(&self, operand: &Operand) -> String {
        match operand {
            Operand::Copy(place) => self.place(place),
            Operand::Constant(constant) => match &constant.value {
                ConstantValue::Unit => "(sao2_unit){0}".to_owned(),
                ConstantValue::Integer(value) if *value == i64::MIN =>
                    "(-INT64_C(9223372036854775807) - INT64_C(1))".to_owned(),
                ConstantValue::Integer(value) if *value < 0 => format!("(-INT64_C({}))", value.unsigned_abs()),
                ConstantValue::Integer(value) => format!("INT64_C({value})"),
                ConstantValue::Float(bits) => format!("sao2_float_from_bits(UINT64_C({bits}))"),
                ConstantValue::String(bytes) => {
                    let index = self.strings.iter().position(|candidate| candidate.bytes.as_slice() == bytes)
                        .expect("collected string");
                    format!("&sao2_string_descriptor_{index}")
                }
                ConstantValue::Character(value) => format!("UINT8_C({value})"),
                ConstantValue::Boolean(value) => value.to_string(),
            },
        }
    }

    fn local(&self, local: LocalId) -> String {
        let function = self.current_function.expect("local rendered outside a function");
        if self.root_plan.contains(function, local) { format!("sao2_frame.local_{}", local.index()) }
        else { format!("sao2_local_{}", local.index()) }
    }

    fn operand_type(&self, function: &Function, operand: &Operand) -> TypeId {
        match operand {
            Operand::Constant(constant) => constant.ty,
            Operand::Copy(place) => self.place_type(function, place),
        }
    }

    fn place(&self, place: &Place) -> String {
        self.place_through(place, place.projections.len())
    }

    fn place_through(&self, place: &Place, count: usize) -> String {
        let mut rendered = self.local(place.local);
        for projection in place.projections.iter().take(count) {
            match projection {
                Projection::TupleField { field, .. } => {
                    let _ = write!(rendered, ".field_{}", field.index());
                }
                Projection::StructField { definition, field, storage } => {
                    let layout = self.struct_layout(*definition);
                    let field_layout = layout.fields.iter().find(|item| item.id == *field).expect("validated struct field");
                    let parent = rendered;
                    let body = format!("((sao2_body_def_{0} *)sao2_resolve_body({parent}, &sao2_layout_def_{0}))", definition.index());
                    if *storage == MemberStorage::Inline && field_layout.nested_layout.is_some() && self.is_struct_type(field_layout.ty) {
                        let Type::Nominal(child) = &self.program.types[field_layout.ty.index()] else { unreachable!() };
                        rendered = format!("sao2_project_inline({parent}, &sao2_layout_def_{}, &sao2_layout_fields_def_{}[{}], &sao2_layout_def_{})", definition.index(), definition.index(), field.index(), child.index());
                    } else {
                        rendered = format!("{body}->field_{}", field.index());
                    }
                }
                Projection::ListIndex { .. } | Projection::MapIndex { .. } => {
                    unreachable!("capability validation rejected projection")
                }
            }
        }
        rendered
    }

    fn struct_layout(&self, definition: DefinitionId) -> &StructLayout {
        self.struct_layouts.iter().find(|layout| layout.definition == definition).expect("validated struct layout")
    }

    fn inline_assignment(&self, place: &Place) -> Option<(DefinitionId, ir::FieldId)> {
        match place.projections.last()? {
            Projection::StructField { definition, field, storage: MemberStorage::Inline }
                if self.struct_layout(*definition).fields.iter().any(|item| item.id == *field && item.nested_layout.is_some()) => {
                    let layout = self.struct_layout(*definition);
                    let item = layout.fields.iter().find(|item| item.id == *field).expect("validated inline field");
                    let Type::Nominal(child) = &self.program.types[item.ty.index()] else { unreachable!() };
                    Some((*child, *field))
                }
            _ => None,
        }
    }

    fn inline_destination_reference(&self, place: &Place, child: DefinitionId, field: ir::FieldId) -> String {
        let parent = self.place_through(place, place.projections.len() - 1);
        let Projection::StructField { definition: parent_definition, .. } = place.projections.last().expect("inline projection") else { unreachable!() };
        format!("sao2_project_inline({parent}, &sao2_layout_def_{}, &sao2_layout_fields_def_{}[{}], &sao2_layout_def_{})", parent_definition.index(), parent_definition.index(), field.index(), child.index())
    }

    fn place_type(&self, function: &Function, place: &Place) -> TypeId {
        let mut ty = function.locals[place.local.index()].ty;
        for projection in &place.projections {
            ty = match projection {
                Projection::TupleField { definition, field } => {
                    let DefinitionLayout::Tuple(fields) = &self.program.definitions[definition.index()].layout
                        else { unreachable!("validated tuple projection") };
                    fields[field.index()]
                }
                Projection::StructField { definition, field, .. } => {
                    let DefinitionLayout::Struct(fields) = &self.program.definitions[definition.index()].layout
                        else { unreachable!("validated struct projection") };
                    fields[field.index()].ty
                }
                Projection::ListIndex { .. } | Projection::MapIndex { .. } => {
                    unreachable!("capability validation rejected projection")
                }
            };
        }
        ty
    }

    fn is_entry_args(&self, function: FunctionId, ty: TypeId) -> bool {
        Some(function) == self.program.entry && self.is_string_list(ty)
    }

    fn is_string_list(&self, ty: TypeId) -> bool {
        matches!(&self.program.types[ty.index()], Type::List(element)
            if matches!(&self.program.types[element.index()], Type::Primitive(PrimitiveType::Str)))
    }

    fn c_type(&self, ty: TypeId) -> String {
        match &self.program.types[ty.index()] {
            Type::Unit => "sao2_unit".to_owned(),
            Type::Primitive(PrimitiveType::Int) => "int64_t".to_owned(),
            Type::Primitive(PrimitiveType::Float) => "double".to_owned(),
            Type::Primitive(PrimitiveType::Str) => "sao2_string".to_owned(),
            Type::Primitive(PrimitiveType::Bool) => "bool".to_owned(),
            Type::Primitive(PrimitiveType::Char) => "uint8_t".to_owned(),
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Struct(_) => "sao2_ref".to_owned(),
                DefinitionLayout::Tuple(_) | DefinitionLayout::Union(_) => format!("sao2_def_{}", definition.index()),
            },
            Type::Union(_) => format!("sao2_union_ty_{}", ty.index()),
            Type::List(_) | Type::Map { .. } => unreachable!("capability validation rejected container C type"),
        }
    }

    fn is_struct_type(&self, ty: TypeId) -> bool {
        matches!(&self.program.types[ty.index()], Type::Nominal(definition)
            if matches!(self.program.definitions[definition.index()].layout, DefinitionLayout::Struct(_)))
    }
}

const ALL_FAILURE_OPERATIONS: &[FailureOperation] = &[
    FailureOperation::IntegerAdd, FailureOperation::IntegerSubtract,
    FailureOperation::IntegerMultiply, FailureOperation::IntegerNegation,
    FailureOperation::IntegerDivision, FailureOperation::IntegerRemainder,
    FailureOperation::ShiftLeft, FailureOperation::ShiftRight,
    FailureOperation::FloatAdd, FailureOperation::FloatSubtract,
    FailureOperation::FloatMultiply, FailureOperation::FloatDivision,
    FailureOperation::FloatToInt, FailureOperation::ListIndex,
    FailureOperation::MapIndex, FailureOperation::StringIndex,
    FailureOperation::ListAppend, FailureOperation::ListRemoveIndex,
    FailureOperation::MapRemoveKey, FailureOperation::Output,
    FailureOperation::ExplicitPanic, FailureOperation::UnhandledError,
    FailureOperation::StructAllocation,
];

fn failure_operation_code(operation: FailureOperation) -> u32 {
    match operation {
        FailureOperation::IntegerAdd => 0,
        FailureOperation::IntegerSubtract => 1,
        FailureOperation::IntegerMultiply => 2,
        FailureOperation::IntegerNegation => 3,
        FailureOperation::IntegerDivision => 4,
        FailureOperation::IntegerRemainder => 5,
        FailureOperation::ShiftLeft => 6,
        FailureOperation::ShiftRight => 7,
        FailureOperation::FloatAdd => 8,
        FailureOperation::FloatSubtract => 9,
        FailureOperation::FloatMultiply => 10,
        FailureOperation::FloatDivision => 11,
        FailureOperation::FloatToInt => 12,
        FailureOperation::ListIndex => 13,
        FailureOperation::MapIndex => 14,
        FailureOperation::StringIndex => 15,
        FailureOperation::ListAppend => 16,
        FailureOperation::ListRemoveIndex => 17,
        FailureOperation::MapRemoveKey => 18,
        FailureOperation::Output => 19,
        FailureOperation::ExplicitPanic => 20,
        FailureOperation::UnhandledError => 21,
        FailureOperation::StructAllocation => 22,
    }
}

fn failure_operation_macro(operation: FailureOperation) -> &'static str {
    match operation {
        FailureOperation::IntegerAdd => "SAO2_FAILURE_INTEGER_ADD",
        FailureOperation::IntegerSubtract => "SAO2_FAILURE_INTEGER_SUBTRACT",
        FailureOperation::IntegerMultiply => "SAO2_FAILURE_INTEGER_MULTIPLY",
        FailureOperation::IntegerNegation => "SAO2_FAILURE_INTEGER_NEGATION",
        FailureOperation::IntegerDivision => "SAO2_FAILURE_INTEGER_DIVISION",
        FailureOperation::IntegerRemainder => "SAO2_FAILURE_INTEGER_REMAINDER",
        FailureOperation::ShiftLeft => "SAO2_FAILURE_SHIFT_LEFT",
        FailureOperation::ShiftRight => "SAO2_FAILURE_SHIFT_RIGHT",
        FailureOperation::FloatAdd => "SAO2_FAILURE_FLOAT_ADD",
        FailureOperation::FloatSubtract => "SAO2_FAILURE_FLOAT_SUBTRACT",
        FailureOperation::FloatMultiply => "SAO2_FAILURE_FLOAT_MULTIPLY",
        FailureOperation::FloatDivision => "SAO2_FAILURE_FLOAT_DIVISION",
        FailureOperation::FloatToInt => "SAO2_FAILURE_FLOAT_TO_INT",
        FailureOperation::ListIndex => "SAO2_FAILURE_LIST_INDEX",
        FailureOperation::MapIndex => "SAO2_FAILURE_MAP_INDEX",
        FailureOperation::StringIndex => "SAO2_FAILURE_STRING_INDEX",
        FailureOperation::ListAppend => "SAO2_FAILURE_LIST_APPEND",
        FailureOperation::ListRemoveIndex => "SAO2_FAILURE_LIST_REMOVE_INDEX",
        FailureOperation::MapRemoveKey => "SAO2_FAILURE_MAP_REMOVE_KEY",
        FailureOperation::StructAllocation => "SAO2_FAILURE_STRUCT_ALLOCATION",
        FailureOperation::Output => "SAO2_FAILURE_OUTPUT",
        FailureOperation::ExplicitPanic => "SAO2_FAILURE_EXPLICIT_PANIC",
        FailureOperation::UnhandledError => "SAO2_FAILURE_UNHANDLED_ERROR",
    }
}

const ARENA_RUNTIME: &str = r#"

/*
 * Milestone 10 block-heap foundation. Heap blocks can be walked, reclaimed,
 * coalesced, and reused. Stage 2 tracing may update allocated mark epochs;
 * automatic root discovery, sweeping, and collection are still pending.
 * Scoped allocation remains an independent, restorable bump arena.
 */
#ifndef SAO2_ARENA_CAPACITY
#define SAO2_ARENA_CAPACITY UINT64_C(4294967296)
#endif
#define SAO2_ARENA_INITIAL_CURSOR UINT64_C(8)
#define SAO2_REF_OWNER_TAG_MASK UINT32_C(7)
#define SAO2_REF_HEAP_TAG UINT32_C(0)
#define SAO2_REF_SCOPED_TAG UINT32_C(1)
#define SAO2_REF_ALIGNMENT UINT64_C(8)
#define SAO2_HEAP_LIFETIME UINT32_C(0)
#define SAO2_HEAP_BLOCK_ALLOCATED UINT32_C(1)
#define SAO2_HEAP_BLOCK_FREE UINT32_C(2)

typedef enum {
    SAO2_ARENA_OK,
    SAO2_ARENA_EXHAUSTED,
    SAO2_ARENA_COMMIT_FAILED,
    SAO2_ARENA_INVALID
} sao2_arena_result;

typedef enum { SAO2_ARENA_HEAP, SAO2_ARENA_SCOPED } sao2_arena_kind;

typedef struct {
    unsigned char *base;
    uint64_t capacity;
    uint64_t page_size;
    uint64_t committed;
    uint64_t frontier;
    uint64_t free_head;
} sao2_arena;

typedef struct {
    uint64_t span_size;
    uint64_t body_size;
    uint64_t layout_identity;
    uint64_t next_free;
    uint32_t lifetime;
    uint32_t block_state;
    uint32_t mark_epoch;
    uint32_t reserved;
} sao2_heap_header;

_Static_assert(sizeof(sao2_heap_header) != 0, "SAO2 heap header must not be empty");
_Static_assert(sizeof(sao2_heap_header) == 48, "SAO2 heap header ABI");
_Static_assert(sizeof(sao2_heap_header) % 8 == 0, "SAO2 heap header must preserve body alignment");

static sao2_arena sao2_heap_arena;
static sao2_arena sao2_scoped_arena;
static bool sao2_arenas_initialized;

static bool sao2_is_power_of_two(uint64_t value) {
    return value != 0 && (value & (value - UINT64_C(1))) == 0;
}

static bool sao2_platform_page_size(uint64_t *result) {
#ifdef _WIN32
    SYSTEM_INFO information;
    uint64_t allocation_granularity;
    GetSystemInfo(&information);
    *result = (uint64_t)information.dwPageSize;
    allocation_granularity = (uint64_t)information.dwAllocationGranularity;
    if (!sao2_is_power_of_two(allocation_granularity)
        || SAO2_ARENA_CAPACITY % allocation_granularity != 0) return false;
#else
    long value = sysconf(_SC_PAGESIZE);
    if (value <= 0) return false;
    *result = (uint64_t)value;
#endif
    return sao2_is_power_of_two(*result) && SAO2_ARENA_CAPACITY % *result == 0;
}

static unsigned char *sao2_platform_reserve(uint64_t size) {
    if (size > (uint64_t)SIZE_MAX) return NULL;
#ifdef _WIN32
    return (unsigned char *)VirtualAlloc(NULL, (SIZE_T)size, MEM_RESERVE, PAGE_NOACCESS);
#else
    void *mapping = mmap(NULL, (size_t)size, PROT_NONE, MAP_PRIVATE |
#if defined(MAP_ANONYMOUS)
        MAP_ANONYMOUS,
#else
        MAP_ANON,
#endif
        -1, 0);
    return mapping == MAP_FAILED ? NULL : (unsigned char *)mapping;
#endif
}

static bool sao2_platform_commit(unsigned char *base, uint64_t offset, uint64_t size) {
    if (base == NULL || offset > (uint64_t)SIZE_MAX || size > (uint64_t)SIZE_MAX) return false;
#ifdef SAO2_RUNTIME_PROBE
    if (sao2_probe_fail_next_commit) { sao2_probe_fail_next_commit = false; return false; }
#endif
#ifdef _WIN32
    return VirtualAlloc(base + (size_t)offset, (SIZE_T)size, MEM_COMMIT, PAGE_READWRITE) != NULL;
#else
    return mprotect(base + (size_t)offset, (size_t)size, PROT_READ | PROT_WRITE) == 0;
#endif
}

static void sao2_platform_release(unsigned char *base, uint64_t size) {
    if (base == NULL) return;
#ifdef _WIN32
    (void)size;
    (void)VirtualFree(base, 0, MEM_RELEASE);
#else
    (void)munmap(base, (size_t)size);
#endif
}

static void sao2_arena_clear(sao2_arena *arena) {
    arena->base = NULL;
    arena->capacity = 0;
    arena->page_size = 0;
    arena->committed = 0;
    arena->frontier = 0;
    arena->free_head = 0;
}

static void sao2_arena_release(sao2_arena *arena) {
    sao2_platform_release(arena->base, arena->capacity);
    sao2_arena_clear(arena);
}

static bool sao2_arena_runtime_init(void) {
    uint64_t page_size;
    if (sao2_arenas_initialized || !sao2_platform_page_size(&page_size)) return false;
    sao2_arena_clear(&sao2_heap_arena);
    sao2_arena_clear(&sao2_scoped_arena);
    sao2_heap_arena.base = sao2_platform_reserve(SAO2_ARENA_CAPACITY);
    if (sao2_heap_arena.base == NULL) return false;
    sao2_heap_arena.capacity = SAO2_ARENA_CAPACITY;
    sao2_heap_arena.page_size = page_size;
    sao2_heap_arena.frontier = SAO2_ARENA_INITIAL_CURSOR;
    sao2_scoped_arena.base = sao2_platform_reserve(SAO2_ARENA_CAPACITY);
    if (sao2_scoped_arena.base == NULL) {
        sao2_arena_release(&sao2_heap_arena);
        return false;
    }
    sao2_scoped_arena.capacity = SAO2_ARENA_CAPACITY;
    sao2_scoped_arena.page_size = page_size;
    sao2_scoped_arena.frontier = SAO2_ARENA_INITIAL_CURSOR;
    sao2_arenas_initialized = true;
    return true;
}

static void sao2_arena_runtime_release(void) {
    sao2_arena_release(&sao2_scoped_arena);
    sao2_arena_release(&sao2_heap_arena);
    sao2_arenas_initialized = false;
}

static sao2_arena *sao2_arena_for_kind(sao2_arena_kind kind) {
    return kind == SAO2_ARENA_HEAP ? &sao2_heap_arena : &sao2_scoped_arena;
}

static bool sao2_align_eight(uint64_t value, uint64_t *result) {
    uint64_t remainder = value % SAO2_REF_ALIGNMENT;
    if (remainder == 0) { *result = value; return true; }
    if (value > UINT64_MAX - (SAO2_REF_ALIGNMENT - remainder)) return false;
    *result = value + (SAO2_REF_ALIGNMENT - remainder);
    return true;
}

static sao2_arena_result sao2_arena_commit_through(sao2_arena *arena, uint64_t end) {
    uint64_t committed_end, remainder;
    if (arena->base == NULL || arena->page_size == 0 || end > arena->capacity)
        return SAO2_ARENA_INVALID;
    remainder = end % arena->page_size;
    if (remainder == 0) committed_end = end;
    else {
        if (end > UINT64_MAX - (arena->page_size - remainder)) return SAO2_ARENA_EXHAUSTED;
        committed_end = end + (arena->page_size - remainder);
    }
    if (committed_end > arena->capacity) return SAO2_ARENA_EXHAUSTED;
    if (committed_end > arena->committed) {
        if (!sao2_platform_commit(arena->base, arena->committed, committed_end - arena->committed))
            return SAO2_ARENA_COMMIT_FAILED;
        arena->committed = committed_end;
    }
    return SAO2_ARENA_OK;
}

static sao2_arena_result sao2_scoped_bump_allocate(sao2_arena *arena, uint64_t size,
    uint64_t alignment, uint64_t *offset) {
    uint64_t start, body_size, end, rounded_end;
    sao2_arena_result status;
    if (arena->base == NULL || alignment != SAO2_REF_ALIGNMENT) return SAO2_ARENA_INVALID;
    if (!sao2_align_eight(arena->frontier, &start)) return SAO2_ARENA_EXHAUSTED;
    if (start > arena->capacity) return SAO2_ARENA_EXHAUSTED;
    body_size = size == 0 ? SAO2_REF_ALIGNMENT : size;
    if (body_size > arena->capacity - start) return SAO2_ARENA_EXHAUSTED;
    end = start + body_size;
    if (!sao2_align_eight(end, &rounded_end) || rounded_end > arena->capacity)
        return SAO2_ARENA_EXHAUSTED;
    status = sao2_arena_commit_through(arena, rounded_end);
    if (status != SAO2_ARENA_OK) return status;
    arena->frontier = rounded_end;
    *offset = start;
    return SAO2_ARENA_OK;
}

static bool sao2_ref_root(sao2_arena_kind kind, uint64_t offset, sao2_ref *result) {
    uint32_t tag = kind == SAO2_ARENA_HEAP ? SAO2_REF_HEAP_TAG : SAO2_REF_SCOPED_TAG;
    if (offset == 0 || offset > UINT32_MAX || offset % SAO2_REF_ALIGNMENT != 0) return false;
    result->owner_ptr = (uint32_t)offset | tag;
    result->member_ptr = (uint32_t)offset;
    return true;
}

static bool sao2_ref_interior(sao2_ref owner, uint64_t member_offset, sao2_ref *result) {
    uint32_t tag = owner.owner_ptr & SAO2_REF_OWNER_TAG_MASK;
    uint64_t owner_offset = (uint64_t)(owner.owner_ptr & ~SAO2_REF_OWNER_TAG_MASK);
    if (owner_offset == 0 || tag > SAO2_REF_SCOPED_TAG || member_offset == 0 || member_offset > UINT32_MAX)
        return false;
    result->owner_ptr = owner.owner_ptr;
    result->member_ptr = (uint32_t)member_offset;
    return true;
}

static bool sao2_ref_kind(sao2_ref reference, sao2_arena_kind *kind) {
    uint32_t tag;
    if (reference.owner_ptr == 0 && reference.member_ptr == 0) return false;
    if (reference.owner_ptr == 0 || reference.member_ptr == 0) return false;
    tag = reference.owner_ptr & SAO2_REF_OWNER_TAG_MASK;
    if (tag == SAO2_REF_HEAP_TAG) *kind = SAO2_ARENA_HEAP;
    else if (tag == SAO2_REF_SCOPED_TAG) *kind = SAO2_ARENA_SCOPED;
    else return false;
    return true;
}

static bool sao2_heap_span(uint64_t body_size, uint64_t *result) {
    uint64_t payload = body_size < SAO2_REF_ALIGNMENT ? SAO2_REF_ALIGNMENT : body_size;
    uint64_t rounded_payload;
    if (!sao2_align_eight(payload, &rounded_payload)
        || rounded_payload > UINT64_MAX - (uint64_t)sizeof(sao2_heap_header)) return false;
    *result = (uint64_t)sizeof(sao2_heap_header) + rounded_payload;
    return true;
}

static sao2_heap_header *sao2_heap_header_at(uint64_t offset) {
    return (sao2_heap_header *)(sao2_heap_arena.base + (size_t)offset);
}

static bool sao2_heap_block_valid(uint64_t offset, uint64_t frontier, uint64_t *next) {
    sao2_heap_header *header;
    uint64_t minimum_span, end, payload_capacity;
    if (!sao2_heap_span(0, &minimum_span) || sao2_heap_arena.base == NULL
        || offset < SAO2_ARENA_INITIAL_CURSOR || offset % SAO2_REF_ALIGNMENT != 0
        || offset > frontier || (uint64_t)sizeof(sao2_heap_header) > frontier - offset)
        return false;
    header = sao2_heap_header_at(offset);
    if (header->span_size < minimum_span || header->span_size % SAO2_REF_ALIGNMENT != 0
        || header->span_size > frontier - offset) return false;
    end = offset + header->span_size;
    payload_capacity = header->span_size - (uint64_t)sizeof(sao2_heap_header);
    if (header->block_state == SAO2_HEAP_BLOCK_ALLOCATED) {
        if (header->body_size > payload_capacity || header->next_free != 0
            || header->lifetime != SAO2_HEAP_LIFETIME || header->reserved != 0) return false;
    } else if (header->block_state == SAO2_HEAP_BLOCK_FREE) {
        if (header->body_size != 0 || header->layout_identity != 0 || header->lifetime != 0
            || header->mark_epoch != 0 || header->reserved != 0
            || (header->next_free != 0 && (header->next_free <= offset
                || header->next_free >= frontier || header->next_free % SAO2_REF_ALIGNMENT != 0)))
            return false;
    } else return false;
    *next = end;
    return true;
}

static bool sao2_heap_find_block(uint64_t wanted, sao2_heap_header **result) {
    uint64_t offset = SAO2_ARENA_INITIAL_CURSOR, next;
    while (offset < sao2_heap_arena.frontier) {
        if (!sao2_heap_block_valid(offset, sao2_heap_arena.frontier, &next)) return false;
        if (offset == wanted) { *result = sao2_heap_header_at(offset); return true; }
        if (offset > wanted) return false;
        offset = next;
    }
    return false;
}

static bool sao2_heap_validate(void) {
    uint64_t offset, next, prior_free = 0, free_offset, free_count = 0, listed_count = 0;
    bool previous_free = false;
    if (sao2_heap_arena.base == NULL || sao2_heap_arena.frontier < SAO2_ARENA_INITIAL_CURSOR
        || sao2_heap_arena.frontier > sao2_heap_arena.capacity
        || (sao2_heap_arena.frontier > SAO2_ARENA_INITIAL_CURSOR
            && sao2_heap_arena.frontier > sao2_heap_arena.committed)
        || sao2_heap_arena.frontier % SAO2_REF_ALIGNMENT != 0) return false;
    offset = SAO2_ARENA_INITIAL_CURSOR;
    while (offset < sao2_heap_arena.frontier) {
        sao2_heap_header *header;
        if (!sao2_heap_block_valid(offset, sao2_heap_arena.frontier, &next)) return false;
        header = sao2_heap_header_at(offset);
        if (header->block_state == SAO2_HEAP_BLOCK_FREE) {
            if (previous_free) return false;
            previous_free = true;
            free_count++;
        } else previous_free = false;
        offset = next;
    }
    if (offset != sao2_heap_arena.frontier) return false;
    free_offset = sao2_heap_arena.free_head;
    while (free_offset != 0) {
        sao2_heap_header *header;
        if (free_offset <= prior_free || !sao2_heap_find_block(free_offset, &header)
            || header->block_state != SAO2_HEAP_BLOCK_FREE) return false;
        prior_free = free_offset;
        free_offset = header->next_free;
        listed_count++;
        if (listed_count > free_count) return false;
    }
    if (listed_count != free_count) return false;
    offset = SAO2_ARENA_INITIAL_CURSOR;
    while (offset < sao2_heap_arena.frontier) {
        sao2_heap_header *header = sao2_heap_header_at(offset);
        uint64_t scan = sao2_heap_arena.free_head, appearances = 0;
        while (scan != 0) {
            if (scan == offset) appearances++;
            scan = sao2_heap_header_at(scan)->next_free;
        }
        if ((header->block_state == SAO2_HEAP_BLOCK_FREE && appearances != 1)
            || (header->block_state == SAO2_HEAP_BLOCK_ALLOCATED && appearances != 0)) return false;
        offset += header->span_size;
    }
    return true;
}

static void sao2_heap_initialize_allocated(sao2_heap_header *header, uint64_t span,
    uint64_t body_size, uint64_t layout_identity) {
    header->span_size = span;
    header->body_size = body_size;
    header->layout_identity = layout_identity;
    header->next_free = 0;
    header->lifetime = SAO2_HEAP_LIFETIME;
    header->block_state = SAO2_HEAP_BLOCK_ALLOCATED;
    header->mark_epoch = 0;
    header->reserved = 0;
}

static void sao2_heap_initialize_free(sao2_heap_header *header, uint64_t span, uint64_t next_free) {
    header->span_size = span;
    header->body_size = 0;
    header->layout_identity = 0;
    header->next_free = next_free;
    header->lifetime = 0;
    header->block_state = SAO2_HEAP_BLOCK_FREE;
    header->mark_epoch = 0;
    header->reserved = 0;
}

static sao2_arena_result sao2_heap_allocate(uint64_t body_size, uint64_t alignment,
    uint64_t layout_identity, sao2_ref *result) {
    uint64_t required_span, minimum_span, previous = 0, selected, body_offset;
    sao2_ref published;
    if (result == NULL || alignment != SAO2_REF_ALIGNMENT) return SAO2_ARENA_INVALID;
    if (!sao2_heap_span(body_size, &required_span) || !sao2_heap_span(0, &minimum_span))
        return SAO2_ARENA_EXHAUSTED;
    if (!sao2_heap_validate()) return SAO2_ARENA_INVALID;
    selected = sao2_heap_arena.free_head;
    while (selected != 0) {
        sao2_heap_header *header = sao2_heap_header_at(selected);
        if (header->span_size >= required_span) break;
        previous = selected;
        selected = header->next_free;
    }
    if (selected != 0) {
        sao2_heap_header *header = sao2_heap_header_at(selected);
        uint64_t selected_span = header->span_size, successor = header->next_free;
        uint64_t allocation_span = selected_span, remainder_span = selected_span - required_span;
        uint64_t remainder_offset = 0;
        body_offset = selected + (uint64_t)sizeof(sao2_heap_header);
        if (!sao2_ref_root(SAO2_ARENA_HEAP, body_offset, &published)) return SAO2_ARENA_INVALID;
        if (remainder_span >= minimum_span) {
            allocation_span = required_span;
            remainder_offset = selected + required_span;
            sao2_heap_initialize_free(sao2_heap_header_at(remainder_offset), remainder_span, successor);
            if (previous == 0) sao2_heap_arena.free_head = remainder_offset;
            else sao2_heap_header_at(previous)->next_free = remainder_offset;
        } else {
            if (previous == 0) sao2_heap_arena.free_head = successor;
            else sao2_heap_header_at(previous)->next_free = successor;
        }
        sao2_heap_initialize_allocated(header, allocation_span, body_size, layout_identity);
        memset(sao2_heap_arena.base + (size_t)body_offset, 0,
            (size_t)(allocation_span - (uint64_t)sizeof(sao2_heap_header)));
        *result = published;
        return SAO2_ARENA_OK;
    } else {
        sao2_arena_result status;
        uint64_t header_offset = sao2_heap_arena.frontier, end;
        sao2_heap_header *header;
        if (header_offset > sao2_heap_arena.capacity || required_span > sao2_heap_arena.capacity - header_offset)
            return SAO2_ARENA_EXHAUSTED;
        end = header_offset + required_span;
        body_offset = header_offset + (uint64_t)sizeof(sao2_heap_header);
        if (end > UINT64_C(4294967296) || !sao2_ref_root(SAO2_ARENA_HEAP, body_offset, &published))
            return SAO2_ARENA_EXHAUSTED;
        status = sao2_arena_commit_through(&sao2_heap_arena, end);
        if (status != SAO2_ARENA_OK) return status;
        header = sao2_heap_header_at(header_offset);
        sao2_heap_initialize_allocated(header, required_span, body_size, layout_identity);
        memset(sao2_heap_arena.base + (size_t)body_offset, 0,
            (size_t)(required_span - (uint64_t)sizeof(sao2_heap_header)));
        sao2_heap_arena.frontier = end;
        *result = published;
    }
    return SAO2_ARENA_OK;
}

static bool sao2_heap_header_for(sao2_ref reference, sao2_heap_header **result) {
    sao2_arena_kind kind;
    uint64_t owner, header_offset;
    sao2_heap_header *header;
    if (result == NULL || !sao2_ref_kind(reference, &kind)
        || kind != SAO2_ARENA_HEAP || !sao2_heap_validate()) return false;
    owner = (uint64_t)(reference.owner_ptr & ~SAO2_REF_OWNER_TAG_MASK);
    if (owner == 0 || owner < (uint64_t)sizeof(sao2_heap_header) + SAO2_ARENA_INITIAL_CURSOR)
        return false;
    header_offset = owner - (uint64_t)sizeof(sao2_heap_header);
    if (!sao2_heap_find_block(header_offset, &header)
        || header->block_state != SAO2_HEAP_BLOCK_ALLOCATED
        || header_offset + (uint64_t)sizeof(sao2_heap_header) != owner
        || header->lifetime != SAO2_HEAP_LIFETIME
        || header->body_size > header->span_size - (uint64_t)sizeof(sao2_heap_header)) return false;
    *result = header;
    return true;
}

static bool sao2_heap_reclaim(uint64_t owner_offset) {
    uint64_t header_offset, previous = 0, next_free, merged_offset;
    sao2_heap_header *header, *merged;
    if (!sao2_heap_validate() || owner_offset < (uint64_t)sizeof(sao2_heap_header) + SAO2_ARENA_INITIAL_CURSOR)
        return false;
    header_offset = owner_offset - (uint64_t)sizeof(sao2_heap_header);
    if (!sao2_heap_find_block(header_offset, &header)
        || header->block_state != SAO2_HEAP_BLOCK_ALLOCATED
        || header_offset + (uint64_t)sizeof(sao2_heap_header) != owner_offset) return false;
    next_free = sao2_heap_arena.free_head;
    while (next_free != 0 && next_free < header_offset) {
        previous = next_free;
        next_free = sao2_heap_header_at(next_free)->next_free;
    }
    sao2_heap_initialize_free(header, header->span_size, next_free);
    if (previous == 0) sao2_heap_arena.free_head = header_offset;
    else sao2_heap_header_at(previous)->next_free = header_offset;
    merged_offset = header_offset;
    merged = header;
    if (previous != 0) {
        sao2_heap_header *predecessor = sao2_heap_header_at(previous);
        if (previous + predecessor->span_size == header_offset) {
            predecessor->span_size += merged->span_size;
            predecessor->next_free = merged->next_free;
            merged_offset = previous;
            merged = predecessor;
        }
    }
    if (merged->next_free != 0 && merged_offset + merged->span_size == merged->next_free) {
        sao2_heap_header *successor = sao2_heap_header_at(merged->next_free);
        merged->span_size += successor->span_size;
        merged->next_free = successor->next_free;
    }
    while (true) {
        uint64_t scan = sao2_heap_arena.free_head, scan_previous = 0;
        sao2_heap_header *tail = NULL;
        while (scan != 0) {
            sao2_heap_header *candidate = sao2_heap_header_at(scan);
            if (scan + candidate->span_size == sao2_heap_arena.frontier) { tail = candidate; break; }
            scan_previous = scan;
            scan = candidate->next_free;
        }
        if (tail == NULL) break;
        if (scan_previous == 0) sao2_heap_arena.free_head = tail->next_free;
        else sao2_heap_header_at(scan_previous)->next_free = tail->next_free;
        memset(sao2_heap_arena.base + (size_t)scan, 0, (size_t)tail->span_size);
        sao2_heap_arena.frontier = scan;
    }
    return sao2_heap_validate();
}

typedef uint64_t sao2_scoped_mark;

static sao2_scoped_mark sao2_scoped_mark_current(void) { return sao2_scoped_arena.frontier; }

static sao2_arena_result sao2_scoped_allocate(uint64_t body_size, sao2_ref *result) {
    uint64_t offset, predicted_start;
    if (!sao2_align_eight(sao2_scoped_arena.frontier, &predicted_start) || predicted_start > UINT32_MAX)
        return SAO2_ARENA_EXHAUSTED;
    sao2_arena_result status = sao2_scoped_bump_allocate(&sao2_scoped_arena, body_size, SAO2_REF_ALIGNMENT, &offset);
    if (status != SAO2_ARENA_OK) return status;
    if (!sao2_ref_root(SAO2_ARENA_SCOPED, offset, result)) return SAO2_ARENA_INVALID;
    memset(sao2_scoped_arena.base + (size_t)offset, 0, (size_t)(body_size == 0 ? SAO2_REF_ALIGNMENT : body_size));
    return SAO2_ARENA_OK;
}

static bool sao2_scoped_restore(sao2_scoped_mark mark) {
    if (sao2_scoped_arena.base == NULL || mark < SAO2_ARENA_INITIAL_CURSOR || mark > sao2_scoped_arena.frontier)
        return false;
    sao2_scoped_arena.frontier = mark;
    return true;
}
"#;

const TRACE_RUNTIME: &str = r#"

typedef enum {
    SAO2_TRACE_OK,
    SAO2_TRACE_SCRATCH_EXHAUSTED,
    SAO2_TRACE_INVALID
} sao2_trace_result;

typedef struct {
    sao2_ref reference;
    const sao2_layout_descriptor *layout;
} sao2_trace_item;

struct sao2_trace_context {
    uint32_t epoch;
    sao2_trace_result result;
    sao2_trace_item *items;
    size_t item_count;
    size_t item_capacity;
    size_t next_item;
    size_t *slots;
    size_t slot_count;
    size_t slot_capacity;
};

static void *sao2_trace_realloc(void *pointer, size_t size) {
#ifdef SAO2_TRACE_PROBE
    if (sao2_probe_fail_next_trace_allocation) {
        sao2_probe_fail_next_trace_allocation = false;
        return NULL;
    }
#endif
    return realloc(pointer, size);
}

static void *sao2_trace_calloc(size_t count, size_t size) {
#ifdef SAO2_TRACE_PROBE
    if (sao2_probe_fail_next_trace_allocation) {
        sao2_probe_fail_next_trace_allocation = false;
        return NULL;
    }
#endif
    return calloc(count, size);
}

static void sao2_trace_context_init(sao2_trace_context *context, uint32_t epoch) {
    if (context == NULL) return;
    memset(context, 0, sizeof *context);
    context->epoch = epoch;
    context->result = epoch == 0 ? SAO2_TRACE_INVALID : SAO2_TRACE_OK;
}

static void sao2_trace_context_dispose(sao2_trace_context *context) {
    if (context == NULL) return;
    free(context->slots);
    free(context->items);
    memset(context, 0, sizeof *context);
}

static uint64_t sao2_trace_hash(sao2_ref reference, uint64_t identity) {
    uint64_t value = ((uint64_t)reference.owner_ptr << 32) | (uint64_t)reference.member_ptr;
    value ^= identity + UINT64_C(0x9e3779b97f4a7c15) + (value << 6) + (value >> 2);
    value ^= value >> 30;
    value *= UINT64_C(0xbf58476d1ce4e5b9);
    value ^= value >> 27;
    value *= UINT64_C(0x94d049bb133111eb);
    return value ^ (value >> 31);
}

static bool sao2_trace_key_equal(const sao2_trace_item *item, sao2_ref reference,
    const sao2_layout_descriptor *layout) {
    return item->reference.owner_ptr == reference.owner_ptr
        && item->reference.member_ptr == reference.member_ptr
        && item->layout->identity == layout->identity;
}

static size_t sao2_trace_slot_for(const sao2_trace_context *context, sao2_ref reference,
    const sao2_layout_descriptor *layout, bool *found) {
    size_t mask = context->slot_capacity - 1;
    size_t slot = (size_t)sao2_trace_hash(reference, layout->identity) & mask;
    while (context->slots[slot] != 0) {
        size_t index = context->slots[slot] - 1;
        if (sao2_trace_key_equal(&context->items[index], reference, layout)) {
            *found = true;
            return slot;
        }
        slot = (slot + 1) & mask;
    }
    *found = false;
    return slot;
}

static bool sao2_trace_grow_items(sao2_trace_context *context) {
    size_t capacity = context->item_capacity == 0 ? 8 : context->item_capacity * 2;
    sao2_trace_item *items;
    if (capacity < context->item_capacity || capacity > SIZE_MAX / sizeof *items) return false;
    items = (sao2_trace_item *)sao2_trace_realloc(context->items, capacity * sizeof *items);
    if (items == NULL) return false;
    context->items = items;
    context->item_capacity = capacity;
    return true;
}

static bool sao2_trace_rebuild_slots(sao2_trace_context *context, size_t capacity) {
    size_t *slots;
    size_t index;
    if (capacity < 16 || (capacity & (capacity - 1)) != 0
        || capacity > SIZE_MAX / sizeof *slots) return false;
    slots = (size_t *)sao2_trace_calloc(capacity, sizeof *slots);
    if (slots == NULL) return false;
    free(context->slots);
    context->slots = slots;
    context->slot_capacity = capacity;
    context->slot_count = 0;
    for (index = 0; index < context->item_count; index++) {
        bool found;
        size_t slot = sao2_trace_slot_for(context, context->items[index].reference,
            context->items[index].layout, &found);
        if (found) { free(slots); context->slots = NULL; context->slot_capacity = 0; return false; }
        context->slots[slot] = index + 1;
        context->slot_count++;
    }
    return true;
}

static void sao2_trace_enqueue(sao2_trace_context *context, sao2_ref reference,
    const sao2_layout_descriptor *layout) {
    sao2_arena_kind kind;
    bool found;
    size_t slot;
    if (context == NULL || context->result != SAO2_TRACE_OK) return;
    if (reference.owner_ptr == 0 && reference.member_ptr == 0) return;
    if (!sao2_ref_kind(reference, &kind) || !sao2_layout_registered(layout)) {
        context->result = SAO2_TRACE_INVALID;
        return;
    }
    (void)kind;
    if (context->slot_capacity == 0) {
        if (!sao2_trace_rebuild_slots(context, 16)) {
            context->result = SAO2_TRACE_SCRATCH_EXHAUSTED;
            return;
        }
    }
    slot = sao2_trace_slot_for(context, reference, layout, &found);
    if (found) return;
    if (context->item_count == context->item_capacity && !sao2_trace_grow_items(context)) {
        context->result = SAO2_TRACE_SCRATCH_EXHAUSTED;
        return;
    }
    if (context->slot_count + 1 > context->slot_capacity / 2) {
        size_t capacity = context->slot_capacity * 2;
        if (capacity < context->slot_capacity || !sao2_trace_rebuild_slots(context, capacity)) {
            context->result = SAO2_TRACE_SCRATCH_EXHAUSTED;
            return;
        }
        slot = sao2_trace_slot_for(context, reference, layout, &found);
        if (found) return;
    }
    context->items[context->item_count].reference = reference;
    context->items[context->item_count].layout = layout;
    context->slots[slot] = context->item_count + 1;
    context->item_count++;
    context->slot_count++;
}

static bool sao2_trace_resolve(sao2_trace_context *context, const sao2_trace_item *item,
    sao2_arena_kind *kind_result, sao2_heap_header **header_result,
    const unsigned char **body_result) {
    sao2_arena_kind kind;
    sao2_arena *arena;
    sao2_heap_header *header = NULL;
    const sao2_layout_descriptor *root;
    uint64_t owner, member, end, owner_end;
    if (!sao2_layout_registered(item->layout) || item->layout->alignment == 0
        || !sao2_ref_kind(item->reference, &kind)) return false;
    arena = sao2_arena_for_kind(kind);
    owner = (uint64_t)(item->reference.owner_ptr & ~SAO2_REF_OWNER_TAG_MASK);
    member = (uint64_t)item->reference.member_ptr;
    if (arena->base == NULL || owner == 0 || member == 0 || owner > member
        || owner >= arena->capacity || member >= arena->capacity
        || member > UINT64_MAX - item->layout->size) return false;
    end = member + item->layout->size;
    if (end > arena->capacity
        || (uintptr_t)(arena->base + (size_t)member) % item->layout->alignment != 0) return false;
    if (kind == SAO2_ARENA_HEAP) {
        if (!sao2_heap_header_for(item->reference, &header)) return false;
        root = sao2_layout_find(header->layout_identity);
        if (root == NULL || root->alignment == 0 || root->size != header->body_size
            || (uintptr_t)(arena->base + (size_t)owner) % root->alignment != 0
            || header->body_size > UINT64_MAX - owner) return false;
        owner_end = owner + header->body_size;
        if (end > owner_end || (member == owner && item->layout != root)) return false;
    } else {
        if (owner % SAO2_REF_ALIGNMENT != 0 || end > arena->frontier) return false;
    }
    *kind_result = kind;
    *header_result = header;
    *body_result = arena->base + (size_t)member;
    (void)context;
    return true;
}

static sao2_trace_result sao2_trace_drain(sao2_trace_context *context) {
    if (context == NULL) return SAO2_TRACE_INVALID;
    if (context->epoch == 0 && context->result == SAO2_TRACE_OK)
        context->result = SAO2_TRACE_INVALID;
    while (context->result == SAO2_TRACE_OK && context->next_item < context->item_count) {
        sao2_trace_item item = context->items[context->next_item++];
        sao2_arena_kind kind;
        sao2_heap_header *header;
        const unsigned char *body;
        if (!sao2_trace_resolve(context, &item, &kind, &header, &body)) {
            context->result = SAO2_TRACE_INVALID;
            break;
        }
        if (kind == SAO2_ARENA_HEAP) header->mark_epoch = context->epoch;
        if (item.layout->trace_body != NULL) item.layout->trace_body(context, body);
    }
    return context->result;
}
"#;

const STRUCT_ALLOCATION_RUNTIME: &str = r#"

static inline bool sao2_ref_equal(sao2_ref left, sao2_ref right) {
    return left.owner_ptr == right.owner_ptr && left.member_ptr == right.member_ptr;
}

static unsigned char *sao2_resolve_body(sao2_ref reference, const sao2_layout_descriptor *layout) {
    sao2_arena_kind kind;
    sao2_arena *arena;
    sao2_heap_header *header;
    uint64_t owner, member, end, owner_end;
    if (!sao2_ref_kind(reference, &kind)) sao2_compiler_invariant();
    arena = sao2_arena_for_kind(kind);
    owner = (uint64_t)(reference.owner_ptr & ~SAO2_REF_OWNER_TAG_MASK);
    member = (uint64_t)reference.member_ptr;
    if (arena->base == NULL || owner == 0 || member == 0 || owner > member
        || owner >= arena->capacity || member >= arena->capacity
        || layout->alignment == 0 || member > UINT64_MAX - layout->size) sao2_compiler_invariant();
    end = member + layout->size;
    if (end > arena->capacity || (uintptr_t)(arena->base + (size_t)member) % layout->alignment != 0)
        sao2_compiler_invariant();
    if (kind == SAO2_ARENA_HEAP) {
        if (!sao2_heap_header_for(reference, &header) || header->lifetime != SAO2_HEAP_LIFETIME
            || header->body_size > UINT64_MAX - owner)
            sao2_compiler_invariant();
        owner_end = owner + header->body_size;
        if (end > owner_end) sao2_compiler_invariant();
    } else if (end > arena->frontier) {
        sao2_compiler_invariant();
    }
    return arena->base + (size_t)member;
}

static sao2_ref sao2_project_inline(sao2_ref parent, const sao2_layout_descriptor *parent_layout,
    const sao2_layout_field_descriptor *field, const sao2_layout_descriptor *child_layout) {
    sao2_ref child;
    uint64_t member, field_end;
    (void)sao2_resolve_body(parent, parent_layout);
    if (field->storage != SAO2_LAYOUT_FIELD_INLINE || field->inline_layout != child_layout->identity
        || field->size != child_layout->size || field->alignment != child_layout->alignment
        || field->offset > parent_layout->size || child_layout->size > parent_layout->size - field->offset
        || field->offset > UINT64_MAX - (uint64_t)parent.member_ptr) sao2_compiler_invariant();
    member = (uint64_t)parent.member_ptr + field->offset;
    field_end = member + child_layout->size;
    if (member == 0 || member > UINT32_MAX || field_end < member) sao2_compiler_invariant();
    if (!sao2_ref_interior(parent, member, &child)) sao2_compiler_invariant();
    (void)sao2_resolve_body(child, child_layout);
    return child;
}

static void sao2_allocate_struct(const sao2_layout_descriptor *layout, size_t site,
    sao2_ref *reference, unsigned char **body) {
    sao2_arena_result status = sao2_heap_allocate(layout->size, SAO2_REF_ALIGNMENT,
        layout->identity, reference);
    sao2_heap_header *header;
    uint32_t owner_offset;
    if (status == SAO2_ARENA_EXHAUSTED) {
        static const unsigned char reason[] = "heap arena exhausted";
        sao2_fail(site, SAO2_FAILURE_STRUCT_ALLOCATION, reason, sizeof reason - 1);
    }
    if (status == SAO2_ARENA_COMMIT_FAILED) {
        static const unsigned char reason[] = "unable to commit heap storage";
        sao2_fail(site, SAO2_FAILURE_STRUCT_ALLOCATION, reason, sizeof reason - 1);
    }
    if (status != SAO2_ARENA_OK) sao2_compiler_invariant();
    owner_offset = reference->owner_ptr & ~SAO2_REF_OWNER_TAG_MASK;
    if ((reference->owner_ptr & SAO2_REF_OWNER_TAG_MASK) != SAO2_REF_HEAP_TAG
        || owner_offset == 0 || reference->member_ptr != owner_offset
        || !sao2_heap_header_for(*reference, &header)
        || header->body_size != layout->size || header->layout_identity != layout->identity
        ) sao2_compiler_invariant();
    *body = sao2_resolve_body(*reference, layout);
}

static void sao2_allocate_scoped_struct(const sao2_layout_descriptor *layout, size_t site,
    sao2_ref *reference, unsigned char **body) {
    sao2_arena_result status = sao2_scoped_allocate(layout->size, reference);
    uint32_t owner_offset;
    if (status == SAO2_ARENA_EXHAUSTED) {
        static const unsigned char reason[] = "scoped arena exhausted";
        sao2_fail(site, SAO2_FAILURE_STRUCT_ALLOCATION, reason, sizeof reason - 1);
    }
    if (status == SAO2_ARENA_COMMIT_FAILED) {
        static const unsigned char reason[] = "unable to commit scoped storage";
        sao2_fail(site, SAO2_FAILURE_STRUCT_ALLOCATION, reason, sizeof reason - 1);
    }
    if (status != SAO2_ARENA_OK) sao2_compiler_invariant();
    owner_offset = reference->owner_ptr & ~SAO2_REF_OWNER_TAG_MASK;
    if ((reference->owner_ptr & SAO2_REF_OWNER_TAG_MASK) != SAO2_REF_SCOPED_TAG
        || owner_offset == 0 || owner_offset % SAO2_REF_ALIGNMENT != 0
        || reference->member_ptr != owner_offset) sao2_compiler_invariant();
    *body = sao2_resolve_body(*reference, layout);
}
"#;

const SCALAR_RUNTIME: &str = r#"
_Static_assert(FLT_RADIX == 2, "SAO2 requires binary floating point");
_Static_assert(DBL_MANT_DIG == 53, "SAO2 requires binary64 precision");
_Static_assert(DBL_MIN_EXP == -1021, "SAO2 requires binary64 minimum exponent");
_Static_assert(DBL_MAX_EXP == 1024, "SAO2 requires binary64 maximum exponent");
_Static_assert(sizeof(double) == 8, "SAO2 requires eight-byte double");

static double sao2_float_from_bits(uint64_t bits) {
    double value;
    memcpy(&value, &bits, sizeof value);
    return value;
}

static uint64_t sao2_int_to_bits(int64_t value) {
    return value >= 0 ? (uint64_t)value : UINT64_MAX - (uint64_t)(-(value + INT64_C(1)));
}

static int64_t sao2_int_from_bits(uint64_t bits) {
    return bits <= (uint64_t)INT64_MAX ? (int64_t)bits : -INT64_C(1) - (int64_t)(UINT64_MAX - bits);
}

static uint64_t sao2_int_magnitude(int64_t value) {
    return value >= 0 ? (uint64_t)value : UINT64_C(1) + (uint64_t)(-(value + INT64_C(1)));
}

static int sao2_string_compare(sao2_string left, sao2_string right) {
    size_t length = left->length < right->length ? left->length : right->length;
    int comparison = memcmp(left->bytes, right->bytes, length);
    if (comparison != 0) return comparison;
    if (left->length < right->length) return -1;
    if (left->length > right->length) return 1;
    return 0;
}

static int64_t sao2_shift_right(int64_t value, int64_t amount) {
    uint64_t bits = sao2_int_to_bits(value);
    if (amount == 0) return value;
    if (value < 0) bits = (bits >> (uint64_t)amount) | (UINT64_MAX << (64 - (uint64_t)amount));
    else bits >>= (uint64_t)amount;
    return sao2_int_from_bits(bits);
}

static void sao2_write_stderr(const unsigned char *bytes, size_t length) {
    if (fwrite(bytes, 1, length, stderr) != length) return;
}

static void sao2_write_stderr_char(int value) {
    if (fputc(value, stderr) == EOF) return;
}

static void sao2_write_stderr_size(size_t value) {
    if (fprintf(stderr, "%zu", value) < 0) return;
}

static _Noreturn void sao2_pre_entry_panic(
    const unsigned char *message,
    size_t message_length,
    bool append_argument,
    size_t argument
) {
    static const unsigned char prefix[] = "sao2: panic: ";
    static const unsigned char argument_suffix[] = " is not ASCII";
    sao2_write_stderr(prefix, sizeof prefix - 1);
    sao2_write_stderr(message, message_length);
    if (append_argument) {
        sao2_write_stderr_size(argument);
        sao2_write_stderr(argument_suffix, sizeof argument_suffix - 1);
    }
    sao2_write_stderr_char('\n');
    exit(EXIT_FAILURE);
}

static _Noreturn void sao2_pre_entry_panic_exit_status(void) {
    static const unsigned char message[] = "main returned an exit status outside the host int range";
    sao2_pre_entry_panic(message, sizeof message - 1, false, 0);
}

static _Noreturn void sao2_pre_entry_panic_arena(void) {
    static const unsigned char message[] = "unable to initialize arena runtime";
    sao2_pre_entry_panic(message, sizeof message - 1, false, 0);
}

static _Noreturn void sao2_pre_entry_panic_argument(size_t argument) {
    static const unsigned char message[] = "command-line argument ";
    sao2_pre_entry_panic(message, sizeof message - 1, true, argument);
}

static _Noreturn void sao2_compiler_invariant(void) {
    static const unsigned char message[] = "sao2: internal compiler error: reached unreachable IR\n";
    sao2_write_stderr(message, sizeof message - 1);
    abort();
}

static const sao2_failure_site *sao2_failure(size_t site) {
    if (site >= sao2_failure_count) sao2_compiler_invariant();
    if (sao2_failures[site].function >= sao2_function_count) sao2_compiler_invariant();
    return &sao2_failures[site];
}

static void sao2_require_operation(size_t site, uint32_t operation) {
    if (sao2_failure(site)->operation != operation) sao2_compiler_invariant();
}

static void sao2_write_panic_prefix(void) {
    static const unsigned char prefix[] = "sao2: panic: ";
    sao2_write_stderr(prefix, sizeof prefix - 1);
}

static _Noreturn void sao2_finish_panic(size_t site) {
    static const unsigned char at[] = " at ";
    static const unsigned char in[] = " in ";
    const sao2_failure_site *failure = sao2_failure(site);
    const sao2_bytes *function = &sao2_function_names[failure->function];
    sao2_write_stderr(at, sizeof at - 1);
    sao2_write_stderr(sao2_filename.bytes, sao2_filename.length);
    sao2_write_stderr_char(':');
    sao2_write_stderr_size(failure->line);
    sao2_write_stderr_char(':');
    sao2_write_stderr_size(failure->column);
    sao2_write_stderr(in, sizeof in - 1);
    sao2_write_stderr(function->bytes, function->length);
    sao2_write_stderr_char('\n');
    exit(EXIT_FAILURE);
}

static _Noreturn void sao2_fail(size_t site, uint32_t operation, const unsigned char *reason, size_t length) {
    sao2_require_operation(site, operation);
    sao2_write_panic_prefix();
    sao2_write_stderr(reason, length);
    sao2_finish_panic(site);
}

static uint8_t sao2_string_index(sao2_string value, int64_t index, size_t site) {
    static const unsigned char reason[] = "string index out of range";
    uint64_t magnitude;
    size_t position;
    sao2_require_operation(site, SAO2_FAILURE_STRING_INDEX);
    if (index >= 0) {
        magnitude = (uint64_t)index;
        if (magnitude >= (uint64_t)value->length)
            sao2_fail(site, SAO2_FAILURE_STRING_INDEX, reason, sizeof reason - 1);
        position = (size_t)magnitude;
    } else {
        magnitude = sao2_int_magnitude(index);
        if (magnitude > (uint64_t)value->length)
            sao2_fail(site, SAO2_FAILURE_STRING_INDEX, reason, sizeof reason - 1);
        position = value->length - (size_t)magnitude;
    }
    return value->bytes[position];
}

static void sao2_check_integer_add(int64_t left, int64_t right, size_t site) {
    static const unsigned char reason[] = "integer addition overflow";
    sao2_require_operation(site, SAO2_FAILURE_INTEGER_ADD);
    if ((right > 0 && left > INT64_MAX - right) || (right < 0 && left < INT64_MIN - right))
        sao2_fail(site, SAO2_FAILURE_INTEGER_ADD, reason, sizeof reason - 1);
}

static void sao2_check_integer_subtract(int64_t left, int64_t right, size_t site) {
    static const unsigned char reason[] = "integer subtraction overflow";
    sao2_require_operation(site, SAO2_FAILURE_INTEGER_SUBTRACT);
    if ((right < 0 && left > INT64_MAX + right) || (right > 0 && left < INT64_MIN + right))
        sao2_fail(site, SAO2_FAILURE_INTEGER_SUBTRACT, reason, sizeof reason - 1);
}

static void sao2_check_integer_multiply(int64_t left, int64_t right, size_t site) {
    static const unsigned char reason[] = "integer multiplication overflow";
    uint64_t left_magnitude;
    uint64_t right_magnitude;
    uint64_t limit;
    sao2_require_operation(site, SAO2_FAILURE_INTEGER_MULTIPLY);
    if (left == 0 || right == 0) return;
    left_magnitude = sao2_int_magnitude(left);
    right_magnitude = sao2_int_magnitude(right);
    limit = (left < 0) != (right < 0) ? UINT64_C(9223372036854775808) : (uint64_t)INT64_MAX;
    if (right_magnitude > limit / left_magnitude)
        sao2_fail(site, SAO2_FAILURE_INTEGER_MULTIPLY, reason, sizeof reason - 1);
}

static void sao2_check_integer_negation(int64_t operand, size_t site) {
    static const unsigned char reason[] = "integer negation overflow";
    sao2_require_operation(site, SAO2_FAILURE_INTEGER_NEGATION);
    if (operand == INT64_MIN)
        sao2_fail(site, SAO2_FAILURE_INTEGER_NEGATION, reason, sizeof reason - 1);
}

static void sao2_check_division_int(int64_t left, int64_t right, size_t site) {
    static const unsigned char zero[] = "integer division by zero";
    static const unsigned char overflow[] = "integer division overflow";
    sao2_require_operation(site, SAO2_FAILURE_INTEGER_DIVISION);
    if (right == 0) sao2_fail(site, SAO2_FAILURE_INTEGER_DIVISION, zero, sizeof zero - 1);
    if (left == INT64_MIN && right == -INT64_C(1))
        sao2_fail(site, SAO2_FAILURE_INTEGER_DIVISION, overflow, sizeof overflow - 1);
}

static void sao2_check_remainder(int64_t left, int64_t right, size_t site) {
    static const unsigned char zero[] = "integer remainder by zero";
    static const unsigned char overflow[] = "integer remainder overflow";
    sao2_require_operation(site, SAO2_FAILURE_INTEGER_REMAINDER);
    if (right == 0) sao2_fail(site, SAO2_FAILURE_INTEGER_REMAINDER, zero, sizeof zero - 1);
    if (left == INT64_MIN && right == -INT64_C(1))
        sao2_fail(site, SAO2_FAILURE_INTEGER_REMAINDER, overflow, sizeof overflow - 1);
}

static void sao2_check_shift_range(int64_t amount, size_t site) {
    static const unsigned char reason[] = "shift count out of range";
    uint32_t operation = sao2_failure(site)->operation;
    if (operation != SAO2_FAILURE_SHIFT_LEFT && operation != SAO2_FAILURE_SHIFT_RIGHT)
        sao2_compiler_invariant();
    if (amount < 0 || amount > 63) sao2_fail(site, operation, reason, sizeof reason - 1);
}

static void sao2_check_shift_left(int64_t left, int64_t amount, size_t site) {
    static const unsigned char reason[] = "integer left shift overflow";
    uint64_t limit;
    sao2_require_operation(site, SAO2_FAILURE_SHIFT_LEFT);
    if (amount < 0 || amount > 63) sao2_compiler_invariant();
    limit = left < 0 ? UINT64_C(9223372036854775808) : (uint64_t)INT64_MAX;
    if (sao2_int_magnitude(left) > (limit >> (uint64_t)amount))
        sao2_fail(site, SAO2_FAILURE_SHIFT_LEFT, reason, sizeof reason - 1);
}

static void sao2_check_division_float(double left, double right, size_t site) {
    static const unsigned char reason[] = "floating-point division by zero";
    (void)left;
    sao2_require_operation(site, SAO2_FAILURE_FLOAT_DIVISION);
    if (right == 0.0) sao2_fail(site, SAO2_FAILURE_FLOAT_DIVISION, reason, sizeof reason - 1);
}

static void sao2_check_finite_float(double operand, size_t site) {
    static const unsigned char add[] = "floating-point addition produced a non-finite result";
    static const unsigned char subtract[] = "floating-point subtraction produced a non-finite result";
    static const unsigned char multiply[] = "floating-point multiplication produced a non-finite result";
    static const unsigned char divide[] = "floating-point division produced a non-finite result";
    uint32_t operation = sao2_failure(site)->operation;
    const unsigned char *reason;
    size_t reason_length;
    switch (operation) {
        case SAO2_FAILURE_FLOAT_ADD: reason = add; reason_length = sizeof add - 1; break;
        case SAO2_FAILURE_FLOAT_SUBTRACT: reason = subtract; reason_length = sizeof subtract - 1; break;
        case SAO2_FAILURE_FLOAT_MULTIPLY: reason = multiply; reason_length = sizeof multiply - 1; break;
        case SAO2_FAILURE_FLOAT_DIVISION: reason = divide; reason_length = sizeof divide - 1; break;
        default: sao2_compiler_invariant();
    }
    if (!isfinite(operand)) sao2_fail(site, operation, reason, reason_length);
}

static void sao2_check_float_to_int(double operand, size_t site) {
    static const unsigned char reason[] = "float-to-int conversion out of range";
    sao2_require_operation(site, SAO2_FAILURE_FLOAT_TO_INT);
    if (!(operand >= -0x1p63 && operand < 0x1p63))
        sao2_fail(site, SAO2_FAILURE_FLOAT_TO_INT, reason, sizeof reason - 1);
}

static _Noreturn void sao2_panic(sao2_string message, size_t site) {
    sao2_require_operation(site, SAO2_FAILURE_EXPLICIT_PANIC);
    sao2_write_panic_prefix();
    sao2_writer writer = { stderr, 0, false, false };
    sao2_format_string(&writer, message);
    if (writer.failed) exit(EXIT_FAILURE);
    sao2_finish_panic(site);
}

static void sao2_error_panic_begin(size_t site) {
    static const unsigned char error[] = "Error(";
    sao2_require_operation(site, SAO2_FAILURE_UNHANDLED_ERROR);
    sao2_write_panic_prefix();
    sao2_write_stderr(error, sizeof error - 1);
}

static _Noreturn void sao2_error_panic_int(int64_t value, size_t site) {
    sao2_error_panic_begin(site);
    sao2_writer writer = { stderr, 0, false, false };
    sao2_format_int(&writer, value);
    if (writer.failed) exit(EXIT_FAILURE);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_error_panic_bool(bool value, size_t site) {
    sao2_error_panic_begin(site);
    sao2_writer writer = { stderr, 0, false, false };
    sao2_format_bool(&writer, value);
    if (writer.failed) exit(EXIT_FAILURE);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_error_panic_char(uint8_t value, size_t site) {
    sao2_error_panic_begin(site);
    sao2_writer writer = { stderr, 0, false, false };
    sao2_format_char(&writer, value);
    if (writer.failed) exit(EXIT_FAILURE);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_error_panic_float(double value, size_t site) {
    sao2_error_panic_begin(site);
    sao2_writer writer = { stderr, 0, false, false };
    sao2_format_float(&writer, value);
    if (writer.failed) exit(EXIT_FAILURE);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_error_panic_string(sao2_string value, size_t site) {
    sao2_error_panic_begin(site);
    sao2_writer writer = { stderr, 0, false, false };
    sao2_format_string(&writer, value);
    if (writer.failed) exit(EXIT_FAILURE);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_output_failure(size_t site) {
    static const unsigned char reason[] = "standard output failure";
    sao2_fail(site, SAO2_FAILURE_OUTPUT, reason, sizeof reason - 1);
}

static void sao2_prepare_output(size_t site) {
#ifdef _WIN32
    if (_setmode(_fileno(stdout), _O_BINARY) == -1) sao2_output_failure(site);
#else
    (void)site;
#endif
}
"#;

const FORMAT_RUNTIME: &str = r#"
static void sao2_writer_bytes(sao2_writer *writer, const unsigned char *bytes, size_t length) {
    if (writer->failed) return;
    if (fwrite(bytes, 1, length, writer->stream) != length) {
        if (writer->checked) sao2_output_failure(writer->site);
        writer->failed = true;
    }
}

static void sao2_writer_char(sao2_writer *writer, unsigned char value) {
    sao2_writer_bytes(writer, &value, 1);
}

static void sao2_format_unit(sao2_writer *writer, sao2_unit value) {
    static const unsigned char text[] = { UINT8_C(40), UINT8_C(41) };
    (void)value;
    sao2_writer_bytes(writer, text, sizeof text);
}

static void sao2_format_int(sao2_writer *writer, int64_t value) {
    char buffer[32];
    int length = snprintf(buffer, sizeof buffer, "%" PRId64, value);
    if (length < 0 || (size_t)length >= sizeof buffer) {
        if (writer->checked) sao2_output_failure(writer->site);
        writer->failed = true;
        return;
    }
    sao2_writer_bytes(writer, (const unsigned char *)buffer, (size_t)length);
}

static uint64_t sao2_float_to_bits(double value) {
    uint64_t bits;
    memcpy(&bits, &value, sizeof bits);
    return bits;
}

static void sao2_normalize_float(char *buffer) {
    char *exponent = strchr(buffer, 'e');
    if (exponent == NULL) exponent = strchr(buffer, 'E');
    if (exponent == NULL) return;
    *exponent++ = 'e';
    if (*exponent == '+') memmove(exponent, exponent + 1, strlen(exponent));
    else if (*exponent == '-') ++exponent;
    while (exponent[0] == '0' && exponent[1] != '\0') memmove(exponent, exponent + 1, strlen(exponent));
}

static void sao2_format_float(sao2_writer *writer, double value) {
    char buffer[64];
    if (value == 0.0) {
        if (signbit(value)) sao2_writer_bytes(writer, (const unsigned char *)"-0", 2);
        else sao2_writer_bytes(writer, (const unsigned char *)"0", 1);
        return;
    }
    for (int precision = 1; precision <= 17; ++precision) {
        int length = snprintf(buffer, sizeof buffer, "%.*g", precision, value);
        if (length < 0 || (size_t)length >= sizeof buffer) {
            if (writer->checked) sao2_output_failure(writer->site);
            writer->failed = true;
            return;
        }
        sao2_normalize_float(buffer);
        char *end;
        double parsed = strtod(buffer, &end);
        if (*end == '\0' && sao2_float_to_bits(parsed) == sao2_float_to_bits(value)) {
            sao2_writer_bytes(writer, (const unsigned char *)buffer, strlen(buffer));
            return;
        }
    }
    sao2_compiler_invariant();
}

static void sao2_format_string(sao2_writer *writer, sao2_string value) {
    sao2_writer_bytes(writer, value->bytes, value->length);
}

static void sao2_format_bool(sao2_writer *writer, bool value) {
    static const unsigned char yes[] = { UINT8_C(116), UINT8_C(114), UINT8_C(117), UINT8_C(101) };
    static const unsigned char no[] = { UINT8_C(102), UINT8_C(97), UINT8_C(108), UINT8_C(115), UINT8_C(101) };
    sao2_writer_bytes(writer, value ? yes : no, value ? sizeof yes : sizeof no);
}

static void sao2_format_char(sao2_writer *writer, uint8_t value) {
    sao2_writer_char(writer, value);
}
"#;

fn string_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(14695981039346656037_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}

fn collect_strings(program: &ir::Program) -> Vec<StringLiteral> {
    let mut strings = Vec::<StringLiteral>::new();
    let mut collect = |operand: &Operand| {
        if let Operand::Constant(ir::Constant { value: ConstantValue::String(bytes), .. }) = operand
            && !strings.iter().any(|item| item.bytes.as_slice() == bytes)
        {
            strings.push(StringLiteral { bytes: bytes.clone(), hash: string_hash(bytes) });
        }
    };
    for function in &program.functions {
        for block in &function.blocks {
            for operation in &block.operations {
                visit_operation_operands(&operation.kind, &mut collect);
            }
            visit_terminator_operands(
                &block.terminator.as_ref().expect("validated block").kind,
                &mut collect,
            );
        }
    }
    strings
}

fn visit_operation_operands(operation: &OperationKind, visitor: &mut impl FnMut(&Operand)) {
    match operation {
        OperationKind::Copy { operand, .. } | OperationKind::Unary { operand, .. }
        | OperationKind::Convert { operand, .. } => visitor(operand),
        OperationKind::Binary { left, right, .. } => { visitor(left); visitor(right); }
        OperationKind::Aggregate { aggregate, .. } => match aggregate {
            ir::Aggregate::Struct { fields, .. } => for (_, operand) in fields { visitor(operand); },
            ir::Aggregate::Tuple { elements, .. } | ir::Aggregate::List { elements, .. } =>
                for operand in elements { visitor(operand); },
            ir::Aggregate::Map { entries, .. } => for (key, value) in entries { visitor(key); visitor(value); },
        },
        OperationKind::UnionInject { payload, .. } => visitor(payload),
        OperationKind::UnionTest { union, .. } | OperationKind::UnionPayload { union, .. } => visitor(union),
        OperationKind::StringIndex { string, index, .. } => { visitor(string); visitor(index); }
        OperationKind::Assign { value, .. } => visitor(value),
        OperationKind::Call { arguments, .. } | OperationKind::Intrinsic { arguments, .. } =>
            for operand in arguments { visitor(operand); },
        OperationKind::Builtin { receiver, arguments, .. } => {
            visitor(receiver);
            for operand in arguments { visitor(operand); }
        }
        OperationKind::BeginIteration { iterable } | OperationKind::EndIteration { iterable } => visitor(iterable),
        OperationKind::IterationValue { iterable, index, .. } => { visitor(iterable); visitor(index); }
        OperationKind::Check(check) => match check {
            RuntimeCheck::IntegerOverflow { left, right, .. } | RuntimeCheck::Division { left, right, .. }
            | RuntimeCheck::Remainder { left, right, .. } => { visitor(left); visitor(right); }
            RuntimeCheck::IntegerNegation { operand, .. } | RuntimeCheck::FiniteFloat { operand, .. }
            | RuntimeCheck::NumericConversion { operand, .. } => visitor(operand),
            RuntimeCheck::ShiftRange { amount, .. } => visitor(amount),
            RuntimeCheck::IterationUnlocked { receiver, .. } => visitor(receiver),
        },
    }
}

fn visit_terminator_operands(terminator: &TerminatorKind, visitor: &mut impl FnMut(&Operand)) {
    match terminator {
        TerminatorKind::Jump(_) | TerminatorKind::Unreachable => {}
        TerminatorKind::Branch { condition, .. } => visitor(condition),
        TerminatorKind::Switch { union, .. } => visitor(union),
        TerminatorKind::Return(value) => visitor(value),
        TerminatorKind::Panic { message, .. } => visitor(message),
        TerminatorKind::ErrorPanic { payload, .. } => visitor(payload),
    }
}

fn aggregate_name(aggregate: AggregateId) -> String {
    match aggregate {
        AggregateId::Definition(id) => format!("sao2_def_{}", id.index()),
        AggregateId::AnonymousUnion(id) => format!("sao2_union_ty_{}", id.index()),
    }
}

fn trace_aggregate_suffix(aggregate: AggregateId) -> String {
    match aggregate {
        AggregateId::Definition(id) => format!("def_{}", id.index()),
        AggregateId::AnonymousUnion(id) => format!("ty_{}", id.index()),
    }
}

fn visit_operand_places(operand: &Operand, visitor: &mut impl FnMut(&Place)) {
    if let Operand::Copy(place) = operand { visitor(place); }
}

fn visit_operands<'a>(operands: impl IntoIterator<Item = &'a Operand>, visitor: &mut impl FnMut(&Place)) {
    for operand in operands { visit_operand_places(operand, visitor); }
}

fn visit_operation_places(operation: &OperationKind, visitor: &mut impl FnMut(&Place)) {
    match operation {
        OperationKind::Copy { operand, .. } | OperationKind::Unary { operand, .. }
        | OperationKind::Convert { operand, .. } => visit_operand_places(operand, visitor),
        OperationKind::Binary { left, right, .. } => visit_operands([left, right], visitor),
        OperationKind::Aggregate { aggregate, .. } => match aggregate {
            ir::Aggregate::Struct { fields, .. } => visit_operands(fields.iter().map(|(_, value)| value), visitor),
            ir::Aggregate::Tuple { elements, .. } | ir::Aggregate::List { elements, .. } => visit_operands(elements, visitor),
            ir::Aggregate::Map { entries, .. } => for (key, value) in entries { visit_operands([key, value], visitor); },
        },
        OperationKind::UnionInject { payload, .. } => visit_operand_places(payload, visitor),
        OperationKind::UnionTest { union, .. } | OperationKind::UnionPayload { union, .. } => visit_operand_places(union, visitor),
        OperationKind::StringIndex { string, index, .. } => visit_operands([string, index], visitor),
        OperationKind::Assign { destination, value } => { visitor(destination); visit_operand_places(value, visitor); }
        OperationKind::Call { arguments, .. } | OperationKind::Intrinsic { arguments, .. } => visit_operands(arguments, visitor),
        OperationKind::Builtin { receiver, arguments, .. } => { visit_operand_places(receiver, visitor); visit_operands(arguments, visitor); }
        OperationKind::BeginIteration { iterable } | OperationKind::EndIteration { iterable } => visit_operand_places(iterable, visitor),
        OperationKind::IterationValue { iterable, index, .. } => visit_operands([iterable, index], visitor),
        OperationKind::Check(check) => match check {
            RuntimeCheck::IntegerOverflow { left, right, .. } | RuntimeCheck::Division { left, right, .. }
            | RuntimeCheck::Remainder { left, right, .. } => visit_operands([left, right], visitor),
            RuntimeCheck::IntegerNegation { operand, .. } | RuntimeCheck::FiniteFloat { operand, .. }
            | RuntimeCheck::NumericConversion { operand, .. } => visit_operand_places(operand, visitor),
            RuntimeCheck::ShiftRange { amount, .. } => visit_operand_places(amount, visitor),
            RuntimeCheck::IterationUnlocked { receiver, .. } => visit_operand_places(receiver, visitor),
        },
    }
}

fn visit_terminator_places(terminator: &TerminatorKind, visitor: &mut impl FnMut(&Place)) {
    match terminator {
        TerminatorKind::Jump(_) | TerminatorKind::Unreachable => {}
        TerminatorKind::Branch { condition, .. } => visit_operand_places(condition, visitor),
        TerminatorKind::Switch { union, .. } => visit_operand_places(union, visitor),
        TerminatorKind::Return(value) => visit_operand_places(value, visitor),
        TerminatorKind::Panic { message, .. } => visit_operand_places(message, visitor),
        TerminatorKind::ErrorPanic { payload, .. } => visit_operand_places(payload, visitor),
    }
}

fn place_has_unsupported_read_projection(place: &Place) -> bool {
    place.projections.iter().any(|projection| matches!(projection,
        Projection::ListIndex { .. } | Projection::MapIndex { .. }))
}

fn operation_operands_have_unsupported_projection(operation: &OperationKind) -> bool {
    let mut found = false;
    visit_operation_operands(operation, &mut |operand| {
        if let Operand::Copy(place) = operand {
            found |= place_has_unsupported_read_projection(place);
        }
    });
    found
}

fn terminator_operands_have_unsupported_projection(terminator: &TerminatorKind) -> bool {
    let mut found = false;
    visit_terminator_operands(terminator, &mut |operand| {
        if let Operand::Copy(place) = operand {
            found |= place_has_unsupported_read_projection(place);
        }
    });
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        Aggregate, ByteSpan, Constant, ConstantValue, FailureOperation, FailureSite,
        LocalOrigin, NominalDefinition, Operation, UnionAlternative,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct NativeProbeDirectory(PathBuf);

    impl NativeProbeDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let path = std::env::temp_dir().join(format!(
                "sao2 heap probe {} {nonce}", std::process::id(),
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for NativeProbeDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[derive(Clone, Copy)]
    struct CoreTypes {
        unit: TypeId,
        int: TypeId,
        float: TypeId,
        string: TypeId,
        boolean: TypeId,
        character: TypeId,
    }

    fn program() -> (ir::Program, CoreTypes, ir::LocationId) {
        let mut program = ir::Program::new("backend-test.sao2", 1);
        let types = CoreTypes {
            unit: program.intern_type(Type::Unit),
            int: program.intern_type(Type::Primitive(PrimitiveType::Int)),
            float: program.intern_type(Type::Primitive(PrimitiveType::Float)),
            string: program.intern_type(Type::Primitive(PrimitiveType::Str)),
            boolean: program.intern_type(Type::Primitive(PrimitiveType::Bool)),
            character: program.intern_type(Type::Primitive(PrimitiveType::Char)),
        };
        let location = program.intern_location(ByteSpan::new(0, 1));
        (program, types, location)
    }

    #[test]
    fn hashes_string_literals_with_fixed_fnv1a_vectors() {
        assert_eq!(string_hash(b""), 14695981039346656037);
        assert_eq!(string_hash(b"a"), 12638187200555641996);
        assert_eq!(string_hash(&[b'a', 0, b'z']), 16560493500796669818);
        assert_eq!(string_hash(&[0, 127]), 590614798587856096);
    }

    #[test]
    fn renders_stage_one_packed_reference_and_arena_runtime_before_functions() {
        let (mut program, types, location) = program();
        let mut main = Function::new("main", types.unit);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.unit, ConstantValue::Unit)), location,
        );
        let main = program.add_function(main);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        let reference = emitted.find("typedef struct {\n    uint32_t owner_ptr;\n    uint32_t member_ptr;\n} sao2_ref;").unwrap();
        let aggregate = emitted.find("typedef struct sao2_interned_string {").unwrap();
        let runtime = emitted.find("/*\n * Milestone 10 block-heap foundation.").unwrap();
        let prototype = emitted.find("sao2_unit sao2_fn_0(void);").unwrap();
        assert!(reference < aggregate && aggregate < runtime && runtime < prototype);
        assert!(emitted.contains("_Static_assert(sizeof(sao2_ref) == 8"));
        assert!(emitted.contains("#define SAO2_REF_OWNER_TAG_MASK UINT32_C(7)"));
        assert!(emitted.contains("#define SAO2_REF_HEAP_TAG UINT32_C(0)"));
        assert!(emitted.contains("#define SAO2_REF_SCOPED_TAG UINT32_C(1)"));
        assert!(emitted.contains("result->member_ptr = (uint32_t)member_offset;"));
        assert!(!emitted.contains("reference.member_ptr & ~SAO2_REF_OWNER_TAG_MASK"));
        assert!(emitted.contains("#ifdef _WIN32\n    SYSTEM_INFO information;"));
        assert!(emitted.contains("#else\n    long value = sysconf(_SC_PAGESIZE);"));
        assert!(emitted.contains("if (!sao2_arena_runtime_init())"));
        assert!(emitted.contains("sao2_arena_runtime_release();\n    return EXIT_SUCCESS;"));
        assert!(emitted.contains("static sao2_arena_result sao2_heap_allocate("));
        assert!(emitted.contains("uint64_t span_size;\n    uint64_t body_size;\n    uint64_t layout_identity;\n    uint64_t next_free;"));
        assert!(emitted.contains("uint32_t lifetime;\n    uint32_t block_state;\n    uint32_t mark_epoch;\n    uint32_t reserved;"));
        assert!(emitted.contains("#define SAO2_HEAP_BLOCK_ALLOCATED UINT32_C(1)"));
        assert!(emitted.contains("#define SAO2_HEAP_BLOCK_FREE UINT32_C(2)"));
        assert!(emitted.contains("_Static_assert(sizeof(sao2_heap_header) != 0"));
        assert!(emitted.contains("_Static_assert(sizeof(sao2_heap_header) == 48"));
        assert!(emitted.contains("static bool sao2_heap_validate(void)"));
        assert!(emitted.contains("static bool sao2_heap_reclaim(uint64_t owner_offset)"));
        assert!(emitted.contains("selected = sao2_heap_arena.free_head;"));
        assert!(emitted.contains("if (remainder_span >= minimum_span)"));
        assert!(emitted.contains("*result = published;"));
        assert!(emitted.contains("sao2_scoped_bump_allocate(&sao2_scoped_arena"));
        assert!(!emitted.contains("sao2_heap_arena.cursor"));
        assert!(!emitted.contains("sao2_collect"));
        assert!(!emitted.contains("shadow_frame"));
        assert!(!emitted.contains("trace_callback"));
    }

    #[test]
    fn native_block_heap_probe_uses_the_production_runtime() {
        let directory = NativeProbeDirectory::new();
        let source_path = directory.0.join("block heap probe.c");
        let mut source = String::from(concat!(
            "#ifndef _WIN32\n",
            "#ifndef _POSIX_C_SOURCE\n#define _POSIX_C_SOURCE 200809L\n#endif\n",
            "#ifndef _DEFAULT_SOURCE\n#define _DEFAULT_SOURCE\n#endif\n",
            "#endif\n",
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n",
            "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n",
            "#ifdef _WIN32\n#include <windows.h>\n#else\n#include <sys/mman.h>\n#include <unistd.h>\n#endif\n",
            "typedef struct { uint32_t owner_ptr; uint32_t member_ptr; } sao2_ref;\n",
            "#define SAO2_ARENA_CAPACITY UINT64_C(65536)\n",
            "#define SAO2_RUNTIME_PROBE\n",
            "static bool sao2_probe_fail_next_commit;\n",
        ));
        source.push_str(ARENA_RUNTIME);
        source.push_str(r#"
#define CHECK(condition) do { if (!(condition)) { fprintf(stderr, "probe check failed at line %d\n", __LINE__); return 1; } } while (0)
#define OWNER(reference) ((uint64_t)((reference).owner_ptr & ~SAO2_REF_OWNER_TAG_MASK))

static bool probe_reset(void) {
    if (sao2_arenas_initialized) sao2_arena_runtime_release();
    return sao2_arena_runtime_init();
}

int main(void) {
    sao2_ref first, second, third, fourth, replacement, interior, unchanged;
    sao2_heap_header *header;
    uint64_t minimum_span, padded_span, committed, first_header;
    size_t index;

    CHECK(sao2_heap_span(0, &minimum_span));
    CHECK(minimum_span == (uint64_t)sizeof(sao2_heap_header) + SAO2_REF_ALIGNMENT);
    CHECK(sao2_heap_span(9, &padded_span));
    CHECK(padded_span == (uint64_t)sizeof(sao2_heap_header) + UINT64_C(16));
    CHECK(!sao2_heap_span(UINT64_MAX, &padded_span));

    CHECK(probe_reset());
    CHECK(sao2_heap_arena.frontier == SAO2_ARENA_INITIAL_CURSOR);
    CHECK(sao2_heap_arena.free_head == 0 && sao2_heap_validate());
    CHECK(sao2_heap_allocate(0, SAO2_REF_ALIGNMENT, UINT64_C(0), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(0, SAO2_REF_ALIGNMENT, UINT64_C(91), &second) == SAO2_ARENA_OK);
    CHECK(OWNER(first) != OWNER(second) && OWNER(first) % SAO2_REF_ALIGNMENT == 0);
    CHECK(sao2_heap_header_for(first, &header));
    CHECK(header->body_size == 0 && header->layout_identity == 0);
    CHECK(header->block_state == SAO2_HEAP_BLOCK_ALLOCATED && header->span_size == minimum_span);
    interior = first;
    interior.member_ptr += UINT32_C(1);
    CHECK(sao2_heap_header_for(interior, &header) && header->layout_identity == 0);

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(16, SAO2_REF_ALIGNMENT, UINT64_C(1), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(2), &second) == SAO2_ARENA_OK);
    memset(sao2_heap_arena.base + (size_t)OWNER(first), 0xa5, 16);
    committed = sao2_heap_arena.committed;
    CHECK(sao2_heap_reclaim(OWNER(first)));
    CHECK(!sao2_heap_header_for(first, &header));
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(3), &replacement) == SAO2_ARENA_OK);
    CHECK(OWNER(replacement) == OWNER(first) && sao2_heap_arena.committed == committed);
    CHECK(sao2_heap_header_for(replacement, &header));
    CHECK(header->span_size == (uint64_t)sizeof(sao2_heap_header) + UINT64_C(16));
    for (index = 0; index < 16; index++)
        CHECK(sao2_heap_arena.base[(size_t)OWNER(replacement) + index] == 0);
    CHECK(OWNER(second) != OWNER(replacement));

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(10), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(11), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(80, SAO2_REF_ALIGNMENT, UINT64_C(12), &third) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(13), &fourth) == SAO2_ARENA_OK);
    CHECK(sao2_heap_reclaim(OWNER(first)) && sao2_heap_reclaim(OWNER(third)));
    CHECK(sao2_heap_allocate(16, SAO2_REF_ALIGNMENT, UINT64_C(14), &replacement) == SAO2_ARENA_OK);
    CHECK(OWNER(replacement) == OWNER(third));
    CHECK(OWNER(second) != OWNER(replacement) && OWNER(fourth) != OWNER(replacement));

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(64, SAO2_REF_ALIGNMENT, UINT64_C(20), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(21), &second) == SAO2_ARENA_OK);
    first_header = OWNER(first) - (uint64_t)sizeof(sao2_heap_header);
    CHECK(sao2_heap_reclaim(OWNER(first)));
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(22), &replacement) == SAO2_ARENA_OK);
    CHECK(OWNER(replacement) == OWNER(first));
    CHECK(sao2_heap_arena.free_head == first_header + minimum_span);
    CHECK(sao2_heap_header_at(sao2_heap_arena.free_head)->span_size == minimum_span);

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(56, SAO2_REF_ALIGNMENT, UINT64_C(30), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(31), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_reclaim(OWNER(first)));
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(32), &replacement) == SAO2_ARENA_OK);
    CHECK(sao2_heap_arena.free_head == 0);
    CHECK(sao2_heap_header_for(replacement, &header));
    CHECK(header->span_size == (uint64_t)sizeof(sao2_heap_header) + UINT64_C(56));

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(40), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(41), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(42), &third) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(43), &fourth) == SAO2_ARENA_OK);
    first_header = OWNER(first) - (uint64_t)sizeof(sao2_heap_header);
    CHECK(sao2_heap_reclaim(OWNER(second)));
    CHECK(sao2_heap_reclaim(OWNER(third)));
    CHECK(sao2_heap_header_at(OWNER(second) - sizeof(sao2_heap_header))->span_size == minimum_span * 2);
    CHECK(sao2_heap_reclaim(OWNER(first)));
    CHECK(sao2_heap_arena.free_head == first_header);
    CHECK(sao2_heap_header_at(first_header)->span_size == minimum_span * 3);
    committed = sao2_heap_arena.committed;
    CHECK(sao2_heap_reclaim(OWNER(fourth)));
    CHECK(sao2_heap_arena.frontier == SAO2_ARENA_INITIAL_CURSOR);
    CHECK(sao2_heap_arena.free_head == 0 && sao2_heap_arena.committed == committed);

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(44), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(45), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(46), &third) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(47), &fourth) == SAO2_ARENA_OK);
    first_header = OWNER(first) - (uint64_t)sizeof(sao2_heap_header);
    CHECK(sao2_heap_reclaim(OWNER(first)) && sao2_heap_reclaim(OWNER(third)));
    CHECK(sao2_heap_reclaim(OWNER(second)));
    CHECK(sao2_heap_arena.free_head == first_header);
    CHECK(sao2_heap_header_at(first_header)->span_size == minimum_span * 3);
    CHECK(sao2_heap_validate() && sao2_heap_header_for(fourth, &header));

    CHECK(probe_reset());
    unchanged.owner_ptr = UINT32_C(0x11223344);
    unchanged.member_ptr = UINT32_C(0x55667788);
    {
        sao2_arena before = sao2_heap_arena;
        sao2_probe_fail_next_commit = true;
        CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(50), &unchanged)
            == SAO2_ARENA_COMMIT_FAILED);
        CHECK(memcmp(&before, &sao2_heap_arena, sizeof before) == 0);
        CHECK(unchanged.owner_ptr == UINT32_C(0x11223344)
            && unchanged.member_ptr == UINT32_C(0x55667788));
    }

    CHECK(probe_reset());
    {
        sao2_scoped_mark mark = sao2_scoped_mark_current();
        CHECK(sao2_scoped_allocate(13, &first) == SAO2_ARENA_OK);
        CHECK((first.owner_ptr & SAO2_REF_OWNER_TAG_MASK) == SAO2_REF_SCOPED_TAG);
        memset(sao2_scoped_arena.base + (size_t)OWNER(first), 0x7f, 13);
        CHECK(sao2_scoped_restore(mark));
        CHECK(sao2_scoped_allocate(13, &second) == SAO2_ARENA_OK);
        CHECK(OWNER(first) == OWNER(second));
        for (index = 0; index < 13; index++)
            CHECK(sao2_scoped_arena.base[(size_t)OWNER(second) + index] == 0);
    }

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(60), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(61), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_header_for(first, &header));
    header->reserved = UINT32_C(1);
    CHECK(!sao2_heap_validate());
    header->reserved = 0;
    header->lifetime = UINT32_C(1);
    CHECK(!sao2_heap_validate());
    header->lifetime = SAO2_HEAP_LIFETIME;
    header->span_size = 0;
    CHECK(!sao2_heap_validate());
    header->span_size = minimum_span - SAO2_REF_ALIGNMENT;
    CHECK(!sao2_heap_validate());
    header->span_size = minimum_span + UINT64_C(1);
    CHECK(!sao2_heap_validate());
    header->span_size = UINT64_MAX;
    CHECK(!sao2_heap_validate());
    header->span_size = minimum_span;
    header->block_state = UINT32_C(99);
    CHECK(!sao2_heap_validate());
    header->block_state = SAO2_HEAP_BLOCK_ALLOCATED;
    CHECK(sao2_heap_validate());
    header->mark_epoch = UINT32_C(9);
    CHECK(sao2_heap_validate());
    header->mark_epoch = 0;
    sao2_heap_arena.frontier += SAO2_REF_ALIGNMENT;
    CHECK(!sao2_heap_validate());
    sao2_heap_arena.frontier -= SAO2_REF_ALIGNMENT;
    CHECK(sao2_heap_reclaim(OWNER(first)));
    header = sao2_heap_header_at(SAO2_ARENA_INITIAL_CURSOR);
    sao2_heap_arena.free_head = 0;
    CHECK(!sao2_heap_validate());
    sao2_heap_arena.free_head = SAO2_ARENA_INITIAL_CURSOR;
    header->next_free = SAO2_ARENA_INITIAL_CURSOR;
    CHECK(!sao2_heap_validate());

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(62), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(63), &second) == SAO2_ARENA_OK);
    sao2_heap_arena.free_head = SAO2_ARENA_INITIAL_CURSOR;
    CHECK(!sao2_heap_validate());

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(64), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(65), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(66), &third) == SAO2_ARENA_OK);
    CHECK(sao2_heap_reclaim(OWNER(first)));
    header = sao2_heap_header_at(OWNER(second) - (uint64_t)sizeof(sao2_heap_header));
    sao2_heap_initialize_free(header, minimum_span, 0);
    sao2_heap_header_at(SAO2_ARENA_INITIAL_CURSOR)->next_free =
        OWNER(second) - (uint64_t)sizeof(sao2_heap_header);
    CHECK(!sao2_heap_validate());

    CHECK(probe_reset());
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(67), &first) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(68), &second) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(69), &third) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(8, SAO2_REF_ALIGNMENT, UINT64_C(70), &fourth) == SAO2_ARENA_OK);
    CHECK(sao2_heap_reclaim(OWNER(first)) && sao2_heap_reclaim(OWNER(third)));
    sao2_heap_header_at(OWNER(third) - (uint64_t)sizeof(sao2_heap_header))->next_free =
        OWNER(first) - (uint64_t)sizeof(sao2_heap_header);
    CHECK(!sao2_heap_validate());

    CHECK(probe_reset());
    unchanged.owner_ptr = UINT32_C(0x01020304);
    unchanged.member_ptr = UINT32_C(0x05060708);
    while (sao2_heap_allocate(512, SAO2_REF_ALIGNMENT, UINT64_C(70), &first) == SAO2_ARENA_OK) {}
    CHECK(sao2_heap_allocate(512, SAO2_REF_ALIGNMENT, UINT64_C(70), &unchanged) == SAO2_ARENA_EXHAUSTED);
    CHECK(unchanged.owner_ptr == UINT32_C(0x01020304)
        && unchanged.member_ptr == UINT32_C(0x05060708));
    CHECK(sao2_heap_validate());

    sao2_arena_runtime_release();
    CHECK(!sao2_arenas_initialized && sao2_heap_arena.base == NULL
        && sao2_heap_arena.frontier == 0 && sao2_heap_arena.free_head == 0);
    return 0;
}
"#);
        fs::write(&source_path, source).unwrap();

        let executable = match crate::host_compiler::compile(&source_path) {
            Ok(executable) => executable,
            Err(error) if error.to_string().contains("no supported C compiler found") => return,
            Err(error) => panic!("native block-heap probe did not compile: {error}"),
        };
        let output = Command::new(executable).output().unwrap();
        assert!(
            output.status.success(),
            "native block-heap probe failed with {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn emits_unused_struct_bodies_and_deterministic_descriptors() {
        let (mut program, types, location) = program();
        let inner = program.add_definition(NominalDefinition::structure("names must not leak"));
        let inner_type = program.intern_type(Type::Nominal(inner));
        program.definitions[inner.index()].add_struct_field("value", types.int, MemberStorage::Inline);

        let outer = program.add_definition(NominalDefinition::structure("Outer"));
        program.definitions[outer.index()].add_struct_field("flag", types.boolean, MemberStorage::Inline);
        program.definitions[outer.index()].add_struct_field("child", inner_type, MemberStorage::Inline);
        program.definitions[outer.index()].add_struct_field("shared", inner_type, MemberStorage::Referenced);
        let _outer_type = program.intern_type(Type::Nominal(outer));
        add_main(&mut program, types, location);

        let layouts = LayoutPlanner::new(&program).plan().unwrap();
        let outer_layout = layouts.structs.iter().find(|layout| layout.definition == outer).unwrap();
        assert_eq!(outer_layout.identity, layout_identity(outer).unwrap());
        assert_eq!(outer_layout.fields.iter().map(|field| field.ty).collect::<Vec<_>>(),
            vec![types.boolean, inner_type, inner_type]);
        assert_eq!(outer_layout.fields.iter().map(|field| field.storage).collect::<Vec<_>>(),
            vec![MemberStorage::Inline, MemberStorage::Inline, MemberStorage::Referenced]);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("typedef struct sao2_body_def_0 sao2_body_def_0;"));
        assert!(emitted.contains("struct sao2_body_def_0 {\n    int64_t field_0;\n};"));
        assert!(emitted.contains("struct sao2_body_def_1 {\n    bool field_0;\n    sao2_body_def_0 field_1;\n    sao2_ref field_2;\n};"));
        assert!(emitted.contains("offsetof(sao2_body_def_1, field_1) == UINT64_C(8)"));
        assert!(emitted.contains("offsetof(sao2_body_def_1, field_2) == UINT64_C(16)"));
        assert!(emitted.contains("sao2_layout_def_0 = { UINT64_C(0), UINT64_C(8), UINT64_C(8), 1"));
        assert!(emitted.contains("sao2_layout_def_1 = { UINT64_C(1), UINT64_C(24), UINT64_C(8), 3"));
        assert!(emitted.contains("static void sao2_allocate_struct(const sao2_layout_descriptor *layout, size_t site,"));
        assert!(emitted.contains("owner_end = owner + header->body_size;\n        if (end > owner_end)"));
        assert!(!emitted.contains("names must not leak"));
    }

    #[test]
    fn plans_exact_struct_tuple_and_union_trace_callbacks() {
        let (mut program, types, location) = program();
        let node = program.add_definition(NominalDefinition::structure("Node"));
        let node_type = program.intern_type(Type::Nominal(node));
        program.definitions[node.index()].add_struct_field("next", node_type, MemberStorage::Referenced);

        let tuple = program.add_definition(NominalDefinition::tuple("Pair"));
        program.definitions[tuple.index()].add_tuple_field(types.int);
        program.definitions[tuple.index()].add_tuple_field(node_type);
        let tuple_type = program.intern_type(Type::Nominal(tuple));

        let named_union = program.add_definition(NominalDefinition::union("Choice"));
        program.definitions[named_union.index()].add_alternative(UnionAlternative::untagged(types.int));
        program.definitions[named_union.index()].add_alternative(UnionAlternative::untagged(node_type));
        let named_union_type = program.intern_type(Type::Nominal(named_union));
        let anonymous_union = program.intern_type(Type::Union(vec![
            UnionAlternative::untagged(types.boolean),
            UnionAlternative::untagged(node_type),
        ]));

        let holder = program.add_definition(NominalDefinition::structure("Holder"));
        program.definitions[holder.index()].add_struct_field("tuple", tuple_type, MemberStorage::Inline);
        program.definitions[holder.index()].add_struct_field("named", named_union_type, MemberStorage::Inline);
        program.definitions[holder.index()].add_struct_field("anonymous", anonymous_union, MemberStorage::Inline);
        let _holder_type = program.intern_type(Type::Nominal(holder));
        add_main(&mut program, types, location);

        let layouts = LayoutPlanner::new(&program).plan().unwrap();
        let traces = TracePlanner::new(&program, &layouts).plan().unwrap();
        assert!(traces.type_contains_reference[node_type.index()]);
        assert!(traces.type_contains_reference[tuple_type.index()]);
        assert!(traces.type_contains_reference[named_union_type.index()]);
        assert!(traces.type_contains_reference[anonymous_union.index()]);
        assert_eq!(traces.struct_callbacks, vec![node, holder]);
        assert!(traces.aggregate_callbacks.contains(&AggregateId::Definition(tuple)));
        assert!(traces.aggregate_callbacks.contains(&AggregateId::Definition(named_union)));
        assert!(traces.aggregate_callbacks.contains(&AggregateId::AnonymousUnion(anonymous_union)));

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("static void sao2_trace_body_def_0(sao2_trace_context *context"));
        assert!(emitted.contains("sao2_layout_fields_def_0, sao2_trace_body_def_0"));
        assert!(emitted.contains("sao2_trace_value_def_1(context"));
        assert!(emitted.contains("sao2_trace_value_def_2(context"));
        assert!(emitted.contains(&format!("sao2_trace_value_ty_{}(context", anonymous_union.index())));
        assert!(emitted.contains("case 0: break;"));
        assert!(emitted.contains("default: context->result = SAO2_TRACE_INVALID;"));
        assert!(emitted.contains("static const sao2_layout_descriptor *const sao2_layout_registry[]"));
        assert!(emitted.contains("sao2_trace_item item = context->items[context->next_item++]"));
        assert!(!emitted.contains("shadow_frame"));
        assert!(!emitted.contains("sao2_collect"));
    }

    #[test]
    fn rejects_reference_bearing_container_trace_shapes() {
        let (mut program, _types, _location) = program();
        let node = program.add_definition(NominalDefinition::structure("Node"));
        let node_type = program.intern_type(Type::Nominal(node));
        let _nodes = program.intern_type(Type::List(node_type));
        let layouts = LayoutPlanner::new(&program).plan().unwrap();
        let error = TracePlanner::new(&program, &layouts).plan().unwrap_err();
        assert!(error.to_string().contains("reference-bearing list traversal"));
    }

    #[test]
    fn native_exact_trace_probe_uses_generated_plans_and_runtime() {
        let directory = NativeProbeDirectory::new();
        let source_path = directory.0.join("exact trace probe.c");
        let (mut program, types, location) = program();
        let node = program.add_definition(NominalDefinition::structure("Node"));
        let node_type = program.intern_type(Type::Nominal(node));
        program.definitions[node.index()].add_struct_field("next", node_type, MemberStorage::Referenced);
        let pair = program.add_definition(NominalDefinition::structure("InteriorPair"));
        program.definitions[pair.index()].add_struct_field("prefix", types.character, MemberStorage::Inline);
        program.definitions[pair.index()].add_struct_field("left", node_type, MemberStorage::Inline);
        program.definitions[pair.index()].add_struct_field("right", node_type, MemberStorage::Inline);
        let _pair_type = program.intern_type(Type::Nominal(pair));
        add_main(&mut program, types, location);

        let mut source = emit(&program).unwrap();
        source = source.replacen(
            "typedef struct { uint8_t value; } sao2_unit;",
            concat!(
                "#define main sao2_generated_main\n",
                "#define SAO2_ARENA_CAPACITY UINT64_C(65536)\n",
                "#define SAO2_TRACE_PROBE\n",
                "static bool sao2_probe_fail_next_trace_allocation;\n",
                "typedef struct { uint8_t value; } sao2_unit;",
            ),
            1,
        );
        source.push_str(r#"
#undef main
#define CHECK(condition) do { if (!(condition)) { fprintf(stderr, "trace probe check failed at line %d\n", __LINE__); return 1; } } while (0)
#define OWNER(reference) ((uint64_t)((reference).owner_ptr & ~SAO2_REF_OWNER_TAG_MASK))

static uint32_t probe_epoch(sao2_ref reference) {
    sao2_heap_header *header = NULL;
    return sao2_heap_header_for(reference, &header) ? header->mark_epoch : UINT32_MAX;
}

int main(void) {
    sao2_ref cycle_a, cycle_b, child_a, child_b, pair_owner, left, right, scoped;
    sao2_ref empty = {0}, partial = {0};
    sao2_body_def_0 *node_body;
    sao2_body_def_1 *pair_body;
    sao2_trace_context context;

    CHECK(sao2_arena_runtime_init());
    CHECK(sao2_heap_allocate(sao2_layout_def_0.size, SAO2_REF_ALIGNMENT,
        sao2_layout_def_0.identity, &cycle_a) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(sao2_layout_def_0.size, SAO2_REF_ALIGNMENT,
        sao2_layout_def_0.identity, &cycle_b) == SAO2_ARENA_OK);
    node_body = (sao2_body_def_0 *)sao2_resolve_body(cycle_a, &sao2_layout_def_0);
    node_body->field_0 = cycle_b;
    node_body = (sao2_body_def_0 *)sao2_resolve_body(cycle_b, &sao2_layout_def_0);
    node_body->field_0 = cycle_a;
    sao2_trace_context_init(&context, UINT32_C(3));
    sao2_trace_enqueue(&context, cycle_a, &sao2_layout_def_0);
    CHECK(sao2_trace_drain(&context) == SAO2_TRACE_OK);
    CHECK(context.item_count == 2 && probe_epoch(cycle_a) == 3 && probe_epoch(cycle_b) == 3);
    sao2_trace_context_dispose(&context);

    CHECK(sao2_heap_allocate(sao2_layout_def_0.size, SAO2_REF_ALIGNMENT,
        sao2_layout_def_0.identity, &child_a) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(sao2_layout_def_0.size, SAO2_REF_ALIGNMENT,
        sao2_layout_def_0.identity, &child_b) == SAO2_ARENA_OK);
    CHECK(sao2_heap_allocate(sao2_layout_def_1.size, SAO2_REF_ALIGNMENT,
        sao2_layout_def_1.identity, &pair_owner) == SAO2_ARENA_OK);
    pair_body = (sao2_body_def_1 *)sao2_resolve_body(pair_owner, &sao2_layout_def_1);
    pair_body->field_1.field_0 = child_a;
    pair_body->field_2.field_0 = child_b;
    CHECK(sao2_ref_interior(pair_owner, OWNER(pair_owner) + offsetof(sao2_body_def_1, field_1), &left));
    CHECK(sao2_ref_interior(pair_owner, OWNER(pair_owner) + offsetof(sao2_body_def_1, field_2), &right));
    sao2_trace_context_init(&context, UINT32_C(7));
    sao2_trace_enqueue(&context, left, &sao2_layout_def_0);
    CHECK(sao2_trace_drain(&context) == SAO2_TRACE_OK);
    CHECK(probe_epoch(pair_owner) == 7 && probe_epoch(child_a) == 7 && probe_epoch(child_b) == 0);
    sao2_trace_context_dispose(&context);
    sao2_trace_context_init(&context, UINT32_C(8));
    sao2_trace_enqueue(&context, right, &sao2_layout_def_0);
    CHECK(sao2_trace_drain(&context) == SAO2_TRACE_OK);
    CHECK(probe_epoch(pair_owner) == 8 && probe_epoch(child_a) == 7 && probe_epoch(child_b) == 8);
    sao2_trace_context_dispose(&context);

    CHECK(sao2_scoped_allocate(sao2_layout_def_0.size, &scoped) == SAO2_ARENA_OK);
    node_body = (sao2_body_def_0 *)sao2_resolve_body(scoped, &sao2_layout_def_0);
    node_body->field_0 = child_a;
    sao2_trace_context_init(&context, UINT32_C(11));
    sao2_trace_enqueue(&context, scoped, &sao2_layout_def_0);
    CHECK(sao2_trace_drain(&context) == SAO2_TRACE_OK && probe_epoch(child_a) == 11);
    sao2_trace_context_dispose(&context);

    sao2_trace_context_init(&context, UINT32_C(12));
    sao2_trace_enqueue(&context, empty, &sao2_layout_def_0);
    CHECK(context.item_count == 0 && sao2_trace_drain(&context) == SAO2_TRACE_OK);
    sao2_trace_context_dispose(&context);
    sao2_trace_context_init(&context, UINT32_C(13));
    sao2_probe_fail_next_trace_allocation = true;
    sao2_trace_enqueue(&context, cycle_a, &sao2_layout_def_0);
    CHECK(context.result == SAO2_TRACE_SCRATCH_EXHAUSTED);
    sao2_trace_context_dispose(&context);
    CHECK(context.items == NULL && context.slots == NULL && context.epoch == 0);

    sao2_trace_context_init(&context, UINT32_C(14));
    partial.owner_ptr = cycle_a.owner_ptr;
    sao2_trace_enqueue(&context, partial, &sao2_layout_def_0);
    CHECK(context.result == SAO2_TRACE_INVALID);
    sao2_trace_context_dispose(&context);
    CHECK(sao2_heap_validate());
    sao2_arena_runtime_release();
    return 0;
}
"#);
        fs::write(&source_path, source).unwrap();

        let executable = match crate::host_compiler::compile(&source_path) {
            Ok(executable) => executable,
            Err(error) if error.to_string().contains("no supported C compiler found") => return,
            Err(error) => panic!("native exact-trace probe did not compile: {error}"),
        };
        let output = Command::new(executable).output().unwrap();
        assert!(
            output.status.success(),
            "native exact-trace probe failed with {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    #[test]
    fn renders_scoped_struct_construction_and_reference_equality() {
        let (mut program, types, location) = program();
        let definition = program.add_definition(NominalDefinition::structure("Point"));
        let point = program.intern_type(Type::Nominal(definition));
        program.definitions[definition.index()].add_struct_field("x", types.int, MemberStorage::Inline);
        let failure = program.intern_failure_site(ir::FailureSite {
            location, function: FunctionId::from_index(0), operation: FailureOperation::StructAllocation,
            line: 1, column: 1,
        });
        let mut main = Function::new("main", types.int);
        let value = main.add_local(point, None, LocalOrigin::Temporary);
        let equal = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let projected = main.add_local(types.int, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Aggregate {
            destination: value,
            aggregate: Aggregate::Struct { definition, fields: vec![(ir::FieldId::from_index(0), constant(types.int, ConstantValue::Integer(7)))], failure },
        }, location);
        main.blocks[block.index()].push(OperationKind::Binary {
            destination: equal, operator: BinaryOperator::Equal,
            left: Operand::Copy(Place::local(value)), right: Operand::Copy(Place::local(value)),
        }, location);
        main.blocks[block.index()].push(OperationKind::Copy {
            destination: projected,
            operand: Operand::Copy(Place::projected(value, vec![Projection::StructField {
                definition, field: ir::FieldId::from_index(0), storage: MemberStorage::Inline,
            }])),
        }, location);
        main.blocks[block.index()].terminate(TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location);
        let main = program.add_function(main);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("static inline bool sao2_ref_equal(sao2_ref left, sao2_ref right)"));
        assert!(emitted.contains("sao2_allocate_scoped_struct(&sao2_layout_def_0, 0, &sao2_struct_ref_0, &sao2_struct_bytes_0);"));
        assert!(emitted.contains("sao2_scoped_mark sao2_function_mark = sao2_scoped_mark_current();"));
        assert!(emitted.contains("int64_t sao2_return_value = INT64_C(0);"));
        assert!(emitted.contains("if (!sao2_scoped_restore(sao2_function_mark)) sao2_compiler_invariant();"));
        assert!(emitted.contains("sao2_struct_body_0->field_0 = INT64_C(7);"));
        assert!(emitted.contains("sao2_shadow_frame_fn_0 sao2_frame = {0};"));
        assert!(emitted.contains("sao2_frame.local_0 = sao2_struct_ref_0;"));
        assert!(emitted.contains("sao2_local_1 = sao2_ref_equal(sao2_frame.local_0, sao2_frame.local_0);"));
        assert!(emitted.contains("sao2_local_2 = ((sao2_body_def_0 *)sao2_resolve_body(sao2_frame.local_0, &sao2_layout_def_0))->field_0;"));
        assert!(emitted.contains("static unsigned char *sao2_resolve_body"));
        assert!(emitted.contains("sao2_fail(site, SAO2_FAILURE_STRUCT_ALLOCATION"));
    }

    #[test]
    fn renders_mixed_struct_lifetimes_by_operation_coordinate() {
        let (mut program, types, location) = program();
        let definition = program.add_definition(NominalDefinition::structure("Point"));
        let point = program.intern_type(Type::Nominal(definition));
        program.definitions[definition.index()].add_struct_field("x", types.int, MemberStorage::Inline);
        let failure = program.intern_failure_site(ir::FailureSite {
            location, function: FunctionId::from_index(0), operation: FailureOperation::StructAllocation,
            line: 1, column: 1,
        });
        let mut main = Function::new("main", types.int);
        let value = main.add_local(point, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        for integer in [1, 2] {
            main.blocks[block.index()].push(OperationKind::Aggregate {
                destination: value,
                aggregate: Aggregate::Struct { definition, fields: vec![(ir::FieldId::from_index(0), constant(types.int, ConstantValue::Integer(integer)))], failure },
            }, location);
        }
        main.blocks[block.index()].terminate(TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location);
        let main_id = program.add_function(main);
        program.entry = Some(main_id);
        let plan = AllocationPlan {
            summaries: vec![escape::FunctionSummary { parameter_escapes: Vec::new(), conservative: false }],
            allocations: vec![
                (AllocationId { function: main_id, block, operation: 0 }, AllocationClass::Scoped),
                (AllocationId { function: main_id, block, operation: 1 }, AllocationClass::Heap),
            ],
            has_scoped: vec![true],
        };

        let emitted = emit_with_plan(&program, &plan).unwrap();
        assert_eq!(emitted.matches("sao2_allocate_scoped_struct(&sao2_layout_def_0").count(), 1);
        assert_eq!(emitted.matches("sao2_allocate_struct(&sao2_layout_def_0").count(), 1);
        assert_eq!(emitted.matches("sao2_scoped_mark sao2_function_mark").count(), 1);
        assert_eq!(emitted.matches("sao2_scoped_restore(sao2_function_mark)").count(), 1);
    }

    #[test]
    fn renders_tuple_value_helpers_only_for_eligible_definitions() {
        let (mut program, types, location) = program();
        let mut inner = NominalDefinition::tuple("Inner");
        inner.add_tuple_field(types.int);
        inner.add_tuple_field(types.string);
        inner.add_tuple_field(types.boolean);
        let inner_definition = program.add_definition(inner);
        let inner_type = program.intern_type(Type::Nominal(inner_definition));

        let mut outer = NominalDefinition::tuple("Outer");
        outer.add_tuple_field(inner_type);
        outer.add_tuple_field(types.int);
        let outer_definition = program.add_definition(outer);
        let outer_type = program.intern_type(Type::Nominal(outer_definition));

        let mut float_tuple = NominalDefinition::tuple("FloatTuple");
        float_tuple.add_tuple_field(types.float);
        let float_definition = program.add_definition(float_tuple);
        program.intern_type(Type::Nominal(float_definition));

        let union = program.intern_type(Type::Union(vec![
            UnionAlternative::untagged(types.int),
            UnionAlternative::untagged(types.boolean),
        ]));
        let mut union_tuple = NominalDefinition::tuple("UnionTuple");
        union_tuple.add_tuple_field(union);
        let union_definition = program.add_definition(union_tuple);
        program.intern_type(Type::Nominal(union_definition));

        let mut main = Function::new("main", types.int);
        let inner_value = main.add_local(inner_type, None, LocalOrigin::Temporary);
        let outer_value = main.add_local(outer_type, None, LocalOrigin::Temporary);
        let equal = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Aggregate {
            destination: inner_value,
            aggregate: Aggregate::Tuple {
                definition: inner_definition,
                elements: vec![
                    constant(types.int, ConstantValue::Integer(-7)),
                    constant(types.string, ConstantValue::String(b"a\0z".to_vec())),
                    constant(types.boolean, ConstantValue::Boolean(true)),
                ],
            },
        }, location);
        main.blocks[block.index()].push(OperationKind::Aggregate {
            destination: outer_value,
            aggregate: Aggregate::Tuple {
                definition: outer_definition,
                elements: vec![
                    Operand::Copy(Place::local(inner_value)),
                    constant(types.int, ConstantValue::Integer(9)),
                ],
            },
        }, location);
        main.blocks[block.index()].push(OperationKind::Binary {
            destination: equal,
            operator: BinaryOperator::Equal,
            left: Operand::Copy(Place::local(outer_value)),
            right: Operand::Copy(Place::local(outer_value)),
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = program.add_function(main);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("static inline bool sao2_tuple_equal_def_0(sao2_def_0 left, sao2_def_0 right)"));
        assert!(emitted.contains("sao2_tuple_equal_def_0(left.field_0, right.field_0)"));
        assert!(emitted.contains("static inline bool sao2_tuple_equal_def_1(sao2_def_1 left, sao2_def_1 right)"));
        assert!(!emitted.contains("sao2_tuple_equal_def_3"));
        assert!(emitted.contains("static inline uint64_t sao2_hash_combine(uint64_t state, uint64_t value)"));
        assert!(emitted.contains("static inline uint64_t sao2_tuple_hash_def_0(sao2_def_0 value)"));
        assert!(emitted.contains("value.field_1->hash"));
        assert!(emitted.contains("sao2_tuple_hash_def_0(value.field_0)"));
        assert!(emitted.contains("sao2_local_0 = (sao2_def_0){0};\n    sao2_local_0.field_0 = (-INT64_C(7));"));
        assert!(emitted.contains("sao2_local_2 = sao2_tuple_equal_def_1(sao2_local_1, sao2_local_1);"));
        assert!(!emitted.contains("sao2_tuple_hash_def_2"));
        assert!(!emitted.contains("sao2_tuple_hash_def_3"));
    }

    fn constant(ty: TypeId, value: ConstantValue) -> Operand {
        Operand::Constant(Constant { ty, value })
    }

    fn returning_function(
        program: &mut ir::Program,
        name: &str,
        result: TypeId,
        value: Operand,
        location: ir::LocationId,
    ) -> FunctionId {
        let mut function = Function::new(name, result);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].terminate(TerminatorKind::Return(value), location);
        program.add_function(function)
    }

    fn add_main(program: &mut ir::Program, types: CoreTypes, location: ir::LocationId) -> FunctionId {
        let main = returning_function(
            program,
            "main",
            types.int,
            constant(types.int, ConstantValue::Integer(0)),
            location,
        );
        program.entry = Some(main);
        main
    }

    #[test]
    fn renders_canonical_sections_scalar_types_and_identity_only_names() {
        let (mut program, types, location) = program();
        let mut function = Function::new("while-punctuation!", types.unit);
        for (index, ty) in [types.unit, types.int, types.float, types.string, types.boolean, types.character]
            .into_iter().enumerate()
        {
            function.add_local(ty, Some(format!("C keyword {index}: while")), LocalOrigin::Parameter);
        }
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.unit, ConstantValue::Unit)), location,
        );
        program.add_function(function);
        add_main(&mut program, types, location);

        let emitted = emit(&program).unwrap();
        assert!(emitted.starts_with(concat!(
            "/* Generated by sao2. */\n\n",
            "#ifndef _WIN32\n",
            "#ifndef _POSIX_C_SOURCE\n#define _POSIX_C_SOURCE 200809L\n#endif\n",
            "#ifndef _DEFAULT_SOURCE\n#define _DEFAULT_SOURCE\n#endif\n",
            "#endif\n",
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n",
            "#include <float.h>\n#include <inttypes.h>\n#include <limits.h>\n#include <math.h>\n",
            "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n",
        )));
        assert!(emitted.contains(concat!(
            "sao2_unit sao2_fn_0(sao2_unit sao2_arg_0, int64_t sao2_arg_1, ",
            "double sao2_arg_2, sao2_string sao2_arg_3, bool sao2_arg_4, ",
            "uint8_t sao2_arg_5);\n",
        )));
        let descriptor = emitted.find("typedef struct sao2_interned_string {").unwrap();
        let comparison = emitted.find("static int sao2_string_compare(").unwrap();
        assert!(descriptor < comparison);
        assert!(!emitted.contains("typedef struct { const unsigned char *bytes; size_t length; } sao2_string;"));
        assert!(emitted.contains("sao2_unit sao2_local_0 = {0};\n"));
        assert!(emitted.contains("    goto sao2_block_0;\nsao2_block_0:\n"));
        assert_eq!(emitted, emit(&program).unwrap());
        assert!(emitted.ends_with('\n'));
        assert!(!emitted.ends_with("\n\n"));
    }

    #[test]
    fn orders_forward_declarations_by_identity_and_definitions_by_dependencies() {
        let (mut program, types, location) = program();
        let inner_type = program.intern_type(Type::Nominal(DefinitionId::from_index(1)));
        let mut outer = NominalDefinition::tuple("struct; outer");
        outer.add_tuple_field(inner_type);
        let outer_definition = program.add_definition(outer);
        let mut inner = NominalDefinition::tuple("union inner!");
        inner.add_tuple_field(types.int);
        inner.add_tuple_field(types.boolean);
        let inner_definition = program.add_definition(inner);
        assert_eq!(inner_definition, DefinitionId::from_index(1));
        let outer_type = program.intern_type(Type::Nominal(outer_definition));
        let anonymous = program.intern_type(Type::Union(vec![
            UnionAlternative::tagged("case-with-punctuation", outer_type),
            UnionAlternative::tagged("while", types.character),
        ]));
        let mut named_union = NominalDefinition::union("named union");
        named_union.add_alternative(UnionAlternative::untagged(types.float));
        named_union.add_alternative(UnionAlternative::error(types.int));
        let named_definition = program.add_definition(named_union);
        let named_type = program.intern_type(Type::Nominal(named_definition));

        let mut signature = Function::new("mutual-a", anonymous);
        signature.add_local(outer_type, Some("for".to_owned()), LocalOrigin::Parameter);
        signature.add_local(named_type, Some("switch".to_owned()), LocalOrigin::Parameter);
        signature.add_local(anonymous, None, LocalOrigin::Temporary);
        let block = signature.add_block();
        signature.entry = Some(block);
        signature.blocks[block.index()].terminate(TerminatorKind::Unreachable, location);
        program.add_function(signature);
        add_main(&mut program, types, location);

        let emitted = emit(&program).unwrap();
        let forwards = concat!(
            "typedef struct sao2_def_0 sao2_def_0;\n",
            "typedef struct sao2_def_1 sao2_def_1;\n",
            "typedef struct sao2_def_2 sao2_def_2;\n",
            "typedef struct sao2_union_ty_8 sao2_union_ty_8;\n",
        );
        assert!(emitted.contains(forwards));
        let inner_at = emitted.find("struct sao2_def_1 {").unwrap();
        let outer_at = emitted.find("struct sao2_def_0 {").unwrap();
        let anonymous_at = emitted.find("struct sao2_union_ty_8 {").unwrap();
        assert!(inner_at < outer_at && outer_at < anonymous_at);
        assert!(emitted.contains("int64_t field_0;\n    bool field_1;"));
        assert!(emitted.contains("sao2_def_0 alternative_0; /* tag 1 */"));
        assert!(emitted.contains("uint8_t alternative_1; /* tag 2 */"));
        assert!(emitted.contains("double alternative_0; /* tag 1 */"));
        assert!(emitted.contains("int64_t alternative_1; /* tag 2 */"));
        assert!(emitted.contains(
            "sao2_union_ty_8 sao2_fn_0(sao2_def_0 sao2_arg_0, sao2_def_2 sao2_arg_1);"
        ));
        assert!(!emitted.contains("case-with-punctuation"));
        assert!(!emitted.contains("named union"));
    }

    #[test]
    fn renders_empty_parameters_in_order_and_the_reserved_entry_args_abi() {
        let (mut program, types, location) = program();
        let list = program.intern_type(Type::List(types.string));
        let helper = returning_function(
            &mut program,
            "helper",
            types.boolean,
            constant(types.boolean, ConstantValue::Boolean(true)),
            location,
        );
        let mut main = Function::new("main", types.int);
        main.add_local(list, Some("args".to_owned()), LocalOrigin::Parameter);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = program.add_function(main);
        program.entry = Some(main);
        assert_eq!(helper, FunctionId::from_index(0));

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("bool sao2_fn_0(void);"));
        assert!(emitted.contains("int64_t sao2_fn_1(sao2_args sao2_arg_0);"));
        assert!(emitted.contains("sao2_args sao2_local_0 = {0};\n    sao2_local_0 = sao2_arg_0;"));
    }

    #[test]
    fn prototypes_allow_direct_and_mutual_recursion() {
        let (mut program, types, location) = program();
        for (name, callee) in [("first", FunctionId::from_index(1)), ("second", FunctionId::from_index(0))] {
            let mut function = Function::new(name, types.int);
            let result = function.add_local(types.int, None, LocalOrigin::Temporary);
            let block = function.add_block();
            function.entry = Some(block);
            function.blocks[block.index()].push(OperationKind::Call {
                destination: result,
                function: callee,
                arguments: Vec::new(),
            }, location);
            function.blocks[block.index()].terminate(
                TerminatorKind::Return(Operand::Copy(Place::local(result))), location,
            );
            program.add_function(function);
        }
        program.entry = Some(FunctionId::from_index(0));

        let emitted = emit(&program).unwrap();
        let first = emitted.find("int64_t sao2_fn_0(void);").unwrap();
        let second = emitted.find("int64_t sao2_fn_1(void);").unwrap();
        assert!(first < second);
    }

    #[test]
    fn rejects_invalid_ir_before_capability_checks_with_original_context() {
        let (mut program, types, location) = program();
        let mut function = Function::new("main", types.int);
        let destination = function.add_local(types.int, None, LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].push(OperationKind::Copy {
            destination,
            operand: constant(types.boolean, ConstantValue::Boolean(true)),
        }, location);
        function.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = program.add_function(function);
        program.entry = Some(main);

        let CEmissionError::InvalidIr(error) = emit(&program).unwrap_err() else { panic!() };
        assert_eq!(error.function, Some(main));
        assert_eq!(error.block, Some(block));
        assert_eq!(error.site, Some(OperationSite::Operation(0)));
    }

    #[test]
    fn rejects_structs_containers_and_entry_args_use_while_rendering_tuple_aggregates() {
        let (mut structure, types, location) = program();
        let mut definition = NominalDefinition::structure("keyword while");
        definition.add_struct_field("field", types.int, ir::MemberStorage::Inline);
        let id = structure.add_definition(definition);
        structure.intern_type(Type::Nominal(id));
        add_main(&mut structure, types, location);
        let emitted = emit(&structure).unwrap();
        assert!(emitted.contains("struct sao2_body_def_0"));

        let (mut container, types, location) = program();
        let list = container.intern_type(Type::List(types.int));
        let mut main = Function::new("main", types.int);
        main.add_local(list, Some("items".to_owned()), LocalOrigin::Binding);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = container.add_function(main);
        container.entry = Some(main_id);
        assert_unsupported(emit(&container), "local storage uses container storage", None);

        let (mut tuple_program, types, location) = program();
        let mut tuple = NominalDefinition::tuple("Pair");
        tuple.add_tuple_field(types.int);
        let definition = tuple_program.add_definition(tuple);
        let tuple_ty = tuple_program.intern_type(Type::Nominal(definition));
        let mut main = Function::new("main", types.int);
        let tuple_local = main.add_local(tuple_ty, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].operations.push(Operation {
            kind: OperationKind::Aggregate {
                destination: tuple_local,
                aggregate: Aggregate::Tuple {
                    definition,
                    elements: vec![constant(types.int, ConstantValue::Integer(1))],
                },
            },
            location,
        });
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = tuple_program.add_function(main);
        tuple_program.entry = Some(main_id);
        let emitted = emit(&tuple_program).unwrap();
        assert!(emitted.contains("sao2_local_0 = (sao2_def_0){0};\n    sao2_local_0.field_0 = INT64_C(1);"));

        let (mut args_program, types, location) = program();
        let args_ty = args_program.intern_type(Type::List(types.string));
        let mut main = Function::new("main", types.int);
        let args = main.add_local(args_ty, Some("args".to_owned()), LocalOrigin::Parameter);
        let destination = main.add_local(types.int, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Builtin {
            destination,
            method: ir::BuiltinMethod::ListLen,
            receiver: Operand::Copy(Place::local(args)),
            arguments: Vec::new(),
            failure: None,
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = args_program.add_function(main);
        args_program.entry = Some(main_id);
        assert_unsupported(emit(&args_program), "use of the entry args parameter", Some(OperationSite::Operation(0)));
    }

    #[test]
    fn renders_tuple_projection_and_keeps_container_projection_boundaries() {
        let (mut projection_program, types, location) = program();
        let mut tuple = NominalDefinition::tuple("Tuple");
        let field = tuple.add_tuple_field(types.int);
        let definition = projection_program.add_definition(tuple);
        let tuple_ty = projection_program.intern_type(Type::Nominal(definition));
        let mut main = Function::new("main", types.int);
        let tuple_local = main.add_local(tuple_ty, None, LocalOrigin::Temporary);
        let destination = main.add_local(types.int, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Copy {
            destination,
            operand: Operand::Copy(Place::projected(tuple_local, vec![
                Projection::TupleField { definition, field },
            ])),
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = projection_program.add_function(main);
        projection_program.entry = Some(main_id);
        let emitted = emit(&projection_program).unwrap();
        assert!(emitted.contains("sao2_local_1 = sao2_local_0.field_0;"));

        let (mut assignment_program, types, location) = program();
        let mut tuple = NominalDefinition::tuple("Tuple");
        let field = tuple.add_tuple_field(types.int);
        let definition = assignment_program.add_definition(tuple);
        let tuple_ty = assignment_program.intern_type(Type::Nominal(definition));
        let mut main = Function::new("main", types.int);
        let tuple_local = main.add_local(tuple_ty, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Assign {
            destination: Place::projected(tuple_local, vec![Projection::TupleField { definition, field }]),
            value: constant(types.int, ConstantValue::Integer(1)),
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = assignment_program.add_function(main);
        assignment_program.entry = Some(main);
        let emitted = emit(&assignment_program).unwrap();
        assert!(emitted.contains("sao2_local_0.field_0 = INT64_C(1);"));

        let (mut index_program, types, location) = program();
        let failure = index_program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::StringIndex,
            line: 1,
            column: 1,
        });
        let mut main = Function::new("main", types.int);
        let character = main.add_local(types.character, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::StringIndex {
            destination: character,
            string: constant(types.string, ConstantValue::String(b"x".to_vec())),
            index: constant(types.int, ConstantValue::Integer(0)),
            failure,
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = index_program.add_function(main);
        index_program.entry = Some(main_id);
        let indexed = emit(&index_program).unwrap();
        assert!(indexed.contains("sao2_string_index(&sao2_string_descriptor_0, INT64_C(0), 0);"));
        assert!(indexed.contains("static uint8_t sao2_string_index(sao2_string value, int64_t index, size_t site)"));

        let (mut print_program, types, location) = program();
        let failure = print_program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::Output,
            line: 1,
            column: 1,
        });
        let mut main = Function::new("main", types.int);
        let unit = main.add_local(types.unit, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Intrinsic {
            destination: unit,
            intrinsic: Intrinsic::Print,
            arguments: vec![constant(types.character, ConstantValue::Character(b'x'))],
            failure,
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = print_program.add_function(main);
        print_program.entry = Some(main_id);
        let printed = emit(&print_program).unwrap();
        assert!(printed.contains("sao2_format_char(&sao2_output_writer_0, UINT8_C(120));"));

        let (mut builtin_program, types, location) = program();
        let mut main = Function::new("main", types.int);
        let destination = main.add_local(types.int, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Builtin {
            destination,
            method: ir::BuiltinMethod::StrLen,
            receiver: constant(types.string, ConstantValue::String(Vec::new())),
            arguments: Vec::new(),
            failure: None,
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = builtin_program.add_function(main);
        builtin_program.entry = Some(main_id);
        assert_unsupported(
            emit(&builtin_program),
            "built-in container or string operation",
            Some(OperationSite::Operation(0)),
        );

        let (mut panic_program, types, location) = program();
        let failure = panic_program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::UnhandledError,
            line: 1,
            column: 1,
        });
        let mut main = Function::new("main", types.int);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].terminate(TerminatorKind::ErrorPanic {
            payload: constant(types.float, ConstantValue::Float(0)),
            failure,
        }, location);
        let main_id = panic_program.add_function(main);
        panic_program.entry = Some(main_id);
        assert!(emit(&panic_program).unwrap().contains("sao2_error_panic_float(sao2_float_from_bits(UINT64_C(0)), 0);"));
    }

    #[test]
    fn renders_scalar_constants_storage_calls_and_literal_data() {
        let (mut program, types, location) = program();
        let mut identity = Function::new("source names never appear", types.int);
        let parameter = identity.add_local(types.int, Some("while".to_owned()), LocalOrigin::Parameter);
        let identity_block = identity.add_block();
        identity.entry = Some(identity_block);
        identity.blocks[identity_block.index()].terminate(
            TerminatorKind::Return(Operand::Copy(Place::local(parameter))), location,
        );
        program.add_function(identity);

        let mut main = Function::new("main", types.int);
        let integer = main.add_local(types.int, None, LocalOrigin::Temporary);
        let float = main.add_local(types.float, None, LocalOrigin::Temporary);
        let string = main.add_local(types.string, None, LocalOrigin::Binding);
        let character = main.add_local(types.character, None, LocalOrigin::Temporary);
        let boolean = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let unit = main.add_local(types.unit, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        let body = &mut main.blocks[block.index()];
        for (destination, operand) in [
            (integer, constant(types.int, ConstantValue::Integer(i64::MIN))),
            (float, constant(types.float, ConstantValue::Float((-0.0f64).to_bits()))),
            (string, constant(types.string, ConstantValue::String(vec![b'a', 0, b'z']))),
            (character, constant(types.character, ConstantValue::Character(0x7f))),
            (boolean, constant(types.boolean, ConstantValue::Boolean(false))),
            (unit, constant(types.unit, ConstantValue::Unit)),
        ] {
            body.push(OperationKind::Copy { destination, operand }, location);
        }
        body.push(OperationKind::Copy {
            destination: string,
            operand: constant(types.string, ConstantValue::String(Vec::new())),
        }, location);
        body.push(OperationKind::Call {
            destination: integer,
            function: FunctionId::from_index(0),
            arguments: vec![constant(types.int, ConstantValue::Integer(-7))],
        }, location);
        body.terminate(TerminatorKind::Return(Operand::Copy(Place::local(integer))), location);
        let main_id = program.add_function(main);
        program.entry = Some(main_id);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("static const unsigned char sao2_string_data_0[] = { UINT8_C(97), UINT8_C(0), UINT8_C(122) };"));
        assert!(emitted.contains("static const unsigned char sao2_string_data_1[] = { UINT8_C(0) };"));
        assert!(emitted.contains("static const sao2_interned_string sao2_string_descriptor_0 = { sao2_string_data_0, 3, UINT64_C(16560493500796669818) };"));
        assert!(emitted.contains("static const sao2_interned_string sao2_string_descriptor_1 = { sao2_string_data_1, 0, UINT64_C(14695981039346656037) };"));
        assert!(emitted.contains("(-INT64_C(9223372036854775807) - INT64_C(1))"));
        assert!(emitted.contains("sao2_float_from_bits(UINT64_C(9223372036854775808))"));
        assert!(emitted.contains("sao2_local_2 = &sao2_string_descriptor_1;"));
        assert!(!emitted.contains("(sao2_string){"));
        assert!(emitted.contains("UINT8_C(127)"));
        assert!(emitted.contains("sao2_local_0 = sao2_fn_0((-INT64_C(7)));"));
        assert!(!emitted.contains("source names never appear"));
        // Formatter runtime code legitimately uses C control-flow keywords; source
        // names must still never be used as generated identifiers.
        assert!(!emitted.contains("sao2_fn_0_while"));
    }

    #[test]
    fn renders_scalar_operators_checks_and_literal_cfg_order() {
        let (mut program, types, location) = program();
        let failure = program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::IntegerAdd,
            line: 1,
            column: 1,
        });
        let mut main = Function::new("main", types.int);
        let result = main.add_local(types.int, None, LocalOrigin::Temporary);
        let condition = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let blocks = (0..4).map(|_| main.add_block()).collect::<Vec<_>>();
        main.entry = Some(blocks[2]);
        main.blocks[0].terminate(TerminatorKind::Jump(blocks[2]), location);
        main.blocks[1].terminate(TerminatorKind::Unreachable, location);
        let left = constant(types.int, ConstantValue::Integer(4));
        let right = constant(types.int, ConstantValue::Integer(5));
        main.blocks[2].push(OperationKind::Check(RuntimeCheck::IntegerOverflow {
            operation: IntegerOperation::Add,
            left: left.clone(),
            right: right.clone(),
            failure,
        }), location);
        main.blocks[2].push(OperationKind::Binary {
            destination: result,
            operator: BinaryOperator::Add,
            left,
            right,
        }, location);
        main.blocks[2].push(OperationKind::Binary {
            destination: condition,
            operator: BinaryOperator::Equal,
            left: Operand::Copy(Place::local(result)),
            right: constant(types.int, ConstantValue::Integer(9)),
        }, location);
        main.blocks[2].terminate(TerminatorKind::Branch {
            condition: Operand::Copy(Place::local(condition)),
            then_block: blocks[3],
            else_block: blocks[0],
        }, location);
        main.blocks[3].terminate(TerminatorKind::Return(Operand::Copy(Place::local(result))), location);
        let main_id = program.add_function(main);
        program.entry = Some(main_id);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("    goto sao2_block_2;\nsao2_block_0:\n    goto sao2_block_2;\nsao2_block_1:\n    sao2_compiler_invariant();\nsao2_block_2:"));
        let check = emitted.find("sao2_check_integer_add(INT64_C(4), INT64_C(5), 0);").unwrap();
        let addition = emitted.find("sao2_local_0 = INT64_C(4) + INT64_C(5);").unwrap();
        assert!(check < addition);
        assert!(emitted.contains("if (sao2_local_1) {\n        goto sao2_block_3;\n    } else {\n        goto sao2_block_0;\n    }"));
    }

    #[test]
    fn renders_output_with_exact_failure_sites_and_windows_guard() {
        let (mut program, types, location) = program();
        let failure = program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::Output,
            line: 1,
            column: 1,
        });
        let mut main = Function::new("main", types.int);
        let unit = main.add_local(types.unit, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        for (intrinsic, arguments) in [
            (Intrinsic::Print, vec![constant(types.string, ConstantValue::String(vec![b'x', 0]))]),
            (Intrinsic::Println, vec![constant(types.int, ConstantValue::Integer(12))]),
            (Intrinsic::Println, vec![constant(types.boolean, ConstantValue::Boolean(true))]),
            (Intrinsic::Println, Vec::new()),
        ] {
            main.blocks[block.index()].push(OperationKind::Intrinsic {
                destination: unit,
                intrinsic,
                arguments,
                failure,
            }, location);
        }
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = program.add_function(main);
        program.entry = Some(main_id);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains(concat!(
            "#ifdef _WIN32\n#include <windows.h>\n#include <fcntl.h>\n#include <io.h>\n",
            "#else\n#include <sys/mman.h>\n#include <unistd.h>\n#endif",
        )));
        assert!(emitted.contains("sao2_format_string(&sao2_output_writer_0, &sao2_string_descriptor_0);"));
        assert!(emitted.contains("sao2_format_int(&sao2_output_writer_0, INT64_C(12));"));
        assert!(emitted.contains("sao2_format_bool(&sao2_output_writer_0, true);"));
        assert_eq!(emitted.matches("sao2_writer_char(&sao2_output_writer_0, UINT8_C(10));").count(), 3);
        assert_eq!(emitted.matches("sao2_local_0 = (sao2_unit){0};").count(), 4);
    }

    #[test]
    fn renders_stage_three_metadata_binary64_contract_and_checked_helpers() {
        let (mut program, types, location) = program();
        program.source.filename = "dir/panic-name.sao2".into();
        let main = add_main(&mut program, types, location);
        program.functions[main.index()].name = "fn-with-punctuation!".to_owned();
        program.intern_failure_site(FailureSite {
            location,
            function: main,
            operation: FailureOperation::IntegerMultiply,
            line: 12,
            column: 34,
        });

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains(
            "#include <float.h>\n#include <inttypes.h>\n#include <limits.h>\n#include <math.h>\n"
        ));
        assert!(emitted.contains("_Static_assert(FLT_RADIX == 2"));
        assert!(emitted.contains("_Static_assert(DBL_MANT_DIG == 53"));
        assert!(emitted.contains("_Static_assert(DBL_MIN_EXP == -1021"));
        assert!(emitted.contains("_Static_assert(DBL_MAX_EXP == 1024"));
        assert!(emitted.contains("_Static_assert(sizeof(double) == 8"));
        assert!(emitted.contains(concat!(
            "static const unsigned char sao2_filename_data[] = { UINT8_C(100), UINT8_C(105), ",
            "UINT8_C(114), UINT8_C(47), UINT8_C(112), UINT8_C(97), UINT8_C(110), UINT8_C(105), ",
        )));
        assert!(emitted.contains("static const sao2_bytes sao2_filename = { sao2_filename_data, 19 };"));
        assert!(emitted.contains(concat!(
            "static const unsigned char sao2_function_name_0[] = { UINT8_C(102), UINT8_C(110), ",
            "UINT8_C(45), UINT8_C(119), UINT8_C(105), UINT8_C(116), UINT8_C(104), ",
        )));
        assert!(emitted.contains("{ sao2_function_name_0, 20 },"));
        assert!(emitted.contains("{ SAO2_FAILURE_INTEGER_MULTIPLY, 0, 12, 34 },"));
        assert!(emitted.contains("static const size_t sao2_failure_count = 1;"));
        assert!(emitted.contains("#define SAO2_FAILURE_UNHANDLED_ERROR UINT32_C(21)"));
        assert!(emitted.contains("static void sao2_check_integer_add(int64_t left, int64_t right, size_t site)"));
        assert!(emitted.contains("right_magnitude > limit / left_magnitude"));
        assert!(emitted.contains("operand >= -0x1p63 && operand < 0x1p63"));
        for reason in [
            "integer addition overflow",
            "integer subtraction overflow",
            "integer multiplication overflow",
            "integer negation overflow",
            "integer division by zero",
            "integer division overflow",
            "integer remainder by zero",
            "integer remainder overflow",
            "shift count out of range",
            "integer left shift overflow",
            "floating-point division by zero",
            "floating-point addition produced a non-finite result",
            "floating-point subtraction produced a non-finite result",
            "floating-point multiplication produced a non-finite result",
            "floating-point division produced a non-finite result",
            "float-to-int conversion out of range",
            "standard output failure",
        ] {
            assert!(emitted.contains(reason), "missing locked panic reason {reason}");
        }
        assert!(!emitted.contains("void sao2_check_integer_add(int64_t, int64_t, size_t);"));
        assert_eq!(emitted, emit(&program).unwrap());
    }

    #[test]
    fn renders_empty_failure_metadata_sentinel_and_defensive_panic_paths() {
        let (mut program, types, location) = program();
        add_main(&mut program, types, location);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains(concat!(
            "static const sao2_failure_site sao2_failures[] = {\n",
            "    { UINT32_C(0), 0, 0, 0 },\n",
            "};\n",
            "static const size_t sao2_failure_count = 0;\n",
        )));
        assert!(emitted.contains("if (site >= sao2_failure_count) sao2_compiler_invariant();"));
        assert!(emitted.contains("if (sao2_failures[site].function >= sao2_function_count) sao2_compiler_invariant();"));
        assert!(emitted.contains("sao2: internal compiler error: reached unreachable IR\\n"));
        assert!(emitted.contains("static _Noreturn void sao2_output_failure(size_t site)"));
        assert!(emitted.contains("sao2_fail(site, SAO2_FAILURE_OUTPUT, reason, sizeof reason - 1);"));
        assert!(!emitted.contains("Stage 3 replaces this temporary failure path"));
    }

    #[test]
    fn keeps_float_result_check_immediately_after_materialization() {
        let (mut program, types, location) = program();
        let failure = program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::FloatAdd,
            line: 7,
            column: 9,
        });
        let mut main = Function::new("float-main", types.int);
        let result = main.add_local(types.float, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::Binary {
            destination: result,
            operator: BinaryOperator::Add,
            left: constant(types.float, ConstantValue::Float(1.0f64.to_bits())),
            right: constant(types.float, ConstantValue::Float(2.0f64.to_bits())),
        }, location);
        main.blocks[block.index()].push(OperationKind::Check(RuntimeCheck::FiniteFloat {
            operand: Operand::Copy(Place::local(result)),
            failure,
        }), location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main_id = program.add_function(main);
        program.entry = Some(main_id);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains(concat!(
            "    sao2_local_0 = sao2_float_from_bits(UINT64_C(4607182418800017408)) + ",
            "sao2_float_from_bits(UINT64_C(4611686018427387904));\n",
            "    sao2_check_finite_float(sao2_local_0, 0);\n",
        )));
    }

    #[test]
    fn renders_explicit_and_error_panics_with_raw_payloads_and_exact_sites() {
        let (mut program, types, location) = program();
        let explicit = program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::ExplicitPanic,
            line: 2,
            column: 3,
        });
        let unhandled = program.intern_failure_site(FailureSite {
            location,
            function: FunctionId::from_index(0),
            operation: FailureOperation::UnhandledError,
            line: 4,
            column: 5,
        });
        let mut main = Function::new("panic-main", types.int);
        let entry = main.add_block();
        let panic_block = main.add_block();
        let error_block = main.add_block();
        main.entry = Some(entry);
        main.blocks[entry.index()].terminate(TerminatorKind::Branch {
            condition: constant(types.boolean, ConstantValue::Boolean(true)),
            then_block: panic_block,
            else_block: error_block,
        }, location);
        main.blocks[panic_block.index()].terminate(TerminatorKind::Panic {
            message: constant(types.string, ConstantValue::String(vec![b'x', 0, b'\n'])),
            failure: explicit,
        }, location);
        main.blocks[error_block.index()].terminate(TerminatorKind::ErrorPanic {
            payload: constant(types.character, ConstantValue::Character(b'\n')),
            failure: unhandled,
        }, location);
        let main_id = program.add_function(main);
        program.entry = Some(main_id);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("static const unsigned char sao2_string_data_0[] = { UINT8_C(120), UINT8_C(0), UINT8_C(10) };"));
        assert!(emitted.contains("sao2_panic(&sao2_string_descriptor_0, 0);"));
        assert!(emitted.contains("sao2_error_panic_char(UINT8_C(10), 1);"));
        assert!(emitted.contains("{ SAO2_FAILURE_EXPLICIT_PANIC, 0, 2, 3 },"));
        assert!(emitted.contains("{ SAO2_FAILURE_UNHANDLED_ERROR, 0, 4, 5 },"));
    }

    #[test]
    fn renders_union_injection_tests_guarded_payloads_and_ordered_switches() {
        let (mut program, types, location) = program();
        let union = program.intern_type(Type::Union(vec![
            UnionAlternative::tagged("source-name-a", types.int),
            UnionAlternative::tagged("source-name-b", types.boolean),
        ]));
        let first = ir::AlternativeId::from_index(0);
        let second = ir::AlternativeId::from_index(1);
        let mut main = Function::new("main", types.int);
        let value = main.add_local(union, None, LocalOrigin::Temporary);
        let tested = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let payload = main.add_local(types.int, None, LocalOrigin::Temporary);
        let entry = main.add_block();
        let first_target = main.add_block();
        let second_target = main.add_block();
        main.entry = Some(entry);
        main.blocks[entry.index()].push(OperationKind::UnionInject {
            destination: value,
            union_type: union,
            alternative: first,
            payload: constant(types.int, ConstantValue::Integer(7)),
        }, location);
        main.blocks[entry.index()].push(OperationKind::UnionTest {
            destination: tested,
            union: Operand::Copy(Place::local(value)),
            alternative: first,
        }, location);
        main.blocks[entry.index()].push(OperationKind::UnionPayload {
            destination: payload,
            union: Operand::Copy(Place::local(value)),
            alternative: first,
        }, location);
        main.blocks[entry.index()].terminate(TerminatorKind::Switch {
            union: Operand::Copy(Place::local(value)),
            targets: vec![(second, second_target), (first, first_target)],
        }, location);
        main.blocks[first_target.index()].terminate(
            TerminatorKind::Return(Operand::Copy(Place::local(payload))), location,
        );
        main.blocks[second_target.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = program.add_function(main);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        let zero = emitted.find("    sao2_local_0 = (sao2_union_ty_6){0};").unwrap();
        let payload_write = emitted.find("    sao2_local_0.payload.alternative_0 = INT64_C(7);").unwrap();
        let tag_write = emitted.find("    sao2_local_0.tag = UINT32_C(1);").unwrap();
        assert!(zero < payload_write && payload_write < tag_write);
        assert!(emitted.contains("sao2_local_1 = (sao2_local_0).tag == UINT32_C(1);"));
        assert!(emitted.contains(concat!(
            "if ((sao2_local_0).tag != UINT32_C(1)) sao2_compiler_invariant();\n",
            "    sao2_local_2 = (sao2_local_0).payload.alternative_0;",
        )));
        assert!(emitted.contains(concat!(
            "switch ((sao2_local_0).tag) {\n",
            "        case UINT32_C(1): goto sao2_block_1;\n",
            "        case UINT32_C(2): goto sao2_block_2;\n",
            "        default: sao2_compiler_invariant();\n",
        )));
        assert!(!emitted.contains("source-name-a"));
        assert_eq!(emitted, emit(&program).unwrap());
    }

    #[test]
    fn preserves_nested_named_unions_through_copies_calls_and_results() {
        let (mut program, types, location) = program();
        let mut inner_definition = NominalDefinition::union("Inner source name");
        let inner_int = inner_definition.add_alternative(UnionAlternative::untagged(types.int));
        inner_definition.add_alternative(UnionAlternative::untagged(types.boolean));
        let inner_definition = program.add_definition(inner_definition);
        let inner = program.intern_type(Type::Nominal(inner_definition));
        let mut outer_definition = NominalDefinition::union("Outer source name");
        let outer_inner = outer_definition.add_alternative(UnionAlternative::untagged(inner));
        outer_definition.add_alternative(UnionAlternative::untagged(types.character));
        let outer_definition = program.add_definition(outer_definition);
        let outer = program.intern_type(Type::Nominal(outer_definition));

        let mut identity = Function::new("union identity", outer);
        let parameter = identity.add_local(outer, Some("value".to_owned()), LocalOrigin::Parameter);
        let block = identity.add_block();
        identity.entry = Some(block);
        identity.blocks[block.index()].terminate(
            TerminatorKind::Return(Operand::Copy(Place::local(parameter))), location,
        );
        let identity = program.add_function(identity);

        let mut main = Function::new("main", types.int);
        let inner_value = main.add_local(inner, None, LocalOrigin::Temporary);
        let outer_value = main.add_local(outer, None, LocalOrigin::Temporary);
        let copied = main.add_local(outer, None, LocalOrigin::Temporary);
        let result = main.add_local(outer, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::UnionInject {
            destination: inner_value,
            union_type: inner,
            alternative: inner_int,
            payload: constant(types.int, ConstantValue::Integer(9)),
        }, location);
        main.blocks[block.index()].push(OperationKind::UnionInject {
            destination: outer_value,
            union_type: outer,
            alternative: outer_inner,
            payload: Operand::Copy(Place::local(inner_value)),
        }, location);
        main.blocks[block.index()].push(OperationKind::Copy {
            destination: copied,
            operand: Operand::Copy(Place::local(outer_value)),
        }, location);
        main.blocks[block.index()].push(OperationKind::Call {
            destination: result,
            function: identity,
            arguments: vec![Operand::Copy(Place::local(copied))],
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = program.add_function(main);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("sao2_def_0 alternative_0; /* tag 1 */"));
        assert!(emitted.contains("sao2_local_2 = sao2_local_1;"));
        assert!(emitted.contains("sao2_local_3 = sao2_fn_0(sao2_local_2);"));
        assert!(!emitted.contains("Inner source name"));
        assert!(!emitted.contains("Outer source name"));
    }

    #[test]
    fn renders_all_four_entry_adapter_shapes_and_raw_diagnostics() {
        for (with_args, unit_result) in [(false, false), (false, true), (true, false), (true, true)] {
            let (mut program, types, location) = program();
            let result = if unit_result { types.unit } else { types.int };
            let value = if unit_result {
                constant(types.unit, ConstantValue::Unit)
            } else {
                constant(types.int, ConstantValue::Integer(0))
            };
            let mut main = Function::new("main", result);
            if with_args {
                let args = program.intern_type(Type::List(types.string));
                main.add_local(args, Some("args".to_owned()), LocalOrigin::Parameter);
            }
            let block = main.add_block();
            main.entry = Some(block);
            main.blocks[block.index()].terminate(TerminatorKind::Return(value), location);
            let main = program.add_function(main);
            program.entry = Some(main);

            let emitted = emit(&program).unwrap();
            if with_args {
                assert!(emitted.contains("int main(int argc, char **argv) {"));
                assert!(emitted.contains("for (sao2_argument = 1; sao2_argument < argc; ++sao2_argument)"));
                assert!(emitted.contains("sao2_pre_entry_panic_argument((size_t)(sao2_argument - 1));"));
                assert!(emitted.contains(concat!(
                    "sao2_args sao2_entry_args = { argc > 0 ? argc - 1 : 0, ",
                    "argc > 0 ? argv + 1 : argv };",
                )));
            } else {
                assert!(emitted.contains("int main(void) {"));
            }
            if unit_result {
                let call = if with_args { "sao2_unit sao2_result = sao2_fn_0(sao2_entry_args);" }
                    else { "sao2_unit sao2_result = sao2_fn_0();" };
                assert_eq!(emitted.matches(call).count(), 1);
                assert!(emitted.contains("return EXIT_SUCCESS;"));
            } else {
                let call = if with_args { "int64_t sao2_result = sao2_fn_0(sao2_entry_args);" }
                    else { "int64_t sao2_result = sao2_fn_0();" };
                assert_eq!(emitted.matches(call).count(), 1);
                assert!(emitted.contains("sao2_result < INT_MIN || sao2_result > INT_MAX"));
                assert!(emitted.contains("return (int)sao2_result;"));
            }
            assert!(emitted.contains("main returned an exit status outside the host int range"));
            assert!(emitted.contains("command-line argument "));
            assert!(emitted.contains(" is not ASCII"));
            assert!(emitted.ends_with('\n'));
            assert!(!emitted.ends_with("\n\n"));
        }
    }

    #[test]
    fn renders_executable_union_with_a_reachable_tuple_payload() {
        let (mut program, types, location) = program();
        let mut tuple = NominalDefinition::tuple("Tuple");
        tuple.add_tuple_field(types.int);
        let tuple_definition = program.add_definition(tuple);
        let tuple_type = program.intern_type(Type::Nominal(tuple_definition));
        let union = program.intern_type(Type::Union(vec![
            UnionAlternative::untagged(tuple_type),
            UnionAlternative::untagged(types.int),
        ]));
        let mut main = Function::new("main", types.int);
        let value = main.add_local(union, None, LocalOrigin::Temporary);
        let tested = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let block = main.add_block();
        main.entry = Some(block);
        main.blocks[block.index()].push(OperationKind::UnionTest {
            destination: tested,
            union: Operand::Copy(Place::local(value)),
            alternative: ir::AlternativeId::from_index(1),
        }, location);
        main.blocks[block.index()].terminate(
            TerminatorKind::Return(constant(types.int, ConstantValue::Integer(0))), location,
        );
        let main = program.add_function(main);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        assert!(emitted.contains("sao2_def_0 alternative_0; /* tag 1 */"));
        assert!(emitted.contains("sao2_local_1 = (sao2_local_0).tag == UINT32_C(2);"));
    }

    #[test]
    fn reports_recursive_inline_layout_as_a_backend_invariant() {
        let (mut program, types, location) = program();
        let recursive = program.intern_type(Type::Nominal(DefinitionId::from_index(0)));
        let mut tuple = NominalDefinition::tuple("Recursive");
        tuple.add_tuple_field(recursive);
        let definition = program.add_definition(tuple);
        assert_eq!(definition, DefinitionId::from_index(0));
        add_main(&mut program, types, location);

        let CEmissionError::Invariant(error) = emit(&program).unwrap_err() else { panic!() };
        assert_eq!(error.definition, Some(definition));
        assert_eq!(error.message, "cyclic by-value aggregate layout");
    }

    fn assert_unsupported(
        result: Result<String, CEmissionError>,
        message: &str,
        site: Option<OperationSite>,
    ) {
        let CEmissionError::Unsupported(error) = result.unwrap_err() else { panic!() };
        assert_eq!(error.message, message);
        if site.is_some() { assert_eq!(error.site, site); }
    }
}
