# Current Work: Semantic Analysis

Status: planned.

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
- Postfix `?` removes the top-level `Error` alternative. One remaining success
  alternative produces its payload type directly; multiple remaining success
  alternatives produce their union while preserving their tags and nesting.
- Successful `build` and `run` commands render source warnings to standard error
  without changing their normal exit status.

## Phase 1: Pass boundary, diagnostics, and entry points

- Add a distinct semantic pass after existing name-and-type analysis and before
  temporary backend validation.
- Keep fatal source diagnostics separate from non-fatal source warnings. Render
  both with filenames, byte-derived line and column information, source lines,
  and carets; warnings use the `source warning` category.
- Bound warnings independently from errors and retain deterministic source
  ordering within each collection.
- Return successful compilation output together with warnings so the CLI can
  print warnings to standard error before invoking the host compiler.
- Move missing, duplicate, and invalid executable-entry-point diagnostics into
  semantic validation. The compiler boundary consumes the validated
  `EntryPoint` and does not repeat signature rules.
- Stop before semantic analysis when earlier name/type diagnostics exist, and
  stop before C emission when semantic errors exist.

Exit criterion: a valid primitive program reaches the unchanged emitter,
entry-point failures are semantic source diagnostics, and a synthetic semantic
warning is visible without failing compilation.

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
