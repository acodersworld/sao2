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

Add a new private `c_backend` module with an IR-only construction boundary:

```text
emit(&ir::Program) -> Result<String, CEmissionError>
```

Keep the current `c_emitter` and its production compiler call unchanged until
Stage 5. The new module remains disconnected from production so the walking
skeleton continues to work while the IR backend is built.

Run `Program::validate` first, followed by capability validation, layout
planning, and rendering. Never return partial C. Define owned errors for invalid
input IR, unsupported post-milestone features, and backend invariants such as
impossible or cyclic layouts. Preserve the context already available from the
IR: type or definition identity, then function, block, operation, or terminator
where applicable. When Stage 5 connects the backend, all of these become
compiler diagnostics rather than new language errors.

### Capability classification

Define the complete milestone-7 capability boundary now, even though later
stages implement the operation bodies.

- Accept scalar storage for unit, `int`, `float`, `bool`, `char`, and the
  temporary Stage-7 string slice.
- Accept tuple declarations and scalar-payload named or anonymous union
  declarations.
- Accept function signatures composed of supported scalar, tuple, and union C
  types.
- Reserve the validated entry `[str]` parameter for the temporary borrowed-argv
  ABI. Reject every other list or map signature or local.
- Accept scalar operations, direct calls, explicit control flow, runtime checks,
  compatible output and panic operations, and supported union operations.
- Reject struct definitions, container storage and operations, tuple
  construction and projection, string indexing and comparison, unsupported
  printing, and actual use of the entry `args` parameter.

Scan definitions, functions, locals, blocks, operations, and terminators in
table order and stop at the first error. Do not reject an otherwise supported
signature merely because Stage 1 has not emitted its body yet.

### C ABI and layout planning

Emit standard headers in fixed order: `<stdbool.h>`, `<stddef.h>`, and
`<stdint.h>`. Define these fixed helper types:

- a concrete one-byte `sao2_unit`;
- `{ const unsigned char *bytes; size_t length; }` for the temporary string
  slice; and
- `{ int count; char **values; }` for the temporary borrowed-argv view.

Map core types to fixed C representations:

- `int` uses `int64_t`;
- `float` uses `double`;
- `bool` uses `bool`;
- `char` uses `uint8_t`; and
- unit uses `sao2_unit`;
- nominal tuples and unions use `sao2_def_<DefinitionId>`; and
- anonymous unions use `sao2_union_ty_<TypeId>`.

Pass and return every Stage-1 value by value. Forward-declare all aggregate
structs in stable identity order. Build one dependency graph covering nominal
tuples, nominal unions, and anonymous unions, with an edge for every by-value
tuple field or union payload. Traverse roots and dependencies by stable identity
and append each node in deterministic DFS postorder, so dependencies are
defined first. Encountering a visiting node is a backend invariant failure; the
validated frontend should already exclude recursive inline layouts.

Preserve tuple field and union alternative order. Represent each union as a
struct containing a `uint32_t tag` followed by a C union named `payload`, whose
members are named `alternative_<AlternativeId>`. Map alternatives to one-based
C tags so zero remains the reserved inactive state. Keep physical tag numbers
private to the backend.

After all complete value-type definitions, emit function prototypes in
`FunctionId` order. Use `sao2_fn_<FunctionId>` for functions and
`sao2_arg_<parameter-position>` for parameters. Use `(void)` for an empty
parameter list. Forward declarations permit direct and mutual recursion in
Stage 2.

Source spelling must never become a C identifier. Use only stable IR identities
for types, fields, alternatives, functions, and later locals and blocks.

### Deterministic rendering

Render sections in this exact order:

1. generated-file comment;
2. standard headers;
3. fixed helper types;
4. aggregate forward declarations;
5. aggregate definitions in dependency order; and
6. function prototypes in `FunctionId` order.

Use `\n` line endings, separate sections consistently, and emit exactly one
final newline. The declaration-only output need not be connected to or compiled
by production during this stage.

### Tests and completion

Construct IR directly in backend tests without parsing source. Add coverage for:

- canonical headers, fixed helper types, and exact section ordering;
- every scalar C type and source-independent identity-derived names;
- identity-ordered forward declarations and dependency-ordered definitions;
- nested tuple declarations and named and anonymous unions;
- tuple fields, union payload members, and one-based alternative tags;
- empty and ordered parameter lists, tuple and union parameters and results,
  and mutually recursive function prototypes;
- malformed IR preserving its original validation context;
- every unsupported definition, signature, local, operation, projection,
  intrinsic, built-in, and terminator category;
- handcrafted cyclic aggregate layouts producing backend invariants; and
- byte-for-byte equality across repeated rendering.

Include source names containing punctuation and C keywords to prove they never
affect generated identifiers. Do not change compiler output, CLI behavior,
filesystem boundaries, or existing temporary-emitter tests in this stage.

Exit criterion: a validated IR program can be classified and its complete C
type and declaration layer rendered deterministically without consulting the
AST, analysis, or semantic tables; all deliberate limitations fail before
rendering; and the production compiler remains on the working temporary
backend.

## Stage 2: Scalar functions and explicit control flow

Extend the Stage-1 translation unit with scalar support helpers, temporary
string-literal data, and function definitions. Preserve the existing generated
comment, base-header order, helper types, aggregate declarations, and function
prototypes. Add any required standard headers after the Stage-1 headers in a
fixed order, including `<inttypes.h>`, `<stdio.h>`, `<stdlib.h>`, and
`<string.h>`. Keep the Windows-only `<fcntl.h>` and `<io.h>` includes behind an
`_WIN32` guard. Emit scalar helpers and string data before the function
prototypes, then emit definitions in `FunctionId` order. Continue to use `\n`
line endings and exactly one final newline.

### Function storage and control flow

Give every IR local explicit C storage named `sao2_local_<LocalId>`, independent
of its source name or origin. At function entry, declare all locals in
`LocalId` order and zero-initialize them with `{0}`. Keep C parameters in IR
signature order using the Stage-1 `sao2_arg_<position>` names, then copy each
argument into its parameter local in signature order. The reserved entry
`[str]` parameter uses `sao2_args` for both its C parameter and its otherwise
unused local. After the prologue, jump explicitly to the function's IR entry
block.

Render blocks in `BlockId` table order and name their labels
`sao2_block_<BlockId>`. Emit operations in stored order and end every block with
its explicit terminator. Translate jumps to `goto`, branches to an `if` whose
two arms jump to their identity-derived labels, and returns directly from the C
function. Back edges, unreachable stored blocks, and a nonzero entry block need
no special ordering. Do not recover loops, short-circuit expressions, or any
other source-level structure.

An IR `Unreachable` terminator calls a declaration-only compiler-invariant hook
and is followed by no ordinary control-flow edge. Explicit panic and Error panic
terminators likewise call declaration-only failure hooks carrying their operand
and `FailureSiteId`. Stage 3 defines all three paths and their diagnostics.
Stage 4 retains ownership of union switches and other union-specific executable
behavior.

### Constants and operands

Render operands only as constants or reads of unprojected local storage. Never
nest emitted IR operations inside a C expression: calls, assignments, and
operator statements consume already-materialized operands, so unspecified C
operand or argument evaluation order cannot affect SAO2 behavior.

Render scalar constants from their owned IR values:

- Use `INT64_C` decimal forms for integers. Render negative values without
  applying unary minus to an out-of-range positive literal, and spell
  `i64::MIN` as `(-INT64_C(9223372036854775807) - INT64_C(1))`.
- Reconstruct floats from their recorded `u64` bits through a small `memcpy`
  helper. Do not use decimal formatting, pointer punning, or inactive union
  members.
- Use `true` and `false` for booleans, `UINT8_C` for characters, and an
  all-zero `sao2_unit` compound value for unit.
- Collect string bytes before rendering. Emit deterministic identity-derived
  `static const unsigned char` backing arrays and construct `sao2_string`
  values from a pointer and exact byte length. Give an empty string a one-byte
  zero backing array but a logical length of zero. Source spelling and C string
  escaping must not affect the representation.

The temporary string slice is copied by value. Pooling identical literal byte
sequences is permitted only by first occurrence in the deterministic IR scan;
it must not introduce observable identity or become the permanent milestone-8
interning design.

### Scalar operations and calls

Emit copies, unprojected local assignments, scalar unary and binary operations,
numeric conversions, and direct calls. Every operation writes its recorded
destination local. Direct calls use `sao2_fn_<FunctionId>`, preserve argument
order, and assign the returned value before the next IR operation. This permits
direct and mutual recursion through the Stage-1 prototypes.

Use ordinary C operators only where their semantics match SAO2 after the
adjacent IR checks have succeeded. Logical negation operates on `bool`; unary
plus and checked negation operate on numeric values; arithmetic and comparisons
operate on their validated scalar types. Unit equality and inequality render as
constant boolean results. Int-to-float uses an explicit `double` conversion;
float-to-int uses an explicit `int64_t` conversion only after its recorded
range check.

Do not depend on the C implementation's representation or shifts of signed
negative integers. Add helpers that map an `int64_t` value to its mathematical
two's-complement `uint64_t` bit pattern and back without out-of-range signed
casts. Use those helpers for bitwise complement, AND, OR, XOR, and left shift.
Implement arithmetic right shift with an unsigned logical shift plus explicit
sign-bit fill, handling a zero shift count separately so no operation shifts by
64. Stage 3 guarantees shift range and checked-left-shift preconditions before
these operations execute.

### Runtime-check bridge

Preserve every explicit `RuntimeCheck` as a separate C statement at its exact
IR position. Call a stable helper family named for the check, such as
`sao2_check_integer_add`, with the already-materialized operands and a final
`size_t` failure-site index. A precondition check remains immediately before
its checked operation; a finite-result check remains immediately after the
float operation and names its destination local.

Stage 2 emits declarations for these check helpers but deliberately does not
define them. Stage 3 supplies their portable semantics and failure reporting.
Consequently, Stage-2 native verification may compile and execute only IR
fixtures which contain no `RuntimeCheck`; check-bearing functions receive exact
C rendering tests but are not linked or executed during this stage. Do not add
temporary checks which reject valid arithmetic, and do not move Stage-3
arithmetic or diagnostic behavior into this stage.

### Temporary output compatibility

Preserve the output walking skeleton for `print` and `println` with strings,
integers, booleans, and zero-argument `println`. Write string slices with
`fwrite`, integers with `PRId64`, booleans from fixed `true` and `false` byte
sequences, and newlines as a separate checked byte. Embedded zero bytes remain
ordinary string data. Assign the intrinsic's unit destination only after all
requested output succeeds.

Before an output attempt on Windows, ensure stdout is in binary mode with
`_setmode(_fileno(stdout), _O_BINARY)`. Treat setup failure, a short `fwrite`, a
negative formatted write, or `EOF` from newline output as failure at the
intrinsic's recorded `FailureSiteId`. Route these cases through a visibly
temporary nonzero-terminating output-failure helper which accepts and retains
the site index; Stage 3 replaces its body with the permanent source-attributed
panic path. Do not silently return from the current SAO2 function or translate
an output failure into one of its ordinary result values.

Float, character, unit, tuple, and union printing remain capability errors in
this stage. Milestone 8 replaces the temporary string representation and output
subset with interned strings and universal printing.

### Tests and completion

Construct IR directly and add exact-C coverage for:

- every scalar constant, including signed minimum, positive and negative zero
  float bits, empty strings, embedded zero bytes, and character boundaries;
- parameter, binding, and temporary locals; zero initialization; parameter
  copying; and source-independent local names;
- copies, assignments, every supported unary and binary operation, both numeric
  conversions, and unit comparisons;
- zero- and multi-argument calls, returned values, direct recursion, and mutual
  recursion;
- a nonzero entry block, stored block order, forward and backward jumps,
  diamonds, dead blocks, branches, returns, and unreachable terminators;
- every runtime-check declaration and call at the required side of its checked
  operation, without executing the placeholder;
- string, integer, boolean, and empty-line output, with and without newlines,
  plus exact failure-site propagation and Windows guards; and
- byte-for-byte equality across repeated rendering, one final newline, and the
  absence of source names in generated identifiers.

Contributor guidance continues to prohibit compiling or executing generated C
during implementation. During the required external verification, compile and
run only check-free Stage-2 fixtures by appending a test-only C harness which
calls the generated `sao2_fn_<FunctionId>` entry. Exercise scalar calls, CFGs,
recursion, and the temporary output subset through that harness. The backend
does not emit a host `main` adapter until Stage 4.

Exit criterion: direct IR tests render complete scalar functions and literal
CFGs deterministically; check-free generated functions execute through the
external harness; check sites are preserved for Stage 3 without provisional
semantics; and the production compiler remains on the working temporary
resolved-AST emitter.

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
