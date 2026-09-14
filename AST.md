# Frontend handoff

The parser produces the immutable syntax tree in `src/ast.rs`. Name-and-type
analysis in `src/analysis.rs` and semantic analysis in `src/semantic.rs` attach
all facts required by lowering in separate side tables. A frontend result is
available to lowering only after `semantic::validate_handoff` accepts the
completed `Analysis` and `SemanticResult`.

The handoff validator is a production compiler boundary, not a debug assertion.
It runs after all source diagnostics are empty and before entry-point extraction,
temporary-backend capability checks, C rendering, or filesystem output. A
contradiction becomes a compiler diagnostic, preserves semantic warnings, and
stops compilation without creating or changing generated output.

## Source and syntax ownership

Every syntax node has a half-open UTF-8 byte span into its owning `SourceFile`.
Identifiers retain source spans rather than copied spellings. Literal nodes keep
their source spans while decoded literal values live in analysis. Comments,
whitespace, and semantically irrelevant punctuation are not represented.

The AST preserves source order, explicit type and expression parentheses,
nested-union structure, argument order and form, assignment paths, postfix
operation order, statement-versus-expression `if` forms, and colon-versus-block
bodies. An AST is valid only with the source file from which it was parsed.

## Stable identities and canonical types

Declarations, functions, nominal types, bindings, and deferred obligations have
stable, range-checked identities. Records retain direct references to their AST
subjects and their original spans. Binding identities cover parameters, locals,
and loop bindings and are assigned independently of spelling or shadowing.

Resolved value types are interned in one canonical type table. Primitive and
unit types have fixed canonical identities; list, map, nominal, and anonymous
union types refer to canonical `TypeId` values. Nominal definitions distinguish
structs, tuples, and unions. Struct members retain inline or referenced storage,
tuple members retain positions, and union alternatives retain tags, payload
types, `Error`, and explicit nesting.

`TypeState::Resolved` denotes a language value type. `TypeState::Never` is the
only non-value state permitted at a successful handoff and means evaluation
cannot continue. `Error` and `Deferred` are never valid final states. Latest
annotations supersede earlier inference annotations; lowering always consumes
the latest annotation for a subject. The syntactic callee node of a resolved
direct call is not a first-class value and therefore has no value-type
annotation; its enclosing call's `CallTarget` is the complete fact. Receiver
expressions within method callees remain ordinarily typed.

## Names, values, calls, and construction

Every value-name use records a resolved binding, function, constructor, or
intrinsic identity and its access classification. Lowering does not perform
lexical or top-level lookup. Literal annotations contain the decoded integer,
binary64 float, ASCII string, character, or boolean value.

Explicit `int(expression)` and `float(expression)` conversions have dedicated
syntax nodes rather than ordinary call targets. Each valid conversion has one
resolution record containing its canonical source and destination types; the
only permitted pairs are `float` to `int` and `int` to `float`.

Every call has one final `CallTarget`: a function, constructor, qualified tagged
constructor, special `Error` constructor, intrinsic, or built-in method.
Constructor records identify the selected struct, tuple, tagged union, untagged
union, or error alternative. Struct constructor records map source-order
arguments to declaration members while preserving evaluation order.

Every ordinary value-level member or index access has one resolved projection
record. Struct projections identify the declaration, field ordinal, storage
mode, and result type; tuple projections identify the declaration, position,
and result type; list, map, and string projections retain their canonical
element, key, value, and character types. Built-in method callee members and
qualified union-constructor callees are excluded because their call-target
records are authoritative.

Implicit conversion into an expected union is explicit in `UnionInjection`.
The record identifies the expression, destination union, and selected direct
alternative. An independently retained subject record lets the handoff gate
detect a missing or duplicated injection without repeating expected-type
analysis. Explicitly nested unions are not flattened.

## Assignments and mutation

Every executable assignment target has a resolved root `BindingId`, final type,
and ordered typed path. Path steps distinguish struct members, tuple members,
list indices, and map indices. Member steps retain declaration and member
identity; struct steps also retain inline-versus-referenced storage. Index steps
retain canonical element, key, and value types. Struct steps additionally carry
their declaration-field ordinal.

Semantic mutation authorizations are separate from name/type annotations. Each
authorized assignment, mutating built-in receiver, or object-reaching `var`
argument records its exact AST subject, root binding, `BindingAccess`, operation,
and typed path. The authorization is the completed proof of constant/`var`,
transitive mutability, direct-reference rebinding, and tuple-immutability rules.
The semantic result also retains the classified mutation-subject set, allowing
the handoff gate to prove one-to-one authorization coverage without repeating
those permission rules. No deferred mutation obligation is permitted at handoff.

## Returns and control flow

Every explicit return records its statement, enclosing function, optional value,
latest value state, and any unit-union injection. When fallthrough supplies unit,
a `FunctionCompletion` records the function body and optional unit-union
injection. Omitted results, bare returns, and ordinary empty completion use the
canonical unit type.

Every valid `break` and `continue` records the nearest lexically enclosing loop.
Every block, statement, statement body, expression, expression body, and switch
arm has exactly one final `FlowSummary`. Summaries can contain fallthrough,
return, break, continue, and divergence flags. Multiple exit flags are valid;
`SwitchDependent` fallthrough and unresolved switch dependencies are not.
Unreachable syntax remains represented and validated. Its first construct in
each contiguous region produces a non-fatal source warning.

## Narrowing, switches, and postfix try

Each applicable `is` expression has one `UnionTestResolution` selecting a direct
alternative and payload type, plus the narrowed binding when the operand is an
exact binding place. Each switch has one resolution containing its operand
union, narrowed binding, unique arm alternatives and payloads, covered set,
optional `else`, and exhaustive result. Arm flow has already been recomposed
after coverage is known.

Each reachable postfix `?` has one `TryResolution`. It records the operator
span, operand union, direct source `Error`, successful type, and action. A single
success alternative produces its payload directly; multiple alternatives use a
canonical anonymous union preserving tags and nesting. The action is either
exact-error propagation to the enclosing function or panic in the validated
`main`. A `Never` operand is the sole case in which a try syntax node has no try
record. Try flow contains its success path and the corresponding return or
divergence exit.

## Diagnostics and lowering contract

Source errors and unreachable-source warnings are independently capped and
source-ordered. Warnings do not change successful status and remain attached to
semantic, handoff, and temporary-backend failures.

Milestone 6 typed-IR lowering may consume all facts described here. It must not
repeat source name lookup, literal decoding, type inference, constructor
selection, union injection or label resolution, narrowing, assignability or
mutability validation, switch coverage, postfix-try propagation, loop targeting,
or return-path proof. A missing or contradictory fact is a compiler invariant
failure.

The milestone 4 resolved-AST C emitter remains a separate temporary consumer.
It accepts only its documented primitive subset and may report unsupported valid
syntax after the handoff succeeds. It is not the typed-IR interface and must not
be expanded as part of semantic analysis.

## External verification

Contributor guidance requires Rust 1.90 or newer. Phase 6 remains awaiting
external verification until the following is run outside this implementation
session:

```text
rustc --version
SAO2_CC=cc cargo test
```

Replace `cc` when another supported compiler executable is required; omitting
`SAO2_CC` is also valid when one is found on `PATH`. Native end-to-end
assertions must run rather than skip. Only after that evidence is supplied may
milestone 5 be marked complete and milestone 6 become current.
