//! Owned typed control-flow IR.
//!
//! Lowering is the sole owned construction boundary for this IR. Milestone 6
//! validates and retains it alongside the temporary resolved-AST backend;
//! milestone 7 replaces that backend with IR-based C generation. No IR value
//! borrows the frontend.

use std::collections::HashSet;
use std::fmt::{self, Write as _};
use std::path::PathBuf;

macro_rules! index_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub(crate) struct $name(usize);

        impl $name {
            pub(crate) fn index(self) -> usize { self.0 }
            pub(crate) fn from_index(index: usize) -> Self { Self(index) }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }
    };
}

index_id!(TypeId, "ty");
index_id!(DefinitionId, "def");
index_id!(FunctionId, "fn");
index_id!(LocalId, "_");
index_id!(BlockId, "bb");
index_id!(FieldId, "field");
index_id!(AlternativeId, "alt");
index_id!(LocationId, "loc");
index_id!(FailureSiteId, "fail");

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceMetadata {
    pub(crate) filename: PathBuf,
    pub(crate) byte_len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ByteSpan {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl ByteSpan {
    pub(crate) fn new(start: usize, end: usize) -> Self { Self { start, end } }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum PrimitiveType { Int, Float, Str, Bool, Char }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Type {
    Unit,
    Primitive(PrimitiveType),
    List(TypeId),
    Map { key: TypeId, value: TypeId },
    Nominal(DefinitionId),
    Union(Vec<UnionAlternative>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum MemberStorage { Inline, Referenced }

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AlternativeConstructor {
    Untagged,
    Tagged(String),
    Error,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct UnionAlternative {
    pub(crate) constructor: AlternativeConstructor,
    pub(crate) payload: TypeId,
}

impl UnionAlternative {
    pub(crate) fn untagged(payload: TypeId) -> Self {
        Self { constructor: AlternativeConstructor::Untagged, payload }
    }

    pub(crate) fn tagged(tag: impl Into<String>, payload: TypeId) -> Self {
        Self { constructor: AlternativeConstructor::Tagged(tag.into()), payload }
    }

    pub(crate) fn error(payload: TypeId) -> Self {
        Self { constructor: AlternativeConstructor::Error, payload }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StructField {
    pub(crate) name: String,
    pub(crate) ty: TypeId,
    pub(crate) storage: MemberStorage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DefinitionLayout {
    Struct(Vec<StructField>),
    Tuple(Vec<TypeId>),
    Union(Vec<UnionAlternative>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NominalDefinition {
    pub(crate) name: String,
    pub(crate) layout: DefinitionLayout,
}

impl NominalDefinition {
    pub(crate) fn structure(name: impl Into<String>) -> Self {
        Self { name: name.into(), layout: DefinitionLayout::Struct(Vec::new()) }
    }

    pub(crate) fn tuple(name: impl Into<String>) -> Self {
        Self { name: name.into(), layout: DefinitionLayout::Tuple(Vec::new()) }
    }

    pub(crate) fn union(name: impl Into<String>) -> Self {
        Self { name: name.into(), layout: DefinitionLayout::Union(Vec::new()) }
    }

    pub(crate) fn add_struct_field(
        &mut self,
        name: impl Into<String>,
        ty: TypeId,
        storage: MemberStorage,
    ) -> FieldId {
        let DefinitionLayout::Struct(fields) = &mut self.layout else {
            panic!("cannot add a struct field to a non-struct definition")
        };
        let id = FieldId(fields.len());
        fields.push(StructField { name: name.into(), ty, storage });
        id
    }

    pub(crate) fn add_tuple_field(&mut self, ty: TypeId) -> FieldId {
        let DefinitionLayout::Tuple(fields) = &mut self.layout else {
            panic!("cannot add a tuple field to a non-tuple definition")
        };
        let id = FieldId(fields.len());
        fields.push(ty);
        id
    }

    pub(crate) fn add_alternative(&mut self, alternative: UnionAlternative) -> AlternativeId {
        let DefinitionLayout::Union(alternatives) = &mut self.layout else {
            panic!("cannot add a union alternative to a non-union definition")
        };
        let id = AlternativeId(alternatives.len());
        alternatives.push(alternative);
        id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalOrigin { Parameter, Binding, Temporary }

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Local {
    pub(crate) ty: TypeId,
    pub(crate) source_name: Option<String>,
    pub(crate) origin: LocalOrigin,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Function {
    pub(crate) name: String,
    pub(crate) parameters: Vec<LocalId>,
    pub(crate) result: TypeId,
    pub(crate) locals: Vec<Local>,
    pub(crate) entry: Option<BlockId>,
    pub(crate) blocks: Vec<BasicBlock>,
}

impl Function {
    pub(crate) fn new(name: impl Into<String>, result: TypeId) -> Self {
        Self {
            name: name.into(), parameters: Vec::new(), result,
            locals: Vec::new(), entry: None, blocks: Vec::new(),
        }
    }

    pub(crate) fn add_local(
        &mut self,
        ty: TypeId,
        source_name: Option<String>,
        origin: LocalOrigin,
    ) -> LocalId {
        let id = LocalId(self.locals.len());
        self.locals.push(Local { ty, source_name, origin });
        if origin == LocalOrigin::Parameter { self.parameters.push(id); }
        id
    }

    pub(crate) fn add_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len());
        self.blocks.push(BasicBlock::new());
        id
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct BasicBlock {
    pub(crate) operations: Vec<Operation>,
    pub(crate) terminator: Option<Terminator>,
}

impl BasicBlock {
    pub(crate) fn new() -> Self { Self::default() }
    pub(crate) fn push(&mut self, kind: OperationKind, location: LocationId) {
        self.operations.push(Operation { kind, location });
    }
    pub(crate) fn terminate(&mut self, kind: TerminatorKind, location: LocationId) {
        self.terminator = Some(Terminator { kind, location });
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Place {
    pub(crate) local: LocalId,
    pub(crate) projections: Vec<Projection>,
}

impl Place {
    pub(crate) fn local(local: LocalId) -> Self { Self { local, projections: Vec::new() } }
    pub(crate) fn projected(local: LocalId, projections: Vec<Projection>) -> Self {
        Self { local, projections }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Projection {
    StructField { definition: DefinitionId, field: FieldId, storage: MemberStorage },
    TupleField { definition: DefinitionId, field: FieldId },
    ListIndex { index: LocalId, failure: FailureSiteId },
    MapIndex { key: LocalId, failure: FailureSiteId },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ConstantValue {
    Unit,
    Integer(i64),
    Float(u64),
    String(Vec<u8>),
    Character(u8),
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Constant {
    pub(crate) ty: TypeId,
    pub(crate) value: ConstantValue,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Operand { Constant(Constant), Copy(Place) }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnaryOperator { LogicalNot, BitwiseNot, Plus, Minus }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BinaryOperator {
    BitwiseOr, BitwiseXor, BitwiseAnd, Equal, NotEqual, Less, LessEqual,
    Greater, GreaterEqual, In, ShiftLeft, ShiftRight, Add, Subtract, Multiply,
    Divide, Remainder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NumericConversion { IntToFloat, FloatToInt }

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Aggregate {
    Struct { definition: DefinitionId, fields: Vec<(FieldId, Operand)> },
    Tuple { definition: DefinitionId, elements: Vec<Operand> },
    List { ty: TypeId, elements: Vec<Operand> },
    Map { ty: TypeId, entries: Vec<(Operand, Operand)> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Intrinsic { Print, Println }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BuiltinMethod {
    ListAppend, ListRemoveIndex, ListLen, MapRemoveKey, MapLen, StrLen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IntegerOperation { Add, Subtract, Multiply, ShiftLeft }

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FailureOperation {
    IntegerAdd, IntegerSubtract, IntegerMultiply, IntegerNegation,
    IntegerDivision, IntegerRemainder, ShiftLeft, ShiftRight,
    FloatAdd, FloatSubtract, FloatMultiply, FloatDivision, FloatToInt,
    ListIndex, MapIndex, StringIndex, ListAppend, ListRemoveIndex,
    MapRemoveKey, Output, ExplicitPanic, UnhandledError,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct FailureSite {
    pub(crate) location: LocationId,
    pub(crate) function: FunctionId,
    pub(crate) operation: FailureOperation,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RuntimeCheck {
    IntegerOverflow { operation: IntegerOperation, left: Operand, right: Operand, failure: FailureSiteId },
    IntegerNegation { operand: Operand, failure: FailureSiteId },
    Division { left: Operand, right: Operand, failure: FailureSiteId },
    Remainder { left: Operand, right: Operand, failure: FailureSiteId },
    ShiftRange { amount: Operand, failure: FailureSiteId },
    FiniteFloat { operand: Operand, failure: FailureSiteId },
    NumericConversion { operand: Operand, conversion: NumericConversion, failure: FailureSiteId },
    IterationUnlocked { receiver: Operand, failure: FailureSiteId },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Operation {
    pub(crate) kind: OperationKind,
    pub(crate) location: LocationId,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum OperationKind {
    Copy { destination: LocalId, operand: Operand },
    Unary { destination: LocalId, operator: UnaryOperator, operand: Operand },
    Binary { destination: LocalId, operator: BinaryOperator, left: Operand, right: Operand },
    Convert { destination: LocalId, conversion: NumericConversion, operand: Operand },
    Aggregate { destination: LocalId, aggregate: Aggregate },
    UnionInject { destination: LocalId, union_type: TypeId, alternative: AlternativeId, payload: Operand },
    UnionTest { destination: LocalId, union: Operand, alternative: AlternativeId },
    UnionPayload { destination: LocalId, union: Operand, alternative: AlternativeId },
    StringIndex { destination: LocalId, string: Operand, index: Operand, failure: FailureSiteId },
    Assign { destination: Place, value: Operand },
    Call { destination: LocalId, function: FunctionId, arguments: Vec<Operand> },
    Intrinsic { destination: LocalId, intrinsic: Intrinsic, arguments: Vec<Operand>, failure: FailureSiteId },
    Builtin { destination: LocalId, method: BuiltinMethod, receiver: Operand, arguments: Vec<Operand>, failure: Option<FailureSiteId> },
    BeginIteration { iterable: Operand },
    EndIteration { iterable: Operand },
    IterationValue { destination: LocalId, iterable: Operand, index: Operand },
    Check(RuntimeCheck),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Terminator {
    pub(crate) kind: TerminatorKind,
    pub(crate) location: LocationId,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TerminatorKind {
    Jump(BlockId),
    Branch { condition: Operand, then_block: BlockId, else_block: BlockId },
    Switch { union: Operand, targets: Vec<(AlternativeId, BlockId)> },
    Return(Operand),
    Panic { message: Operand, failure: FailureSiteId },
    ErrorPanic { payload: Operand, failure: FailureSiteId },
    Unreachable,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Program {
    pub(crate) source: SourceMetadata,
    pub(crate) types: Vec<Type>,
    pub(crate) definitions: Vec<NominalDefinition>,
    pub(crate) functions: Vec<Function>,
    pub(crate) entry: Option<FunctionId>,
    pub(crate) locations: Vec<ByteSpan>,
    pub(crate) failure_sites: Vec<FailureSite>,
}

impl Program {
    pub(crate) fn new(filename: impl Into<PathBuf>, byte_len: usize) -> Self {
        Self {
            source: SourceMetadata { filename: filename.into(), byte_len },
            types: Vec::new(), definitions: Vec::new(), functions: Vec::new(),
            entry: None, locations: Vec::new(), failure_sites: Vec::new(),
        }
    }

    pub(crate) fn intern_type(&mut self, ty: Type) -> TypeId {
        if let Some(index) = self.types.iter().position(|item| item == &ty) {
            return TypeId(index);
        }
        let id = TypeId(self.types.len());
        self.types.push(ty);
        id
    }

    pub(crate) fn add_definition(&mut self, definition: NominalDefinition) -> DefinitionId {
        let id = DefinitionId(self.definitions.len());
        self.definitions.push(definition);
        id
    }

    pub(crate) fn add_function(&mut self, function: Function) -> FunctionId {
        let id = FunctionId(self.functions.len());
        self.functions.push(function);
        id
    }

    pub(crate) fn intern_location(&mut self, span: ByteSpan) -> LocationId {
        if let Some(index) = self.locations.iter().position(|item| item == &span) {
            return LocationId(index);
        }
        let id = LocationId(self.locations.len());
        self.locations.push(span);
        id
    }

    pub(crate) fn intern_failure_site(&mut self, site: FailureSite) -> FailureSiteId {
        if let Some(index) = self.failure_sites.iter().position(|item| item.location == site.location && item.function == site.function && item.operation == site.operation) {
            return FailureSiteId(index);
        }
        let id = FailureSiteId(self.failure_sites.len());
        self.failure_sites.push(site);
        id
    }

    pub(crate) fn validate(&self) -> Result<(), ValidationError> { Validator::new(self).validate() }

    pub(crate) fn render(&self) -> String {
        let mut output = String::new();
        Renderer { program: self, output: &mut output }.render();
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationSite { Operation(usize), Terminator }

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidationError {
    pub(crate) function: Option<FunctionId>,
    pub(crate) block: Option<BlockId>,
    pub(crate) site: Option<OperationSite>,
    pub(crate) message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid IR")?;
        if let Some(function) = self.function { write!(f, " in {function}")?; }
        if let Some(block) = self.block { write!(f, " {block}")?; }
        match self.site {
            Some(OperationSite::Operation(index)) => write!(f, " op{index}")?,
            Some(OperationSite::Terminator) => write!(f, " terminator")?,
            None => {}
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for ValidationError {}

struct Validator<'a> {
    program: &'a Program,
    function: Option<FunctionId>,
    block: Option<BlockId>,
    site: Option<OperationSite>,
}

struct Renderer<'a, 'out> {
    program: &'a Program,
    output: &'out mut String,
}

impl Renderer<'_, '_> {
    fn render(&mut self) {
        let _ = writeln!(self.output, "source \"{}\" bytes {}", escape_text(&self.program.source.filename.to_string_lossy()), self.program.source.byte_len);
        for (index, span) in self.program.locations.iter().enumerate() {
            let _ = writeln!(self.output, "  loc{index} = {}..{}", span.start, span.end);
        }
        for (index, site) in self.program.failure_sites.iter().enumerate() {
            let _ = writeln!(self.output, "  fail{index} = {} {} {} at {}:{}", site.location, site.function, failure_operation_name(site.operation), site.line, site.column);
        }
        for (index, ty) in self.program.types.iter().enumerate() {
            let _ = writeln!(self.output, "type ty{index} = {}", self.ty(ty));
        }
        for (index, definition) in self.program.definitions.iter().enumerate() {
            let kind = match &definition.layout { DefinitionLayout::Struct(_) => "struct", DefinitionLayout::Tuple(_) => "tuple", DefinitionLayout::Union(_) => "union" };
            let _ = writeln!(self.output, "{kind} def{index} \"{}\" {{", escape_text(&definition.name));
            match &definition.layout {
                DefinitionLayout::Struct(fields) => for (field, value) in fields.iter().enumerate() {
                    let _ = writeln!(self.output, "  field{field} \"{}\": {} {}", escape_text(&value.name), value.ty, storage_name(value.storage));
                },
                DefinitionLayout::Tuple(fields) => for (field, ty) in fields.iter().enumerate() {
                    let _ = writeln!(self.output, "  field{field}: {ty}");
                },
                DefinitionLayout::Union(alternatives) => for (alternative, value) in alternatives.iter().enumerate() {
                    let _ = writeln!(self.output, "  alt{alternative}: {}", self.alternative(value));
                },
            }
            let _ = writeln!(self.output, "}}");
        }
        let entry = self.program.entry.map_or_else(|| "<unset>".to_owned(), |id| id.to_string());
        let _ = writeln!(self.output, "entry {entry}");
        for (index, function) in self.program.functions.iter().enumerate() {
            let parameters = function.parameters.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
            let entry = function.entry.map_or_else(|| "<unset>".to_owned(), |id| id.to_string());
            let _ = writeln!(self.output, "fn fn{index} \"{}\"({parameters}) -> {} entry {entry} {{", escape_text(&function.name), function.result);
            for (local, value) in function.locals.iter().enumerate() {
                let name = value.source_name.as_ref().map_or_else(|| "-".to_owned(), |name| format!("\"{}\"", escape_text(name)));
                let _ = writeln!(self.output, "  _{local}: {} {} {name}", value.ty, origin_name(value.origin));
            }
            for (block, value) in function.blocks.iter().enumerate() {
                let _ = writeln!(self.output, "  bb{block}:");
                for operation in &value.operations {
                    let _ = writeln!(self.output, "    {} @ {}", self.operation(&operation.kind), operation.location);
                }
                match &value.terminator {
                    Some(terminator) => { let _ = writeln!(self.output, "    {} @ {}", self.terminator(&terminator.kind), terminator.location); }
                    None => { let _ = writeln!(self.output, "    <unterminated>"); }
                }
            }
            let _ = writeln!(self.output, "}}");
        }
    }

    fn ty(&self, ty: &Type) -> String {
        match ty {
            Type::Unit => "unit".to_owned(),
            Type::Primitive(value) => primitive_name(*value).to_owned(),
            Type::List(element) => format!("list<{element}>"),
            Type::Map { key, value } => format!("map<{key}, {value}>"),
            Type::Nominal(definition) => format!("nominal {definition}"),
            Type::Union(alternatives) => format!("union [{}]", alternatives.iter().map(|item| self.alternative(item)).collect::<Vec<_>>().join(", ")),
        }
    }

    fn alternative(&self, alternative: &UnionAlternative) -> String {
        match &alternative.constructor {
            AlternativeConstructor::Untagged => format!("untagged({})", alternative.payload),
            AlternativeConstructor::Tagged(tag) => format!("tagged \"{}\"({})", escape_text(tag), alternative.payload),
            AlternativeConstructor::Error => format!("Error({})", alternative.payload),
        }
    }

    fn operation(&self, operation: &OperationKind) -> String {
        match operation {
            OperationKind::Copy { destination, operand } => format!("{destination} = copy {}", self.operand(operand)),
            OperationKind::Unary { destination, operator, operand } => format!("{destination} = {} {}", unary_name(*operator), self.operand(operand)),
            OperationKind::Binary { destination, operator, left, right } => format!("{destination} = {} {}, {}", binary_name(*operator), self.operand(left), self.operand(right)),
            OperationKind::Convert { destination, conversion, operand } => format!("{destination} = {} {}", conversion_name(*conversion), self.operand(operand)),
            OperationKind::Aggregate { destination, aggregate } => format!("{destination} = {}", self.aggregate(aggregate)),
            OperationKind::UnionInject { destination, union_type, alternative, payload } => format!("{destination} = inject {union_type}.{alternative} {}", self.operand(payload)),
            OperationKind::UnionTest { destination, union, alternative } => format!("{destination} = union-test {alternative} {}", self.operand(union)),
            OperationKind::UnionPayload { destination, union, alternative } => format!("{destination} = payload {alternative} {}", self.operand(union)),
            OperationKind::StringIndex { destination, string, index, failure } => format!("{destination} = string-index {}, {} ! {failure}", self.operand(string), self.operand(index)),
            OperationKind::Assign { destination, value } => format!("assign {} = {}", self.place(destination), self.operand(value)),
            OperationKind::Call { destination, function, arguments } => format!("{destination} = call {function}({})", self.operands(arguments)),
            OperationKind::Intrinsic { destination, intrinsic, arguments, failure } => format!("{destination} = intrinsic {}({}) ! {failure}", intrinsic_name(*intrinsic), self.operands(arguments)),
            OperationKind::Builtin { destination, method, receiver, arguments, failure } => {
                let failure = failure.map_or_else(String::new, |site| format!(" ! {site}"));
                format!("{destination} = builtin {} {}({}){failure}", builtin_name(*method), self.operand(receiver), self.operands(arguments))
            }
            OperationKind::BeginIteration { iterable } => format!("begin-iteration {}", self.operand(iterable)),
            OperationKind::EndIteration { iterable } => format!("end-iteration {}", self.operand(iterable)),
            OperationKind::IterationValue { destination, iterable, index } => format!("{destination} = iteration-value {}, {}", self.operand(iterable), self.operand(index)),
            OperationKind::Check(check) => format!("check {}", self.check(check)),
        }
    }

    fn aggregate(&self, aggregate: &Aggregate) -> String {
        match aggregate {
            Aggregate::Struct { definition, fields } => format!("struct {definition} {{{}}}", fields.iter().map(|(field, operand)| format!("{field}: {}", self.operand(operand))).collect::<Vec<_>>().join(", ")),
            Aggregate::Tuple { definition, elements } => format!("tuple {definition}({})", self.operands(elements)),
            Aggregate::List { ty, elements } => format!("list {ty}[{}]", self.operands(elements)),
            Aggregate::Map { ty, entries } => format!("map {ty} {{{}}}", entries.iter().map(|(key, value)| format!("{}: {}", self.operand(key), self.operand(value))).collect::<Vec<_>>().join(", ")),
        }
    }

    fn check(&self, check: &RuntimeCheck) -> String {
        match check {
            RuntimeCheck::IntegerOverflow { operation, left, right, failure } => format!("integer-{} {}, {} ! {failure}", integer_operation_name(*operation), self.operand(left), self.operand(right)),
            RuntimeCheck::IntegerNegation { operand, failure } => format!("integer-negation {} ! {failure}", self.operand(operand)),
            RuntimeCheck::Division { left, right, failure } => format!("division {}, {} ! {failure}", self.operand(left), self.operand(right)),
            RuntimeCheck::Remainder { left, right, failure } => format!("remainder {}, {} ! {failure}", self.operand(left), self.operand(right)),
            RuntimeCheck::ShiftRange { amount, failure } => format!("shift-range {} ! {failure}", self.operand(amount)),
            RuntimeCheck::FiniteFloat { operand, failure } => format!("finite-float {} ! {failure}", self.operand(operand)),
            RuntimeCheck::NumericConversion { operand, conversion, failure } => format!("numeric-{} {} ! {failure}", conversion_name(*conversion), self.operand(operand)),
            RuntimeCheck::IterationUnlocked { receiver, failure } => format!("iteration-unlocked {} ! {failure}", self.operand(receiver)),
        }
    }

    fn terminator(&self, terminator: &TerminatorKind) -> String {
        match terminator {
            TerminatorKind::Jump(target) => format!("jump {target}"),
            TerminatorKind::Branch { condition, then_block, else_block } => format!("branch {} -> {then_block}, {else_block}", self.operand(condition)),
            TerminatorKind::Switch { union, targets } => format!("switch {} -> [{}]", self.operand(union), targets.iter().map(|(alternative, block)| format!("{alternative}: {block}")).collect::<Vec<_>>().join(", ")),
            TerminatorKind::Return(value) => format!("return {}", self.operand(value)),
            TerminatorKind::Panic { message, failure } => format!("panic {} ! {failure}", self.operand(message)),
            TerminatorKind::ErrorPanic { payload, failure } => format!("error-panic {} ! {failure}", self.operand(payload)),
            TerminatorKind::Unreachable => "unreachable".to_owned(),
        }
    }

    fn operands(&self, operands: &[Operand]) -> String {
        operands.iter().map(|item| self.operand(item)).collect::<Vec<_>>().join(", ")
    }

    fn operand(&self, operand: &Operand) -> String {
        match operand {
            Operand::Copy(place) => format!("copy {}", self.place(place)),
            Operand::Constant(constant) => match &constant.value {
                ConstantValue::Unit => format!("const {} ()", constant.ty),
                ConstantValue::Integer(value) => format!("const {} {value}", constant.ty),
                ConstantValue::Float(bits) => format!("const {} 0x{bits:016x}", constant.ty),
                ConstantValue::String(bytes) => format!("const {} b\"{}\"", constant.ty, escape_bytes(bytes)),
                ConstantValue::Character(byte) => format!("const {} b'{}'", constant.ty, escape_bytes(&[*byte])),
                ConstantValue::Boolean(value) => format!("const {} {value}", constant.ty),
            },
        }
    }

    fn place(&self, place: &Place) -> String {
        let mut rendered = place.local.to_string();
        for projection in &place.projections {
            match projection {
                Projection::StructField { definition, field, storage } => { let _ = write!(rendered, ".{definition}.{field}:{}", storage_name(*storage)); }
                Projection::TupleField { definition, field } => { let _ = write!(rendered, ".{definition}.{field}"); }
                Projection::ListIndex { index, failure } => { let _ = write!(rendered, "[{index} ! {failure}]"); }
                Projection::MapIndex { key, failure } => { let _ = write!(rendered, "[key {key} ! {failure}]"); }
            }
        }
        rendered
    }
}

fn escape_text(text: &str) -> String { escape_bytes(text.as_bytes()) }

fn escape_bytes(bytes: &[u8]) -> String {
    let mut output = String::new();
    for byte in bytes {
        match byte {
            b'\\' => output.push_str("\\\\"),
            b'\"' => output.push_str("\\\""),
            b'\'' => output.push_str("\\'"),
            b'\n' => output.push_str("\\n"),
            b'\r' => output.push_str("\\r"),
            b'\t' => output.push_str("\\t"),
            0 => output.push_str("\\0"),
            0x20..=0x7e => output.push(char::from(*byte)),
            _ => { let _ = write!(output, "\\x{byte:02x}"); }
        }
    }
    output
}

fn primitive_name(value: PrimitiveType) -> &'static str { match value { PrimitiveType::Int => "int", PrimitiveType::Float => "float", PrimitiveType::Str => "str", PrimitiveType::Bool => "bool", PrimitiveType::Char => "char" } }
fn storage_name(value: MemberStorage) -> &'static str { match value { MemberStorage::Inline => "inline", MemberStorage::Referenced => "referenced" } }
fn origin_name(value: LocalOrigin) -> &'static str { match value { LocalOrigin::Parameter => "parameter", LocalOrigin::Binding => "binding", LocalOrigin::Temporary => "temporary" } }
fn unary_name(value: UnaryOperator) -> &'static str { match value { UnaryOperator::LogicalNot => "not", UnaryOperator::BitwiseNot => "bit-not", UnaryOperator::Plus => "positive", UnaryOperator::Minus => "negative" } }
fn binary_name(value: BinaryOperator) -> &'static str { match value { BinaryOperator::BitwiseOr => "bit-or", BinaryOperator::BitwiseXor => "bit-xor", BinaryOperator::BitwiseAnd => "bit-and", BinaryOperator::Equal => "equal", BinaryOperator::NotEqual => "not-equal", BinaryOperator::Less => "less", BinaryOperator::LessEqual => "less-equal", BinaryOperator::Greater => "greater", BinaryOperator::GreaterEqual => "greater-equal", BinaryOperator::In => "in", BinaryOperator::ShiftLeft => "shift-left", BinaryOperator::ShiftRight => "shift-right", BinaryOperator::Add => "add", BinaryOperator::Subtract => "subtract", BinaryOperator::Multiply => "multiply", BinaryOperator::Divide => "divide", BinaryOperator::Remainder => "remainder" } }
fn conversion_name(value: NumericConversion) -> &'static str { match value { NumericConversion::IntToFloat => "int-to-float", NumericConversion::FloatToInt => "float-to-int" } }
fn intrinsic_name(value: Intrinsic) -> &'static str { match value { Intrinsic::Print => "print", Intrinsic::Println => "println" } }
fn builtin_name(value: BuiltinMethod) -> &'static str { match value { BuiltinMethod::ListAppend => "list.append", BuiltinMethod::ListRemoveIndex => "list.remove-index", BuiltinMethod::ListLen => "list.len", BuiltinMethod::MapRemoveKey => "map.remove-key", BuiltinMethod::MapLen => "map.len", BuiltinMethod::StrLen => "str.len" } }
fn integer_operation_name(value: IntegerOperation) -> &'static str { match value { IntegerOperation::Add => "add-overflow", IntegerOperation::Subtract => "subtract-overflow", IntegerOperation::Multiply => "multiply-overflow", IntegerOperation::ShiftLeft => "shift-left-overflow" } }
fn failure_operation_name(value: FailureOperation) -> &'static str { match value {
    FailureOperation::IntegerAdd => "integer-add", FailureOperation::IntegerSubtract => "integer-subtract",
    FailureOperation::IntegerMultiply => "integer-multiply", FailureOperation::IntegerNegation => "integer-negation",
    FailureOperation::IntegerDivision => "integer-division", FailureOperation::IntegerRemainder => "integer-remainder",
    FailureOperation::ShiftLeft => "shift-left", FailureOperation::ShiftRight => "shift-right",
    FailureOperation::FloatAdd => "float-add", FailureOperation::FloatSubtract => "float-subtract",
    FailureOperation::FloatMultiply => "float-multiply", FailureOperation::FloatDivision => "float-division",
    FailureOperation::FloatToInt => "float-to-int", FailureOperation::ListIndex => "list-index",
    FailureOperation::MapIndex => "map-index", FailureOperation::StringIndex => "string-index",
    FailureOperation::ListAppend => "list-append", FailureOperation::ListRemoveIndex => "list-remove-index",
    FailureOperation::MapRemoveKey => "map-remove-key", FailureOperation::Output => "output",
    FailureOperation::ExplicitPanic => "explicit-panic", FailureOperation::UnhandledError => "unhandled-error",
} }
fn runtime_check_failure(check: &RuntimeCheck) -> FailureSiteId { match check {
    RuntimeCheck::IntegerOverflow { failure, .. } | RuntimeCheck::IntegerNegation { failure, .. }
    | RuntimeCheck::Division { failure, .. } | RuntimeCheck::Remainder { failure, .. }
    | RuntimeCheck::ShiftRange { failure, .. } | RuntimeCheck::FiniteFloat { failure, .. }
    | RuntimeCheck::NumericConversion { failure, .. } | RuntimeCheck::IterationUnlocked { failure, .. } => *failure,
} }

#[cfg(test)]
mod tests {
    use super::*;

    struct CoreTypes {
        unit: TypeId,
        int: TypeId,
        float: TypeId,
        string: TypeId,
        boolean: TypeId,
        character: TypeId,
    }

    fn program() -> (Program, CoreTypes, LocationId) {
        let mut program = Program::new(PathBuf::from("test.sao2"), 100);
        let types = CoreTypes {
            unit: program.intern_type(Type::Unit),
            int: program.intern_type(Type::Primitive(PrimitiveType::Int)),
            float: program.intern_type(Type::Primitive(PrimitiveType::Float)),
            string: program.intern_type(Type::Primitive(PrimitiveType::Str)),
            boolean: program.intern_type(Type::Primitive(PrimitiveType::Bool)),
            character: program.intern_type(Type::Primitive(PrimitiveType::Char)),
        };
        let location = program.intern_location(ByteSpan::new(4, 9));
        (program, types, location)
    }

    fn constant(ty: TypeId, value: ConstantValue) -> Operand {
        Operand::Constant(Constant { ty, value })
    }

    fn integer(types: &CoreTypes, value: i64) -> Operand {
        constant(types.int, ConstantValue::Integer(value))
    }

    fn add_returning_function(program: &mut Program, name: &str, result: TypeId, value: Operand, location: LocationId) -> FunctionId {
        let mut function = Function::new(name.to_owned(), result);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].terminate(TerminatorKind::Return(value), location);
        program.add_function(function)
    }

    #[test]
    fn identities_are_allocated_in_insertion_order_and_rendering_is_canonical() {
        let (mut program, types, location) = program();
        assert_eq!(types.unit.index(), 0);
        assert_eq!(types.character.index(), 5);
        assert_eq!(program.intern_type(Type::Unit), types.unit);

        let mut function = Function::new("main\"line", types.float);
        let result = function.add_local(types.float, Some("temporary\nname".to_owned()), LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].push(
            OperationKind::Copy {
                destination: result,
                operand: constant(types.float, ConstantValue::Float(1.5f64.to_bits())),
            },
            location,
        );
        function.blocks[block.index()].terminate(TerminatorKind::Return(Operand::Copy(Place::local(result))), location);
        let main = program.add_function(function);
        program.entry = Some(main);

        assert!(program.validate().is_ok());
        let expected = concat!(
            "source \"test.sao2\" bytes 100\n",
            "  loc0 = 4..9\n",
            "type ty0 = unit\n",
            "type ty1 = int\n",
            "type ty2 = float\n",
            "type ty3 = str\n",
            "type ty4 = bool\n",
            "type ty5 = char\n",
            "entry fn0\n",
            "fn fn0 \"main\\\"line\"() -> ty2 entry bb0 {\n",
            "  _0: ty2 temporary \"temporary\\nname\"\n",
            "  bb0:\n",
            "    _0 = copy const ty2 0x3ff8000000000000 @ loc0\n",
            "    return copy _0 @ loc0\n",
            "}\n",
        );
        assert_eq!(program.render(), expected);
        assert_eq!(program.render(), program.clone().render());
    }

    #[test]
    fn validates_the_complete_operation_and_control_flow_vocabulary() {
        let (mut program, types, location) = program();

        let mut point = NominalDefinition::structure("Point");
        let point_x = point.add_struct_field("x", types.int, MemberStorage::Inline);
        let point_name = point.add_struct_field("name", types.string, MemberStorage::Inline);
        let point_definition = program.add_definition(point);
        let point_type = program.intern_type(Type::Nominal(point_definition));

        let mut pair = NominalDefinition::tuple("Pair");
        let pair_first = pair.add_tuple_field(types.int);
        pair.add_tuple_field(types.float);
        let pair_definition = program.add_definition(pair);
        let pair_type = program.intern_type(Type::Nominal(pair_definition));

        let mut choice = NominalDefinition::union("Choice");
        let choice_a = choice.add_alternative(UnionAlternative::tagged("A", types.int));
        let choice_b = choice.add_alternative(UnionAlternative::tagged("B", types.float));
        let choice_definition = program.add_definition(choice);
        let choice_type = program.intern_type(Type::Nominal(choice_definition));
        let list_type = program.intern_type(Type::List(types.int));
        let map_type = program.intern_type(Type::Map { key: types.int, value: types.string });
        program.intern_type(Type::Union(vec![
            UnionAlternative::untagged(types.int),
            UnionAlternative::error(types.string),
        ]));

        let mut identity = Function::new("identity", types.int);
        let identity_parameter = identity.add_local(types.int, Some("value".to_owned()), LocalOrigin::Parameter);
        let identity_block = identity.add_block();
        identity.entry = Some(identity_block);
        identity.blocks[identity_block.index()].terminate(TerminatorKind::Return(Operand::Copy(Place::local(identity_parameter))), location);
        let identity_id = program.add_function(identity);
        let main_identity = FunctionId::from_index(1);
        let site = |program: &mut Program, operation| program.intern_failure_site(FailureSite { location, function: main_identity, operation, line: 1, column: 5 });
        let list_index_failure = site(&mut program, FailureOperation::ListIndex);
        let map_index_failure = site(&mut program, FailureOperation::MapIndex);
        let string_index_failure = site(&mut program, FailureOperation::StringIndex);
        let output_failure = site(&mut program, FailureOperation::Output);
        let append_failure = site(&mut program, FailureOperation::ListAppend);
        let remove_failure = site(&mut program, FailureOperation::MapRemoveKey);
        let panic_failure = site(&mut program, FailureOperation::ExplicitPanic);
        let error_failure = site(&mut program, FailureOperation::UnhandledError);

        let mut main = Function::new("main", types.int);
        let input = main.add_local(types.int, Some("input".to_owned()), LocalOrigin::Parameter);
        let index = main.add_local(types.int, Some("index".to_owned()), LocalOrigin::Binding);
        let int_temp = main.add_local(types.int, None, LocalOrigin::Temporary);
        let float_temp = main.add_local(types.float, None, LocalOrigin::Temporary);
        let bool_temp = main.add_local(types.boolean, None, LocalOrigin::Temporary);
        let unit_temp = main.add_local(types.unit, None, LocalOrigin::Temporary);
        let point_local = main.add_local(point_type, Some("point".to_owned()), LocalOrigin::Binding);
        let pair_local = main.add_local(pair_type, None, LocalOrigin::Temporary);
        let choice_local = main.add_local(choice_type, None, LocalOrigin::Temporary);
        let list_local = main.add_local(list_type, Some("items".to_owned()), LocalOrigin::Binding);
        let map_local = main.add_local(map_type, Some("lookup".to_owned()), LocalOrigin::Binding);
        let string_local = main.add_local(types.string, None, LocalOrigin::Temporary);
        let character_temp = main.add_local(types.character, None, LocalOrigin::Temporary);
        let blocks = (0..7).map(|_| main.add_block()).collect::<Vec<_>>();
        main.entry = Some(blocks[0]);
        let entry = &mut main.blocks[blocks[0].index()];
        entry.push(OperationKind::Copy { destination: int_temp, operand: Operand::Copy(Place::local(input)) }, location);
        entry.push(OperationKind::Unary { destination: int_temp, operator: UnaryOperator::Plus, operand: integer(&types, 1) }, location);
        entry.push(OperationKind::Binary { destination: int_temp, operator: BinaryOperator::BitwiseAnd, left: Operand::Copy(Place::local(input)), right: integer(&types, 1) }, location);
        entry.push(OperationKind::Convert { destination: float_temp, conversion: NumericConversion::IntToFloat, operand: Operand::Copy(Place::local(input)) }, location);
        entry.push(OperationKind::Aggregate { destination: point_local, aggregate: Aggregate::Struct { definition: point_definition, fields: vec![
            (point_x, Operand::Copy(Place::local(input))),
            (point_name, constant(types.string, ConstantValue::String(b"p\n\0".to_vec()))),
        ] } }, location);
        entry.push(OperationKind::Aggregate { destination: pair_local, aggregate: Aggregate::Tuple { definition: pair_definition, elements: vec![
            Operand::Copy(Place::local(input)), constant(types.float, ConstantValue::Float(0)),
        ] } }, location);
        entry.push(OperationKind::Aggregate { destination: list_local, aggregate: Aggregate::List { ty: list_type, elements: vec![integer(&types, 1)] } }, location);
        entry.push(OperationKind::Aggregate { destination: map_local, aggregate: Aggregate::Map { ty: map_type, entries: vec![(integer(&types, 1), constant(types.string, ConstantValue::String(b"one".to_vec())))] } }, location);
        entry.push(OperationKind::UnionInject { destination: choice_local, union_type: choice_type, alternative: choice_a, payload: Operand::Copy(Place::local(input)) }, location);
        entry.push(OperationKind::UnionTest { destination: bool_temp, union: Operand::Copy(Place::local(choice_local)), alternative: choice_a }, location);
        entry.push(OperationKind::UnionPayload { destination: int_temp, union: Operand::Copy(Place::local(choice_local)), alternative: choice_a }, location);
        entry.push(OperationKind::StringIndex { destination: character_temp, string: Operand::Copy(Place::local(string_local)), index: Operand::Copy(Place::local(index)), failure: string_index_failure }, location);
        entry.push(OperationKind::Assign { destination: Place::projected(point_local, vec![Projection::StructField { definition: point_definition, field: point_x, storage: MemberStorage::Inline }]), value: integer(&types, 3) }, location);
        entry.push(OperationKind::Copy { destination: int_temp, operand: Operand::Copy(Place::projected(pair_local, vec![Projection::TupleField { definition: pair_definition, field: pair_first }])) }, location);
        entry.push(OperationKind::Assign { destination: Place::projected(list_local, vec![Projection::ListIndex { index, failure: list_index_failure }]), value: integer(&types, 4) }, location);
        entry.push(OperationKind::Assign { destination: Place::projected(map_local, vec![Projection::MapIndex { key: index, failure: map_index_failure }]), value: constant(types.string, ConstantValue::String(b"four".to_vec())) }, location);
        entry.push(OperationKind::Call { destination: int_temp, function: identity_id, arguments: vec![Operand::Copy(Place::local(input))] }, location);
        entry.push(OperationKind::Intrinsic { destination: unit_temp, intrinsic: Intrinsic::Print, arguments: vec![Operand::Copy(Place::local(pair_local))], failure: output_failure }, location);
        entry.push(OperationKind::Check(RuntimeCheck::IterationUnlocked { receiver: Operand::Copy(Place::local(list_local)), failure: append_failure }), location);
        entry.push(OperationKind::Builtin { destination: unit_temp, method: BuiltinMethod::ListAppend, receiver: Operand::Copy(Place::local(list_local)), arguments: vec![integer(&types, 2)], failure: Some(append_failure) }, location);
        entry.push(OperationKind::Builtin { destination: int_temp, method: BuiltinMethod::ListLen, receiver: Operand::Copy(Place::local(list_local)), arguments: vec![], failure: None }, location);
        entry.push(OperationKind::Check(RuntimeCheck::IterationUnlocked { receiver: Operand::Copy(Place::local(map_local)), failure: remove_failure }), location);
        entry.push(OperationKind::Builtin { destination: unit_temp, method: BuiltinMethod::MapRemoveKey, receiver: Operand::Copy(Place::local(map_local)), arguments: vec![integer(&types, 1)], failure: Some(remove_failure) }, location);
        entry.push(OperationKind::Builtin { destination: int_temp, method: BuiltinMethod::MapLen, receiver: Operand::Copy(Place::local(map_local)), arguments: vec![], failure: None }, location);
        entry.push(OperationKind::Builtin { destination: int_temp, method: BuiltinMethod::StrLen, receiver: Operand::Copy(Place::local(string_local)), arguments: vec![], failure: None }, location);
        entry.push(OperationKind::BeginIteration { iterable: Operand::Copy(Place::local(list_local)) }, location);
        entry.push(OperationKind::IterationValue { destination: int_temp, iterable: Operand::Copy(Place::local(list_local)), index: Operand::Copy(Place::local(index)) }, location);
        entry.push(OperationKind::EndIteration { iterable: Operand::Copy(Place::local(list_local)) }, location);
        entry.terminate(TerminatorKind::Branch { condition: Operand::Copy(Place::local(bool_temp)), then_block: blocks[1], else_block: blocks[2] }, location);
        main.blocks[blocks[1].index()].terminate(TerminatorKind::Switch { union: Operand::Copy(Place::local(choice_local)), targets: vec![(choice_a, blocks[3]), (choice_b, blocks[4])] }, location);
        main.blocks[blocks[2].index()].terminate(TerminatorKind::Panic { message: constant(types.string, ConstantValue::String(b"failed".to_vec())), failure: panic_failure }, location);
        main.blocks[blocks[3].index()].terminate(TerminatorKind::Return(Operand::Copy(Place::local(input))), location);
        main.blocks[blocks[4].index()].terminate(TerminatorKind::Unreachable, location);
        main.blocks[blocks[5].index()].terminate(TerminatorKind::Jump(blocks[3]), location);
        main.blocks[blocks[6].index()].terminate(TerminatorKind::ErrorPanic { payload: integer(&types, 7), failure: error_failure }, location);
        let main_id = program.add_function(main);
        program.entry = Some(main_id);

        assert!(program.validate().is_ok());
        let rendered = program.render();
        assert!(rendered.contains("bb5:\n    jump bb3"));
        assert!(rendered.contains("bb6:\n    error-panic const ty1 7"));
        assert!(rendered.contains("const ty3 b\"p\\n\\0\""));
        assert!(rendered.contains("switch copy _8 -> [alt0: bb3, alt1: bb4]"));
        assert!(rendered.contains("begin-iteration copy _9"));
        assert!(rendered.contains("_2 = iteration-value copy _9, copy _1"));
    }

    #[test]
    fn construction_owns_all_temporary_inputs() {
        let (mut program, types, location) = program();
        let function_id;
        {
            let definition_name = String::from("Owned layout");
            let field_name = String::from("owned field");
            let tag = String::from("OwnedTag");
            let mut definition = NominalDefinition::structure(definition_name);
            definition.add_struct_field(field_name, types.int, MemberStorage::Inline);
            let definition_id = program.add_definition(definition);
            program.intern_type(Type::Nominal(definition_id));
            program.intern_type(Type::Union(vec![
                UnionAlternative::tagged(tag, types.int),
                UnionAlternative::tagged(String::from("OtherTag"), types.float),
            ]));
            let name = String::from("owned function");
            let local_name = String::from("owned local");
            let bytes = Vec::from(&b"owned bytes"[..]);
            let mut function = Function::new(name, types.string);
            let local = function.add_local(types.string, Some(local_name), LocalOrigin::Binding);
            let block = function.add_block();
            function.entry = Some(block);
            function.blocks[block.index()].push(OperationKind::Copy { destination: local, operand: constant(types.string, ConstantValue::String(bytes)) }, location);
            function.blocks[block.index()].terminate(TerminatorKind::Return(Operand::Copy(Place::local(local))), location);
            function_id = program.add_function(function);
        }
        program.entry = Some(function_id);
        assert!(program.validate().is_ok());
        assert!(program.render().contains("owned bytes"));
        assert!(program.render().contains("Owned layout"));
        assert_eq!(program.source.filename, PathBuf::from("test.sao2"));
    }

    #[test]
    fn malformed_ir_reports_first_operation_with_full_context() {
        let (mut program, types, location) = program();
        let mut function = Function::new("main", types.int);
        let destination = function.add_local(types.int, None, LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].push(OperationKind::Copy {
            destination,
            operand: constant(types.boolean, ConstantValue::Boolean(true)),
        }, location);
        function.blocks[block.index()].terminate(TerminatorKind::Return(integer(&types, 0)), location);
        let main = program.add_function(function);
        program.entry = Some(main);

        let error = program.validate().unwrap_err();
        assert_eq!(error.function, Some(main));
        assert_eq!(error.block, Some(block));
        assert_eq!(error.site, Some(OperationSite::Operation(0)));
        assert_eq!(error.to_string(), "invalid IR in fn0 bb0 op0: copy type does not match destination");
    }

    #[test]
    fn validates_iteration_operation_types() {
        let (mut program, types, location) = program();
        let mut function = Function::new("main", types.int);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[block.index()].push(OperationKind::BeginIteration { iterable: integer(&types, 1) }, location);
        function.blocks[block.index()].terminate(TerminatorKind::Return(integer(&types, 0)), location);
        let main = program.add_function(function);
        program.entry = Some(main);
        assert_eq!(program.validate().unwrap_err().message, "iteration operand is not a list or map");
    }

    #[test]
    fn rejects_out_of_range_identities_and_unterminated_blocks() {
        let (mut program, types, location) = program();
        let main = add_returning_function(&mut program, "main", types.int, integer(&types, 0), location);
        program.entry = Some(FunctionId(99));
        assert!(program.validate().unwrap_err().message.contains("fn99"));

        program.entry = Some(main);
        program.functions[main.index()].blocks[0].terminator = None;
        let error = program.validate().unwrap_err();
        assert_eq!(error.site, Some(OperationSite::Terminator));
        assert_eq!(error.message, "block is not terminated");
    }

    #[test]
    fn rejects_every_category_of_out_of_range_identity() {
        let (mut invalid_type, types, location) = program();
        let main = add_returning_function(&mut invalid_type, "main", TypeId(90), integer(&types, 0), location);
        invalid_type.entry = Some(main);
        assert!(invalid_type.validate().unwrap_err().message.contains("ty90"));

        let (mut invalid_definition, types, location) = program();
        invalid_definition.types.push(Type::Nominal(DefinitionId(91)));
        let main = add_returning_function(&mut invalid_definition, "main", types.int, integer(&types, 0), location);
        invalid_definition.entry = Some(main);
        assert!(invalid_definition.validate().unwrap_err().message.contains("def91"));

        let (mut invalid_local, types, location) = program();
        let main = add_returning_function(&mut invalid_local, "main", types.int, Operand::Copy(Place::local(LocalId(92))), location);
        invalid_local.entry = Some(main);
        assert!(invalid_local.validate().unwrap_err().message.contains("_92"));

        let (mut invalid_location, types, location) = program();
        let main = add_returning_function(&mut invalid_location, "main", types.int, integer(&types, 0), location);
        invalid_location.entry = Some(main);
        invalid_location.functions[main.index()].blocks[0].terminator.as_mut().unwrap().location = LocationId(93);
        assert!(invalid_location.validate().unwrap_err().message.contains("loc93"));

        let (mut invalid_function, types, location) = program();
        let mut function = Function::new("main", types.int);
        let destination = function.add_local(types.int, None, LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[0].push(OperationKind::Call { destination, function: FunctionId(94), arguments: vec![] }, location);
        function.blocks[0].terminate(TerminatorKind::Return(integer(&types, 0)), location);
        let main = invalid_function.add_function(function);
        invalid_function.entry = Some(main);
        assert!(invalid_function.validate().unwrap_err().message.contains("fn94"));

        let (mut invalid_field, types, location) = program();
        let mut tuple = NominalDefinition::tuple("One");
        tuple.add_tuple_field(types.int);
        let definition = invalid_field.add_definition(tuple);
        let tuple_ty = invalid_field.intern_type(Type::Nominal(definition));
        let mut function = Function::new("main", types.int);
        let tuple_local = function.add_local(tuple_ty, None, LocalOrigin::Binding);
        let destination = function.add_local(types.int, None, LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[0].push(OperationKind::Copy { destination, operand: Operand::Copy(Place::projected(tuple_local, vec![Projection::TupleField { definition, field: FieldId(95) }])) }, location);
        function.blocks[0].terminate(TerminatorKind::Return(integer(&types, 0)), location);
        let main = invalid_field.add_function(function);
        invalid_field.entry = Some(main);
        assert!(invalid_field.validate().unwrap_err().message.contains("field95"));

        let (mut invalid_alternative, types, location) = program();
        let union_ty = invalid_alternative.intern_type(Type::Union(vec![UnionAlternative::untagged(types.int), UnionAlternative::untagged(types.float)]));
        let mut function = Function::new("main", types.int);
        let union_local = function.add_local(union_ty, None, LocalOrigin::Binding);
        let destination = function.add_local(types.int, None, LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        function.blocks[0].push(OperationKind::UnionPayload { destination, union: Operand::Copy(Place::local(union_local)), alternative: AlternativeId(96) }, location);
        function.blocks[0].terminate(TerminatorKind::Return(integer(&types, 0)), location);
        let main = invalid_alternative.add_function(function);
        invalid_alternative.entry = Some(main);
        assert!(invalid_alternative.validate().unwrap_err().message.contains("alt96"));
    }

    #[test]
    fn rejects_malformed_definitions_constants_projections_calls_and_edges() {
        let (mut program, types, location) = program();
        let mut malformed = NominalDefinition::union("Bad");
        malformed.add_alternative(UnionAlternative::untagged(types.int));
        program.add_definition(malformed);
        let main = add_returning_function(&mut program, "main", types.int, integer(&types, 0), location);
        program.entry = Some(main);
        assert!(program.validate().unwrap_err().message.contains("fewer than two"));

        program.definitions.clear();
        program.functions[main.index()].blocks[0].terminator = Some(Terminator {
            kind: TerminatorKind::Jump(BlockId(17)), location,
        });
        assert!(program.validate().unwrap_err().message.contains("bb17"));

        program.functions[main.index()].blocks[0].terminator = Some(Terminator {
            kind: TerminatorKind::Return(constant(types.int, ConstantValue::Boolean(false))), location,
        });
        assert_eq!(program.validate().unwrap_err().message, "constant value does not agree with its type");
    }

    #[test]
    fn validates_failure_sites_and_required_numeric_check_order() {
        let (mut program, types, location) = program();
        let function_id = FunctionId::from_index(0);
        let failure = program.intern_failure_site(FailureSite {
            location,
            function: function_id,
            operation: FailureOperation::IntegerAdd,
            line: 1,
            column: 5,
        });
        let mut function = Function::new("main", types.int);
        let result = function.add_local(types.int, None, LocalOrigin::Temporary);
        let block = function.add_block();
        function.entry = Some(block);
        let left = integer(&types, i64::MIN);
        let right = integer(&types, 1);
        function.blocks[block.index()].push(OperationKind::Check(RuntimeCheck::IntegerOverflow {
            operation: IntegerOperation::Add,
            left: left.clone(),
            right: right.clone(),
            failure,
        }), location);
        function.blocks[block.index()].push(OperationKind::Binary {
            destination: result,
            operator: BinaryOperator::Add,
            left,
            right,
        }, location);
        function.blocks[block.index()].terminate(TerminatorKind::Return(Operand::Copy(Place::local(result))), location);
        let main = program.add_function(function);
        program.entry = Some(main);

        assert!(program.validate().is_ok());
        assert!(program.render().contains("fail0 = loc0 fn0 integer-add at 1:5"));
        assert!(program.render().contains("const ty1 -9223372036854775808"));

        let mut missing_check = program.clone();
        missing_check.functions[main.index()].blocks[block.index()].operations.remove(0);
        assert!(missing_check.validate().unwrap_err().message.contains("pre-check"));

        let mut duplicate = program.clone();
        duplicate.failure_sites.push(duplicate.failure_sites[0]);
        assert!(duplicate.validate().unwrap_err().message.contains("duplicates"));
    }
}

impl<'a> Validator<'a> {
    fn new(program: &'a Program) -> Self {
        Self { program, function: None, block: None, site: None }
    }

    fn error(&self, message: impl Into<String>) -> ValidationError {
        ValidationError {
            function: self.function,
            block: self.block,
            site: self.site,
            message: message.into(),
        }
    }

    fn validate(mut self) -> Result<(), ValidationError> {
        let program = self.program;
        if program.source.filename.as_os_str().is_empty() {
            return Err(self.error("source filename is empty"));
        }
        for (index, span) in program.locations.iter().enumerate() {
            if span.start > span.end || span.end > program.source.byte_len {
                return Err(self.error(format!("loc{index} has invalid byte span {}..{}", span.start, span.end)));
            }
            if program.locations[..index].contains(span) {
                return Err(self.error(format!("loc{index} duplicates an earlier source location")));
            }
        }
        for (index, failure) in program.failure_sites.iter().enumerate() {
            if failure.location.index() >= program.locations.len() {
                return Err(self.error(format!("fail{index} has invalid location {}", failure.location)));
            }
            if failure.function.index() >= program.functions.len() {
                return Err(self.error(format!("fail{index} has invalid function {}", failure.function)));
            }
            if failure.line == 0 || failure.column == 0 {
                return Err(self.error(format!("fail{index} has an invalid line or column")));
            }
            if program.failure_sites[..index].iter().any(|earlier| earlier.location == failure.location && earlier.function == failure.function && earlier.operation == failure.operation) {
                return Err(self.error(format!("fail{index} duplicates an earlier failure site")));
            }
        }
        for (index, ty) in program.types.iter().enumerate() {
            self.validate_type(TypeId(index), ty)?;
            if program.types[..index].contains(ty) {
                return Err(self.error(format!("ty{index} duplicates an earlier canonical type")));
            }
        }
        let mut definition_names = HashSet::new();
        for (index, definition) in program.definitions.iter().enumerate() {
            if !definition_names.insert(definition.name.as_str()) {
                return Err(self.error(format!("def{index} repeats a nominal definition name")));
            }
            self.validate_definition(DefinitionId(index), definition)?;
        }
        for index in 0..program.definitions.len() {
            self.nominal_type(DefinitionId(index))?;
        }
        let entry = self.program.entry.ok_or_else(|| self.error("entry function is not set"))?;
        self.function(entry)?;
        let mut function_names = HashSet::new();
        for (index, function) in program.functions.iter().enumerate() {
            self.function = Some(FunctionId(index));
            if !function_names.insert(function.name.as_str()) {
                return Err(self.error(format!("fn{index} repeats a function name")));
            }
            self.validate_function(FunctionId(index), function)?;
        }
        Ok(())
    }

    fn validate_type(&self, id: TypeId, ty: &Type) -> Result<(), ValidationError> {
        match ty {
            Type::Unit | Type::Primitive(_) => Ok(()),
            Type::List(element) => { self.ty(*element)?; Ok(()) }
            Type::Map { key, value } => { self.ty(*key)?; self.ty(*value)?; Ok(()) }
            Type::Nominal(definition) => { self.definition(*definition)?; Ok(()) }
            Type::Union(alternatives) => self.validate_alternatives(id.to_string(), alternatives),
        }
    }

    fn validate_definition(&self, id: DefinitionId, definition: &NominalDefinition) -> Result<(), ValidationError> {
        if definition.name.is_empty() { return Err(self.error(format!("{id} has an empty name"))); }
        match &definition.layout {
            DefinitionLayout::Struct(fields) => {
                if fields.is_empty() { return Err(self.error(format!("{id} struct layout is empty"))); }
                let mut names = HashSet::new();
                for (index, field) in fields.iter().enumerate() {
                    if field.name.is_empty() || !names.insert(field.name.as_str()) {
                        return Err(self.error(format!("{id} field{index} has an empty or duplicate name")));
                    }
                    self.ty(field.ty)?;
                    if !self.storage_is_legal(field.ty, field.storage) {
                        return Err(self.error(format!("{id} field{index} has invalid storage for {}", field.ty)));
                    }
                }
                Ok(())
            }
            DefinitionLayout::Tuple(fields) => {
                if fields.is_empty() { return Err(self.error(format!("{id} tuple layout is empty"))); }
                for ty in fields { self.ty(*ty)?; }
                Ok(())
            }
            DefinitionLayout::Union(alternatives) => self.validate_alternatives(id.to_string(), alternatives),
        }
    }

    fn validate_alternatives(&self, owner: String, alternatives: &[UnionAlternative]) -> Result<(), ValidationError> {
        if alternatives.len() < 2 { return Err(self.error(format!("{owner} union has fewer than two alternatives"))); }
        let mut tags = HashSet::new();
        let mut payloads = HashSet::new();
        let mut tagged = None;
        for (index, alternative) in alternatives.iter().enumerate() {
            self.ty(alternative.payload)?;
            match &alternative.constructor {
                AlternativeConstructor::Untagged => {
                    if tagged == Some(true) { return Err(self.error(format!("{owner} mixes tagged and untagged alternatives"))); }
                    tagged = Some(false);
                    if !payloads.insert(alternative.payload) {
                        return Err(self.error(format!("{owner} repeats an untagged payload type")));
                    }
                }
                AlternativeConstructor::Tagged(tag) => {
                    if tagged == Some(false) { return Err(self.error(format!("{owner} mixes tagged and untagged alternatives"))); }
                    tagged = Some(true);
                    if tag.is_empty() || tag == "Error" || !tags.insert(tag.as_str()) {
                        return Err(self.error(format!("{owner} has an empty, reserved, or duplicate tag")));
                    }
                }
                AlternativeConstructor::Error => {
                    if index + 1 != alternatives.len() {
                        return Err(self.error(format!("{owner} Error alternative is not last")));
                    }
                    if !matches!(self.ty(alternative.payload)?, Type::Primitive(_)) {
                        return Err(self.error(format!("{owner} Error payload is not primitive")));
                    }
                }
            }
        }
        Ok(())
    }

    fn storage_is_legal(&self, ty: TypeId, storage: MemberStorage) -> bool {
        if storage == MemberStorage::Inline {
            return self.program.types.get(ty.index()).is_some();
        }
        matches!(self.program.types.get(ty.index()), Some(Type::Nominal(definition))
            if matches!(self.program.definitions.get(definition.index()).map(|item| &item.layout), Some(DefinitionLayout::Struct(_))))
    }

    fn validate_function(&mut self, id: FunctionId, function: &Function) -> Result<(), ValidationError> {
        self.function = Some(id);
        if function.name.is_empty() { return Err(self.error("function name is empty")); }
        self.ty(function.result)?;
        let entry = function.entry.ok_or_else(|| self.error("entry block is not set"))?;
        self.block_in(function, entry)?;
        let mut parameters = HashSet::new();
        for parameter in &function.parameters {
            let local = self.local_in(function, *parameter)?;
            if local.origin != LocalOrigin::Parameter || !parameters.insert(*parameter) {
                return Err(self.error("parameter list contains a duplicate or non-parameter local"));
            }
        }
        for (index, local) in function.locals.iter().enumerate() {
            self.ty(local.ty)?;
            if local.origin == LocalOrigin::Parameter && !parameters.contains(&LocalId(index)) {
                return Err(self.error(format!("_{index} is a parameter local missing from the signature")));
            }
        }
        for (index, block) in function.blocks.iter().enumerate() {
            self.block = Some(BlockId(index));
            for (operation_index, operation) in block.operations.iter().enumerate() {
                self.site = Some(OperationSite::Operation(operation_index));
                self.location(operation.location)?;
                self.validate_operation(function, operation)?;
            }
            self.validate_check_sequence(function, block)?;
            self.site = Some(OperationSite::Terminator);
            let terminator = block.terminator.as_ref().ok_or_else(|| self.error("block is not terminated"))?;
            self.location(terminator.location)?;
            self.validate_terminator(function, terminator)?;
        }
        self.block = None;
        self.site = None;
        Ok(())
    }

    fn validate_check_sequence(&self, function: &Function, block: &BasicBlock) -> Result<(), ValidationError> {
        let int = self.primitive(PrimitiveType::Int)?;
        let float = self.primitive(PrimitiveType::Float)?;
        for (index, operation) in block.operations.iter().enumerate() {
            match &operation.kind {
                OperationKind::Unary { operator: UnaryOperator::Minus, operand, .. } if self.operand_type(function, operand)? == int => {
                    if !matches!(index.checked_sub(1).and_then(|i| block.operations.get(i)).map(|op| &op.kind),
                        Some(OperationKind::Check(RuntimeCheck::IntegerNegation { operand: checked, .. })) if checked == operand)
                    { return Err(self.error("integer negation lacks its immediately preceding check")); }
                }
                OperationKind::Convert { conversion: NumericConversion::FloatToInt, operand, .. } => {
                    if !matches!(index.checked_sub(1).and_then(|i| block.operations.get(i)).map(|op| &op.kind),
                        Some(OperationKind::Check(RuntimeCheck::NumericConversion { operand: checked, conversion: NumericConversion::FloatToInt, .. })) if checked == operand)
                    { return Err(self.error("float-to-int conversion lacks its immediately preceding check")); }
                }
                OperationKind::Binary { destination, operator, left, right } => {
                    let ty = self.operand_type(function, left)?;
                    let previous = index.checked_sub(1).and_then(|i| block.operations.get(i)).map(|op| &op.kind);
                    let valid_precheck = match (ty, operator) {
                        (ty, BinaryOperator::Add) if ty == int => matches!(previous, Some(OperationKind::Check(RuntimeCheck::IntegerOverflow { operation: IntegerOperation::Add, left: a, right: b, .. })) if a == left && b == right),
                        (ty, BinaryOperator::Subtract) if ty == int => matches!(previous, Some(OperationKind::Check(RuntimeCheck::IntegerOverflow { operation: IntegerOperation::Subtract, left: a, right: b, .. })) if a == left && b == right),
                        (ty, BinaryOperator::Multiply) if ty == int => matches!(previous, Some(OperationKind::Check(RuntimeCheck::IntegerOverflow { operation: IntegerOperation::Multiply, left: a, right: b, .. })) if a == left && b == right),
                        (ty, BinaryOperator::Divide) if ty == int || ty == float => matches!(previous, Some(OperationKind::Check(RuntimeCheck::Division { left: a, right: b, .. })) if a == left && b == right),
                        (ty, BinaryOperator::Remainder) if ty == int => matches!(previous, Some(OperationKind::Check(RuntimeCheck::Remainder { left: a, right: b, .. })) if a == left && b == right),
                        (ty, BinaryOperator::ShiftRight) if ty == int => matches!(previous, Some(OperationKind::Check(RuntimeCheck::ShiftRange { amount, .. })) if amount == right),
                        (ty, BinaryOperator::ShiftLeft) if ty == int => matches!(previous, Some(OperationKind::Check(RuntimeCheck::IntegerOverflow { operation: IntegerOperation::ShiftLeft, left: a, right: b, .. })) if a == left && b == right)
                            && matches!(index.checked_sub(2).and_then(|i| block.operations.get(i)).map(|op| &op.kind), Some(OperationKind::Check(RuntimeCheck::ShiftRange { amount, .. })) if amount == right),
                        _ => true,
                    };
                    if !valid_precheck { return Err(self.error("numeric operation has an invalid or missing pre-check sequence")); }
                    if ty == float && matches!(operator, BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide) {
                        let result = Operand::Copy(Place::local(*destination));
                        let Some(OperationKind::Check(RuntimeCheck::FiniteFloat { operand, failure })) = block.operations.get(index + 1).map(|op| &op.kind) else {
                            return Err(self.error("float arithmetic lacks its immediately following finite-result check"));
                        };
                        if operand != &result { return Err(self.error("float finite-result check names the wrong destination")); }
                        let expected = match operator { BinaryOperator::Add => FailureOperation::FloatAdd, BinaryOperator::Subtract => FailureOperation::FloatSubtract, BinaryOperator::Multiply => FailureOperation::FloatMultiply, BinaryOperator::Divide => FailureOperation::FloatDivision, _ => unreachable!() };
                        self.failure_site(*failure, expected)?;
                    }
                }
                OperationKind::Builtin { method, receiver, failure: Some(failure), .. }
                    if matches!(method, BuiltinMethod::ListAppend | BuiltinMethod::ListRemoveIndex | BuiltinMethod::MapRemoveKey) => {
                    if !matches!(index.checked_sub(1).and_then(|i| block.operations.get(i)).map(|op| &op.kind),
                        Some(OperationKind::Check(RuntimeCheck::IterationUnlocked { receiver: checked, failure: site })) if checked == receiver && site == failure)
                    { return Err(self.error("structural mutation lacks its immediately preceding iteration-lock check")); }
                }
                OperationKind::Check(check) => {
                    let next = block.operations.get(index + 1).map(|op| &op.kind);
                    let previous = index.checked_sub(1).and_then(|i| block.operations.get(i)).map(|op| &op.kind);
                    let consumed = match check {
                        RuntimeCheck::IntegerOverflow { operation, left, right, .. } => matches!(next,
                            Some(OperationKind::Binary { operator, left: a, right: b, .. })
                                if a == left && b == right && matches!((operation, operator),
                                    (IntegerOperation::Add, BinaryOperator::Add)
                                    | (IntegerOperation::Subtract, BinaryOperator::Subtract)
                                    | (IntegerOperation::Multiply, BinaryOperator::Multiply)
                                    | (IntegerOperation::ShiftLeft, BinaryOperator::ShiftLeft))),
                        RuntimeCheck::IntegerNegation { operand, .. } => matches!(next, Some(OperationKind::Unary { operator: UnaryOperator::Minus, operand: value, .. }) if value == operand),
                        RuntimeCheck::Division { left, right, .. } => matches!(next, Some(OperationKind::Binary { operator: BinaryOperator::Divide, left: a, right: b, .. }) if a == left && b == right),
                        RuntimeCheck::Remainder { left, right, .. } => matches!(next, Some(OperationKind::Binary { operator: BinaryOperator::Remainder, left: a, right: b, .. }) if a == left && b == right),
                        RuntimeCheck::ShiftRange { amount, .. } => matches!(next,
                            Some(OperationKind::Binary { operator: BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight, right, .. }) if right == amount)
                            || matches!(next, Some(OperationKind::Check(RuntimeCheck::IntegerOverflow { operation: IntegerOperation::ShiftLeft, right, .. })) if right == amount),
                        RuntimeCheck::FiniteFloat { operand, .. } => matches!(previous, Some(OperationKind::Binary { destination, .. }) if operand == &Operand::Copy(Place::local(*destination))),
                        RuntimeCheck::NumericConversion { operand, conversion, .. } => matches!(next, Some(OperationKind::Convert { operand: value, conversion: actual, .. }) if value == operand && actual == conversion),
                        RuntimeCheck::IterationUnlocked { receiver, failure } => matches!(next, Some(OperationKind::Builtin { receiver: actual, failure: Some(site), .. }) if actual == receiver && site == failure),
                    };
                    if !consumed { return Err(self.error("runtime check is not adjacent to its checked operation")); }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn failure_site(&self, id: FailureSiteId, operation: FailureOperation) -> Result<(), ValidationError> {
        self.failure_site_one_of(id, &[operation])
    }

    fn failure_site_one_of(&self, id: FailureSiteId, operations: &[FailureOperation]) -> Result<(), ValidationError> {
        let site = self.program.failure_sites.get(id.index()).ok_or_else(|| self.error(format!("invalid failure-site identity {id}")))?;
        if Some(site.function) != self.function { return Err(self.error(format!("{id} belongs to a different function"))); }
        if !operations.contains(&site.operation) { return Err(self.error(format!("{id} operation disagrees with its use"))); }
        Ok(())
    }

    fn validate_operation(&self, function: &Function, operation: &Operation) -> Result<(), ValidationError> {
        use OperationKind::*;
        match &operation.kind {
            Copy { destination, operand } => self.destination(function, *destination, self.operand_type(function, operand)?, "copy"),
            Unary { destination, operator, operand } => {
                let operand_ty = self.operand_type(function, operand)?;
                let result = match operator {
                    UnaryOperator::LogicalNot if self.is_primitive(operand_ty, PrimitiveType::Bool) => operand_ty,
                    UnaryOperator::BitwiseNot if self.is_primitive(operand_ty, PrimitiveType::Int) => operand_ty,
                    UnaryOperator::Plus | UnaryOperator::Minus if self.is_numeric(operand_ty) => operand_ty,
                    _ => return Err(self.error("invalid unary operand type")),
                };
                self.destination(function, *destination, result, "unary operation")
            }
            Binary { destination, operator, left, right } => {
                let left_ty = self.operand_type(function, left)?;
                let right_ty = self.operand_type(function, right)?;
                let bool_ty = self.primitive(PrimitiveType::Bool)?;
                let int_ty = self.primitive(PrimitiveType::Int)?;
                let result = match operator {
                    BinaryOperator::BitwiseOr | BinaryOperator::BitwiseXor | BinaryOperator::BitwiseAnd |
                    BinaryOperator::ShiftLeft | BinaryOperator::ShiftRight if left_ty == int_ty && right_ty == int_ty => int_ty,
                    BinaryOperator::Add | BinaryOperator::Subtract | BinaryOperator::Multiply | BinaryOperator::Divide
                        if left_ty == right_ty && self.is_numeric(left_ty) => left_ty,
                    BinaryOperator::Remainder if left_ty == int_ty && right_ty == int_ty => int_ty,
                    BinaryOperator::Equal | BinaryOperator::NotEqual if left_ty == right_ty && self.supports_equality(left_ty, &mut Vec::new()) => bool_ty,
                    BinaryOperator::Less | BinaryOperator::LessEqual | BinaryOperator::Greater | BinaryOperator::GreaterEqual
                        if left_ty == right_ty && self.supports_ordering(left_ty) => bool_ty,
                    BinaryOperator::In if self.membership_matches(left_ty, right_ty) => bool_ty,
                    _ => return Err(self.error("invalid binary operand types")),
                };
                self.destination(function, *destination, result, "binary operation")
            }
            Convert { destination, conversion, operand } => {
                let source = self.operand_type(function, operand)?;
                let result = self.conversion_result(*conversion, source)?;
                self.destination(function, *destination, result, "conversion")
            }
            Aggregate { destination, aggregate } => {
                let result = self.validate_aggregate(function, aggregate)?;
                self.destination(function, *destination, result, "aggregate")
            }
            UnionInject { destination, union_type, alternative, payload } => {
                let expected = self.alternative(*union_type, *alternative)?.payload;
                self.expect_operand(function, payload, expected, "union payload")?;
                self.destination(function, *destination, *union_type, "union injection")
            }
            UnionTest { destination, union, alternative } => {
                let union_ty = self.operand_type(function, union)?;
                self.alternative(union_ty, *alternative)?;
                self.destination(function, *destination, self.primitive(PrimitiveType::Bool)?, "union test")
            }
            UnionPayload { destination, union, alternative } => {
                let union_ty = self.operand_type(function, union)?;
                let payload = self.alternative(union_ty, *alternative)?.payload;
                self.destination(function, *destination, payload, "union payload")
            }
            StringIndex { destination, string, index, failure } => {
                self.expect_operand(function, string, self.primitive(PrimitiveType::Str)?, "string index receiver")?;
                self.expect_operand(function, index, self.primitive(PrimitiveType::Int)?, "string index")?;
                self.failure_site(*failure, FailureOperation::StringIndex)?;
                self.destination(function, *destination, self.primitive(PrimitiveType::Char)?, "string index")
            }
            Assign { destination, value } => {
                let destination_ty = self.place_type(function, destination)?;
                self.expect_operand(function, value, destination_ty, "assignment")
            }
            Call { destination, function: callee, arguments } => {
                let callee = self.function(*callee)?;
                if arguments.len() != callee.parameters.len() { return Err(self.error("call argument count does not match signature")); }
                for (argument, parameter) in arguments.iter().zip(&callee.parameters) {
                    let parameter_ty = self.local_in(callee, *parameter)?.ty;
                    self.expect_operand(function, argument, parameter_ty, "call argument")?;
                }
                self.destination(function, *destination, callee.result, "call")
            }
            Intrinsic { destination, intrinsic, arguments, failure } => {
                let unit = self.primitive_or_unit(Type::Unit)?;
                match intrinsic {
                    self::Intrinsic::Print if arguments.len() == 1 => {
                        let ty = self.operand_type(function, &arguments[0])?;
                        if !self.is_printable(ty, &mut Vec::new()) { return Err(self.error("print argument is not printable")); }
                    }
                    self::Intrinsic::Println if arguments.len() <= 1 => {
                        if let Some(argument) = arguments.first() {
                            let ty = self.operand_type(function, argument)?;
                            if !self.is_printable(ty, &mut Vec::new()) { return Err(self.error("println argument is not printable")); }
                        }
                    }
                    _ => return Err(self.error("intrinsic argument count does not match signature")),
                }
                self.failure_site(*failure, FailureOperation::Output)?;
                if self.program.failure_sites[failure.index()].location != operation.location { return Err(self.error("output failure site has the wrong source location")); }
                self.destination(function, *destination, unit, "intrinsic call")
            }
            Builtin { destination, method, receiver, arguments, failure } => {
                self.validate_builtin(function, *destination, *method, receiver, arguments, *failure)?;
                if let Some(failure) = failure && self.program.failure_sites[failure.index()].location != operation.location { return Err(self.error("built-in failure site has the wrong source location")); }
                Ok(())
            }
            BeginIteration { iterable } | EndIteration { iterable } => {
                let ty = self.operand_type(function, iterable)?;
                if !matches!(self.ty(ty)?, Type::List(_) | Type::Map { .. }) {
                    return Err(self.error("iteration operand is not a list or map"));
                }
                Ok(())
            }
            IterationValue { destination, iterable, index } => {
                let iterable_ty = self.operand_type(function, iterable)?;
                let result = match self.ty(iterable_ty)? {
                    Type::List(element) => *element,
                    Type::Map { key, .. } => *key,
                    _ => return Err(self.error("iteration operand is not a list or map")),
                };
                self.expect_operand(function, index, self.primitive(PrimitiveType::Int)?, "iteration index")?;
                self.destination(function, *destination, result, "iteration value")
            }
            Check(check) => {
                self.validate_check(function, check)?;
                if self.program.failure_sites[runtime_check_failure(check).index()].location != operation.location { return Err(self.error("runtime check failure site has the wrong source location")); }
                Ok(())
            }
        }
    }

    fn validate_aggregate(&self, function: &Function, aggregate: &Aggregate) -> Result<TypeId, ValidationError> {
        match aggregate {
            Aggregate::Struct { definition, fields } => {
                let DefinitionLayout::Struct(layout) = &self.definition(*definition)?.layout else { return Err(self.error("struct aggregate names a non-struct definition")); };
                if fields.len() != layout.len() { return Err(self.error("struct aggregate is incomplete")); }
                let mut seen = HashSet::new();
                for (field, operand) in fields {
                    let expected = layout.get(field.index()).ok_or_else(|| self.error(format!("invalid field identity {field}")))?.ty;
                    if !seen.insert(*field) { return Err(self.error("struct aggregate repeats a field")); }
                    self.expect_operand(function, operand, expected, "struct field")?;
                }
                self.nominal_type(*definition)
            }
            Aggregate::Tuple { definition, elements } => {
                let DefinitionLayout::Tuple(layout) = &self.definition(*definition)?.layout else { return Err(self.error("tuple aggregate names a non-tuple definition")); };
                if elements.len() != layout.len() { return Err(self.error("tuple aggregate arity does not match layout")); }
                for (operand, expected) in elements.iter().zip(layout) { self.expect_operand(function, operand, *expected, "tuple field")?; }
                self.nominal_type(*definition)
            }
            Aggregate::List { ty, elements } => {
                let Type::List(element) = self.ty(*ty)? else { return Err(self.error("list aggregate has a non-list type")); };
                for operand in elements { self.expect_operand(function, operand, *element, "list element")?; }
                Ok(*ty)
            }
            Aggregate::Map { ty, entries } => {
                let Type::Map { key, value } = self.ty(*ty)? else { return Err(self.error("map aggregate has a non-map type")); };
                for (entry_key, entry_value) in entries {
                    self.expect_operand(function, entry_key, *key, "map key")?;
                    self.expect_operand(function, entry_value, *value, "map value")?;
                }
                Ok(*ty)
            }
        }
    }

    fn validate_builtin(&self, function: &Function, destination: LocalId, method: BuiltinMethod, receiver: &Operand, arguments: &[Operand], failure: Option<FailureSiteId>) -> Result<(), ValidationError> {
        let receiver_ty = self.operand_type(function, receiver)?;
        let unit = self.primitive_or_unit(Type::Unit)?;
        let int = self.primitive(PrimitiveType::Int)?;
        let result = match (method, self.ty(receiver_ty)?, arguments) {
            (BuiltinMethod::ListAppend, Type::List(element), [argument]) => { self.expect_operand(function, argument, *element, "append argument")?; unit }
            (BuiltinMethod::ListRemoveIndex, Type::List(_), [argument]) => { self.expect_operand(function, argument, int, "list index")?; unit }
            (BuiltinMethod::ListLen, Type::List(_), []) => int,
            (BuiltinMethod::MapRemoveKey, Type::Map { key, .. }, [argument]) => { self.expect_operand(function, argument, *key, "map key")?; unit }
            (BuiltinMethod::MapLen, Type::Map { .. }, []) => int,
            (BuiltinMethod::StrLen, Type::Primitive(PrimitiveType::Str), []) => int,
            _ => return Err(self.error("built-in receiver or arguments do not match its signature")),
        };
        let expected_failure = match method {
            BuiltinMethod::ListAppend => Some(FailureOperation::ListAppend),
            BuiltinMethod::ListRemoveIndex => Some(FailureOperation::ListRemoveIndex),
            BuiltinMethod::MapRemoveKey => Some(FailureOperation::MapRemoveKey),
            BuiltinMethod::ListLen | BuiltinMethod::MapLen | BuiltinMethod::StrLen => None,
        };
        match (failure, expected_failure) {
            (Some(site), Some(operation)) => { self.failure_site(site, operation)?; }
            (None, None) => {}
            _ => return Err(self.error("built-in failure attribution does not match its operation")),
        }
        self.destination(function, destination, result, "built-in call")
    }

    fn validate_check(&self, function: &Function, check: &RuntimeCheck) -> Result<(), ValidationError> {
        let int = self.primitive(PrimitiveType::Int)?;
        let (failure, allowed): (FailureSiteId, &[FailureOperation]) = match check {
            RuntimeCheck::IntegerOverflow { operation, failure, .. } => (*failure, match operation {
                IntegerOperation::Add => &[FailureOperation::IntegerAdd], IntegerOperation::Subtract => &[FailureOperation::IntegerSubtract],
                IntegerOperation::Multiply => &[FailureOperation::IntegerMultiply], IntegerOperation::ShiftLeft => &[FailureOperation::ShiftLeft],
            }),
            RuntimeCheck::IntegerNegation { failure, .. } => (*failure, &[FailureOperation::IntegerNegation]),
            RuntimeCheck::Division { failure, .. } => (*failure, &[FailureOperation::IntegerDivision, FailureOperation::FloatDivision]),
            RuntimeCheck::Remainder { failure, .. } => (*failure, &[FailureOperation::IntegerRemainder]),
            RuntimeCheck::ShiftRange { failure, .. } => (*failure, &[FailureOperation::ShiftLeft, FailureOperation::ShiftRight]),
            RuntimeCheck::FiniteFloat { failure, .. } => (*failure, &[FailureOperation::FloatAdd, FailureOperation::FloatSubtract, FailureOperation::FloatMultiply, FailureOperation::FloatDivision]),
            RuntimeCheck::NumericConversion { failure, .. } => (*failure, &[FailureOperation::FloatToInt]),
            RuntimeCheck::IterationUnlocked { failure, .. } => (*failure, &[FailureOperation::ListAppend, FailureOperation::ListRemoveIndex, FailureOperation::MapRemoveKey]),
        };
        self.failure_site_one_of(failure, allowed)?;
        match check {
            RuntimeCheck::IntegerOverflow { left, right, .. } => {
                self.expect_operand(function, left, int, "overflow operand")?;
                self.expect_operand(function, right, int, "overflow operand")
            }
            RuntimeCheck::IntegerNegation { operand, .. } | RuntimeCheck::ShiftRange { amount: operand, .. } => self.expect_operand(function, operand, int, "integer check operand"),
            RuntimeCheck::Division { left, right, .. } => {
                let ty = self.operand_type(function, left)?;
                if !self.is_numeric(ty) { return Err(self.error("division check operand is not numeric")); }
                let RuntimeCheck::Division { failure, .. } = check else { unreachable!() };
                self.failure_site(*failure, if ty == int { FailureOperation::IntegerDivision } else { FailureOperation::FloatDivision })?;
                self.expect_operand(function, right, ty, "division check operand")
            }
            RuntimeCheck::Remainder { left, right, .. } => {
                self.expect_operand(function, left, int, "remainder operand")?;
                self.expect_operand(function, right, int, "remainder operand")
            }
            RuntimeCheck::FiniteFloat { operand, .. } => self.expect_operand(function, operand, self.primitive(PrimitiveType::Float)?, "finite-float operand"),
            RuntimeCheck::NumericConversion { operand, conversion, .. } => {
                let source = self.operand_type(function, operand)?;
                self.conversion_result(*conversion, source).map(|_| ())
            }
            RuntimeCheck::IterationUnlocked { receiver, .. } => {
                let ty = self.operand_type(function, receiver)?;
                if matches!(self.ty(ty)?, Type::List(_) | Type::Map { .. }) { Ok(()) }
                else { Err(self.error("iteration-lock check receiver is not a list or map")) }
            }
        }
    }

    fn validate_terminator(&self, function: &Function, terminator: &Terminator) -> Result<(), ValidationError> {
        match &terminator.kind {
            TerminatorKind::Jump(target) => { self.block_in(function, *target)?; Ok(()) }
            TerminatorKind::Branch { condition, then_block, else_block } => {
                self.expect_operand(function, condition, self.primitive(PrimitiveType::Bool)?, "branch condition")?;
                self.block_in(function, *then_block)?; self.block_in(function, *else_block)?; Ok(())
            }
            TerminatorKind::Switch { union, targets } => {
                let ty = self.operand_type(function, union)?;
                let alternatives = self.union_alternatives(ty)?;
                let mut seen = HashSet::new();
                for (alternative, block) in targets {
                    if alternative.index() >= alternatives.len() { return Err(self.error(format!("invalid alternative identity {alternative}"))); }
                    if !seen.insert(*alternative) { return Err(self.error("switch repeats an alternative")); }
                    self.block_in(function, *block)?;
                }
                if seen.len() != alternatives.len() { return Err(self.error("switch does not cover every alternative")); }
                Ok(())
            }
            TerminatorKind::Return(value) => self.expect_operand(function, value, function.result, "return value"),
            TerminatorKind::Panic { message, failure } => {
                self.failure_site(*failure, FailureOperation::ExplicitPanic)?;
                if self.program.failure_sites[failure.index()].location != terminator.location { return Err(self.error("panic failure site has the wrong source location")); }
                self.expect_operand(function, message, self.primitive(PrimitiveType::Str)?, "panic message")
            }
            TerminatorKind::ErrorPanic { payload, failure } => {
                self.failure_site(*failure, FailureOperation::UnhandledError)?;
                if self.program.failure_sites[failure.index()].location != terminator.location { return Err(self.error("Error panic failure site has the wrong source location")); }
                let ty = self.operand_type(function, payload)?;
                if matches!(self.ty(ty)?, Type::Primitive(_)) { Ok(()) } else { Err(self.error("Error panic payload is not primitive")) }
            }
            TerminatorKind::Unreachable => Ok(()),
        }
    }

    fn operand_type(&self, function: &Function, operand: &Operand) -> Result<TypeId, ValidationError> {
        match operand {
            Operand::Copy(place) => self.place_type(function, place),
            Operand::Constant(constant) => {
                let ty = self.ty(constant.ty)?;
                let valid = matches!((ty, &constant.value),
                    (Type::Unit, ConstantValue::Unit) |
                    (Type::Primitive(PrimitiveType::Int), ConstantValue::Integer(_)) |
                    (Type::Primitive(PrimitiveType::Float), ConstantValue::Float(_)) |
                    (Type::Primitive(PrimitiveType::Str), ConstantValue::String(_)) |
                    (Type::Primitive(PrimitiveType::Char), ConstantValue::Character(0..=127)) |
                    (Type::Primitive(PrimitiveType::Bool), ConstantValue::Boolean(_)));
                if !valid { return Err(self.error("constant value does not agree with its type")); }
                Ok(constant.ty)
            }
        }
    }

    fn place_type(&self, function: &Function, place: &Place) -> Result<TypeId, ValidationError> {
        let mut current = self.local_in(function, place.local)?.ty;
        for projection in &place.projections {
            current = match projection {
                Projection::StructField { definition, field, storage } => {
                    if !matches!(self.ty(current)?, Type::Nominal(found) if found == definition) { return Err(self.error("struct projection definition does not match the projected type")); }
                    let DefinitionLayout::Struct(fields) = &self.definition(*definition)?.layout else { return Err(self.error("struct projection names a non-struct definition")); };
                    let selected = fields.get(field.index()).ok_or_else(|| self.error(format!("invalid field identity {field}")))?;
                    if selected.storage != *storage { return Err(self.error("struct projection storage disagrees with its field")); }
                    selected.ty
                }
                Projection::TupleField { definition, field } => {
                    if !matches!(self.ty(current)?, Type::Nominal(found) if found == definition) { return Err(self.error("tuple projection definition does not match the projected type")); }
                    let DefinitionLayout::Tuple(fields) = &self.definition(*definition)?.layout else { return Err(self.error("tuple projection names a non-tuple definition")); };
                    *fields.get(field.index()).ok_or_else(|| self.error(format!("invalid field identity {field}")))?
                }
                Projection::ListIndex { index, failure } => {
                    self.expect_local(function, *index, self.primitive(PrimitiveType::Int)?, "list index")?;
                    self.failure_site(*failure, FailureOperation::ListIndex)?;
                    let Type::List(element) = self.ty(current)? else { return Err(self.error("list-index projection is applied to a non-list")); };
                    *element
                }
                Projection::MapIndex { key, failure } => {
                    let Type::Map { key: expected, value } = self.ty(current)? else { return Err(self.error("map-index projection is applied to a non-map")); };
                    self.expect_local(function, *key, *expected, "map key")?;
                    self.failure_site(*failure, FailureOperation::MapIndex)?;
                    *value
                }
            };
        }
        Ok(current)
    }

    fn destination(&self, function: &Function, destination: LocalId, expected: TypeId, description: &str) -> Result<(), ValidationError> {
        self.expect_local(function, destination, expected, description)
    }

    fn expect_local(&self, function: &Function, local: LocalId, expected: TypeId, description: &str) -> Result<(), ValidationError> {
        if self.local_in(function, local)?.ty != expected { return Err(self.error(format!("{description} type does not match destination"))); }
        Ok(())
    }

    fn expect_operand(&self, function: &Function, operand: &Operand, expected: TypeId, description: &str) -> Result<(), ValidationError> {
        if self.operand_type(function, operand)? != expected { return Err(self.error(format!("{description} has the wrong type"))); }
        Ok(())
    }

    fn conversion_result(&self, conversion: NumericConversion, source: TypeId) -> Result<TypeId, ValidationError> {
        match conversion {
            NumericConversion::IntToFloat if self.is_primitive(source, PrimitiveType::Int) => self.primitive(PrimitiveType::Float),
            NumericConversion::FloatToInt if self.is_primitive(source, PrimitiveType::Float) => self.primitive(PrimitiveType::Int),
            _ => Err(self.error("numeric conversion has the wrong operand type")),
        }
    }

    fn supports_ordering(&self, ty: TypeId) -> bool {
        matches!(self.program.types.get(ty.index()), Some(Type::Primitive(PrimitiveType::Int | PrimitiveType::Float | PrimitiveType::Str | PrimitiveType::Char)))
    }

    fn supports_equality(&self, ty: TypeId, visiting: &mut Vec<DefinitionId>) -> bool {
        match self.program.types.get(ty.index()) {
            Some(Type::Unit | Type::Primitive(_) | Type::List(_) | Type::Map { .. }) => true,
            Some(Type::Nominal(definition)) => {
                if visiting.contains(definition) { return false; }
                match self.program.definitions.get(definition.index()).map(|item| &item.layout) {
                    Some(DefinitionLayout::Struct(_)) => true,
                    Some(DefinitionLayout::Tuple(fields)) => {
                        visiting.push(*definition);
                        let result = fields.iter().all(|field| self.supports_equality(*field, visiting));
                        visiting.pop(); result
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    fn membership_matches(&self, item: TypeId, container: TypeId) -> bool {
        matches!(self.program.types.get(container.index()), Some(Type::List(element)) if *element == item)
            || matches!(self.program.types.get(container.index()), Some(Type::Map { key, .. }) if *key == item)
    }

    fn is_printable(&self, ty: TypeId, visiting: &mut Vec<DefinitionId>) -> bool {
        match self.program.types.get(ty.index()) {
            Some(Type::Unit | Type::Primitive(_)) => true,
            Some(Type::Union(alternatives)) => alternatives.iter().all(|item| self.is_printable(item.payload, visiting)),
            Some(Type::Nominal(definition)) => {
                if visiting.contains(definition) { return false; }
                visiting.push(*definition);
                let result = match self.program.definitions.get(definition.index()).map(|item| &item.layout) {
                    Some(DefinitionLayout::Tuple(fields)) => fields.iter().all(|field| self.is_printable(*field, visiting)),
                    Some(DefinitionLayout::Union(alternatives)) => alternatives.iter().all(|item| self.is_printable(item.payload, visiting)),
                    _ => false,
                };
                visiting.pop(); result
            }
            _ => false,
        }
    }

    fn is_numeric(&self, ty: TypeId) -> bool {
        self.is_primitive(ty, PrimitiveType::Int) || self.is_primitive(ty, PrimitiveType::Float)
    }

    fn is_primitive(&self, ty: TypeId, primitive: PrimitiveType) -> bool {
        matches!(self.program.types.get(ty.index()), Some(Type::Primitive(found)) if *found == primitive)
    }

    fn primitive(&self, primitive: PrimitiveType) -> Result<TypeId, ValidationError> {
        self.primitive_or_unit(Type::Primitive(primitive))
    }

    fn primitive_or_unit(&self, wanted: Type) -> Result<TypeId, ValidationError> {
        self.program.types.iter().position(|ty| ty == &wanted).map(TypeId)
            .ok_or_else(|| self.error("required canonical primitive or unit type is missing"))
    }

    fn nominal_type(&self, definition: DefinitionId) -> Result<TypeId, ValidationError> {
        self.program.types.iter().position(|ty| *ty == Type::Nominal(definition)).map(TypeId)
            .ok_or_else(|| self.error(format!("{definition} has no canonical nominal type")))
    }

    fn alternative(&self, union: TypeId, alternative: AlternativeId) -> Result<&'a UnionAlternative, ValidationError> {
        self.union_alternatives(union)?.get(alternative.index()).ok_or_else(|| self.error(format!("invalid alternative identity {alternative}")))
    }

    fn union_alternatives(&self, ty: TypeId) -> Result<&'a [UnionAlternative], ValidationError> {
        match self.ty(ty)? {
            Type::Union(alternatives) => Ok(alternatives),
            Type::Nominal(definition) => match &self.definition(*definition)?.layout {
                DefinitionLayout::Union(alternatives) => Ok(alternatives),
                _ => Err(self.error(format!("{ty} is not a union type"))),
            },
            _ => Err(self.error(format!("{ty} is not a union type"))),
        }
    }

    fn ty(&self, id: TypeId) -> Result<&'a Type, ValidationError> {
        self.program.types.get(id.index()).ok_or_else(|| self.error(format!("invalid type identity {id}")))
    }
    fn definition(&self, id: DefinitionId) -> Result<&'a NominalDefinition, ValidationError> {
        self.program.definitions.get(id.index()).ok_or_else(|| self.error(format!("invalid definition identity {id}")))
    }
    fn function(&self, id: FunctionId) -> Result<&'a Function, ValidationError> {
        self.program.functions.get(id.index()).ok_or_else(|| self.error(format!("invalid function identity {id}")))
    }
    fn local_in<'b>(&self, function: &'b Function, id: LocalId) -> Result<&'b Local, ValidationError> {
        function.locals.get(id.index()).ok_or_else(|| self.error(format!("invalid local identity {id}")))
    }
    fn block_in<'b>(&self, function: &'b Function, id: BlockId) -> Result<&'b BasicBlock, ValidationError> {
        function.blocks.get(id.index()).ok_or_else(|| self.error(format!("invalid block identity {id}")))
    }
    fn location(&self, id: LocationId) -> Result<&'a ByteSpan, ValidationError> {
        self.program.locations.get(id.index()).ok_or_else(|| self.error(format!("invalid location identity {id}")))
    }
}
