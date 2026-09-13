# Current Work: Typed Intermediate Representation

Status: in progress.

This document expands milestone 6 of `ROADMAP.md`. The objective is to lower a
validated frontend result into a small, owned, typed control-flow IR before the
compiler depends on C evaluation rules. Milestone 4's resolved-AST C emitter
remains the active backend until milestone 7 and must not shape the IR.

Lowering begins only after `semantic::validate_handoff` accepts the completed
`Analysis` and `SemanticResult` described in `AST.md`. Missing or contradictory
frontend facts are compiler invariant failures, not new source diagnostics.
The IR owns everything required after lowering and does not retain AST
references.

## IR direction

Use a Rust-MIR-like representation made from typed functions, locals, basic
blocks, statements, and terminators. Locals cover parameters, source bindings,
and compiler-created temporaries. Basic blocks contain straight-line operations
and end in one explicit control transfer. Places describe assignable storage;
operands and computed values remain distinct from places.

The initial vocabulary covers constants and copies, unary and binary
operations, conversions, aggregate construction, union injection and payload
access, calls and intrinsics, indexing and member access, assignment, runtime
checks, branches, switches, returns, panic, and unreachable control flow. Source
constructs such as blocks, `if`, loops, `switch`, short-circuit operators, and
postfix `?` do not survive as nested IR operations.

Evaluation order is explicit. Any intermediate reference-bearing value that
must survive a call or possible allocation is materialized in a typed local or
temporary. Calls continue to use ordinary native C calling convention in the
future backend; the IR is not a virtual-machine operand stack and does not
prescribe physical stack offsets.

## Shadow-stack compatibility

Milestone 10 will derive GC roots from IR local types. A generated function that
can hold roots will use a function-specific shadow-frame struct whose first
member is a common header linking the caller frame and naming a generated
traversal callback. The callback casts the header to the known frame type and
traces its reference-bearing fields. Functions without roots need no shadow
frame.

The core IR records typed storage but contains no root push or pop instructions,
generic root-slot tables, C field offsets, or mandatory GC liveness maps. Root
storage will be zero-initialized and traceable incoming parameters copied into
their fields before the frame is linked. The all-zero packed reference and zero
union discriminant are reserved non-value states that trace nothing. Initially,
every root field may remain visible for the whole function invocation; clearing
dead roots or using liveness-sensitive traversal is a later optimization.

## Stage 1: IR model and invariants

Implement the complete milestone-6 IR vocabulary as one cohesive change. This
stage defines the representation, construction boundary, deterministic
renderer, structural validator, and focused tests. It does not lower frontend
syntax or alter compiler-pipeline behavior.

Add a private `ir` module and register it in the crate. The module remains
unused by production compilation until later stages connect lowering to the
pipeline.

### Owned program and types

- Define opaque, stable index identities for types, nominal definitions,
  functions, locals, blocks, fields, union alternatives, and source locations.
  Allocate identities in insertion order and render their numeric form
  deterministically.
- Make the IR program own its source filename metadata, canonical types,
  nominal definitions, functions, validated entry-function identity, and
  compact source-location table. Do not retain the source text or any AST
  reference.
- Represent unit, primitive, list, map, nominal, and anonymous-union types in an
  owned canonical type table. IR type identities are independent of frontend
  `TypeId` values; lowering will create an explicit deterministic mapping.
- Give nominal definitions owned names and ordered owned layouts. Preserve
  struct field names and inline-or-referenced storage, tuple positions, union
  tags and payload types, and the constructor form required by universal
  printing. Use stable field and alternative identities rather than source
  spelling to select members after lowering.
- Give each function an owned name, ordered parameter-local identities, result
  type, complete local table, entry block, and block table. A local records its
  type, optional owned source name, and whether it originated as a parameter,
  source binding, or compiler-created temporary. Do not retain frontend
  `BindingId` values in the finished IR.
- Provide insertion methods for arena-like tables which return the newly
  allocated stable identity. Blocks retain an ordered operation list and an
  optional terminator so construction can be incremental; validation rejects
  the incomplete form.
- Intern source provenance behind compact location identities. Each entry
  retains its UTF-8 byte span against the program's owned source filename.
  Stage 5 will populate the runtime-failure-specific entries and the data needed
  to emit line and column tables.

### Places, operands, and operations

Use mutable typed locals rather than SSA. A place starts at one local and has an
ordered projection path. Projections distinguish struct fields, tuple fields,
list indices, and map indices. A dynamic list index or map key must already be
materialized in a local; this prevents a recursive place/operand representation
and makes its evaluation order explicit.

An operand is either a typed constant or a copy from a place. Constants cover
unit, an integer magnitude, binary64 float bits, owned string bytes, a character
byte, and a boolean. Keeping float bits rather than relying on host formatting
or equality makes validation and rendering deterministic.

Define the complete operation vocabulary now so later stages add lowering
without redesigning the core representation:

- copying an operand, unary and binary computation, numeric conversion,
  aggregate construction, union injection, discriminant extraction, and union
  payload extraction;
- assignment to a place, direct function calls, intrinsic calls, and built-in
  list, map, and string methods; and
- explicit integer overflow and negation, division, remainder, shift-range,
  finite-float, numeric-conversion, list or string bounds, and missing-map-key
  checks.

Every value-producing or unit-producing call has an explicit destination.
Operations retain compact source provenance, but contain no C helper names,
physical layout, shadow-stack commands, root-slot tables, or GC liveness
instructions.

### Terminators and control flow

Every completed basic block ends in exactly one terminator. Define terminators
for unconditional jumps, boolean branches, union-alternative switches, returns,
panic, and unreachable continuation. Successor blocks are always named by
stable block identity. Return carries an operand, including an explicit unit
operand for unit-returning functions.

Calls remain ordered operations rather than control-flow terminators. Panic and
unreachable are terminal. Do not embed source blocks, `if`, loops, `switch`,
short-circuit operators, or postfix `?` as nested IR nodes.

### Structural and type validator

Expose validation as a deterministic `Result` with a dedicated IR validation
error. Stop at the first error in program, function, block, and operation order.
An error identifies the containing function and, where applicable, the block
and operation or terminator. Pipeline integration will later wrap such failures
as compiler invariant diagnostics.

Validate all of the following without repeating frontend semantics:

- every referenced identity is in range and selects the expected kind of
  program entity;
- the entry function and each function's entry block exist;
- parameter identities are unique, occur in signature order, and designate
  parameter locals;
- type entries, nominal layouts, fields, and union alternatives are
  structurally complete and internally consistent;
- every block is terminated and every jump, branch, and switch edge names an
  existing block;
- each place projection is legal for the projected type, including its field,
  storage, tuple position, list-index, or map-key identity and type;
- each constant agrees with its declared type;
- operation arity and operand types are valid and every destination agrees with
  the operation's result type;
- aggregate members, union alternatives, calls, intrinsics, built-in methods,
  and explicit checks agree with their referenced signatures and types; and
- branch conditions are boolean, switches select alternatives of their operand
  union, returns match the function result, and panic operands satisfy the
  intrinsic contract.

The validator does not perform reachability, dominance, definite-initialization
analysis, source-level inference, mutability checking, switch coverage,
return-path proof, or GC liveness. Structurally unreachable blocks are valid
because unreachable source remains represented through lowering.

### Deterministic rendering

Implement a custom canonical renderer rather than exposing derived Rust
`Debug`. Render source metadata, type and nominal declarations, function
signatures, locals, blocks, operations, terminators, stable identities,
constants, escaped names and byte strings, and byte spans in table order.
Equivalent IR must render byte-for-byte identically across runs. Keep the text
human-readable for unit-test expectations and milestone-7 backend debugging;
it is a diagnostic form, not a serialized compatibility format.

### Tests and completion

Add IR-only unit tests which construct programs directly without parsing source
and cover:

- a representative straight-line function with parameters, locals, constants,
  arithmetic, a call, and a return;
- branches, union switches, projections, aggregates, explicit checks, panic,
  unreachable termination, and a structurally unreachable block;
- insertion-order identity allocation and byte-for-byte deterministic rendering,
  including escaped strings and binary64 constants;
- ownership of names, paths, bytes, layouts, and types after temporary
  construction inputs have been dropped;
- invalid type, definition, function, local, block, field, alternative, and
  location identities;
- malformed definitions, constants, projections, aggregates, unions, calls,
  intrinsics, built-ins, checks, destinations, branches, switches, returns, and
  unterminated blocks; and
- validation errors containing the appropriate function, block, and operation
  context.

Exit criterion: tests can directly construct, render, and validate the full IR
vocabulary; malformed IR fails deterministically; no IR value borrows the
frontend AST; and later stages can implement lowering without revisiting the
core representation.

## Stage 2: Straight-line lowering

Lower complete straight-line functions from the validated frontend handoff into
owned IR, then run the Stage 1 validator before returning the result. Preserve
left-to-right evaluation through conservative snapshots and support all value
construction, including unions. Do not connect lowering to normal compilation
until Stage 6.

### Complete the frontend handoff

The current frontend does not yet represent the explicit numeric conversions
specified by `DESIGN.md`: the reserved `int` and `float` tokens cannot begin an
expression, and the AST has no conversion node. Add `int(expression)` and
`float(expression)` to `GRAMMAR.ebnf`, the AST, parser, name-and-type analysis,
semantic traversal, and handoff validation. Accept only `int(float)` and
`float(int)`. Record the selected source and destination types; Stage 5 remains
responsible for finite and range checks.

Ordinary member and index reads also need identity-bearing handoff facts. Record
one resolved projection for each value-level access while existing analysis is
already selecting it:

- a struct declaration, field ordinal, inline-or-referenced storage, and result
  type;
- a tuple declaration, position, and result type;
- a list's element type;
- a map's key and value types; or
- string indexing and its `char` result.

Exclude built-in method callees and qualified union constructors from these
projection subjects; their existing call-target records are authoritative.
Extend assignment path records with struct field ordinals as well as their
existing member references. Update `AST.md` and the handoff validator to require
exactly one valid projection record for every applicable expression. Missing,
duplicated, or contradictory conversion and projection facts are compiler
invariant failures, not new source diagnostics.

### Program and identity mapping

Add a lowering entry point which accepts the completed `Analysis` and
`SemanticResult` and returns an owned `ir::Program` or a dedicated lowering
error. The caller must already have passed `semantic::validate_handoff`.
Lowering must not retain either input.

Build the IR in deterministic passes:

1. Reserve nominal definitions and functions in source declaration order.
2. Copy canonical types and complete all owned nominal layouts.
3. Copy every function signature before lowering bodies, allowing direct and
   mutual recursion.
4. Lower function bodies in source order.
5. Validate the completed IR and convert any rejection into a lowering
   invariant failure.

Maintain explicit frontend-to-IR maps for types, nominal definitions,
functions, bindings, fields, and union alternatives. Do not depend on frontend
and IR numeric identities happening to match. Preallocation must support
recursive nominal definitions and recursive function calls without placeholder
AST references.

Within a function, allocate parameter locals first in signature order, source
locals when their declarations are encountered, and compiler temporaries in
evaluation order. Copy all retained names, types, paths, bytes, union tags, and
source metadata into IR-owned storage. Intern operation provenance in order of
first occurrence of each byte span.

### Expression lowering

Use a lowering routine which produces a typed operand and applies any recorded
implicit union injection before returning it. Parentheses are transparent, but
an injection recorded on a parenthesized expression still applies at that exact
expression boundary.

- Lower unit and literals to typed constants and binding identifiers to copies
  from their mapped locals.
- Lower unary operations, non-short-circuit binary operations, membership, and
  numeric conversions into typed temporaries. Leave `&&` and `||` to Stage 3
  because their right operand is conditional.
- Evaluate list elements, each map key followed by its value, and tuple
  constructor arguments from left to right. Stabilize each value before
  evaluating the next and retain that order in the aggregate operation.
- Evaluate struct constructor arguments in source order and stabilize each
  result. Only then use the recorded argument-to-member mapping to reorder
  operands into declaration-field order for construction.
- Lower untagged, tagged, and `Error` union constructors using the exact
  recorded union alternative. Lower recorded implicit injections as explicit
  union-injection operations without flattening nested unions.
- Lower direct calls through mapped function identities. Lower `print` and
  `println` through resolved intrinsic identities. For built-in methods,
  evaluate and stabilize the receiver before arguments and retain the resolved
  built-in method identity.
- Lower member and index reads only through recorded projection identities.
  Materialize computed receivers and every dynamic list index or map key before
  constructing a projected place. Represent string indexing as its dedicated
  read operation rather than an assignable place.

Terminal `panic`, block and `if` expressions, explicit returns, statement
conditionals, loops, switches, `break`, `continue`, and postfix `?` return a
temporary `PendingStage` lowering error naming the owning later stage. This is
not a compiler invariant and cannot reach users because Stage 2 lowering is not
yet in the production pipeline. Remove each temporary case when its stage is
implemented.

### Conservative evaluation order

Do not add effect analysis in this stage. Use a `stabilize` helper whenever an
operand's value must be fixed before a later expression is evaluated. Constants
and existing compiler-created temporaries are already stable. A copy from a
parameter, source binding, or projected place is assigned to a fresh temporary.
Lowering-created temporaries follow a single-assignment convention even though
the IR itself permits mutable locals.

Apply stabilization consistently:

- stabilize the left operand before lowering the right operand of a binary
  operation;
- stabilize receivers before indices, keys, or call arguments;
- stabilize each call, constructor, list, and map argument immediately after
  evaluating it; and
- retain every typed temporary, including reference-bearing values which must
  survive a later call or possible allocation.

This deliberately permits extra copies. Removing unnecessary temporaries is a
later optimization and must not weaken explicit evaluation order.

### Assignment lowering

Use the exact semantic mutation authorization associated with each assignment.
Confirm that its subject, root binding, access classification, operation, and
typed path agree with the name-and-type assignment record, but do not repeat
the permission decision.

- For direct `=`, lower the right-hand value, including any injection, and
  assign it to the mapped binding place.
- For a projected target, snapshot the root object and evaluate each dynamic
  index or key once from left to right before lowering the right-hand value.
  Build the destination from the recorded field and projection identities.
- For compound assignment, additionally copy the old target value before
  lowering the right operand, lower the corresponding binary operation into a
  temporary, and assign the result back to the previously captured place.
- Preserve inline struct-slot replacement versus referenced-member rebinding in
  the projection data without selecting a C representation.

Emit ordinary arithmetic and indexing operations in this stage. Stage 5 adds
the corresponding overflow, division, remainder, shift, float, conversion,
bounds, and missing-key checks without changing operand order.

Lower expression statements for their effects and discard their result. A
straight-line function without a terminal construct ends its entry block by
returning its final block expression, or an explicit unit constant when the
body has no value.

### Tests and completion

Add parser, analysis, semantic, and handoff tests for both conversion forms,
invalid conversion operands, every projection category, excluded method and
qualified-constructor callees, field ordinals, and corrupted or duplicated
handoff records.

Add lowering tests covering:

- owned and deterministic type, definition, function, field, alternative, and
  binding mappings, including recursive definitions and mutual calls;
- parameters, source locals, literals, conversions, unary and ordinary binary
  operations, membership, expression statements, and fallthrough returns;
- operation order and snapshots for nested calls, binary operands, receivers,
  arguments, and later mutations;
- list and map evaluation order, typed empty containers, tuple construction,
  and source-ordered struct evaluation followed by layout reordering;
- explicit untagged, tagged, and `Error` construction, implicit injection, and
  explicitly nested union identity;
- struct and tuple member reads, list and map indexing, and string indexing;
- direct, projected, and compound assignments with every index or key evaluated
  exactly once;
- direct function calls, printable intrinsics, and all built-in methods;
- byte-for-byte deterministic IR rendering followed by successful Stage 1
  validation;
- missing or contradictory frontend facts producing lowering invariant errors;
  and
- each deferred control-flow or postfix-`?` construct producing its explicit
  temporary `PendingStage` result.

Keep existing frontend, warning, temporary C backend, compiler, and executable
tests unchanged. Contributor guidance prohibits compiling, running tests, or
formatting during implementation.

Exit criterion: every function in the defined straight-line subset lowers in
source order to deterministic, owned, validated IR; all earlier observable
values are stabilized before later effects; reference-bearing intermediates
survive calls in typed locals; and the remaining unsupported constructs are
explicitly assigned to Stages 3 and 4 rather than mistaken for broken frontend
invariants.

## Stage 3: Control-flow lowering

Lower value-producing blocks and `if` expressions through destination
temporaries and merge blocks. Lower statement conditionals, short-circuit
operators, `while`, `for`, exhaustive switches, `break`, `continue`, explicit
returns, fallthrough completion, and unreachable paths into named basic blocks
with explicit terminators. Preserve the semantic pass's resolved loop targets,
switch coverage, reachability, and `Never` behavior.

Exit criterion: every accepted control-flow form has a closed, validated graph
with one terminator per block and no syntax-level control construct in the IR.

## Stage 4: Unions and error flow

Lower discriminant tests, payload extraction, branch-local narrowing, and
switch alternatives using the recorded frontend resolutions and the explicit
union values produced by Stage 2. Lower postfix `?` into its success branch and
its exact error-propagation or `main` panic branch, including bare `Error`
success as unit and unions containing unit. Preserve union nesting, alternative
identity, and constructor-form information needed by universal printing.

Exit criterion: union values and all postfix-`?` paths are explicit typed IR
operations and control-flow edges with no renewed type or coverage decisions.

## Stage 5: Runtime checks and source locations

Make every required integer overflow, negation, division, remainder, shift,
float validity, numeric conversion, list or string bounds, and missing-map-key
failure explicit in evaluation order. Intern compact location IDs that map each
runtime failure back to its source file, byte span, operation, and enclosing
function. Keep failure operations target-independent rather than encoding C
helpers in the core IR.

Exit criterion: all specified runtime failure points are represented explicitly
and every such operation has a valid compact source location.

## Stage 6: Pipeline integration and handoff

Run typed lowering and IR validation immediately after the semantic handoff.
Keep the temporary resolved-AST emitter as the downstream code generator until
milestone 7, without expanding its supported subset or teaching it to consume
partial IR. Treat lowering or validation contradictions as compiler diagnostics
while preserving semantic warnings and the existing no-output-on-failure
boundary.

Add focused lowering and validation tests for evaluation order, temporaries,
branch merges, loops, switches, union narrowing, postfix `?`, checked failures,
locations, invalid IR, and frontend-handoff corruption. Retain the existing
compiler and executable tests unchanged as the walking-skeleton regression
boundary.

Exit criterion: every diagnostic-free frontend result lowers to validated,
owned typed IR before the existing backend runs, and milestone 7 can replace
that backend without revisiting syntax or semantic analysis.

## Boundaries

- Do not implement C emission from IR during this milestone.
- Do not implement runtime layouts, the arena allocator, escape analysis,
  shadow-frame generation, garbage collection, containers, or runtime printing.
- Do not repeat name resolution, type inference, mutability checking, switch
  coverage, reachability, or return-path proof in lowering.
- Do not borrow AST nodes from the IR or make source spelling necessary after
  lowering; retain only owned names and compact source-location data required
  for diagnostics and later runtime failures.
- Preserve filenames, UTF-8 byte spans, warning behavior, diagnostic limits,
  and compiler-versus-source failure categories.
