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

Introduce the owned program, function, local, block, place, operand, operation,
terminator, and compact source-location representations. Define stable IDs,
type ownership, deterministic debug rendering, and an IR validator that rejects
invalid IDs, type mismatches, unterminated blocks, invalid destinations, and
malformed control-flow edges.

Exit criterion: tests can construct, render, and validate representative IR
without parsing source, and no IR value borrows the frontend AST.

## Stage 2: Straight-line lowering

Lower function signatures, parameters, local declarations, literals, ordinary
unary and binary expressions, conversions, constructors, calls, intrinsics,
member and index places, and assignments. Preserve source argument order and
materialize operations so C operand evaluation order cannot affect behavior.
Consume existing frontend identities, types, call targets, constructors, union
injections, and mutation authorizations without repeating their analysis.

Exit criterion: straight-line functions lower deterministically into validated
typed locals and operations, including reference-bearing temporaries across
calls.

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

Lower explicit and implicit union construction, discriminant tests, payload
extraction, branch-local narrowing, and switch alternatives using the recorded
frontend resolutions. Lower postfix `?` into its success branch and its exact
error-propagation or `main` panic branch, including bare `Error` success as unit
and unions containing unit. Preserve union nesting, alternative identity, and
constructor-form information needed by universal printing.

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
