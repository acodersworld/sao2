# Current Work: Semantic Analysis

Status: complete. All phases implemented and externally verified.

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
  recorded `FunctionSignature`. Accept the four signature shapes defined before
  unit syntax—no arguments or `args [str]`, each with an omitted or `int`
  result—and diagnose a missing function at the empty span at end of file or an
  invalid signature at the function name. Phase 5 later makes omitted and
  explicit `()` results equivalent.
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

This phase proves that each write or mutation is permitted without changing
the parser AST or repeating type inference. It extends the Phase 1 semantic
result with successful authorization records and explicit obligations for
paths whose types remain deferred until union narrowing. The temporary emitter
continues to consume its existing primitive subset and must not become the
owner of these rules.

### Phase 2.1: Record resolved access paths

- Extend assignment-target analysis to retain the resolved root `BindingId`,
  final target state, and ordered typed path steps. Distinguish struct members,
  tuple members, list indices, and map indices; struct steps also retain the
  declaration member and inline-or-referenced storage selection.
- Produce these path steps while name-and-type analysis already resolves the
  target. Semantic validation must not repeat root lookup, member selection,
  tuple-index parsing, index compatibility, or target type inference.
- Add mutability resolutions to the semantic result. Each resolution retains a
  direct reference to its assignment target, method receiver, or call argument,
  its root `BindingId`, the recorded `BindingAccess`, and the operation that was
  authorized.
- Add an internal place classifier for receiver and argument expressions. Accept
  identifiers followed by member or index access and ignore parentheses around
  any such path. Use the existing name, expression-type, call-target, and member
  facts to classify it; constructors, calls, conditionals, block values, and
  other computed expressions have no mutable binding root.
- Treat a missing root, path step, resolved type, or call target as a compiler
  invariant failure when the corresponding name/type work is not explicitly
  deferred.

Exit criterion: every non-deferred assignment target and every expression that
can serve as a mutable place has one identity-based path description, with no
source lookup left for semantic validation or lowering.

### Phase 2.2: Validate assignment

Apply this permission table after name/type compatibility has succeeded:

| Operation | Permission |
| --- | --- |
| Rebind a primitive, string, tuple, or union binding | The root binding must be `var`. |
| Rebind a direct struct, list, or map reference | Either a constant or `var` binding may be rebound. |
| Replace a struct field | The root binding must be `var`. |
| Replace a list or map element | The root binding must be `var`. |
| Replace a tuple member | Always reject the assignment. |
| Mutate an object reached through a tuple member | Permit when the root binding is `var`. |
| Perform compound assignment | Require both read access and the permission applicable to the write. |

- Apply direct-binding rules equally to locals and parameters. Determine whether
  a nominal value is a struct object, tuple value, or union value from its
  resolved definition rather than treating all nominal types alike.
- Classify the final path operation separately from intermediate traversal. A
  tuple step forbids replacing that tuple slot but does not prevent mutation of
  a struct, list, or map subsequently reached through it. Inline and referenced
  struct steps use the same root permission.
- Require a mutable root for every indirect replacement, including a referenced
  struct field that is being rebound. A constant object reference grants
  read-only access to the object even though the direct reference itself may be
  rebound.
- Keep strings immutable. Name/type analysis continues to reject string index
  assignment before the mutability pass.
- Report a missing-`var` diagnostic at the root binding use, tuple replacement at
  the final tuple-member suffix, and an unrooted operation at the complete
  operation span. Add a successful resolution only after all applicable rules
  pass.

Exit criterion: every resolved simple or compound assignment is either recorded
as an authorized rebind or mutation or produces one focused source diagnostic.

### Phase 2.3: Validate mutation-capable calls

- Classify list `append` and `removeIndex` and map `removeKey` as mutating
  receiver calls from their recorded `BuiltinMethod`. Treat list, map, and
  string `len`, indexing reads, membership, and ordinary calls as non-mutating.
- Require a mutating receiver to be a place rooted in a `var` binding. Permit
  member and index traversal through inline fields, referenced fields,
  containers, and tuple members; reject literals, constructors, function
  results, conditional or block results, and other unrooted temporaries.
- Pair each positional user-function argument with its resolved
  `ParameterSignature`. A `var` parameter requires a mutable rooted argument
  only when its declared type can transitively reach a mutable object.
- Define mutable objects as structs, lists, and maps. Recursively inspect tuple
  and union payloads, including anonymous and nominal unions, and use a visited
  set or memoized tri-state result so recursive nominal types terminate. Treat
  primitives and strings as object-free.
- Permit any compatible expression for a `var` parameter whose value is copied
  and cannot reach a mutable object. This includes primitive temporaries and
  object-free tuple or union values. When the type can reach an object, require
  a place rooted in `var`, even though tuple or union copying leaves nested
  object references shared.
- Retain the call expression, argument node, callee parameter binding, and caller
  root binding in each successful mutable-argument resolution. Constructors,
  intrinsics, and built-in methods do not use user-parameter rules.
- Diagnose a constant receiver or argument at its root binding use and an
  unrooted temporary at the receiver or argument expression.

Exit criterion: every mutation-capable built-in call and every object-reaching
`var` argument has a mutable caller root and a recorded authorization; copied
object-free `var` arguments remain unrestricted.

### Phase 2.4: Traverse and defer safely

- Walk all function bodies, statement bodies, block values, and nested
  expressions in deterministic source order. Visit unreachable-looking source
  normally because Phase 3 has not yet established reachability.
- Consume the existing `Write`, `ReadWrite`, `Mutate`, and `ReadMutate`
  classifications for assignment roots. Derive method-receiver and mutable-
  argument operations from resolved call records rather than changing ordinary
  read uses into mutations retroactively.
- If an assignment, receiver, or argument path depends on a
  `FlowDependentType`, retain a deferred mutability obligation tied to its AST
  node, root when known, and existing `DeferredId`. Do not guess a union
  alternative or reject the operation before narrowing.
- Phase 4 must revisit each deferred mutability obligation after it resolves the
  narrowed path. Diagnostic-free completion in Phase 6 requires every such
  obligation to become either an authorization or a source diagnostic.
- Stop before temporary-backend validation when this phase reports a semantic
  error. Do not create or truncate generated C, and retain semantic diagnostics
  ahead of backend capability diagnostics.
- Keep the temporary emitter unchanged. Its direct mutable-primitive-local
  checks remain defensive subset validation and must agree with, but do not
  replace, the semantic authorization.

Exit criterion: all non-flow-dependent writes and mutations are decided in
source order, while every narrowing-dependent case is represented by one
explicit obligation for Phase 4.

### Phase 2.5: Tests and handoff

- Test direct reassignment of constant and `var` primitive, string, tuple,
  union, struct, list, and map bindings, including both local and parameter
  roots.
- Test struct-field and list/map-element replacement through direct paths,
  nested inline and referenced fields, container elements, and tuple members.
- Prove that no root qualifier permits tuple-slot replacement, while a `var`
  root permits mutation of an object reached through a tuple slot.
- Test every mutating built-in on direct, nested, indexed, parenthesized,
  constant, mutable, and temporary receivers; prove that all `len` forms remain
  read-only.
- Test `var` calls with primitives, strings, object-free tuples and unions,
  structs, containers, and composite types that contain or can select an object.
  Include literals and constructors to distinguish permitted copied temporaries
  from rejected object-reaching temporaries.
- Assert the source spans and ordering of missing-`var`, immutable-tuple-member,
  and unrooted-mutation diagnostics, and avoid duplicate diagnostics for one
  failed operation.
- Test the cycle-safe object-reachability predicate with direct objects, nested
  tuples and unions, and recursive nominal definitions that cross valid object
  references.
- Test that union-narrowed assignment and mutation create deferred obligations
  rather than premature diagnostics, ready for Phase 4 to resolve.
- Add compiler coverage proving a Phase 2 error precedes a temporary-backend
  rejection and leaves a missing or pre-existing generated C file untouched.
- Preserve all Phase 1 diagnostics and entry-point tests and all milestone 4
  generated-C assertions unchanged.

Exit criterion: every immediately decidable write, receiver mutation, and
object-reaching `var` argument is proven or diagnosed; all remaining cases are
explicitly narrowing-dependent, and Phase 3 can add flow analysis without
changing the mutability boundary.

## Phase 3: Returns, loops, and reachability

Implement return validation, loop-control resolution, structural reachability,
and unreachable-source warnings as one semantic-analysis change. Do not split
this phase into independently completed sub-phases: its flow facts and warning
behavior form one handoff to union analysis in Phase 4.

- Add dependency-free control-flow flags for fallthrough, function return, loop
  break, loop continue, and divergence. Record summaries for blocks,
  statements, statement bodies, expressions, and expression bodies using direct
  AST references.
- Compose sequential flow by passing only fallthrough paths into the next
  construct while retaining terminal exits. Store the completed summaries in
  `SemanticResult` for later semantic phases and typed-IR lowering.
- Summarize expression evaluation in language order, including collection
  elements, map keys and values, callees, arguments, receivers, and indices.
  Derive block and `if` expression flow from their nested constructs instead of
  relying only on the expression's final `TypeState`.
- Treat a reached `panic` or other genuinely non-returning call as divergence.
  Continue visiting later source for semantic diagnostics even when it cannot
  be evaluated at runtime.
- Model `&&` and `||` structurally: their right operand may be skipped, so a
  divergent right operand cannot make the complete expression unconditionally
  divergent. Correct the name/type result to `bool` when the left operand is
  boolean and normal short-circuit completion remains possible; a divergent
  left operand still makes the expression divergent.
- Treat postfix `?` provisionally as capable of successful fallthrough. Phase 5
  adds its early-return or main-panic exit after resolving the propagation
  action.
- Validate bare `return;` only in no-value functions and valued returns only in
  value-returning functions. Reuse the expression state, expected-type
  propagation, and union-injection records already produced by name/type
  analysis rather than repeating compatibility checks.
- Record every valid explicit return with its statement, enclosing `FunctionId`,
  optional value, and value state. Continue checking invalid return forms even
  when their statements are unreachable.
- Treat a reachable final expression in the outer function body as an implicit
  return when it has a resolved value. Reject a resolved final value in a
  no-value function, but permit final `NoValue` and `Never` expressions because
  they do not produce a value.
- Accept a value-returning function only when every reachable path explicitly
  returns, implicitly returns, or diverges. Diagnose definite fallthrough at the
  function body's closing brace.
- Maintain a lexical stack of enclosing `while` and `for` statements. Resolve
  each valid `break` and `continue` to the nearest loop and retain both AST
  references in a loop-control record. Diagnose either statement when the stack
  is empty.
- Do not push a loop target for `switch`. A `break` or `continue` inside a switch
  may still target an enclosing loop. At a loop boundary, consume body break and
  continue exits while propagating function returns and divergence.
- Treat every `while` and `for` as structurally capable of falling through,
  regardless of a literal condition, the absence of `break`, or a body that
  always terminates. Do not add constant folding or non-termination proofs.
- Retain a flow summary for every switch arm. A switch with an `else` is
  structurally exhaustive; a switch without one includes possible unmatched
  fallthrough until Phase 4 resolves its labels and coverage.
- Diagnose function fallthrough in this phase only when it remains possible if
  every unresolved switch is assumed exhaustive. When switch coverage is the
  only unknown, retain a deferred return-flow obligation for Phase 4 rather than
  reporting a premature error.
- Likewise, keep source after a non-`else` switch conservatively reachable when
  its reachability depends on exhaustiveness. Phase 4 recomposes the stored arm
  summaries and resolves any deferred return or unreachable-source obligation.
- Emit `source warning: unreachable source` at the first unreachable statement
  or final block expression in each contiguous region. Do not warn on individual
  operands, arguments, or other expression children.
- Continue full semantic validation throughout unreachable regions. Analyze each
  nested branch, loop body, switch arm, and block as independently entered for
  its own internal warning regions even when its enclosing construct is already
  unreachable.
- Preserve the independent 20-warning limit and stable source ordering already
  established in Phase 1. Unreachable source remains valid and warnings never
  determine an exit status.
- Update `DESIGN.md` so `build` and `run` print semantic warnings on both success
  and failure. On failure, render warnings first and then the fatal diagnostic;
  the fatal diagnostic alone determines the command's nonzero status.
- Add `CompileFailure { error, warnings }` while retaining the successful
  `CompileOutput { generated_c, warnings }`. Preserve warnings produced beside
  semantic errors and warnings produced before a temporary-backend failure.
  Parser and name/type failures naturally carry no semantic warnings because
  semantic analysis did not run.
- Keep the temporary emitter's accepted subset and generated C unchanged. Its
  existing structural return handling remains lowering logic rather than the
  source of semantic validity.
- Add semantic tests for bare, valued, implicit, incompatible, missing,
  branching, nested, and panicking return paths. Include definite and
  switch-dependent fallthrough and both resolved and deferred function results.
- Test valid and invalid `break` and `continue`, nearest-loop selection, nested
  loops, switches outside loops, and switches nested within loops.
- Test ordered expression flow, nested block returns, and short-circuit
  expressions whose right operand diverges while the complete expression can
  still return `bool`.
- Test one warning per contiguous unreachable region after return, divergence,
  break, and continue. Cover unreachable final block values, independently
  reachable nested bodies, the 20-warning bound, and semantic errors inside
  unreachable source.
- Test switches with `else`, conservative non-`else` reachability, and the exact
  deferred facts handed to Phase 4.
- Add compiler and CLI coverage using a backend-supported primitive program to
  prove real warnings are non-fatal. Also prove warnings precede semantic and
  temporary-backend errors without replacing their exit codes or creating or
  truncating generated output.
- Preserve all Phase 1 and Phase 2 semantic tests and all milestone 4
  generated-C assertions unchanged.

Exit criterion: every definite function path, explicit return, and loop-control
statement is validated; structurally unreachable regions produce stable
warnings; switch-dependent conclusions are explicitly deferred to Phase 4; and
warnings survive both successful and failed compilation without changing the
temporary backend.

## Phase 4: Union tests, narrowing, and switches

This phase is one semantic change. It resolves contextual union operations,
applies their branch-local types, proves switch coverage, and recomposes the
flow facts deferred by Phase 3. Do not split these responsibilities into
separately landed stages: each depends on the same resolved alternative records
and the phase must leave the compiler boundary internally consistent.

- Reserve `Error` for the built-in error alternative used by postfix `?`.
  Reject that exact spelling in every user-defined identifier position,
  including type and function declarations, parameters, locals, members,
  ordinary union tags, and loop bindings. Preserve only `Error(Type)` as the
  final alternative of a union, `Error(value)` as its constructor, and `Error`
  as the contextual `is` or switch label selecting that alternative. Record
  this restriction in `DESIGN.md`, replacing its previous callable-ambiguity
  rule for user declarations named `Error`.
- Resolve every `is` and switch label against the operand's resolved top-level
  union. A bare identifier selects a tag in a tagged union, an ordinary type
  label is resolved through the type namespace for an untagged union, and
  `Error` selects the special error alternative. Match only direct
  alternatives: an explicitly nested union remains one alternative and is not
  flattened while looking for a label.
- Keep contextual label diagnostics in semantic analysis. Diagnose a non-union
  operand, a label absent from that union, a tag label used for an untagged
  union, a type label used for a tagged union, and a repeated resolved switch
  alternative at the most specific operand or label span. Factor ordinary type
  construction and type-namespace lookup out of the existing resolver where
  needed so untagged labels reuse those rules without rerunning source name
  analysis or placing semantic errors in the earlier diagnostic collection.
- Add durable semantic records for each resolved union test and switch. A test
  record identifies its expression, operand union, selected alternative,
  payload type, and exact tested `BindingId` when present. A switch record
  identifies its statement, operand union, exact tested binding, ordered arm
  records, uniquely covered alternatives, optional `else`, and exhaustiveness.
  Each arm record retains the arm source, selected alternative, and payload
  type. Tagged labels are contextual selectors rather than type expressions and
  must not receive fabricated ordinary type annotations. Later phases consume
  these records instead of repeating label lookup.
- Complete a valid `is` expression as `bool`; complete an invalid one with an
  error annotation after reporting its source diagnostic. Mark its existing
  flow-dependent record resolved exactly once. Strip transparent parentheses
  around the complete condition and its operand when recognizing the exact
  binding tested by an `if`, but do not look through any other expression.
- Narrow only the selected body of a direct `is` condition and each explicitly
  selected switch arm when the operand is an exact binding. Negated tests,
  combined boolean conditions, aliases, and the failed tests represented by
  later `else if` conditions do not introduce narrowing. A final `else` body is
  never narrowed. Key the environment by `BindingId`, not spelling, so lexical
  shadowing neither inherits nor destroys another binding's narrowing.
- Apply the payload type to binding reads, member and index receivers, calls,
  method calls, and indirect mutation in the narrowed body. Recompute only
  annotations that were explicitly deferred for flow-dependent input or whose
  already-recorded child type changed because of narrowing; do not repeat
  literal decoding, source name lookup, constructor selection, or unrelated
  inference. Append final annotations through the existing latest-annotation
  convention and resolve each corresponding deferred record once.
- Continue to check a direct assignment to the tested binding against its
  declared union type, then invalidate that binding's narrowing on the
  assignment's fallthrough path. At a control-flow join, retain a narrowing
  only when every reachable fallthrough path preserves the same payload type;
  terminal paths do not participate in the join. Conservatively invalidate it
  after a loop when an executable body path can directly reassign the binding,
  because the loop may execute. Mutation reached through a narrowed member or
  index does not change the union discriminant and does not invalidate the
  narrowing.
- Revisit Phase 2's deferred mutability obligations after their narrowed target
  types become concrete. Move valid operations into the normal authorization
  records and diagnose invalid ones using the original target and operation
  spans. Leave an obligation deferred only when it still depends on postfix
  `?`, which Phase 5 owns.
- Validate switch coverage by resolved alternative identity rather than source
  spelling. Without `else`, require every direct alternative exactly once and
  emit one diagnostic at the switch for non-exhaustive coverage, listing missing
  alternatives in deterministic union order. An `else` supplies the unmatched
  path. If unique explicit arms already cover the union, retain the valid switch
  and emit one unreachable-source warning at the `else` body. Continue checking
  the bodies of missing, invalid, or duplicate arms so independent semantic
  diagnostics are not lost; only uniquely resolved arms contribute to
  coverage.
- Recompose the Phase 3 switch-dependent flow summaries after coverage is
  known. Remove the unmatched path for an exhaustive switch and preserve it for
  an invalid non-exhaustive switch. Resolve deferred function-return,
  final-block-value, and reachability conclusions after all switch dependencies
  they name are concrete. Suppress a cascading missing-return diagnostic when
  the same path is already invalid solely because its switch is non-exhaustive,
  and do not duplicate warnings emitted by Phase 3. Preserve only conclusions
  whose remaining unknown is postfix `?` for Phase 5.
- Add semantic tests for tagged, untagged, nested, and special `Error`
  alternatives; every contextual-label error; duplicate, missing, exhaustive,
  partial-with-`else`, and redundant-`else` switches; and rejection of the
  reserved `Error` spelling in every declaration and binding category. Cover
  direct and parenthesized tests, shadowing, scope exit, negated and combined
  conditions, aliases, failed branches, and unnarrowed `else` bodies.
- Test narrowed reads, member/index access, calls, methods, direct and indirect
  mutation, deferred mutability, assignment invalidation, branch joins, loops,
  and terminal paths. Cover return and reachability results depending on one or
  multiple switches, coexistence of warnings and semantic errors, and absence
  of duplicate warnings. Preserve all earlier semantic tests, CLI ordering and
  no-output-on-error behavior, and milestone 4 generated-C assertions.

Exit criterion: every `is` expression and switch has a final contextual
alternative record; all branch-local types, assignment invalidations, and
related mutability decisions are final; switch coverage is proven or diagnosed;
dependent return and reachability facts are recomposed without duplicate or
cascading diagnostics; and only postfix-`?` work remains deferred to Phase 5.

## Phase 5: Unit type and postfix `?`

This phase is one language and semantic change. It replaces the temporary
absence-of-value state with the first-class unit type needed to express
`Result<(), E>` as `() | Error(E)`, then resolves every postfix `?` against that
model. Do not land unit and try resolution as separate stages: the source and
destination types of `?`, function completion, and the remaining deferred facts
must agree at the end of the change.

- Update `DESIGN.md` and `GRAMMAR.ebnf` before changing behavior. Add `()` as a
  type and expression with exactly one immutable, zero-sized value. Record that
  an omitted function result is `()`, bare `return;` returns `()`, and normal
  function or block completion produces `()`. Keep standalone `Error(Type)`
  invalid: an operation with only unit success and failure uses
  `() | Error(Type)`.
- Give unit ordinary value semantics. It may be stored, passed, returned, used
  in containers and unions, compared for equality, and used as a map key. Unit
  is printable as `()`. A union is printable only when every alternative's
  payload is recursively printable, and it renders the active alternative in
  constructor form: named untagged `Union(payload)`, named tagged
  `Union.Tag(payload)`, anonymous tagged `Tag(payload)`, special
  `Error(payload)`, and an unwrapped payload for an anonymous untagged union.
  Apply the same rules recursively to nested unions.
- Extend the parser AST with explicit unit type and unit expression variants and
  add one canonical unit entry to the resolved type table. Empty parentheses in
  type or expression position select unit; nonempty parentheses retain their
  existing grouping behavior. Annotate a unit expression directly with the
  canonical `TypeId` and preserve its byte span.
- Remove `TypeState::NoValue`. Resolve omitted function results, blocks without
  final expressions, zero-result intrinsics, and mutating container methods to
  the canonical unit type. This deliberately makes formerly discarded results
  real values: they can be bound or passed wherever `()` is expected. Retain
  `Never`, `Error`, and `Deferred` as non-value analysis states.
- Treat `return;` as a synthesized unit value. Accept fallthrough in a function
  returning `()` or a union with one direct unit alternative, recording an
  implicit unit injection for the latter. Extend return/completion facts so
  typed-IR lowering can distinguish an explicit expression, a bare return, and
  closing-brace fallthrough without requiring a fabricated AST expression.
  Continue to diagnose fallthrough when the declared result cannot accept
  unit.
- Accept explicit `()` results for both parameter forms of `main` as equivalent
  to an omitted result; preserve the existing `int` forms. Adapt the temporary C
  emitter's result classification only enough to keep existing omitted-result
  programs working now that they carry a unit `TypeId`. Do not add general unit,
  union, control-flow, or postfix-`?` lowering to the temporary backend.
- Update printable-type and map-key validation for unit. Printing a union must
  validate every possible payload statically, retain enough resolved union and
  nominal identity for the constructor-form renderer planned by typed IR, and
  reject a union if any alternative is not printable. Existing primitive and
  tuple rendering remains unchanged.
- Add a `TryResolution` semantic record containing the postfix expression,
  operand union, selected source `Error` alternative, final success `TypeId`,
  and its action. Add `TryAction::Propagate`, which records the enclosing
  `FunctionId`, destination union, and destination `Error` alternative, and
  `TryAction::Panic`, which records the `main` function. Retain the direct AST
  reference and operator span needed by diagnostics and later lowering.
- Resolve a try operand before its postfix operator and inspect only the
  operand's top-level union. Require exactly one direct `Error` alternative;
  diagnose a non-union operand or a union without `Error` at the `?` span and do
  not add a cascading propagation error. Preserve an operand already typed
  `Never` as `Never` because execution cannot reach the operator. Do not flatten
  explicitly nested unions while finding `Error`.
- Remove the selected `Error` to compute the successful result. One remaining
  alternative produces its payload type directly, including `()` for
  `() | Error(E)`. Multiple remaining alternatives produce an interned
  anonymous union containing the original alternatives, tags, payloads, and
  explicit nesting. Resolve inner tries first so chained `?` operators can
  successively unwrap explicitly nested result unions.
- Outside `main`, require the enclosing function result to be a top-level union
  with one `Error` whose payload has exactly the same resolved `TypeId` as the
  operand's `Error` payload. Do not use ordinary union injection or structural
  widening for error compatibility. In every accepted `main` form, record a
  panic action instead of requiring an error result. If `main` already has an
  invalid signature, use its declaration identity for panic classification and
  avoid a secondary propagation diagnostic.
- Run try resolution after Phase 4 narrowing and before final mutability and
  flow validation. Refactor the flow-dependent expression retyping shared with
  Phase 4 into a post-order helper so a completed try recomputes only affected
  parents, expected-type checks, bindings, assignment targets, calls,
  collections, block/if results, explicit returns, and final function results.
  Preserve ordinary successful-alternative union injections and mark every
  completed deferred record resolved exactly once without repeating name lookup
  or constructor selection.
- Revisit try-dependent mutability obligations after their receivers, indices,
  arguments, and assignment targets become concrete. Move valid operations into
  the normal authorization records and diagnose invalid ones at their original
  spans. A diagnostic-free Phase 5 result must contain no deferred mutability
  obligation whose input was postfix `?`.
- Make flow validation consume try records in evaluation order. Propagation adds
  an early-return path while retaining the success fallthrough path; `main`
  panic adds a diverging path while retaining success fallthrough. Compose these
  flags through calls, operators, returns, final values, branches, loops, and
  switches. Resolve Phase 3 facts whose last unknown was a try result without
  duplicating existing warnings or emitting a missing-return error for a path
  already explained by a try diagnostic.
- Add parser and analysis tests for unit syntax, grouping disambiguation,
  omitted and explicit unit results, bare and explicit unit returns, block
  completion, binding and passing unit, unit containers, equality, map keys,
  printing `()`, and recursive constructor-form printing of tagged, untagged,
  nominal, anonymous, `Error`, and nested unions. Prove that unions with any
  non-printable payload remain non-printable.
- Add semantic tests for one and multiple success alternatives, unit success,
  nominal and anonymous unions, exact and mismatched error payloads,
  nested-only errors, non-union operands, missing errors, chained tries, and
  `Never` operands. Cover propagation from explicit returns, bare returns,
  implicit unit fallthrough, and final values; panic actions for every accepted
  `main` form; try-dependent parent expressions, mutability, and flow; and the
  absence of unresolved try records after successful analysis.
- Preserve diagnostic ordering, warning behavior, no-output-on-error behavior,
  and the milestone 4 source-to-executable path. Valid unit or try programs
  beyond the primitive emitter must reach its temporary unsupported-feature
  diagnostic rather than changing generated-C behavior early.

Exit criterion: `()` is the single value produced by omitted results, bare
returns, and ordinary empty completion; unit and printable unions have final
semantic types; every reachable postfix `?` has a selected error, final success
type, and explicit propagate-or-panic action; all dependent expression,
return, mutability, and flow facts are concrete; and no unit/no-value
contradiction or try-dependent deferred record remains.

## Phase 6: Integration, tests, and handoff

This phase is one integration change. It turns the completed semantic passes
into a checked lowering boundary, fills only the remaining test-coverage gaps,
and records the exact contract consumed by milestone 6. Do not split invariant
validation, pipeline integration, tests, and handoff documentation into
separate stages: milestone 5 is complete only when they describe and verify the
same result.

- Add one explicit semantic-handoff validator over the completed `Analysis` and
  `SemanticResult`. Invoke it only after name/type and semantic source
  diagnostics are empty, and before the compiler extracts `main` or calls the
  temporary emitter. Retain it as the mandatory gate that milestone 6 typed-IR
  lowering must use rather than relying on debug-only assertions.
- Return a `Diagnostic::compiler` when a diagnostic-free semantic result
  violates its internal contract. Do not panic, turn the contradiction into a
  source error, continue into backend capability checks, or write generated
  output. Preserve already collected source warnings on this compiler-failure
  path exactly as for semantic and temporary-backend failures.
- Traverse the immutable program once to establish the subjects expected at the
  handoff. Require every declaration, binding, type node, expression, assignment
  target, call, constructor, and implicit or explicit union injection needed by
  lowering to have its corresponding stable identity and latest annotation.
  Permit only resolved value types or `Never` as final expression states; a
  diagnostic-free handoff containing `Error` or `Deferred` is a compiler
  invariant failure. Assignment targets which can execute must have concrete
  resolved paths and types.
- Require all `DeferredAnalysis` records to be resolved and both deferred
  mutability and deferred flow collections to be empty. Replace the current
  unconditional assertions for these properties with the handoff validator's
  recoverable compiler diagnostic, while retaining narrow internal assertions
  for conditions which cannot be reached through the compiler boundary.
- Validate the entry-point fact against the resolved function namespace and
  accepted `main` signature. Its `FunctionId` must identify the unique source
  declaration already classified by semantic analysis; omitted and explicit
  unit results are equivalent, and the integer forms retain their existing
  meaning.
- Check semantic record coverage and uniqueness by AST subject. Every applicable
  `is` expression, switch, and reachable postfix `?` must have exactly one
  contextual resolution; a `Never` try operand remains the documented
  exception. Every explicit return, implicit unit completion, `break`, and
  `continue` must have its matching function or loop fact. Every block,
  statement, statement body, expression, expression body, and switch arm must
  have one final flow summary.
- Check each semantic record internally without re-performing source-language
  analysis. Declaration, function, binding, type, and deferred identifiers must
  be in range; retained AST references must belong to the current program;
  recorded types must agree with the latest annotations; selected union
  alternatives must belong directly to their recorded union; switch coverage
  must agree with its unique arms and `else`; and try source/destination errors,
  success type, and propagate-or-panic action must agree with the enclosing
  function.
- Require every mutation subject classified by the semantic pass to end in one
  concrete authorization record. Its root binding, access classification,
  operation, storage/member identity, and final typed path must agree with the
  corresponding name and assignment annotations. Use coverage facts retained
  during mutability validation so the handoff check does not repeat
  assignability or transitive-mutability decisions.
- Validate return and flow consistency. Explicit and synthesized unit returns
  must agree with their enclosing function and recorded union injection;
  closing-brace completions must exist exactly where fallthrough supplies unit;
  loop controls must target their nearest recorded loop; exhaustive switches
  must have no unmatched fallthrough; and try flow must contain success plus the
  recorded return or divergence action. Flow summaries may contain multiple
  exit flags but no unresolved switch dependency.
- Keep compiler orchestration ordered as parse, name/type analysis, semantic
  analysis, handoff validation, temporary-backend capability validation,
  in-memory C generation, and only then filesystem output. Source diagnostics
  stop before the handoff check, handoff contradictions stop before the backend,
  and valid programs outside the primitive subset continue to receive the
  existing temporary-backend source diagnostic.
- Audit the Phase 1–5 unit tests against this document's consolidated test
  requirements instead of duplicating already-covered cases. Add focused gaps
  for unit completion and injection, mutability coverage, return and loop facts,
  narrowing invalidation, switch coverage and recomposed flow, printable union
  classification, exact error propagation, `main` panic actions, warning
  coexistence, and final resolution of all deferred records.
- Add handoff-validator tests which deliberately remove, duplicate, corrupt, or
  cross-link each category of required fact. Cover a missing entry point, stale
  or deferred latest annotations, unresolved obligations, invalid identities,
  foreign AST subjects, missing union/switch/try records, contradictory
  alternatives and actions, absent mutation authorization, invalid loop
  targets, and incomplete flow coverage. Each case must produce a compiler
  diagnostic rather than a panic, source diagnostic, or temporary-backend
  message.
- Add compiler-pipeline tests proving that invariant failures preserve warnings,
  precede temporary-backend validation, and neither create a build directory nor
  create, truncate, or replace existing generated output. Retain the existing
  tests proving parser and name/type failures skip semantic analysis, semantic
  source errors precede backend limits, and warnings remain non-fatal on both
  supported and unsupported programs.
- Preserve every milestone 4 generated-C assertion and native primitive test.
  Do not broaden the temporary emitter to lower unit values, unions, control
  flow, narrowing, switches, or postfix `?`; changes to it are limited to
  consuming the validated entry point and maintaining its current primitive
  output byte for byte.
- Rewrite `AST.md` as the completed milestone 5 handoff rather than a sequence
  of future phase obligations. Document canonical types and latest annotations,
  declaration and binding identities, literals, calls, constructors, union
  injections, typed assignment paths, mutation authorizations, explicit and
  implicit returns, loop targets, flow summaries, narrowing and switch records,
  try actions, warnings, and the permitted `Never` state. Remove stale text that
  assigns outstanding work to Phase 4 or Phase 5.
- State in `AST.md` that milestone 6 may consume these facts but must not repeat
  source name lookup, literal decoding, type inference, constructor selection,
  union-label resolution, narrowing, mutability validation, switch coverage,
  try propagation, or return-path proof. Keep the temporary resolved-AST
  emitter contract visibly separate from this future typed-IR input.
- Record the external verification commands required by contributor guidance.
  Do not mark the phase or milestone complete without supplied evidence that the
  full test suite passes on Rust 1.90 or newer and native assertions pass when a
  supported C compiler is selected or found. Until then, report Phase 6 as
  awaiting external verification rather than claiming an unverified completion.
- After that evidence is available, set this document to all phases complete,
  mark milestone 5 complete in `ROADMAP.md`, and leave milestone 6 as the next
  implementation target. Do not introduce typed IR, runtime checks, C layouts,
  or additional language behavior as part of this status transition.

Exit criterion: every diagnostic-free frontend result passes the explicit
handoff validator and contains the complete, internally consistent facts needed
by typed-IR lowering; contradictions are reported as compiler errors without
losing warnings or touching output; `AST.md` describes the completed interface;
external verification has passed; and the primitive walking skeleton remains
byte-for-byte unchanged.

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
