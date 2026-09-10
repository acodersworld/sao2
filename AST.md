# Semantic AST invariants

The parser produces the syntax-only semantic tree in `src/ast.rs`. Comments,
whitespace, punctuation that has no later semantic role, and decoded literal
spelling are not represented. Source order is preserved throughout the tree.

## Source representation

- Every semantic node has a half-open UTF-8 byte span, either directly or in
  the value it contains. A parent span covers its complete source construct.
- `Identifier` stores a span into the owning `SourceFile`; later stages obtain
  its spelling from that source rather than from a second allocation.
- Integer and floating-point expressions retain their lexeme through their
  source span. Conversion and range checking belong to type analysis.
- String and character payloads contain the ASCII bytes decoded by the lexer.
  Their expression spans still cover the original quoted spelling.
- An AST is meaningful only with the `SourceFile` from which it was parsed.

## Declarations and types

- A `Program` contains only function and type declarations.
- Type members explicitly distinguish named members from unnamed members.
  Referenced storage is metadata on a named member; `&` is not a first-class
  type constructor.
- A union owns a fixed boxed slice of alternatives. Unparenthesized chains are
  represented by one `TypeKind::Union` node.
- Every explicit pair of type parentheses produces a
  `TypeKind::Parenthesized` node. Consequently `(A | B) | C` remains distinct
  from `A | B | C` without later reconstruction from source text.
- Tagged alternatives retain both the tag identifier and payload type without
  assuming that their surrounding context is a valid tagged union.

## Expressions and statements

- Assignment is never an expression. An `AssignmentTarget` always has an
  identifier root followed only by member or index suffixes.
- Positional and named arguments are distinct AST forms. The parser preserves
  their order and does not decide whether the call is a function or a type
  constructor.
- `is` has a type operand and is distinct from ordinary binary expressions.
- Parenthesized expressions remain explicit. Postfix operations are nested in
  their source order, making calls, indexing, member access, and `?` chains
  unambiguous.
- Empty braces in expression context are an empty map. Nonempty expression
  braces containing the entry colon are maps; other expression braces are
  block expressions. Braces required by control-flow syntax are always blocks.
- Statement-form and expression-form `if` are separate nodes. An if expression
  always has a final else body; an if statement may omit it.
- A `Block` stores semicolon-terminated expressions in `statements`. Its final
  expression, when present without a semicolon, is stored only in `value`.
- Colon-form control-flow bodies contain exactly one statement. Braced bodies
  contain a `Block`.

## Parser and analysis boundary

Parsing succeeds only when the complete token stream is syntactically valid.
On failure it returns source-ordered diagnostics rather than a partial AST.
Recovery occurs at semicolons, closing braces, and top-level `fn` or `type`
boundaries, and diagnostics are capped at 20.

Milestone 3 may rely on the structural invariants above. Name and type analysis
in milestone 3 and semantic analysis in milestone 5 together own contextual
validation; CURRENT_WORK.md defines the current phase boundaries. Analysis
remains responsible for:

- declaration and member uniqueness, name lookup, and type-name resolution;
- struct-versus-tuple member consistency and union alternative validity;
- referenced-member storage validity and recursive layout checks;
- tagged-versus-untagged union consistency, uniqueness, and `Error` ordering;
- argument form and arity, operator operand types, and collection types;
- mutability and assignment validity beyond the syntactic target shape;
- condition types, return-path checking, and compatible branch values;
- loop context for `break` and `continue`, and switch coverage;
- the remaining executable-entry-point validation currently performed at the
  compiler boundary.

## Name-and-type analysis handoff

`src/analysis.rs` owns semantic facts separately from this syntax tree. Its
entry point takes the owning `SourceFile` and parsed `Program`, assigns stable
typed identities to declarations and binding sites, and retains direct AST
references and spans in its records. Binding identities are assigned in source
order; they do not imply that a binding is visible before its initializer has
been analyzed.

Resolved language value types live in a small type table. Primitive types have
canonical identities and later structural types are interned there. Analysis
states distinguish a resolved value type from no-value, non-returning, error,
and explicitly deferred results, so recovery and later flow-sensitive work
cannot masquerade as successfully typed expressions. Type and expression
annotations, diagnostics, and deferred obligations are all analysis-owned.

Phase 2 adds source-order-independent top-level collection with separate type
and value namespaces. It resolves every function parameter and return type
against the complete type namespace, records parameter binding identities, and
registers `print`, `println`, and `panic` with explicit intrinsic rules. The
result also classifies the existing four-form executable entry point and can
distinguish function, constructor, and intrinsic names in call position.

Name collisions follow the rule recorded in `DESIGN.md`: intrinsic names are
reserved in the value namespace, while a call matching both a type and a value
callable is explicitly ambiguous. Primitive conversion calls are not
registered as intrinsics yet because their documented syntax is not admitted
by the current primary-expression grammar; resolving that documented
discrepancy is separate from this phase.

Phase 3 resolves every type declaration into a nominal struct, tuple, or union
definition. Definition records retain member AST nodes, resolved member types,
tuple positions, and inline-versus-referenced struct storage. Structural type
interning makes union alternative order irrelevant while explicit nested union
nodes remain distinct.

The same pass validates mixed member forms, duplicate fields, union alternative
and tag uniqueness, tagged-versus-untagged form, and final-position `Error`.
Referenced storage is accepted only for struct-valued members. A separate
layout walk follows inline structs, tuples, and union payloads, while referenced
struct members and containers terminate a layout path. Map keys are restricted
to `int`, `str`, `bool`, and nominal tuples recursively composed only of those
valid map-key types.

Phase 4 resolves function bodies with lexical binding stacks. Parameters,
locals, and iteration variables retain stable binding identities and declared
mutability. Every identifier use records its binding or callable identity plus
read, direct-write, receiver-mutation, compound, or call access for later
semantic checks. Local
initializers are walked before their new binding is introduced, so same-scope
shadowing retains access to the previous declaration.

Braced bodies introduce lexical scopes. Colon-form bodies do not introduce a
brace scope, while a temporary loop environment limits an iteration binding to
its body without hiding colon-body locals from the surrounding lexical scope.
Direct call targets distinguish functions, constructors, intrinsics, visible
bindings, ambiguous namespace matches, and unknown names. Assignment roots use
only lexical bindings, so lookup never falls through to a type or intrinsic.
Member and index receivers are traversed, with member selection itself left for
type inference.

Phase 5 decodes literal payloads once and annotates ordinary expressions and
inferred local bindings with `TypeState`. Integer annotations retain the parsed
magnitude so unary minus can admit the signed 64-bit minimum without admitting
that magnitude as a positive value. Floating-point literals retain their
binary64 value and reject non-finite results.

Expression inference applies the language's exact operand rules without C
conversions or truthiness. It resolves primitive operations, comparisons,
membership, indexing, struct fields, tuple positions, declared function calls,
intrinsics, and the documented list, map, and string methods. Method calls are
given explicit built-in identities for later lowering. Assignment targets carry
their selected root, field, or indexed-element type separately from expression
annotations so Phase 6 can propagate that expected type. Blocks and compatible
`if` expressions retain their value type; statement-only and non-returning
results remain distinct from language value types.

Flow-sensitive `is`, `switch`, and postfix `?` work remains explicit through
deferred records.

Phase 6 propagates expected types from function parameters and results,
constructor members, assignment targets, and enclosing collections. Nonempty
lists and maps infer a single compatible element or entry type; empty
collections require either propagated context or an explicit ascription, and
that context continues through nested collections. Expressions that Phase 5
deferred only because a child collection lacked a type are recomputed after the
child resolves.

Constructor annotations identify the selected struct, tuple, or union
alternative. Struct annotations also map each source-order argument to its
declaration-order member, preserving evaluation order without requiring later
stages to repeat name lookup. Tagged alternatives and `Error(...)` retain their
selected payload alternative. Implicit conversion of a value to an expected
untagged union is represented by a separate union-injection annotation rather
than hidden in its expression type. Completed expected-type and constructor
deferred records are marked resolved; only flow-sensitive work remains active.

Phase 7 runs analysis immediately after parsing and before any temporary
backend checks. Analysis diagnostics stop compilation without creating an
output file, including diagnostics from declarations outside the backend's
current executable subset. The compiler boundary uses the analyzed entry-point
classification rather than repeating signature resolution.

## Temporary resolved-AST C emitter

Milestone 4's temporary emitter consumes the analyzed no-argument `main`
signature and traverses its body in source order. Primitive declarations use
the binding identity attached to their statement, identifier reads use their
recorded `NameResolution`, and assignments use both their root binding and
latest `AssignmentTargetAnnotation`. Generated C names depend only on
`BindingId`, never source spelling.

Integer and boolean expressions use their latest resolved expression state.
Integer and boolean literals use decoded `LiteralValue` annotations, including
the separately admitted signed minimum magnitude. Direct `print` and `println`
statements use the recorded intrinsic `CallTarget`; string output additionally
requires a direct decoded string literal. No emitted construct reparses source
text or repeats frontend lookup or inference.

The emitter accepts only primitive locals, lexical blocks, direct assignment
to mutable locals, supported primitive expressions, ordered output statements,
and the structurally evident results of `main`. Unsupported resolved syntax is
a source diagnostic, while a missing or contradictory analysis fact is a
compiler invariant failure. C is generated fully in memory before filesystem
output, so frontend or capability errors cannot create or truncate generated
source.

This boundary is deliberately temporary. Milestone 5 remains responsible for
complete mutability and return-path validation, unreachable and loop-context
checks, narrowing, switch coverage, and resolving flow-dependent expressions.
Milestone 6 will replace direct AST emission with typed IR that makes control
flow, evaluation order, runtime checks, and source locations explicit.

The handoff is intentionally a resolved-AST interface rather than a typed IR.
Milestone 4 may consume the stable declaration and binding identities, type
table, expression and assignment annotations, constructor selections, union
injections, literal values, and call targets directly. Milestone 5 still owns
mutability enforcement, loop-context and return-path validation, exhaustive
switch checking, union narrowing, postfix `?`, and completion of explicitly
flow-dependent records. Such deferred expressions cannot enter C generation.
