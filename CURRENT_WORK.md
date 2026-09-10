# Current Work: Early Primitive C Backend

Status: in progress (Phase 2).

This document expands milestone 4 of `ROADMAP.md`. The objective is to replace
the one-call string-print lowering with a small direct C emitter over the
resolved AST produced by milestone 3. The source-to-executable path must remain
working after every phase.

The milestone deliberately implements only a primitive, side-effect-limited
subset. Programs outside that subset remain valid SAO2 where analysis permits
them, but receive a clear temporary-backend source diagnostic. Backend limits
must not be reported before parser or analysis diagnostics.

## Supported subset and boundary

This milestone targets a no-argument `main` with linear primitive computation:

- signed 64-bit integer and boolean literals and values;
- parenthesized expressions and the analyzed integer and boolean unary
  operators;
- integer arithmetic, remainder, bitwise operations, shifts, comparisons, and
  boolean operations;
- primitive local bindings, lexical blocks, identifier reads, direct
  assignment, and compound assignment;
- direct `print` and `println` calls for string literals, integers, and booleans;
- an implicit successful result for `fn main()`, and an integer result for
  `fn main() int` through a final value or an unconditional `return` supported
  by the emitter.

The emitter consumes `Analysis` identities, expression types, converted
literals, binding records, assignment targets, and intrinsic call targets. It
does not repeat source name lookup, numeric parsing, or type inference. C names
are generated from stable identities rather than source spellings, so SAO2
shadowing and C keywords cannot collide.

String values remain limited to the existing direct literal-output path.
Floats, characters, string locals, user function calls, parameters, control
flow, constructors, containers, members, indexing, unions, `panic`, and
flow-dependent expressions remain unsupported by this backend.

Milestone 5 still owns full semantic validity. Until then, the emitter accepts
only structurally safe cases it can lower without relying on missing
mutability, return-path, loop-context, or narrowing checks. In particular,
assignment lowering is limited to mutable local bindings and no expression in
an emitted construct may have `Error` or `Deferred` state.

## Phase 1: Analyzed emitter boundary

- Replace the specialized print-only lowering with a temporary emitter object
  that receives `SourceFile`, `Program`, `Analysis`, and the analyzed `main`.
- Separate subset validation from filesystem output so unsupported programs do
  not create or truncate `build/program.c`.
- Centralize source diagnostics for unsupported declarations, signatures,
  statements, expressions, calls, and types.
- Generate a complete deterministic C translation unit in memory before the
  compiler writes it.
- Preserve the exact existing `print("...")` C output behavior, including
  embedded NUL bytes and Windows binary stdout handling.

Exit criterion: the hello regression travels through the new emitter boundary,
while analyzed but unsupported programs fail before output creation.

## Phase 2: Primitive values and expressions

This phase builds an internal analyzed-expression rendering seam. Binding reads
receive their final identity-based C names, but Phase 3 remains responsible for
emitting the declarations that make those names usable in a translation unit.
Consequently, the existing string-literal `print` program remains the only
CLI-accepted backend subset during this phase and its generated C remains
unchanged.

- Add explicit temporary C representations for SAO2 `int` and `bool`, using
  fixed-width integer and boolean C types.
- Emit converted integer and boolean literals from analysis annotations rather
  than reparsing source text.
- Emit binding reads, parentheses, unary `+`, unary `-`, `~`, and `!` according
  to their resolved operand types.
- Emit integer arithmetic, remainder, bitwise operators, shifts, integer
  comparisons, equality, and short-circuit boolean operators.
- Parenthesize generated expressions deliberately so correctness does not
  depend on matching the C precedence table.
- Reject every expression form or resolved type not explicitly supported.

During this milestone, generated signed C operations are intentionally
unchecked. Tests must avoid overflow, division or remainder by zero, invalid
shift counts, and other cases for which the roadmap permits temporary host-C
behavior. Runtime checks belong to typed-IR lowering in milestone 6.

Exit criterion: pure in-range integer and boolean expressions lower
deterministically from resolved analysis facts in emitter unit tests, including
identity-based binding reads, while the hello regression remains unchanged.

## Phase 3: Bindings, scopes, and assignments

- Emit primitive local declarations with generated names based on `BindingId`.
- Initialize locals in source evaluation order and preserve lexical block
  structure and same-scope shadowing.
- Resolve identifier reads exclusively through recorded `NameResolution`.
- Lower direct assignments through `AssignmentTargetAnnotation`; do not add
  member or index assignment support.
- Lower the supported compound assignments only when analysis gives both sides
  the required primitive type.
- Accept assignment only to mutable local bindings until milestone 5 installs
  complete mutability validation.
- Reject no-value, non-returning, error, or deferred initializers and operands
  instead of manufacturing C values.

Exit criterion: linear programs can declare, shadow, read, and update primitive
locals without exposing source identifiers to C.

## Phase 4: Output and entry-point results

- Generalize the retained byte-safe output support so multiple `print` and
  `println` statements can be emitted in evaluation order.
- Preserve exact string-literal byte output and add decimal integer and
  `true`/`false` boolean output needed by executable arithmetic tests.
- Check C library output results and return a nonzero status on output failure,
  retaining the walking skeleton's current behavior.
- Emit `fn main()` with an implicit successful result.
- Emit supported `fn main() int` final values and unconditional integer returns
  through C `main`; keep native exit-code fixtures within the portable test
  range.
- Keep `main(args [str])` valid at the language boundary but explicitly
  unsupported by this milestone's no-argument backend.

Exit criterion: an analyzed program can compute primitive values, print its
observable result, and return a small integer exit status through generated C.

## Phase 5: Integration, tests, and handoff

- Add emitter unit tests for stable generated C, identity-based names,
  precedence preservation, multiple output operations, and unsupported-node
  diagnostics.
- Add compiler tests proving parser and analysis errors still precede backend
  capability errors and no C file is written on failure.
- Add native end-to-end fixtures for arithmetic, comparisons, boolean output,
  local mutation and shadowing, string-print regression, and integer exit
  status; skip only native assertions when no supported C compiler is present.
- Keep test arithmetic inside the temporary unchecked domain documented above.
- Update `AST.md` with the exact analyzed-AST facts consumed by the emitter and
  the capability obligations handed to milestone 5.
- Mark this emitter as temporary and avoid designing APIs that milestone 6's
  typed IR would be forced to preserve.

Exit criterion: the milestone's complete primitive subset builds and executes
through the existing CLI, unsupported valid programs receive stable source
diagnostics, and milestone 5 can add semantic validation without undoing the
backend boundary.

## Implementation constraints

- Keep the compiler dependency-free.
- Invoke the host compiler with argument lists through the existing toolchain
  boundary; generated source never becomes a shell command.
- Keep generated artifacts under `build/` and preserve current CLI paths and
  source, compiler/toolchain, and program error categories.
- Consume decoded literals and stable semantic identities from `Analysis`.
- Emit only expressions whose latest analysis state is resolved or explicitly
  no-value where the surrounding statement permits it.
- Preserve left-to-right observable evaluation. The supported expression
  subset is otherwise side-effect free, so direct C operators cannot reorder
  visible operations.
- Do not add runtime arithmetic checks during this milestone.

## Non-goals

- Floats, characters, general string values, or primitive conversions
- User-defined function emission or calls
- Parameters or command-line argument decoding
- `if`, loops, `switch`, short-circuit expressions with side-effecting calls,
  or general control-flow validation
- Structs, tuples, unions, errors, lists, maps, members, or indexing
- Full mutability, return-path, unreachable-code, or loop-context validation
- Checked arithmetic, bounds checks, runtime source locations, or typed IR
- Runtime layouts, allocation, escape analysis, or garbage collection

## Test requirements

- The exact string-print and empty-string regressions
- Integer literal boundaries used within portable emitted operations
- Unary, arithmetic, remainder, bitwise, shift, comparison, and boolean cases
- Parenthesized combinations whose SAO2 and C precedence could otherwise differ
- Mutable locals, direct and compound assignment, nested blocks, and shadowing
- Multiple `print`/`println` calls with string, integer, and boolean output
- No-value `main`, integer final value, and unconditional integer return
- Analysis failure in an otherwise backend-unsupported program
- Clear rejection of every excluded declaration, signature, statement,
  expression, call, assignment target, and primitive type encountered by the
  emitter
- No generated file on parser, analysis, or backend-capability failure
