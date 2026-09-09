# Current Work: Names and Types

Status: complete.

This document expands milestone 3 of `ROADMAP.md`. The objective is to resolve
the syntax tree into named declarations, bindings, and static types that later
compiler stages can consume without repeating name lookup or type inference.

Milestone 2 supplies the complete parser and the spanned AST documented in
`AST.md`. The existing hello program must continue to compile and run:

```sao2
fn main() {
    print("hello");
}
```

Milestone 4 begins the early primitive C backend. This milestone supplies its
analysis inputs; it does not expand the current emitter's executable subset.

## Scope and analysis boundary

This milestone owns:

- top-level function and nominal-type tables;
- primitive, container, struct, tuple, union, and error type resolution;
- declaration, constructor, member, and storage validation;
- lexical scopes, parameters, locals, and unrestricted local shadowing;
- identification of calls to functions, constructors, and intrinsics;
- literal conversion and expression type inference; and
- expected-type propagation for unions and empty collections.

Type inference necessarily checks the operands and arguments needed to
establish a result type. Reuse those checks in later analysis. Milestone 5 owns
the complete semantic validation pass, including mutability, return paths,
loop context, exhaustive switches, and flow-sensitive union narrowing.

Distinguish a resolved expression type from a fully validated program.
Flow-dependent expressions whose types require milestone-5 narrowing may
retain explicit deferred analysis records. They must never masquerade as
successfully typed values or reach C generation with unresolved types.

## Phase 1: Analysis representation and diagnostics

- Introduce a name-and-type analysis entry point over `SourceFile` and `Program`.
- Add stable identities for declarations, bindings, and resolved types.
- Represent primitive and container types alongside nominal declaration
  identities; distinguish no-value results and non-returning expressions from
  language value types.
- Retain source spans and connections to AST nodes in analysis results.
- Keep analysis annotations separate from parser syntax invariants.
- Reuse bounded, source-ordered diagnostics. Use an internal error state to
  suppress cascading failures without treating an error as a valid type.
- Keep the representation dependency-free and small; a full typed IR belongs
  to milestone 6.

Exit criterion: analysis results can record identities, types, errors, and
deferred work without changing parsing or the existing executable path.

## Phase 2: Top-level declarations and signatures

- Collect all type and function names before resolving declaration bodies.
- Use separate type and value namespaces.
- Diagnose duplicate declarations within their respective namespaces and
  duplicate function parameter names.
- Resolve function parameter and return annotations independently of function
  body order, supporting forward calls and direct or mutual recursion.
- Register compiler intrinsics with explicit identities and signature rules.
- Distinguish ordinary function calls from constructors and built-in calls;
  do not introduce first-class function values or overloads.
- Preserve the compiler boundary's exactly-one-main and four-signature checks;
  reuse resolved signatures when integrating the new analysis.

Exit criterion: every declared function and type has a stable identity and
resolved signature, or a precise declaration diagnostic.

## Phase 3: Type definitions, unions, and storage

- Resolve primitive, named, list, map, and parenthesized type syntax.
- Classify declarations as structs, tuples, or unions according to the design
  and grammar; reject mixed named and unnamed members.
- Give each named type nominal identity even when another declaration has the
  same structure.
- Validate member uniqueness, union alternative uniqueness, tag uniqueness,
  and the special final-position rule for `Error`.
- Respect the designed distinction between tagged alternatives and the special
  error alternative; reject invalid tagged/untagged combinations.
- Ignore alternative order for union type identity while retaining source
  order for diagnostics and preserving explicitly nested union structure.
- Keep `&` as member storage metadata, permitted only for struct-valued
  members; do not construct a first-class reference type.
- Detect recursive inline layouts, including dependencies through tuples and
  union payloads. Referenced struct members and container references break
  inline layout cycles.
- Validate map key types, including recursively composed immutable tuples.

Exit criterion: type definitions resolve independently of source order, valid
referenced recursion terminates, and invalid or infinitely sized types receive
bounded diagnostics.

## Phase 4: Lexical scopes and binding resolution

- Resolve parameters and local references with explicit binding identities.
- Resolve each initializer before introducing its local binding.
- Permit shadowing in nested scopes and within the same scope.
- Traverse braced and colon-form bodies using the scopes defined by the
  language; resolve loop bindings only where they are visible.
- Resolve assignment roots, call targets, and member receivers.
- Preserve declaration mutability and access information for milestone 5.
- Diagnose unknown names without falling through to a different namespace or
  silently substituting an intrinsic.

Exit criterion: each name use resolves to the correct declaration or binding.
For `x := 1; x := x + 1;`, the second initializer refers to the first binding
and subsequent uses refer to the second.

## Phase 5: Literals and expression type inference

- Convert numeric lexemes once, respecting decimal, hexadecimal, binary, and
  separator syntax; retain decoded values for downstream consumers.
- Check literal representability, including the signed minimum integer under
  unary minus and finite binary64 values. Keep runtime arithmetic checks for
  the later lowering milestone.
- Type primitive literals, identifier references, parentheses, and unary and
  binary expressions according to the design's operand rules.
- Record result types for comparisons, boolean operations, indexing, and
  member access; do not import C's implicit conversions or truthiness.
- Resolve struct fields, tuple positions, and built-in container methods.
- Use declared function signatures to infer call results, including recursive
  calls, without inferring a signature from its body.
- Represent intrinsic argument rules and no-value/non-returning results.
- Infer local types and ordinary block and if-expression result types where
  context suffices; keep statements distinct from value-producing expressions.
- Record type-dependent union tests, switch labels, and postfix `?` analysis
  needs for milestone 5 rather than guessing narrowed types.

Exit criterion: ordinary expressions and local declarations have concrete
static types and converted literal payloads. Invalid type combinations receive
source diagnostics; flow-dependent obligations remain explicitly identified.

## Phase 6: Constructors and expected types

- Validate named struct arguments for missing, duplicate, and unknown members.
- Validate positional tuple arguments and declared member types.
- Preserve source argument evaluation order independently of declaration order.
- Resolve untagged union constructors, qualified tagged constructors, and
  `Error(value)`.
- Propagate expected types from parameter signatures, constructor members,
  assignment targets, return annotations, and enclosing collection contexts.
- Record implicit injection into an expected union alternative explicitly.
- Infer nonempty collection types using the design's compatibility rules.
- Require a suitable expected type or explicit ascription for empty lists and
  maps; propagate that context through nested collections.
- Reject incompatible or ambiguous construction without inventing implicit
  numeric conversions, union flattening, or new inference rules.

Exit criterion: constructors and context-dependent expressions resolve with
explicit types and selected union alternatives, or report useful diagnostics.

## Phase 7: Pipeline integration and handoff

- Run name-and-type analysis after parsing and before temporary backend checks.
- Analyze all declarations, including functions the temporary backend cannot
  yet execute.
- Keep the current print-only executable regression working; distinguish name
  and type errors from valid programs outside the backend's supported subset.
- Supply resolved identities, types, literal values, and intrinsic targets to
  milestone 4. Prevent emission of expressions with error or deferred states.
- Document the obligations retained for milestone 5 in the analysis interface.
- Add focused valid and invalid fixtures for each phase; parser conformance
  fixtures are not automatically semantically valid programs.
- Preserve the CLI, output paths, source/tool/program error categories, and
  existing 20-diagnostic limit.
- Update `AST.md` with the concrete analysis handoff once implemented.

Exit criterion: the compiler has a stable name-and-type analysis boundary,
the hello regression remains executable, and later stages can consume resolved
facts without repeating lookup or literal conversion.

Phase 7 is complete. The compiler runs analysis immediately after parsing,
reports its bounded source diagnostics before considering temporary-backend
limits, and uses the analyzed entry-point, intrinsic-call, expression-type, and
decoded-literal records for the retained string-print path. Milestone 4 can
extend that analyzed lowering boundary without repeating frontend work.

## Design questions encountered during implementation

Use `DESIGN.md` and `GRAMMAR.ebnf` as the authorities. Where they leave a
decision unresolved or disagree, record an explicit design decision before
implementing the affected behavior. Known examples to check include:

- primitive conversion calls such as `int(value)`, which the design describes
  but the current primary-expression grammar does not admit;
- collisions between intrinsic names, user declarations, and constructor
  names in call position; and
- any ambiguous declaration classification or union/error construction case
  not determined by the documented rules.

These questions do not authorize unrelated syntax changes or block independent
analysis work.

## Non-goals

- Expanded C emission or native arithmetic execution (milestone 4)
- Complete mutability, control-flow, narrowing, and return-path validation
  (milestone 5)
- Typed IR, evaluation-order lowering, or runtime arithmetic checks (milestone 6)
- Runtime layouts, allocation, escape analysis, garbage collection, or containers
- New language syntax or undocumented implicit conversions

The early backend's temporary unchecked arithmetic does not change this
milestone's static type rules or literal range validation.

## Test requirements

- Forward declarations, recursive calls, duplicate names, and separate namespaces
- Nominal identity, container identity, union ordering, and preserved nesting
- Valid referenced recursion and rejected inline layout cycles
- Invalid member forms, referenced storage, tags, error ordering, and map keys
- Parameter/local lookup, initializer visibility, and same-scope shadowing
- Literal boundaries and operator, call, index, and member result types
- Struct, tuple, union, and error constructors
- Empty collections with and without expected types, including nested contexts
- Explicit deferred states for flow-dependent analysis
- Multiple errors, source ordering, diagnostic caps, and cascade suppression
- Analysis-to-backend handoff and the existing hello end-to-end regression

Add tests alongside each phase. Execute verification only when repository
instructions permit it; record any verification that remains outstanding.

## Definition of done

Milestone 3 is complete when:

- top-level declarations, signatures, and lexical bindings resolve independently
  of source order where the language permits it;
- all documented type forms, member storage rules, and constructors are covered;
- ordinary expressions carry resolved types and converted literal values;
- expected types resolve union injection and empty collections;
- remaining flow-sensitive work is explicit and reserved for milestone 5;
- milestone 4 can consume resolved primitive programs without redoing analysis;
- source diagnostics remain precise and bounded; and
- the existing executable path is preserved, with test coverage added and
  verification status recorded.
