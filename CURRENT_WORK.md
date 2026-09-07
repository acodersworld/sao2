# Current Work: Lexer and Parser

This document expands milestone 2 of `ROADMAP.md`. The objective is to replace
the walking-skeleton parser with the permanent lexer, semantic syntax tree, and
parser defined by `GRAMMAR.ebnf` while keeping source-to-executable compilation
working.

The executable regression program becomes valid SAO2 immediately:

```sao2
fn main() {
    print("hello");
}
```

The parser recognizes the formal grammar only. The compiler pipeline, rather
than the parser, requires an executable program to contain a valid `main`.

## Scope

This milestone includes:

- Reusable source byte spans and line indexing
- Structured, multi-error source diagnostics
- The complete longest-match lexer
- A semantic AST without comments or whitespace
- Recursive-descent declaration, type, and statement parsing
- Pratt expression parsing
- Syntax error recovery and parser conformance tests
- Early removal of temporary top-level `print` syntax

Name resolution, static type checking, control-flow validation, and contextual
type constraints remain later milestones unless needed to parse unambiguously.

## Phase 1: Source and diagnostic foundations

- Add a reusable half-open `Span { start, end }` using UTF-8 byte offsets.
- Precompute line-start offsets when loading a source file.
- Store structured source diagnostics with primary spans rather than embedding
  locations into message strings.
- Render the filename, one-based line and column, relevant source line, and a
  caret. Tabs expand to four columns.
- Collect diagnostics in source order and stop after 20 errors.
- Preserve the existing usage, input, compiler, and program error categories.

Exit criterion: source locations and multiple diagnostics are represented and
rendered consistently without changing the CLI pipeline.

## Phase 2: Complete lexer

- Define tokens for every keyword, operator, delimiter, identifier, and literal
  in `GRAMMAR.ebnf`, including an explicit EOF token.
- Apply longest-token matching to overlapping operators.
- Discard whitespace and nonnested line and block comments.
- Recognize keywords only as complete tokens.
- Decode string and character escapes into ASCII bytes and reject invalid or
  non-ASCII contents.
- Enforce lexical numeric forms and underscore placement while retaining
  numeric lexemes for later range and type validation.
- Keep signs as separate operator tokens.

Exit criterion: every valid lexical form produces the expected token stream,
and malformed lexical input produces precise source diagnostics.

## Phase 3: Early permanent-parser slice

- Introduce semantic AST foundations with a span on every node.
- Parse functions, blocks, expression statements, calls, identifiers, and
  string literals.
- At the compiler boundary, require exactly one of the four designed `main`
  signatures.
- Keep the temporary backend limited to a no-argument, no-return `main`
  containing one `print` call with one string literal.
- Update `tests/fixtures/hello.sao2` to use `fn main()`.
- Delete `temporary_parser` and reject top-level statements permanently.

Exit criterion: the permanent lexer and parser drive the existing native
`hello` regression from valid SAO2 source.

## Phase 4: Declarations and types

- Parse function declarations, parameters, optional return types, and type
  declarations.
- Parse primitive, named, list, map, referenced-member, union, tagged, and
  parenthesized types.
- Preserve explicit union parentheses in the AST so nested unions remain
  distinct.
- Represent named and unnamed type members without resolving their meaning.
- Leave uniqueness, mixed-form, `Error` ordering, and storage validity checks
  to name and type analysis.

Exit criterion: all declaration and type productions build complete spanned AST
nodes.

## Phase 5: Expressions

- Implement Pratt parsing using the precedence and associativity defined by the
  grammar and design.
- Parse primitive literals, identifiers, unary and binary operators.
- Parse chained calls, indexing, member access, and postfix `?`.
- Parse positional and named arguments without treating named constructor
  arguments as assignment expressions.
- Parse lists, maps, typed empty collections, parenthesized expressions, block
  expressions, and `if` expressions.
- Resolve map-versus-block braces using expression context and colon structure.

Exit criterion: every expression production builds the intended tree and
operator precedence is captured structurally.

## Phase 6: Statements and control flow

- Parse local declarations, assignment and compound-assignment statements,
  expression statements, return, break, and continue.
- Parse blocks, `if`, `while`, `for`, and `switch`.
- Support both braced and colon-form control-flow bodies.
- Bind `else` to the nearest unmatched `if`.
- Represent a final block value separately from semicolon-terminated discarded
  expressions.
- Keep assignment restricted to statement position.

Exit criterion: every statement and control-flow production produces a
complete spanned AST.

## Phase 7: Recovery and conformance

- Recover at semicolons, closing braces, and top-level `fn` or `type`
  boundaries.
- Avoid duplicate diagnostics for the same failed construct.
- Sort diagnostics by primary source position and cap output at 20 errors.
- Add valid fixtures covering every grammar family.
- Add malformed fixtures covering ambiguous constructs and recovery paths.
- Document the AST invariants and the milestone-3 name-resolution handoff.

Exit criterion: all formal grammar productions are covered, invalid programs
produce useful bounded diagnostics, and the full regression suite passes.

## Core interfaces

```text
Span { start: usize, end: usize }
Token { kind: TokenKind, span: Span }
Program { declarations: Vec<Declaration>, span: Span }
parse(source: &SourceFile) -> ParseResult<Program>
```

Tokens store decoded payloads only where required, notably strings and
characters. Numeric tokens retain source spans and are converted and
range-checked during later analysis. Every AST node carries the span of the
source construct that produced it.

The stable outer boundaries remain:

- `compiler::compile`: loaded source to generated C
- `host_compiler::compile`: generated C to native executable
- `program::run`: native executable to process exit status

## Test requirements

- Exhaustive token, keyword, operator, delimiter, and longest-match tests
- Literal, escape, comment, numeric-separator, and unterminated-input tests
- AST snapshots for each declaration, type, expression, and statement family
- Precedence, postfix chaining, dangling-else, nested-union, and brace tests
- Multiple-error recovery, ordering, and 20-error-limit tests
- Missing, duplicate, and invalid `main` compiler-validation tests
- Permanent `fn main()` source-to-native-executable regression

## Definition of done

Milestone 2 is complete when:

- `temporary_parser` and top-level `print` support are removed.
- The lexer covers every token and lexical edge case in the design.
- The parser covers every production in `GRAMMAR.ebnf`.
- Every token and AST node has an accurate half-open byte span.
- Diagnostics render source lines and carets and recover up to 20 errors.
- The compiler requires a designed `main` signature without making it a parser
  grammar rule.
- The existing build, host compiler, and run interfaces remain intact.
- The complete unit and end-to-end suite passes.
