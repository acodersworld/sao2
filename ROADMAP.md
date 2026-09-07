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

Status: next.

The active breakdown for this milestone is in
[Current Work: Lexer and Parser](CURRENT_WORK.md).

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

- Build the top-level function and nominal-type tables.
- Resolve primitive, container, struct, tuple, union, and error types.
- Resolve lexical scopes and unrestricted local shadowing.
- Validate declarations, constructors, inline members, and referenced members.
- Implement expected-type propagation for unions and empty collections.

Outcome: declarations and expressions have resolved, statically known types.

## 4. Semantic analysis

- Type-check expressions, calls, assignments, and returns.
- Enforce constant, `var`, transitive mutability, and tuple immutability rules.
- Validate control flow and value-producing blocks and `if` expressions.
- Implement union narrowing, exhaustive switches, and postfix `?`.
- Check function return paths and entry-point signatures.

Outcome: every accepted program is type-correct and ready for lowering.

## 5. Typed intermediate representation

- Lower the syntax tree into a small typed IR.
- Make evaluation order, temporary values, and control-flow edges explicit.
- Lower block and `if` values into temporaries and branches.
- Insert overflow, shift, bounds, missing-key, and division checks.
- Assign compact source-location IDs to runtime checks.

Outcome: language semantics no longer depend on source-level syntax or C
evaluation details.

## 6. Complete core C backend

- Replace the temporary direct C emitter with typed-IR-based generation.
- Emit C declarations for primitive, tuple, union, and function types.
- Emit functions, expressions, statements, and control flow.
- Emit checked integer and floating-point operations.
- Generate the four supported `main` adapters.
- Compile and execute generated single-file C programs.

Outcome: primitive-only SAO2 programs compile and run end to end.

## 7. Runtime value foundations

- Implement immutable ASCII string interning.
- Implement tuple construction, copying, equality, and hashing.
- Add the `print`, `println`, and `panic` intrinsics.

Outcome: primitive, string, and tuple programs have complete runtime behavior
and observable output.

## 8. Struct layout and escape analysis

- Generate layouts for inline and referenced struct members.
- Add stable GC timestamps to every struct and inline subobject.
- Generate field-copy routines that preserve destination GC metadata.
- Produce context-insensitive escape summaries for functions.
- Process summaries with the direct-callee dependency queue.
- Conservatively heap-allocate recursive and unresolved cases.

Outcome: safe stack allocation works where locally proven, including programs
that pass or return references to inline structs.

## 9. Garbage collector

- Implement a non-moving, stop-the-world mark-and-sweep heap.
- Register generated type and allocation-layout descriptors.
- Enumerate precise global, stack, temporary, and container roots.
- Mark exact referenced structs and recursively trace reachable values.
- Retain a root allocation when any contained struct has the current timestamp.
- Sweep unreachable allocations and handle collection-epoch rollover safely.

Outcome: cyclic object graphs and escaping interior references are reclaimed
safely without exposing allocation placement to programs.

The subobject-marking and allocation-sweeping mechanism should also receive an
early isolated prototype before the full runtime depends on it.

## 10. Containers

- Implement lists, maps, indexing, membership, iteration, and mutation.
- Preserve insertion order in maps.
- Implement container and string length operations.
- Trace container storage and contents through the garbage collector.
- Stress unbounded growth, aliasing, and mutation during iteration.

Outcome: programs can perform useful computation with unbounded containers and
the abstract language is Turing-complete.

## 11. Diagnostics and hardening

- Render compile-time diagnostics with primary and related source spans.
- Map every runtime panic to its SAO2 operation and function.
- Distinguish user errors from generated-C compiler failures.
- Add lexer/parser fuzzing and malformed-program tests.
- Run generated programs under C sanitizers where available.
- Add conformance tests for every rule in `DESIGN.md` and `GRAMMAR.ebnf`.
- Measure compilation time, allocation rates, and GC behavior.

Outcome: the v0 compiler is predictable, testable, and ready for real programs.

## Suggested release gates

1. **Frontend complete:** parsing, diagnostics, name resolution, and typing.
2. **First execution:** primitive arithmetic and control flow run through C.
3. **Memory complete:** escape analysis, inline structs, and GC pass stress tests.
4. **Language complete:** all v0 values, containers, unions, and errors work.
5. **v0 release:** conformance suite passes with no known correctness defects.
