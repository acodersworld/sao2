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
