# Current Work: Complete Core C Backend

Status: in progress.

This document expands milestone 7 of `ROADMAP.md`. The objective is to replace
the temporary resolved-AST emitter with deterministic C11 generation from the
validated, owned typed IR. The completed backend executes scalar primitives,
direct calls, explicit control flow, checked arithmetic, and unions of supported
core values while preserving the existing output walking skeleton.

Milestone 8 retains ownership of permanent strings, complete printing, and
tuple value operations. Milestone 7 does not change the public CLI or
`CompileOutput` interface.

## Stage 1: Backend contract and C type model

Replace the temporary emitter with an IR-only construction boundary:

```text
emit(&ir::Program) -> Result<String, CEmissionError>
```

Validate the IR at the boundary. Report deterministic failures with the
function, block, operation, terminator, or identity context retained by the IR.
Use stable identity-derived C names rather than source spelling, and render the
complete translation unit in memory before the caller performs filesystem
writes.

Map core types to fixed C representations:

- `int` uses `int64_t`;
- `float` uses `double`;
- `bool` uses `bool`;
- `char` uses `uint8_t`; and
- unit uses a concrete generated struct so it remains a complete C type.

Forward-declare generated functions before emitting definitions so direct and
mutual recursion work. Emit tuple structs and named and anonymous union
declarations in deterministic dependency order. Reject cyclic or incomplete C
layout dependencies as backend invariants; the validated frontend and IR should
already exclude them.

Represent each union with a `uint32_t` discriminant and a C union payload. Map
IR alternatives to one-based C tags so the all-zero representation remains the
reserved inactive state. Do not expose those physical tag numbers outside the
C backend.

Add a deterministic backend capability validator which runs before rendering.
It accepts the milestone-7 core and rejects structs, containers, tuple
operations, and other runtime-dependent operations with a dedicated backend
error rather than producing partial C. Backend limitations are compiler
diagnostics, not new language errors, and must preserve semantic warnings and
the existing generated-output boundary.

### Tests and completion

Add emitter-only tests which construct IR directly and cover canonical headers,
type names, forward declarations, dependency ordering, function prototypes,
one-based union tags, exact rendering, and deterministic error context. Cover
invalid IR and every deliberately unsupported category. No production pipeline
behavior changes in this stage.

Exit criterion: a validated IR program can be classified and its complete C
type and declaration layer rendered deterministically without consulting the
AST, analysis, or semantic tables.

## Stage 2: Scalar functions and explicit control flow

Emit executable functions for unit, integer, float, boolean, and character
values. Declare locals at function entry and zero-initialize them. Function
parameters retain IR signature order, and every IR destination names explicit
C storage.

Emit constants from their exact IR values. Reconstruct binary64 constants from
their recorded bits without violating C aliasing rules or depending on decimal
formatting. Emit copies, local assignments, scalar unary and binary operations,
numeric conversions, and direct calls.

Render every IR basic block as an identity-derived C label. Enter through an
explicit jump to the function's IR entry block, and translate jumps, branches,
returns, and unreachable terminators directly. Do not recover structured source
control flow or depend on C expression evaluation order.

Implement signed bitwise operations and arithmetic right shift without relying
on implementation-defined host behavior. Source operations which have explicit
IR checks remain adjacent to their generated checked operation; Stage 3 supplies
the permanent helper implementations before pipeline activation.

Preserve the existing output walking skeleton through a visibly temporary
string-slice representation. It supports string literals and copies plus the
currently exercised `print` and `println` forms for strings, integers, and
booleans. Preserve Windows binary stdout handling. Check output calls for
failure through their recorded IR failure sites rather than silently ignoring
host I/O errors. Milestone 8 replaces this compatibility representation with
interned strings and complete universal printing.

### Tests and completion

Add direct emitter tests for every scalar constant, local origin, copy,
assignment, unary and binary operation, conversion, direct and mutual call,
block order, branch, return, and unreachable path. Compile and execute generated
C only during the required external verification, not during implementation.

Exit criterion: direct IR tests can render complete scalar functions and CFGs,
including recursion and the existing output subset, while the production
compiler still uses the old emitter.

## Stage 3: Checked arithmetic and runtime failures

Generate portable, undefined-behavior-free helpers for all scalar checks already
made explicit in IR:

- integer addition, subtraction, multiplication, and negation overflow;
- integer division and remainder by zero and signed-minimum divided by `-1`;
- shift counts outside `0..=63` and overflowing left shift;
- floating-point division by positive or negative zero;
- non-finite floating-point arithmetic results; and
- float-to-int values outside the half-open representable integer range.

Run every check against the already-materialized operands. Perform the ordinary
C operation only after its preconditions make that operation defined. Implement
arithmetic right shift portably, and implement checked left shift according to
SAO2's mathematical multiplication semantics. Int-to-float remains unchecked.

Emit a compact generated table from the IR failure-site table. Each entry names
the target-independent failure operation, function, line, and column; the
program metadata supplies the filename. All runtime-check diagnostics use this
stable form:

```text
sao2: panic: <reason> at <filename>:<line>:<column> in <function>
```

Explicit panic prints its message in the same location-bearing form when the
temporary string representation is sufficient. An unhandled Error prints
`Error(payload)` for payload kinds supported by this milestone. Every panic
terminates with a nonzero status without unwinding. Reaching an IR
`Unreachable` terminator traps through a separate compiler-invariant path.

Defer float Error-payload formatting, string Error payloads, and other cases
which require milestone 8's complete primitive formatting or interned-string
runtime. Reject them during capability validation rather than emitting
non-conforming output.

### Tests and completion

Add exact C and native execution cases for every boundary and failure class,
including signed minimum, both float zero representations, allowed float
underflow, shift counts `0`, `63`, negative, and `64`, and float-to-int boundary
values. Snapshot runtime diagnostics to prove that reason, SAO2 filename,
function, line, and column are stable.

Exit criterion: every accepted scalar arithmetic operation either produces the
specified result or terminates through its exact IR failure site without
invoking undefined or implementation-defined C arithmetic.

## Stage 4: Core unions and entry adapters

Emit union injection by zero-initializing the destination, assigning its payload,
and then publishing its one-based tag. Emit union copies, parameters, results,
tests, payload extraction, exhaustive switches, and Error propagation for
recursively supported scalar unions. Distinct alternatives may share a target
block, and anonymous and nominal unions retain separate generated identities.

Tuple declarations remain available for ABI and dependency groundwork, but
tuple construction, projection, equality, hashing, and printing remain
milestone-8 operations. A tuple operation reaching this backend is a
deterministic capability error.

Generate all four validated entry adapter shapes:

1. no arguments and unit result;
2. no arguments and integer result;
3. `[str]` arguments and unit result; and
4. `[str]` arguments and integer result.

The no-argument adapters call the IR entry function, translate unit to exit
status zero, and translate an integer result to the host process exit status.

For `main(args [str])`, validate every byte of `argv[1..]` as ASCII before
entering SAO2 code. Pass a clearly marked temporary borrowed-argv value to the
entry function. Permit this adapter only when the IR does not inspect, copy,
return, pass onward, index, or iterate the `args` parameter. Actual `[str]`
behavior waits for the permanent string and container runtimes; using the
parameter before then is a backend capability error. A non-ASCII argument
panics before entering SAO2 code with a stable adapter-specific diagnostic that
does not invent a source location.

### Tests and completion

Add deterministic rendering and native execution tests for scalar union
construction, testing, switches, propagation, nested supported unions, shared
switch targets, and union-valued calls. Add all four adapter shapes, integer
exit results, ASCII argument ordering, non-ASCII rejection, and deterministic
rejection of actual `args` use.

Exit criterion: supported scalar unions execute through physical C tags and
payloads without exposing layout to IR, and every validated entry signature has
a generated adapter without prematurely implementing lists or interned strings.

## Stage 5: Pipeline replacement and handoff

Replace the production temporary backend call with the IR emitter. Pass only
the owned validated `ir::Program`; do not pass the AST, analysis, semantic
tables, or frontend entry signature. The IR entry-function identity becomes
authoritative for adapter selection.

Remove the temporary resolved-AST capability checker and renderer. Remove or
rewrite compiler tests whose assertions intentionally describe its generated C
or unsupported-subset messages. Do not retain duplicate lowering or semantic
logic in the new backend.

Preserve semantic warnings on backend and filesystem failures. An emitter
failure must occur before build-directory creation or output writes, leaving an
existing `program.c` unchanged. Keep `build`, `run`, `--show-c`, host compiler
invocation, generated filenames, and `CompileOutput` unchanged.

Update module documentation to state that milestone 7 consumes typed IR and
that milestone 8 supplies the next runtime-value layer.

### Integration tests and completion

Add compiler and end-to-end coverage for:

- deterministic generated C from repeated compilation;
- scalar functions, recursion, branches, loops, and short-circuit CFG;
- checked integer and floating-point success and failure paths;
- supported unions and Error propagation;
- all four `main` adapters;
- runtime panic source attribution;
- backend limitations preserving warnings and generated output;
- `--show-c`, toolchain failures, and program exit statuses; and
- the existing exact-byte output and reproducible primitive fuzz programs.

Replace obsolete byte-for-byte expectations tied to the temporary emitter with
new canonical IR-backend expectations. Preserve observable behavior rather than
the old implementation's C spelling.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification must run:

```text
rustc --version
SAO2_CC=cc cargo test
```

Replace `cc` only when another supported compiler is required. Native
end-to-end assertions must run rather than skip. After that evidence succeeds,
mark milestone 7 complete in `ROADMAP.md`; milestone 8 then becomes current.

Exit criterion: production C generation consumes only closed validated IR;
supported scalar and union programs compile and execute with portable checked
semantics; the source-to-executable walking skeleton remains operational; and
later runtime milestones can add value representations without revisiting the
frontend or lowering.

## Boundaries

- Do not implement struct layout, escape analysis, arena references,
  shadow-stack generation, garbage collection, or container storage.
- Do not implement permanent string interning, general `[str]` argument values,
  universal printing, or tuple value operations.
- Do not recover source-level structure or repeat frontend name resolution,
  typing, mutability, coverage, or flow analysis.
- Do not expose C layout, tag values, helper names, or calling details in IR.
- Keep generated code dependency-free beyond the C standard library and avoid
  compiler-specific arithmetic built-ins unless a portable fallback has
  identical tested behavior.
- Keep child-process invocation argument-based and preserve source, compiler,
  toolchain, and program failure categories.
