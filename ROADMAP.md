# SAO2 v0 Implementation Roadmap

This roadmap turns the completed language design into a working compiler and
runtime. It describes milestones and outcomes rather than schedules.

Development follows a walking-skeleton approach. A source-to-executable path is
established first and kept working while each compiler layer replaces temporary
code and expands the accepted language subset.

## 1. First end-to-end executable

Status: complete.

This milestone is complete.

- Establish the compiler and runtime source layout.
- Add commands to build and run a single SAO2 source file.
- Select and invoke a supported C compiler.
- Temporarily accept one top-level string-print statement:

  ```text
  print("hello");
  ```

- Parse only the syntax needed by that statement.
- Emit a minimal C `main` that writes the string.
- Compile the generated C into an executable and run it.
- Add an end-to-end test that verifies the executable's output.
- Clearly isolate the temporary top-level syntax so it can be removed when the
  real SAO2 `main` function is supported. It is not part of the language design.

Outcome: the smallest possible SAO2 source file travels through parsing, C
generation, host compilation, executable creation, and successful execution.

## 2. Lexer and parser

Status: complete.

- Replace the temporary parser incrementally while preserving the end-to-end
  print test.
- Implement source files, byte spans, line mapping, and tokens.
- Implement the longest-match lexer and literal decoding.
- Implement recursive-descent declaration and statement parsing.
- Implement Pratt expression parsing with the specified precedence.
- Build the syntax tree defined by `GRAMMAR.ebnf`.
- Recover at safe statement and block boundaries.

Outcome: valid programs produce syntax trees, and invalid programs produce
source-based diagnostics.

## 3. Names and types

Status: complete.

This milestone's completed analysis handoff is documented in `AST.md`.

- Build the top-level function and nominal-type tables.
- Resolve primitive, container, struct, tuple, union, and error types.
- Resolve lexical scopes and unrestricted local shadowing.
- Resolve compiler-provided intrinsic names independently of user functions.
- Validate declarations, constructors, inline members, and referenced members.
- Implement expected-type propagation for unions and empty collections.

Outcome: declarations and expressions have resolved, statically known types.

## 4. Early primitive C backend

Status: complete.

This milestone is complete. Its resolved-AST emitter handoff is documented in
`AST.md`.

- Replace the print-only lowering with a deliberately limited direct emitter
  over the resolved syntax tree.
- Emit a no-argument `main`, integer literals, primitive local variables,
  assignments, basic arithmetic and comparisons, and an integer return value.
- Preserve the existing string-literal `print` path and add only the primitive
  output needed by executable arithmetic tests.
- Emit straightforward signed C arithmetic without runtime checks. During this
  temporary stage, overflow, division by zero, and invalid shifts may inherit
  host C undefined behavior; conformance tests must avoid those cases.
- Keep unsupported valid SAO2 programs as clear source diagnostics rather than
  silently miscompiling them.
- Add end-to-end tests that compile and execute in-range arithmetic programs.
- Keep this resolved-AST emitter visibly temporary so typed-IR generation can
  replace it without preserving its structure.

Outcome: a small, well-defined primitive subset performs useful arithmetic and
runs through generated C immediately after name and type resolution.

## 5. Semantic analysis

Status: complete.

- Type-check expressions, calls, assignments, and returns.
- Enforce constant, `var`, transitive mutability, and tuple immutability rules.
- Validate control flow and value-producing blocks and `if` expressions.
- Implement union narrowing, exhaustive switches, and postfix `?`.
- Check function return paths and entry-point signatures.

Outcome: every accepted program is type-correct and ready for lowering.

## 6. Typed intermediate representation

Status: complete.

- Lower the syntax tree into a small typed IR.
- Make evaluation order, temporary values, and control-flow edges explicit.
- Lower block and `if` values into temporaries and branches.
- Insert overflow, shift, bounds, missing-key, and division checks.
- Assign compact source-location IDs to runtime checks.
- Retain the type and identity of every local and materialized temporary so a
  later backend can derive precise shadow-frame roots without embedding GC
  operations or physical C layout in the IR.

Outcome: language semantics no longer depend on source-level syntax or C
evaluation details.

## 7. Complete core C backend

Status: complete.

- Replace the temporary direct C emitter with typed-IR-based generation.
- Emit C declarations for primitive, tuple, union, and function types.
- Emit functions, expressions, statements, and control flow.
- Emit checked integer and floating-point operations.
- Generate the four supported `main` adapters.
- Compile and execute generated single-file C programs.

Outcome: primitive-only SAO2 programs compile and run end to end.

## 8. Runtime value foundations

Status: complete.

- Implement immutable ASCII string interning.
- Implement tuple construction, copying, equality, and hashing.
- Add the `print`, `println`, and `panic` intrinsics.

Outcome: primitive, string, and tuple programs have complete runtime behavior
and observable output.

## 9. Struct layout and escape analysis

Status: complete.

- Generate layouts for inline and referenced struct members.
- Define the 8-byte packed `owner_ptr` and `member_ptr` reference-struct ABI.
- Reserve separate 4-GiB heap and scoped virtual-address arenas. Tag
  `owner_ptr` using the low bits made available by allocation alignment, then
  reconstruct both native pointers from the selected base.
- Reserve decoded offset zero in both arenas for the all-zero internal null
  reference and keep zero-initialized union storage non-traceable.
- Give proven non-escaping arena allocations scoped lifetimes.
- Generate field-copy routines that preserve destination slot identity.
- Produce context-insensitive escape summaries for functions.
- Process summaries with the direct-callee dependency queue.
- Conservatively give recursive and unresolved cases GC lifetimes.

Outcome: safe scoped allocation works where locally proven, including programs
that pass references to inline structs without letting them escape.

## 10. Garbage collector

Status: complete.

- Implement a non-moving, stop-the-world mark-and-sweep heap.
- Register generated type and allocation-layout descriptors.
- Enumerate precise global, shadow-frame, and temporary roots, with an explicit
  traversal extension point for future container roots.
- Retain native C calls while generating a typed shadow-frame struct and
  traversal callback for each function that can hold roots.
- Link zero-initialized shadow frames for active invocations and traverse their
  generated root fields without inspecting the native C stack.
- Allocate GC-managed storage within the reserved 4-GiB heap arena.
- Mark each referenced object's owning allocation and trace from its exact
  `member_ptr`.
- Deduplicate tracing by `owner_ptr`, `member_ptr`, and referenced layout.
- Retain an allocation when any reference identifies it as its owner.
- Sweep unreachable allocations and handle collection-epoch rollover safely.

Outcome: cyclic object graphs and escaping interior references are reclaimed
safely without exposing allocation placement to programs.

The reserved arenas, packed-reference ABI, exact-interior tracer, and collector
policy are retained in isolated production-runtime probes as well as public
end-to-end coverage.

## 11. Containers

Status: current.

The milestone breakdown is in
[Current Milestone: Containers](CURRENT_MILESTONE.md). The active
implementation plan is in
[Current Stage: Complete Lists and Entry Arguments](CURRENT_STAGE.md).

- Implement lists, maps, indexing, membership, iteration, and mutation.
- Preserve insertion order in maps.
- Implement container and string length operations.
- Trace container storage and contents through the garbage collector.
- Stress unbounded growth, aliasing, and mutation during iteration.

Outcome: programs can perform useful computation with unbounded containers and
the abstract language is Turing-complete.

## 12. Diagnostics and hardening

- Render compile-time diagnostics with primary and related source spans.
- Map every runtime panic to its SAO2 operation and function.
- Distinguish user errors from generated-C compiler failures.
- Add lexer/parser fuzzing and malformed-program tests.
- Run generated programs under C sanitizers where available.
- Add conformance tests for every rule in `DESIGN.md` and `GRAMMAR.ebnf`.
- Measure compilation time, allocation rates, and GC behavior.

Outcome: the v0 compiler is predictable, testable, and ready for real programs.

## Suggested release gates

1. **Syntax complete:** lexing, parsing, source diagnostics, and recovery work.
2. **First arithmetic execution:** resolved primitive arithmetic runs through C.
3. **Frontend complete:** name resolution, typing, and semantic analysis work.
4. **Memory complete:** escape analysis, inline structs, and GC pass stress tests.
5. **Language complete:** all v0 values, containers, unions, and errors work.
6. **v0 release:** conformance suite passes with no known correctness defects.
