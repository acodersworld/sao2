# Current Work: Semantic Analysis

Status: Phase 1 complete; Phase 2 not started.

This document expands milestone 5 of `ROADMAP.md`. The objective is to turn
milestone 3's resolved names and types into a complete semantic proof before
milestone 4's temporary C emitter or milestone 6's typed-IR lowering sees a
program. The source-to-executable path must remain working after every phase.

Milestone 4 is complete. Its resolved-AST emitter remains deliberately limited
to primitive programs and is not expanded here. Semantic errors must precede
temporary-backend capability diagnostics, warnings must not stop compilation,
and no generated C file may be created or truncated after a semantic failure.

## Semantic boundary

The existing name-and-type analysis already owns declaration identities,
lexical binding resolution, static types, constructor selection, expected-type
propagation, assignment target types, and explicit deferred records. This
milestone adds a flow-sensitive semantic pass over those facts. It must not
repeat source name lookup, literal decoding, constructor selection, or ordinary
type inference.

The completed handoff must establish that:

- every binding access is permitted by constant, `var`, transitive mutability,
  and tuple immutability rules;
- every return agrees with its function and every reachable path through a
  value-returning function produces a value or does not return;
- `break` and `continue` occur only inside a lexically enclosing loop;
- every union test and switch label selects one concrete alternative;
- switch coverage is exhaustive unless an `else` arm is present;
- narrowed binding uses have final payload types within their branch or arm;
- every postfix `?` has a resolved success type and propagation action; and
- no diagnostic-free result retains an unresolved deferred record.

Semantic facts remain separate from the parser AST. New records must retain
direct AST references, stable semantic identities, and source spans needed by
typed-IR lowering. A missing or contradictory prerequisite annotation is a
compiler invariant failure, not a source-language error.

## Design decisions to record

Phase 1 updates `DESIGN.md` with these decisions before implementing behavior:

- Statically unreachable source is valid. The compiler emits a source warning
  and continues compilation.
- Reachability is structural during this milestone. Do not infer that a loop is
  non-terminating from a literal condition or perform general constant folding.
- Emit one warning at the first construct in each contiguous unreachable region.
- Cap source warnings at 20 independently of the existing 20-error limit. Sort
  each collection by source position while retaining insertion order for equal
  spans.
- Postfix `?` removes the top-level `Error` alternative. One remaining success
  alternative produces its payload type directly; multiple remaining success
  alternatives produce their union while preserving their tags and nesting.
- Successful `build` and `run` commands render source warnings to standard error
  without changing their normal exit status.

## Phase 1: Pass boundary, diagnostics, and entry points

The current pipeline parses, performs name-and-type analysis, validates `main`
at the compiler boundary, and then invokes the temporary emitter. `Analysis`
also retains an `EntryPoint` classification even though the compiler turns that
classification into diagnostics. This phase replaces that split ownership with
one semantic-validation boundary. It establishes the result and diagnostic
plumbing used by later phases without implementing their semantic rules early.

### Phase 1.1: Record the semantic contract

- Update the diagnostics section of `DESIGN.md` before changing behavior. Replace
  the statement that the language has no warnings with the decisions above:
  only statically unreachable source warns in v0, warnings are non-fatal,
  reachability is structural, and each contiguous unreachable region produces
  one warning.
- Record that source errors and warnings use the same byte-span rendering, have
  independent limits of 20, and are ordered stably by source position within
  their separate collections.
- Record the successful postfix-`?` result rule and the requirement that
  successful `build` and `run` commands print warnings to standard error without
  changing their normal status.
- Keep warning policy beyond unreachable source out of scope. Phase 1 adds no
  production warning condition; Phase 3 remains responsible for detecting
  unreachable source.

Exit criterion: `DESIGN.md` no longer contradicts the milestone's warning and
postfix-`?` decisions, and later phases do not need to choose those semantics.

### Phase 1.2: Separate source errors and warnings

- Add a distinct source-warning diagnostic kind and constructor. Reuse
  `SourceFile::diagnostic_excerpt` and the existing primary-span representation
  so `source warning` renders the filename, one-based line and column, expanded
  source line, and caret exactly as a source error does.
- Retain separate error and warning collections. Each collection accepts at most
  20 entries, and filling one collection must not prevent additions to the
  other.
- Sort each rendered collection by primary span with stable insertion order for
  equal spans. Do not merge errors and warnings into one severity-sorted list.
- Preserve the exit codes and rendering of usage, input, source, compiler, and
  program errors.

Exit criterion: errors and warnings have independent bounded storage and stable
source rendering, and introducing warnings changes no existing error output.

### Phase 1.3: Establish semantic analysis and validate the entry point

- Add a distinct semantic module whose entry point receives the completed
  `Analysis`. Give it mutable access so later phases can complete deferred and
  flow-dependent annotations without replacing the boundary introduced here.
- Return a semantic result containing separate diagnostics and warnings plus an
  optional validated `EntryPoint`. The validated entry point wraps the stable
  `FunctionId`; its absence is permitted only when semantic diagnostics exist.
- Invoke the semantic pass only when name-and-type diagnostics are empty. A
  missing prerequisite annotation in such an input is a compiler invariant
  failure rather than a recoverable source diagnostic.
- Look up `main` through the resolved function namespace and validate its
  recorded `FunctionSignature`. Accept exactly the four forms in `DESIGN.md` and
  diagnose a missing function at the empty span at end of file or an invalid
  signature at the function name.
- Leave duplicate functions, including duplicate declarations named `main`, in
  name-and-type analysis. Its existing duplicate-function diagnostic stops the
  pipeline before semantic analysis; do not add a second duplicate-entry-point
  diagnostic or special-case `main` in name collection.
- Remove the name-and-type `EntryPoint` classification and the compiler's
  `validate_main` helper after semantic validation owns the successful
  `EntryPoint`. Downstream code may retrieve its resolved signature by
  `FunctionId` but must not repeat spelling, parameter, or result checks.
- Do not implement mutability, control-flow, narrowing, switch, or postfix-`?`
  validation in this phase.

Exit criterion: every diagnostic-free semantic result contains one validated
entry point, while missing and invalid entry points are semantic source errors
and duplicate `main` declarations remain earlier name errors.

### Phase 1.4: Rewire compiler and CLI orchestration

- Make the enforced order parse, name-and-type analysis, semantic analysis,
  temporary-backend validation, complete in-memory C rendering, and only then
  filesystem output.
- Return successful compilation as a record containing the generated C path and
  semantic warnings. The CLI renders those warnings to standard error before
  `--show-c` processing or host-compiler invocation, then continues normally.
- Stop before semantic analysis after parser or name/type failure. Stop before
  temporary-backend validation and C rendering after semantic failure. Preserve
  an existing generated C file and do not create the build directory on any
  fatal frontend failure.
- Pass the signature selected by the validated `EntryPoint` to the unchanged
  temporary emitter. Do not broaden its supported subset or alter generated C.
- Keep source failures, host-compiler failures, and program failures in their
  existing categories. Warnings never determine an exit code.

Exit criterion: a valid primitive program reaches the unchanged emitter,
frontend failures cannot create or truncate generated output, and successful
warnings are reported before downstream compilation without preventing it.

### Phase 1.5: Tests and handoff

- Add diagnostic tests for source-warning category and excerpt rendering,
  independent 20-entry error and warning bounds, source ordering, and stable
  insertion order for equal spans.
- Add semantic tests for all four valid `main` forms, missing `main`, each
  invalid parameter or result form, and the invariant that a successful result
  contains a validated `EntryPoint`.
- Update existing duplicate-main coverage to expect the earlier duplicate
  function diagnostic, and prove that name/type errors prevent semantic
  invocation.
- Add compiler tests proving semantic errors prevent temporary-backend
  validation, do not create an output file, and do not truncate an existing
  file. Retain the current tests proving name/type errors precede temporary
  backend capability diagnostics.
- Keep generated-C assertions for valid primitive programs unchanged so this
  infrastructure phase cannot alter the walking skeleton.
- Add an internal test-only semantic-result injection seam and a capturable
  standard-error sink for orchestration tests. Inject a synthetic warning,
  assert that it is rendered before the downstream compiler callback, and
  assert that the command retains its normal successful status. Do not expose a
  sentinel source construct, environment variable, or production-only fake
  warning.

Exit criterion: the pass ordering and entry-point ownership are covered at
their boundaries, a synthetic semantic warning is visible without failing
compilation, and Phase 2 can add facts to the semantic result without another
pipeline redesign.

## Phase 2: Mutability and assignability

- Classify assignment and receiver paths from their recorded root `BindingId`,
  access kind, target annotation, and resolved types.
- Require `var` to reassign primitive, string, tuple, and union bindings. Preserve
  the language rule that both constant and `var` object references may be
  rebound, while only `var` grants mutation of the referred object.
- Require `var` for struct-field replacement, list or map index assignment, and
  mutating list or map methods. Propagate that permission through referenced and
  inline struct fields and through object references reachable from composite
  values.
- Reject replacement of tuple members regardless of the root qualifier. A
  mutable root may still mutate an object reached through a tuple member; it may
  not replace that tuple slot. Strings remain immutable and have no mutating
  element operation.
- Validate compound assignment as both a read and a write after ordinary operand
  typing has succeeded.
- For a `var` parameter, require a mutable argument only when the parameter type
  can transitively reach mutable objects. Primitive and object-free tuple values
  are copied and therefore do not require a mutable caller binding.
- Reject mutation-capable calls on temporaries or other expressions without a
  mutable binding root.

Exit criterion: every recorded write, read-write, mutation, and receiver
mutation is either authorized by one explicit rule or has a source diagnostic.

## Phase 3: Returns, loops, and reachability

- Compute a conservative control-flow summary for blocks, statement bodies,
  conditional branches, switches, loops, return statements, and non-returning
  expressions. Distinguish fallthrough, function return, loop break, and loop
  continue while analyzing nested constructs.
- Validate bare returns only in no-value functions and valued returns only in
  value-returning functions. Reuse expected-type and union-injection annotations
  when checking returned values.
- Treat a reachable final function-body expression as the implicit result. Reject
  a final value in a no-value function and reject every reachable value-function
  path that reaches the closing brace without a value.
- Count `panic` and other `Never` expressions as terminating paths. Treat `while`
  and `for` as capable of falling through even when their bodies always
  terminate.
- Validate `break` and `continue` using lexical loop depth; a switch does not
  establish a loop context.
- Continue fully checking unreachable constructs for semantic errors. Warn only
  at the first statement or final block value in each contiguous unreachable
  region, then resume normal warning detection inside independently reachable
  nested bodies.

Exit criterion: value-returning functions have proven result paths, loop-control
statements have valid targets, and unreachable code remains valid with stable
warnings.

## Phase 4: Union tests, narrowing, and switches

- Resolve `is` and switch labels contextually against the tested union. Bare
  identifiers select tags in tagged unions, `Error` selects the error
  alternative, and ordinary type labels select alternatives in untagged unions.
- Diagnose a non-union operand, a label absent from the union, a tag/type form
  inappropriate for the union style, and duplicate switch alternatives.
- Record the selected union, alternative, payload type, and tested binding when
  one exists. Later stages must not repeat contextual label lookup.
- Narrow only an exact binding operand in the selected `if` branch or switch arm.
  Parentheses around that binding may be ignored, but negated tests, combined
  boolean conditions, aliases, and prior failed branches do not introduce
  narrowing. An `else` body is not narrowed.
- Apply narrowing to reads, member/index receivers, calls, and indirect mutation
  inside the body. Direct reassignment of the tested binding continues to use
  its declared union type.
- Require each union alternative exactly once when no `else` arm exists. An
  `else` makes a partial switch exhaustive; if explicit arms already cover all
  alternatives, retain the valid switch and warn that its `else` is unreachable.
- Use exhaustive switch-arm flow summaries when proving function return paths.

Exit criterion: all `is` expressions and switches have concrete alternative
records, branch-local expressions have final types, and switch coverage is
proven.

## Phase 5: Postfix `?`

- Require the operand's top-level type to be a union containing exactly one
  `Error` alternative. Do not flatten explicitly nested unions while searching.
- Compute the successful result by removing `Error`: unwrap one remaining
  alternative to its payload type, or intern the union of multiple remaining
  alternatives with their existing tags and structure.
- Outside `main`, require the enclosing function result to contain a compatible
  top-level `Error` alternative and record an early-return propagation action.
- In either valid form of `main`, record an action that converts the error case
  into the language's required runtime panic rather than an ordinary return.
- Diagnose `?` on non-unions, unions without exactly one `Error`, and functions
  whose result cannot propagate that error.
- Recompute expressions whose only deferred input was a newly resolved test,
  narrowed use, switch, or try expression. Mark each completed deferred record
  resolved exactly once.

Exit criterion: every postfix `?` has a final expression type, selected error
alternative, and explicit propagate-or-panic action.

## Phase 6: Integration, tests, and handoff

- Add a final invariant check: if semantic errors are absent, every expression,
  assignment target, union operation, and deferred record needed by lowering has
  a final annotation. Report contradictions as compiler errors.
- Add semantic unit tests for all rules below and compiler tests for diagnostic
  ordering, warning behavior, and no-output-on-error behavior.
- Preserve all milestone 4 generated-C snapshots and native tests unchanged.
  Programs valid under semantic analysis but outside the primitive backend must
  continue to receive temporary-backend diagnostics.
- Update `AST.md` with control-flow, mutability, narrowing, switch, and try facts
  available to milestone 6. Keep the milestone 4 direct-emitter handoff visibly
  temporary.
- Mark milestone 5 complete only after the full suite passes under the required
  Rust toolchain and native C tests pass when a supported compiler is available.

Exit criterion: every accepted source program is semantically valid and ready
for typed-IR lowering, while warnings remain non-fatal and the walking skeleton
continues to execute its primitive subset.

## Implementation constraints

- Keep the compiler dependency-free.
- Preserve the immutable parser AST and store semantic results in analysis-owned
  side tables.
- Use stable declaration and binding identities rather than source spellings
  after contextual union-label resolution.
- Preserve byte-oriented source locations and source/program/toolchain failure
  categories.
- Do not create generated files until parsing, name/type analysis, semantic
  validation, and temporary-backend capability validation all succeed.
- Do not add typed IR, runtime arithmetic checks, new C emission, or runtime
  layouts during this milestone.

## Non-goals

- Expanding the temporary C backend beyond milestone 4's primitive subset
- Lowering control flow or postfix `?` into C
- Runtime overflow, shift, bounds, division, missing-key, or source-location checks
- Escape analysis, storage placement, garbage collection, or runtime type layouts
- Constant propagation or proof of non-terminating loops
- General warning policy beyond statically unreachable source

## Test requirements

- Constant and `var` direct assignment across primitive, tuple, union, struct,
  list, map, and string bindings
- Struct member and container mutation through direct, inline, referenced, tuple,
  and union-narrowed paths
- Mutating methods and transitive `var` function arguments, including temporaries
- Bare, valued, implicit, incompatible, partial, exhaustive, and panicking return
  paths
- Valid and invalid nested `break` and `continue`
- One source warning per contiguous unreachable region, warning source rendering,
  successful CLI continuation, and independent error/warning bounds
- Tagged, untagged, `Error`, nested, unknown, duplicate, partial, exhaustive, and
  redundant-`else` union switches
- Branch-local narrowing for statements and expressions without leakage into
  later branches or following statements
- Postfix `?` with one success, multiple successes, incompatible errors, missing
  errors, nested errors, and both valid `main` forms
- Parser and name/type failures preceding semantic work; semantic failures and
  warnings preceding temporary-backend diagnostics
- No generated file creation or truncation after any fatal frontend failure
- No unresolved deferred records in a diagnostic-free completed analysis
