//! C11 declarations and scalar function bodies generated solely from the validated owned IR.
//!
//! This backend is intentionally disconnected from the production compiler
//! until the milestone-7 activation stage.

use std::fmt::{self, Write as _};

use crate::ir::{
    self, BinaryOperator, ConstantValue, DefinitionId, DefinitionLayout, FailureOperation,
    FailureSiteId, Function, FunctionId, IntegerOperation, Intrinsic, LocalId, NumericConversion,
    OperationKind, OperationSite, Operand, Place, PrimitiveType, Projection, RuntimeCheck,
    TerminatorKind, Type, TypeId, UnaryOperator, ValidationError,
};

pub(crate) fn emit(program: &ir::Program) -> Result<String, CEmissionError> {
    program.validate().map_err(CEmissionError::InvalidIr)?;
    CapabilityValidator::new(program).validate()?;
    let definitions = LayoutPlanner::new(program).plan()?;
    Ok(Renderer::new(program, definitions).render())
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
        write!(formatter, "unsupported by the milestone-7 C backend")?;
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
                DefinitionLayout::Struct(_) => return self.unsupported("struct definitions"),
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
        if operation_has_projection(operation) {
            return self.unsupported("place projection");
        }
        match operation {
            OperationKind::Copy { .. } | OperationKind::Call { .. } => Ok(()),
            OperationKind::UnionInject { .. } | OperationKind::UnionTest { .. }
            | OperationKind::UnionPayload { .. } => self.unsupported("union operation"),
            OperationKind::Unary { operand, .. } => {
                self.require_scalar_operand(function, operand, "non-scalar unary operation")
            }
            OperationKind::Binary { left, .. } => {
                let ty = self.operand_type(function, left);
                if matches!(&self.program.types[ty.index()], Type::Primitive(PrimitiveType::Str)) {
                    self.unsupported("string comparison")
                } else if self.is_scalar(ty) {
                    Ok(())
                } else {
                    self.unsupported("non-scalar binary operation")
                }
            }
            OperationKind::Convert { .. } => Ok(()),
            OperationKind::Assign { .. } => Ok(()),
            OperationKind::Aggregate { .. } => self.unsupported("aggregate construction"),
            OperationKind::StringIndex { .. } => self.unsupported("string indexing"),
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
        match terminator {
            TerminatorKind::Jump(_) | TerminatorKind::Branch { .. }
            | TerminatorKind::Return(_)
            | TerminatorKind::Panic { .. } | TerminatorKind::Unreachable => Ok(()),
            TerminatorKind::Switch { .. } => self.unsupported("union switch"),
            TerminatorKind::ErrorPanic { payload, .. } => {
                match &self.program.types[self.operand_type(function, payload).index()] {
                    Type::Primitive(PrimitiveType::Int | PrimitiveType::Bool | PrimitiveType::Char) => Ok(()),
                    _ => self.unsupported("Error panic payload formatting"),
                }
            }
        }
    }

    fn storage_type(&self, ty: TypeId, description: &str) -> Result<(), CEmissionError> {
        match &self.program.types[ty.index()] {
            Type::Unit | Type::Primitive(_) => Ok(()),
            Type::Nominal(definition) => match &self.program.definitions[definition.index()].layout {
                DefinitionLayout::Tuple(_) | DefinitionLayout::Union(_) => Ok(()),
                DefinitionLayout::Struct(_) => self.unsupported(format!("{description} uses a struct")),
            },
            Type::Union(_) => Ok(()),
            Type::List(_) | Type::Map { .. } => self.unsupported(format!("{description} uses container storage")),
        }
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

    fn output_operand(&self, function: &Function, operand: &Operand) -> bool {
        matches!(&self.program.types[self.operand_type(function, operand).index()],
            Type::Primitive(PrimitiveType::Str | PrimitiveType::Int | PrimitiveType::Bool))
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

struct LayoutPlanner<'a> {
    program: &'a ir::Program,
    states: Vec<(AggregateId, VisitState)>,
    ordered: Vec<AggregateId>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum VisitState { Visiting, Complete }

impl<'a> LayoutPlanner<'a> {
    fn new(program: &'a ir::Program) -> Self {
        Self { program, states: Vec::new(), ordered: Vec::new() }
    }

    fn plan(mut self) -> Result<Vec<AggregateId>, CEmissionError> {
        let mut roots = self.program.definitions.iter().enumerate()
            .filter(|(_, definition)| !matches!(&definition.layout, DefinitionLayout::Struct(_)))
            .map(|(index, _)| AggregateId::Definition(DefinitionId::from_index(index)))
            .chain(self.program.types.iter().enumerate().filter_map(|(index, ty)| {
                matches!(ty, Type::Union(_)).then_some(AggregateId::AnonymousUnion(TypeId::from_index(index)))
            }))
            .collect::<Vec<_>>();
        roots.sort();
        for root in roots { self.visit(root)?; }
        Ok(self.ordered)
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
            Type::Nominal(definition) => Some(AggregateId::Definition(*definition)),
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
}

struct Renderer<'a> {
    program: &'a ir::Program,
    definitions: Vec<AggregateId>,
    strings: Vec<Vec<u8>>,
    output: String,
}

impl<'a> Renderer<'a> {
    fn new(program: &'a ir::Program, definitions: Vec<AggregateId>) -> Self {
        Self { program, definitions, strings: collect_strings(program), output: String::new() }
    }

    fn render(mut self) -> String {
        self.output.push_str("/* Generated by sao2. */\n\n");
        self.output.push_str(concat!(
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n",
            "#include <float.h>\n#include <inttypes.h>\n#include <math.h>\n",
            "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n",
            "#ifdef _WIN32\n#include <fcntl.h>\n#include <io.h>\n#endif\n\n",
        ));
        self.output.push_str("typedef struct { uint8_t value; } sao2_unit;\n");
        self.output.push_str("typedef struct { const unsigned char *bytes; size_t length; } sao2_string;\n");
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
        self.render_runtime_metadata();
        self.render_scalar_helpers();
        self.render_string_data();
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

    fn render_string_data(&mut self) {
        if self.strings.is_empty() { return; }
        self.output.push('\n');
        for index in 0..self.strings.len() {
            let bytes = &self.strings[index];
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
        }
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
        self.render_signature(id, function);
        self.output.push_str(" {\n");
        for (index, local) in function.locals.iter().enumerate() {
            let ty = local.ty;
            let c_ty = if self.is_entry_args(id, ty) { "sao2_args".to_owned() }
                else { self.c_type(ty) };
            let _ = writeln!(self.output, "    {c_ty} sao2_local_{index} = {{0}};");
        }
        for (position, local) in function.parameters.iter().enumerate() {
            let _ = writeln!(self.output, "    sao2_local_{} = sao2_arg_{position};", local.index());
        }
        let entry = function.entry.expect("validated function entry");
        let _ = writeln!(self.output, "    goto sao2_block_{};", entry.index());
        for (block_index, block) in function.blocks.iter().enumerate() {
            let _ = writeln!(self.output, "sao2_block_{block_index}:");
            for operation in &block.operations { self.render_operation(function, &operation.kind); }
            self.render_terminator(function, &block.terminator.as_ref().expect("validated block").kind);
        }
        self.output.push_str("}\n");
    }

    fn render_operation(&mut self, function: &Function, operation: &OperationKind) {
        match operation {
            OperationKind::Copy { destination, operand } => {
                let value = self.operand(operand);
                let _ = writeln!(self.output, "    sao2_local_{} = {value};", destination.index());
            }
            OperationKind::Unary { destination, operator, operand } => {
                let value = self.operand(operand);
                let expression = match operator {
                    UnaryOperator::LogicalNot => format!("!{value}"),
                    UnaryOperator::BitwiseNot => format!("sao2_int_from_bits(~sao2_int_to_bits({value}))"),
                    UnaryOperator::Plus => format!("+{value}"),
                    UnaryOperator::Minus => format!("-{value}"),
                };
                let _ = writeln!(self.output, "    sao2_local_{} = {expression};", destination.index());
            }
            OperationKind::Binary { destination, operator, left, right } => {
                let left_value = self.operand(left);
                let right_value = self.operand(right);
                let left_ty = self.operand_type(function, left);
                let expression = self.binary_expression(*operator, left_ty, &left_value, &right_value);
                let _ = writeln!(self.output, "    sao2_local_{} = {expression};", destination.index());
            }
            OperationKind::Convert { destination, conversion, operand } => {
                let value = self.operand(operand);
                let cast = match conversion { NumericConversion::IntToFloat => "double", NumericConversion::FloatToInt => "int64_t" };
                let _ = writeln!(self.output, "    sao2_local_{} = ({cast})({value});", destination.index());
            }
            OperationKind::Assign { destination, value } => {
                let value = self.operand(value);
                let _ = writeln!(self.output, "    sao2_local_{} = {value};", destination.local.index());
            }
            OperationKind::Call { destination, function: callee, arguments } => {
                let arguments = arguments.iter().map(|argument| self.operand(argument)).collect::<Vec<_>>().join(", ");
                let _ = writeln!(self.output, "    sao2_local_{} = sao2_fn_{}({arguments});", destination.index(), callee.index());
            }
            OperationKind::Intrinsic { destination, intrinsic, arguments, failure } => {
                self.render_intrinsic(function, *destination, *intrinsic, arguments, *failure);
            }
            OperationKind::Check(check) => self.render_check(function, check),
            OperationKind::Aggregate { .. } | OperationKind::UnionInject { .. }
            | OperationKind::UnionTest { .. } | OperationKind::UnionPayload { .. }
            | OperationKind::StringIndex { .. } | OperationKind::Builtin { .. }
            | OperationKind::BeginIteration { .. } | OperationKind::EndIteration { .. }
            | OperationKind::IterationValue { .. } => unreachable!("capability validation rejected operation"),
        }
    }

    fn render_terminator(&mut self, function: &Function, terminator: &TerminatorKind) {
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
                let _ = writeln!(self.output, "    return {value};");
            }
            TerminatorKind::Panic { message, failure } => {
                let message = self.operand(message);
                let _ = writeln!(self.output, "    sao2_panic({message}, {});", failure.index());
            }
            TerminatorKind::ErrorPanic { payload, failure } => {
                let payload_value = self.operand(payload);
                let helper = match &self.program.types[self.operand_type(function, payload).index()] {
                    Type::Primitive(PrimitiveType::Int) => "sao2_error_panic_int",
                    Type::Primitive(PrimitiveType::Bool) => "sao2_error_panic_bool",
                    Type::Primitive(PrimitiveType::Char) => "sao2_error_panic_char",
                    _ => unreachable!("capability validation rejected Error payload"),
                };
                let _ = writeln!(self.output, "    {helper}({payload_value}, {});", failure.index());
            }
            TerminatorKind::Unreachable => self.output.push_str("    sao2_compiler_invariant();\n"),
            TerminatorKind::Switch { .. } => unreachable!("capability validation rejected union switch"),
        }
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
        let _ = writeln!(self.output, "    sao2_prepare_output({site});");
        if let Some(argument) = arguments.first() {
            let value = self.operand(argument);
            match &self.program.types[self.operand_type(function, argument).index()] {
                Type::Primitive(PrimitiveType::Str) => {
                    let _ = writeln!(self.output, "    if (fwrite(({value}).bytes, 1, ({value}).length, stdout) != ({value}).length) sao2_output_failure({site});");
                }
                Type::Primitive(PrimitiveType::Int) => {
                    let _ = writeln!(self.output, "    if (fprintf(stdout, \"%\" PRId64, {value}) < 0) sao2_output_failure({site});");
                }
                Type::Primitive(PrimitiveType::Bool) => {
                    let _ = writeln!(self.output, "    if (({value}) ? fwrite(sao2_true_bytes, 1, 4, stdout) != 4 : fwrite(sao2_false_bytes, 1, 5, stdout) != 5) sao2_output_failure({site});");
                }
                _ => unreachable!("capability validation rejected output type"),
            }
        }
        if intrinsic == Intrinsic::Println {
            let _ = writeln!(self.output, "    if (fputc('\\n', stdout) == EOF) sao2_output_failure({site});");
        }
        let _ = writeln!(self.output, "    sao2_local_{} = (sao2_unit){{0}};", destination.index());
    }

    fn binary_expression(&self, operator: BinaryOperator, ty: TypeId, left: &str, right: &str) -> String {
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
            Operand::Copy(place) => format!("sao2_local_{}", place.local.index()),
            Operand::Constant(constant) => match &constant.value {
                ConstantValue::Unit => "(sao2_unit){0}".to_owned(),
                ConstantValue::Integer(value) if *value == i64::MIN =>
                    "(-INT64_C(9223372036854775807) - INT64_C(1))".to_owned(),
                ConstantValue::Integer(value) if *value < 0 => format!("(-INT64_C({}))", value.unsigned_abs()),
                ConstantValue::Integer(value) => format!("INT64_C({value})"),
                ConstantValue::Float(bits) => format!("sao2_float_from_bits(UINT64_C({bits}))"),
                ConstantValue::String(bytes) => {
                    let index = self.strings.iter().position(|candidate| candidate == bytes).expect("collected string");
                    format!("(sao2_string){{sao2_string_data_{index}, {}}}", bytes.len())
                }
                ConstantValue::Character(value) => format!("UINT8_C({value})"),
                ConstantValue::Boolean(value) => value.to_string(),
            },
        }
    }

    fn operand_type(&self, function: &Function, operand: &Operand) -> TypeId {
        match operand {
            Operand::Constant(constant) => constant.ty,
            Operand::Copy(place) => function.locals[place.local.index()].ty,
        }
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
            Type::Nominal(definition) => format!("sao2_def_{}", definition.index()),
            Type::Union(_) => format!("sao2_union_ty_{}", ty.index()),
            Type::List(_) | Type::Map { .. } => unreachable!("capability validation rejected container C type"),
        }
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
        FailureOperation::Output => "SAO2_FAILURE_OUTPUT",
        FailureOperation::ExplicitPanic => "SAO2_FAILURE_EXPLICIT_PANIC",
        FailureOperation::UnhandledError => "SAO2_FAILURE_UNHANDLED_ERROR",
    }
}

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

static void sao2_write_stderr_int(int64_t value) {
    if (fprintf(stderr, "%" PRId64, value) < 0) return;
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
    sao2_write_stderr(message.bytes, message.length);
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
    sao2_write_stderr_int(value);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_error_panic_bool(bool value, size_t site) {
    static const unsigned char true_bytes[] = "true";
    static const unsigned char false_bytes[] = "false";
    sao2_error_panic_begin(site);
    if (value) sao2_write_stderr(true_bytes, sizeof true_bytes - 1);
    else sao2_write_stderr(false_bytes, sizeof false_bytes - 1);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static _Noreturn void sao2_error_panic_char(uint8_t value, size_t site) {
    sao2_error_panic_begin(site);
    sao2_write_stderr(&value, 1);
    sao2_write_stderr_char(')');
    sao2_finish_panic(site);
}

static const unsigned char sao2_true_bytes[] = { UINT8_C(116), UINT8_C(114), UINT8_C(117), UINT8_C(101) };
static const unsigned char sao2_false_bytes[] = { UINT8_C(102), UINT8_C(97), UINT8_C(108), UINT8_C(115), UINT8_C(101) };

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

fn collect_strings(program: &ir::Program) -> Vec<Vec<u8>> {
    let mut strings = Vec::new();
    let mut collect = |operand: &Operand| {
        if let Operand::Constant(ir::Constant { value: ConstantValue::String(bytes), .. }) = operand
            && !strings.iter().any(|item| item == bytes)
        {
            strings.push(bytes.clone());
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

fn operation_has_projection(operation: &OperationKind) -> bool {
    let mut found = false;
    visit_operation_places(operation, &mut |place| found |= !place.projections.is_empty());
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        Aggregate, ByteSpan, Constant, ConstantValue, FailureOperation, FailureSite,
        LocalOrigin, NominalDefinition, Operation, UnionAlternative,
    };

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
        let main = program.add_function(function);
        program.entry = Some(main);

        let emitted = emit(&program).unwrap();
        assert!(emitted.starts_with(concat!(
            "/* Generated by sao2. */\n\n",
            "#include <stdbool.h>\n#include <stddef.h>\n#include <stdint.h>\n",
            "#include <float.h>\n#include <inttypes.h>\n#include <math.h>\n",
            "#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n",
        )));
        assert!(emitted.contains(concat!(
            "sao2_unit sao2_fn_0(sao2_unit sao2_arg_0, int64_t sao2_arg_1, ",
            "double sao2_arg_2, sao2_string sao2_arg_3, bool sao2_arg_4, ",
            "uint8_t sao2_arg_5);\n",
        )));
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
        let result = signature.add_local(anonymous, None, LocalOrigin::Temporary);
        let block = signature.add_block();
        signature.entry = Some(block);
        signature.blocks[block.index()].terminate(
            TerminatorKind::Return(Operand::Copy(Place::local(result))), location,
        );
        let first = program.add_function(signature);
        program.entry = Some(first);

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
    fn rejects_structs_containers_tuple_operations_and_entry_args_use() {
        let (mut structure, types, location) = program();
        let mut definition = NominalDefinition::structure("keyword while");
        definition.add_struct_field("field", types.int, ir::MemberStorage::Inline);
        let id = structure.add_definition(definition);
        structure.intern_type(Type::Nominal(id));
        add_main(&mut structure, types, location);
        assert_unsupported(emit(&structure), "struct definitions", None);

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
        assert_unsupported(emit(&tuple_program), "aggregate construction", Some(OperationSite::Operation(0)));

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
    fn rejects_projection_string_index_unsupported_printing_builtins_and_terminators() {
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
        assert_unsupported(emit(&projection_program), "place projection", Some(OperationSite::Operation(0)));

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
        assert_unsupported(emit(&index_program), "string indexing", Some(OperationSite::Operation(0)));

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
        assert_unsupported(emit(&print_program), "printing this value type", Some(OperationSite::Operation(0)));

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
        assert_unsupported(
            emit(&panic_program),
            "Error panic payload formatting",
            Some(OperationSite::Terminator),
        );
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
        assert!(emitted.contains("(-INT64_C(9223372036854775807) - INT64_C(1))"));
        assert!(emitted.contains("sao2_float_from_bits(UINT64_C(9223372036854775808))"));
        assert!(emitted.contains("(sao2_string){sao2_string_data_1, 0}"));
        assert!(emitted.contains("UINT8_C(127)"));
        assert!(emitted.contains("sao2_local_0 = sao2_fn_0((-INT64_C(7)));"));
        assert!(!emitted.contains("source names never appear"));
        assert!(!emitted.contains("while"));
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
        assert!(emitted.contains("#ifdef _WIN32\n#include <fcntl.h>\n#include <io.h>\n#endif"));
        assert!(emitted.contains("fwrite(((sao2_string){sao2_string_data_0, 2}).bytes, 1"));
        assert!(emitted.contains("fprintf(stdout, \"%\" PRId64, INT64_C(12)) < 0"));
        assert!(emitted.contains("fwrite(sao2_true_bytes, 1, 4, stdout) != 4"));
        assert_eq!(emitted.matches("if (fputc('\\n', stdout) == EOF) sao2_output_failure(0);").count(), 3);
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
        assert!(emitted.contains("#include <float.h>\n#include <inttypes.h>\n#include <math.h>\n"));
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
        assert!(emitted.contains("sao2_panic((sao2_string){sao2_string_data_0, 3}, 0);"));
        assert!(emitted.contains("sao2_error_panic_char(UINT8_C(10), 1);"));
        assert!(emitted.contains("{ SAO2_FAILURE_EXPLICIT_PANIC, 0, 2, 3 },"));
        assert!(emitted.contains("{ SAO2_FAILURE_UNHANDLED_ERROR, 0, 4, 5 },"));
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
