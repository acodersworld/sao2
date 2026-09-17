//! Context-insensitive ownership escape summaries for typed IR.
//!
//! This deliberately models language values only.  In particular it knows
//! nothing about generated C layouts, arena offsets, or allocation tags.

use std::collections::{BTreeSet, VecDeque};
use std::fmt;

use crate::ir::{self, Aggregate, BlockId, DefinitionLayout, FunctionId, MemberStorage,
    Operand, OperationKind, OperationSite, Place, Projection, TerminatorKind, Type, TypeId};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct AllocationId { pub(crate) function: FunctionId, pub(crate) block: BlockId, pub(crate) operation: usize }

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum AllocationClass { Scoped, Heap }

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionSummary { pub(crate) parameter_escapes: Vec<bool>, pub(crate) conservative: bool }

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AllocationPlan {
    pub(crate) summaries: Vec<FunctionSummary>,
    pub(crate) allocations: Vec<(AllocationId, AllocationClass)>,
    pub(crate) has_scoped: Vec<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EscapeError { pub(crate) function: Option<FunctionId>, pub(crate) block: Option<BlockId>, pub(crate) site: Option<OperationSite>, pub(crate) message: String }
impl fmt::Display for EscapeError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "escape analysis invariant")?;
    if let Some(id) = self.function { write!(f, " in {id}")?; }
    if let Some(id) = self.block { write!(f, " {id}")?; }
    if let Some(OperationSite::Operation(id)) = self.site { write!(f, " op{id}")?; }
    if matches!(self.site, Some(OperationSite::Terminator)) { write!(f, " terminator")?; }
    write!(f, ": {}", self.message)
} }
impl std::error::Error for EscapeError {}

impl AllocationPlan {
    /// Returns the lifetime class recorded for one exact struct-construction
    /// operation. The entries are maintained in `AllocationId` order, so the
    /// backend need not reconstruct a renderer-local lookup table.
    pub(crate) fn allocation_class(&self, id: AllocationId) -> Option<AllocationClass> {
        self.allocations.binary_search_by_key(&id, |(entry, _)| *entry)
            .ok()
            .map(|index| self.allocations[index].1)
    }

    /// Whether this function owns an invocation-wide scoped-arena mark.
    pub(crate) fn function_has_scoped(&self, id: FunctionId) -> bool {
        self.has_scoped.get(id.index()).copied().unwrap_or(false)
    }

    #[allow(dead_code)] // Stable test/debug rendering for this compiler-owned artifact.
    pub(crate) fn render(&self) -> String {
        let mut output = String::new();
        for (index, summary) in self.summaries.iter().enumerate() {
            let bits = summary.parameter_escapes.iter().map(|bit| if *bit { '1' } else { '0' }).collect::<String>();
            output.push_str(&format!("fn{index} {} {bits}\n", if summary.conservative { "conservative" } else { "proven" }));
        }
        for (id, class) in &self.allocations {
            output.push_str(&format!("{} {} op{} {}\n", id.function, id.block, id.operation, match class { AllocationClass::Scoped => "scoped", AllocationClass::Heap => "heap" }));
        }
        output
    }

    pub(crate) fn validate(&self, program: &ir::Program) -> Result<(), EscapeError> {
        if self.summaries.len() != program.functions.len() || self.has_scoped.len() != program.functions.len() {
            return Err(error(None, None, None, "function plan does not cover every function"));
        }
        let mut expected = BTreeSet::new();
        for (fi, function) in program.functions.iter().enumerate() {
            if self.summaries[fi].parameter_escapes.len() != function.parameters.len() {
                return Err(error(Some(FunctionId::from_index(fi)), None, None, "parameter summary has the wrong arity"));
            }
            for (bi, block) in function.blocks.iter().enumerate() { for (oi, operation) in block.operations.iter().enumerate() {
                if matches!(operation.kind, OperationKind::Aggregate { aggregate: Aggregate::Struct { .. }, .. }) {
                    expected.insert(AllocationId { function: FunctionId::from_index(fi), block: BlockId::from_index(bi), operation: oi });
                }
            }}
        }
        let actual = self.allocations.iter().map(|(id, _)| *id).collect::<BTreeSet<_>>();
        if actual.len() != self.allocations.len() || actual != expected { return Err(error(None, None, None, "allocation plan has duplicate, missing, or invalid entries")); }
        for fi in 0..program.functions.len() {
            let scoped = self.allocations.iter().any(|(id, class)| id.function == FunctionId::from_index(fi) && *class == AllocationClass::Scoped);
            if scoped != self.has_scoped[fi] { return Err(error(Some(FunctionId::from_index(fi)), None, None, "scoped-allocation index disagrees with allocation entries")); }
        }
        Ok(())
    }
}

pub(crate) fn analyze(program: &ir::Program) -> Result<AllocationPlan, EscapeError> {
    program.validate().map_err(|e| error(e.function, e.block, e.site, e.message))?;
    let count = program.functions.len();
    let reachable = (0..count).map(|i| reachable_blocks(&program.functions[i])).collect::<Vec<_>>();
    let mut callees = vec![BTreeSet::new(); count];
    for fi in 0..count { for bi in &reachable[fi] { for op in &program.functions[fi].blocks[bi.index()].operations {
        if let OperationKind::Call { function, .. } = op.kind { callees[fi].insert(function); }
    }}}
    let mut callers = vec![BTreeSet::new(); count];
    let mut pending = vec![0usize; count];
    for fi in 0..count { pending[fi] = callees[fi].len(); for callee in &callees[fi] { callers[callee.index()].insert(FunctionId::from_index(fi)); } }
    let mut queue = (0..count).filter(|&i| pending[i] == 0).collect::<VecDeque<_>>();
    let mut summaries: Vec<Option<FunctionSummary>> = vec![None; count];
    let mut classes: BTreeSet<(AllocationId, AllocationClass)> = BTreeSet::new();
    while let Some(fi) = queue.pop_front() {
        let result = analyse_function(program, FunctionId::from_index(fi), &reachable[fi], &summaries)?;
        summaries[fi] = Some(result.0);
        classes.extend(result.1);
        for caller in &callers[fi] { pending[caller.index()] -= 1; if pending[caller.index()] == 0 { queue.push_back(caller.index()); } }
    }
    for fi in 0..count {
        if summaries[fi].is_some() { continue; }
        let function = &program.functions[fi];
        summaries[fi] = Some(FunctionSummary { parameter_escapes: function.parameters.iter().map(|p| carries(program, function.locals[p.index()].ty).unwrap_or(true)).collect(), conservative: true });
        for (bi, block) in function.blocks.iter().enumerate() { for (oi, op) in block.operations.iter().enumerate() {
            if matches!(op.kind, OperationKind::Aggregate { aggregate: Aggregate::Struct { .. }, .. }) { classes.insert((AllocationId { function: FunctionId::from_index(fi), block: BlockId::from_index(bi), operation: oi }, AllocationClass::Heap)); }
        }}
    }
    let summaries = summaries.into_iter().map(Option::unwrap).collect::<Vec<_>>();
    let allocations = classes.into_iter().collect::<Vec<_>>();
    let has_scoped = (0..count).map(|fi| allocations.iter().any(|(id, c)| id.function.index() == fi && *c == AllocationClass::Scoped)).collect();
    let plan = AllocationPlan { summaries, allocations, has_scoped };
    plan.validate(program)?;
    Ok(plan)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)] enum Origin { Parameter(usize), Allocation(AllocationId), ExternalHeap }
type Facts = BTreeSet<Origin>;

fn analyse_function(program: &ir::Program, id: FunctionId, reachable: &BTreeSet<BlockId>, summaries: &[Option<FunctionSummary>]) -> Result<(FunctionSummary, BTreeSet<(AllocationId, AllocationClass)>), EscapeError> {
    let function = &program.functions[id.index()];
    let mut locals = vec![Facts::new(); function.locals.len()];
    for (position, parameter) in function.parameters.iter().enumerate() { if carries(program, function.locals[parameter.index()].ty)? { locals[parameter.index()].insert(Origin::Parameter(position)); } }
    let mut conservative = function.locals.iter().any(|local| retention_unknown_type(program, local.ty).unwrap_or(true));
    loop {
        let before = locals.clone();
        for block in reachable { for (oi, op) in function.blocks[block.index()].operations.iter().enumerate() {
            let allocation = AllocationId { function: id, block: *block, operation: oi };
            match &op.kind {
                OperationKind::Copy { destination, operand } => {
                    let values = operand_facts(program, function, &locals, operand)?;
                    add(&mut locals[destination.index()], values);
                }
                OperationKind::Assign { destination, value } if destination.projections.is_empty() => {
                    let values = operand_facts(program, function, &locals, value)?;
                    add(&mut locals[destination.local.index()], values);
                }
                OperationKind::Aggregate { destination, aggregate: Aggregate::Tuple { elements, .. } } => for value in elements {
                    let values = operand_facts(program, function, &locals, value)?;
                    add(&mut locals[destination.index()], values);
                },
                OperationKind::Aggregate { destination, aggregate: Aggregate::Struct { .. } } => { locals[destination.index()].insert(Origin::Allocation(allocation)); },
                OperationKind::UnionInject { destination, payload, .. } | OperationKind::UnionPayload { destination, union: payload, .. } => {
                    let values = operand_facts(program, function, &locals, payload)?;
                    add(&mut locals[destination.index()], values);
                }
                OperationKind::Call { destination, .. } if carries(program, function.locals[destination.index()].ty)? => { locals[destination.index()].insert(Origin::ExternalHeap); },
                _ => {}
            }
        }}
        if locals == before { break; }
    }
    // Any reference-bearing container or unknown retention operation causes the local fallback.
    for block in reachable { for op in &function.blocks[block.index()].operations {
        if unknown_operation(program, function, op)? { conservative = true; }
    }}
    let mut edges: BTreeSet<(Origin, Origin)> = BTreeSet::new();
    let mut escaped = Facts::new();
    for block in reachable { for (oi, op) in function.blocks[block.index()].operations.iter().enumerate() {
        let allocation = AllocationId { function: id, block: *block, operation: oi };
        match &op.kind {
            OperationKind::Aggregate { aggregate: Aggregate::Struct { fields, .. }, .. } => for (_, value) in fields { for origin in operand_facts(program, function, &locals, value)? { edges.insert((Origin::Allocation(allocation), origin)); } },
            OperationKind::Assign { destination, value } if !destination.projections.is_empty() => {
                let destination_facts = place_owner_facts(program, function, &locals, destination)?;
                let source = operand_facts(program, function, &locals, value)?;
                if destination_facts.iter().any(|o| matches!(o, Origin::Parameter(_) | Origin::ExternalHeap)) { escaped.extend(source.iter().copied()); }
                for owner in destination_facts { for source in &source { edges.insert((owner, *source)); } }
            }
            OperationKind::Call { function: callee, arguments, .. } => {
                let summary = summaries[callee.index()].as_ref().ok_or_else(|| error(Some(id), Some(*block), Some(OperationSite::Operation(oi)), "callee was not resolved before caller"))?;
                for (position, argument) in arguments.iter().enumerate() { if summary.parameter_escapes.get(position).copied().unwrap_or(true) { escaped.extend(operand_facts(program, function, &locals, argument)?); } }
            }
            _ => {}
        }
    }}
    for block in reachable { match &function.blocks[block.index()].terminator.as_ref().expect("validated").kind {
        TerminatorKind::Return(value) => escaped.extend(operand_facts(program, function, &locals, value)?),
        _ => {}
    }}
    if conservative { for (position, parameter) in function.parameters.iter().enumerate() { if carries(program, function.locals[parameter.index()].ty)? { escaped.insert(Origin::Parameter(position)); } } }
    let mut queue = escaped.iter().copied().collect::<VecDeque<_>>();
    while let Some(owner) = queue.pop_front() {
        for (_, child) in edges.iter().filter(|(parent, _)| *parent == owner) {
            if escaped.insert(*child) { queue.push_back(*child); }
        }
    }
    let summary = FunctionSummary { parameter_escapes: function.parameters.iter().enumerate().map(|(i, _)| escaped.contains(&Origin::Parameter(i))).collect(), conservative };
    let mut allocations = BTreeSet::new();
    for (bi, block) in function.blocks.iter().enumerate() { for (oi, operation) in block.operations.iter().enumerate() { if matches!(operation.kind, OperationKind::Aggregate { aggregate: Aggregate::Struct { .. }, .. }) {
        let allocation = AllocationId { function: id, block: BlockId::from_index(bi), operation: oi };
        let class = if !conservative && reachable.contains(&BlockId::from_index(bi)) && !escaped.contains(&Origin::Allocation(allocation)) { AllocationClass::Scoped } else { AllocationClass::Heap };
        allocations.insert((allocation, class));
    }}}
    Ok((summary, allocations))
}

fn add(destination: &mut Facts, values: Facts) { destination.extend(values); }
fn operand_facts(program: &ir::Program, function: &ir::Function, locals: &[Facts], operand: &Operand) -> Result<Facts, EscapeError> { match operand { Operand::Constant(_) => Ok(Facts::new()), Operand::Copy(place) => place_facts(program, function, locals, place) } }
fn place_facts(program: &ir::Program, function: &ir::Function, locals: &[Facts], place: &Place) -> Result<Facts, EscapeError> {
    let facts = locals[place.local.index()].clone();
    for projection in &place.projections { match projection {
        Projection::TupleField { .. } | Projection::StructField { storage: MemberStorage::Inline, .. } | Projection::StructField { storage: MemberStorage::Referenced, .. } => {},
        // A function containing a reference-bearing container is already
        // conservative; retaining the base fact here avoids inventing a
        // path-specific container model.
        Projection::ListIndex { .. } | Projection::MapIndex { .. } => {},
    }}
    let _ = (program, function); Ok(facts)
}
fn place_owner_facts(_program: &ir::Program, _function: &ir::Function, locals: &[Facts], place: &Place) -> Result<Facts, EscapeError> {
    if !place.projections.iter().any(|p| matches!(p, Projection::StructField { .. })) { return Ok(Facts::new()); }
    Ok(locals[place.local.index()].clone())
}
fn unknown_operation(program: &ir::Program, function: &ir::Function, op: &ir::Operation) -> Result<bool, EscapeError> {
    let type_carries = |ty| carries(program, ty);
    Ok(match &op.kind {
        OperationKind::Aggregate { aggregate: Aggregate::List { ty, .. } | Aggregate::Map { ty, .. }, .. } => type_carries(*ty)?,
        OperationKind::Builtin { receiver, arguments, .. } => operand_carries(program, function, receiver)? || arguments.iter().any(|v| operand_carries(program, function, v).unwrap_or(true)),
        OperationKind::BeginIteration { iterable } | OperationKind::EndIteration { iterable } => operand_carries(program, function, iterable)?,
        OperationKind::IterationValue { iterable, .. } => operand_carries(program, function, iterable)?,
        _ => false,
    })
}
fn operand_carries(program: &ir::Program, function: &ir::Function, operand: &Operand) -> Result<bool, EscapeError> { let ty = match operand { Operand::Constant(v) => v.ty, Operand::Copy(place) => place_type(program, function, place)? }; carries(program, ty) }
fn retention_unknown_type(program: &ir::Program, ty: TypeId) -> Result<bool, EscapeError> {
    retention_unknown_inner(program, ty, &mut BTreeSet::new())
}
fn retention_unknown_inner(program: &ir::Program, ty: TypeId, visiting: &mut BTreeSet<TypeId>) -> Result<bool, EscapeError> {
    if !visiting.insert(ty) { return Ok(false); }
    let unknown = match &program.types[ty.index()] {
        Type::Unit | Type::Primitive(_) => false,
        Type::List(_) | Type::Map { .. } => carries(program, ty)?,
        Type::Nominal(definition) => match &program.definitions[definition.index()].layout {
            DefinitionLayout::Struct(fields) => fields.iter().any(|field| retention_unknown_inner(program, field.ty, visiting).unwrap_or(true)),
            DefinitionLayout::Tuple(fields) => fields.iter().any(|field| retention_unknown_inner(program, *field, visiting).unwrap_or(true)),
            DefinitionLayout::Union(items) => items.iter().any(|item| retention_unknown_inner(program, item.payload, visiting).unwrap_or(true)),
        },
        Type::Union(items) => items.iter().any(|item| retention_unknown_inner(program, item.payload, visiting).unwrap_or(true)),
    };
    visiting.remove(&ty);
    Ok(unknown)
}
fn place_type(program: &ir::Program, function: &ir::Function, place: &Place) -> Result<TypeId, EscapeError> { let mut ty = function.locals[place.local.index()].ty; for p in &place.projections { ty = match p { Projection::StructField { definition, field, .. } => match &program.definitions[definition.index()].layout { DefinitionLayout::Struct(fields) => fields[field.index()].ty, _ => return Err(error(None,None,None,"struct projection has non-struct definition")) }, Projection::TupleField { definition, field } => match &program.definitions[definition.index()].layout { DefinitionLayout::Tuple(fields) => fields[field.index()], _ => return Err(error(None,None,None,"tuple projection has non-tuple definition")) }, Projection::ListIndex { .. } => match &program.types[ty.index()] { Type::List(item) => *item, _ => return Err(error(None,None,None,"list projection has non-list type")) }, Projection::MapIndex { .. } => match &program.types[ty.index()] { Type::Map { value, .. } => *value, _ => return Err(error(None,None,None,"map projection has non-map type")) } }; } Ok(ty) }
fn carries(program: &ir::Program, ty: TypeId) -> Result<bool, EscapeError> { carries_inner(program, ty, &mut BTreeSet::new()) }
fn carries_inner(program: &ir::Program, ty: TypeId, visiting: &mut BTreeSet<TypeId>) -> Result<bool, EscapeError> {
    if !visiting.insert(ty) { return Ok(false); }
    let value = match &program.types[ty.index()] {
        Type::Unit | Type::Primitive(_) => false,
        // A carrying container is deliberately visible to callers so the
        // operation-level fallback can make its function conservative.
        Type::List(element) => carries_inner(program, *element, visiting)?,
        Type::Map { key, value } => carries_inner(program,*key,visiting)? || carries_inner(program,*value,visiting)?,
        Type::Nominal(definition) => match &program.definitions[definition.index()].layout { DefinitionLayout::Struct(_) => true, DefinitionLayout::Tuple(fields) => fields.iter().any(|f| carries_inner(program,*f,visiting).unwrap_or(true)), DefinitionLayout::Union(items) => items.iter().any(|a| carries_inner(program,a.payload,visiting).unwrap_or(true)) },
        Type::Union(items) => items.iter().any(|a| carries_inner(program,a.payload,visiting).unwrap_or(true)),
    };
    visiting.remove(&ty); Ok(value)
}
fn reachable_blocks(function: &ir::Function) -> BTreeSet<BlockId> { let mut result = BTreeSet::new(); let mut queue = VecDeque::new(); queue.push_back(function.entry.expect("validated")); while let Some(block) = queue.pop_front() { if !result.insert(block) { continue; } let term = &function.blocks[block.index()].terminator.as_ref().expect("validated").kind; match term { TerminatorKind::Jump(target) => queue.push_back(*target), TerminatorKind::Branch { then_block, else_block, .. } => { queue.push_back(*then_block); queue.push_back(*else_block); }, TerminatorKind::Switch { targets, .. } => for (_, target) in targets { queue.push_back(*target); }, _ => {} } } result }
fn error(function: Option<FunctionId>, block: Option<BlockId>, site: Option<OperationSite>, message: impl Into<String>) -> EscapeError { EscapeError { function, block, site, message: message.into() } }

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn program(return_reference: bool) -> ir::Program {
        let mut program = ir::Program::new(PathBuf::from("escape.sao2"), 1);
        let unit = program.intern_type(Type::Unit);
        let integer = program.intern_type(Type::Primitive(ir::PrimitiveType::Int));
        program.intern_type(Type::Primitive(ir::PrimitiveType::Float));
        program.intern_type(Type::Primitive(ir::PrimitiveType::Str));
        program.intern_type(Type::Primitive(ir::PrimitiveType::Bool));
        program.intern_type(Type::Primitive(ir::PrimitiveType::Char));
        let mut point = ir::NominalDefinition::structure("Point");
        let field = point.add_struct_field("x", integer, MemberStorage::Inline);
        let definition = program.add_definition(point);
        let point_type = program.intern_type(Type::Nominal(definition));
        let result = if return_reference { point_type } else { unit };
        let mut function = ir::Function::new("main", result);
        let local = function.add_local(point_type, None, ir::LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        let location = program.intern_location(ir::ByteSpan::new(0, 0));
        let failure = program.intern_failure_site(ir::FailureSite { location, function: FunctionId::from_index(0), operation: ir::FailureOperation::StructAllocation, line: 1, column: 1 });
        function.blocks[block.index()].push(OperationKind::Aggregate { destination: local, aggregate: Aggregate::Struct { definition, fields: vec![(field, Operand::Constant(ir::Constant { ty: integer, value: ir::ConstantValue::Integer(1) }))], failure } }, location);
        function.blocks[block.index()].terminate(if return_reference { TerminatorKind::Return(Operand::Copy(Place::local(local))) } else { TerminatorKind::Return(Operand::Constant(ir::Constant { ty: unit, value: ir::ConstantValue::Unit })) }, location);
        let main = program.add_function(function);
        program.entry = Some(main);
        program
    }

    #[test]
    fn local_struct_allocation_is_scoped_but_returning_it_is_heap() {
        let local = analyze(&program(false)).unwrap();
        assert_eq!(local.allocations[0].1, AllocationClass::Scoped);
        let allocation = local.allocations[0].0;
        assert_eq!(local.allocation_class(allocation), Some(AllocationClass::Scoped));
        assert!(local.function_has_scoped(FunctionId::from_index(0)));
        assert_eq!(local.allocation_class(AllocationId {
            function: FunctionId::from_index(0), block: BlockId::from_index(0), operation: 1,
        }), None);
        assert!(!local.summaries[0].conservative);
        let returned = analyze(&program(true)).unwrap();
        assert_eq!(returned.allocations[0].1, AllocationClass::Heap);
        assert!(!returned.function_has_scoped(FunctionId::from_index(0)));
        assert_eq!(local.render(), analyze(&program(false)).unwrap().render());
    }
}
