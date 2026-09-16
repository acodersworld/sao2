# Current Work: Complete Core C Backend

Status: complete.

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

Replace every Stage-2 declaration-only scalar check with a `static` C helper
definition. Each helper accepts the already-materialized operands followed by a
`size_t` failure-site index and returns normally only when the subsequent C
operation is defined and conforms to SAO2. Keep the IR operation order intact:
precondition helpers remain immediately before their operation and finite-result
helpers remain immediately after the float destination has been written.

Add `<float.h>` and `<math.h>` in the fixed generated-header order. Emit C11
`_Static_assert` declarations proving that `double` has the binary64 radix,
precision, exponent range, and eight-byte width required by SAO2. Continue to
reconstruct float constants from recorded bits with `memcpy`; do not replace
that representation with decimal constants or aliasing-dependent access.

### Checked integer operations

Implement integer checks without first evaluating the possibly undefined signed
operation:

- Addition checks positive and negative right operands against the corresponding
  `INT64_MAX - right` or `INT64_MIN - right` bound before evaluating `left +
  right`.
- Subtraction checks negative and positive right operands against the
  corresponding `INT64_MAX + right` or `INT64_MIN + right` bound before
  evaluating `left - right`.
- Multiplication converts each operand to an unsigned mathematical magnitude
  without negating `INT64_MIN`. Select a magnitude limit of `INT64_MAX` for a
  nonnegative result and `2^63` for a negative result, then compare by division
  before evaluating the signed multiplication. Zero operands always pass.
- Negation rejects `INT64_MIN` before applying unary minus.
- Division and remainder reject a zero divisor. They also reject the
  `INT64_MIN`, `-1` pair before the C `/` or `%` expression is evaluated.
- A shift-range helper rejects counts outside `0..=63` before the count is
  converted for an unsigned C shift. It is shared by left and right shifts but
  reports through the operation recorded at its failure site.
- Checked left shift treats the operation as mathematical multiplication by
  `2^count`. Compare the left operand's unsigned magnitude against the
  sign-dependent result limit shifted right by `count`; do not compute the
  signed product during the check. After success, retain Stage 2's unsigned
  bit-pattern shift and portable conversion back to `int64_t`.

The Stage-2 unsigned implementation remains authoritative for bitwise
operations and arithmetic right shift. In particular, handle a zero right-shift
count separately and fill high bits explicitly for a negative value. No signed
negative value is passed to C's shift operators, and no expression shifts by
64.

### Checked floating-point operations and conversions

For floating-point division, reject the divisor when `right == 0.0`; this
comparison covers both positive and negative zero before the division occurs.
After each float addition, subtraction, multiplication, or division, apply
`isfinite` to the materialized destination. Infinity and NaN fail, while finite
subnormal results and underflow to either signed zero succeed.

Float-to-int accepts exactly the half-open mathematical interval
`[-0x1p63, 0x1p63)`. Test the source against those exactly representable binary
bounds before applying the `int64_t` cast; NaN fails because it satisfies
neither ordered bound. Conversion truncates toward zero after the check.
Int-to-float remains an unchecked explicit `double` conversion and may lose
precision as specified by the language.

### Failure metadata and panic output

Emit runtime metadata before the helper definitions. Use the program's source
filename display spelling once, a function-name table in `FunctionId` order,
and a compact failure table in `FailureSiteId` order. Each failure entry stores
the target-independent `FailureOperation`, function identity, one-based line,
and one-based column. Render filename and function spellings as deterministic
byte arrays rather than C string escapes. When a generated C array would
otherwise have zero elements, emit one inert sentinel element plus an explicit
logical count of zero; never rely on a compiler extension for zero-length
arrays.

Before indexing metadata, check the supplied failure-site index against its
logical count. An out-of-range index enters the compiler-invariant path rather
than reading outside the table. Map the stored function identity through the
function-name table with the same defensive check.

All language panics write exact bytes to `stderr` in this form and terminate
with `exit(EXIT_FAILURE)` without unwinding:

```text
sao2: panic: <reason> at <filename>:<line>:<column> in <function>
```

Use checked `fwrite`, `fputc`, and formatted decimal output as appropriate while
constructing the diagnostic, but do not recursively panic if writing `stderr`
fails. Attempt the remaining diagnostic writes where meaningful, then terminate
with the same failure status. The trailing newline is part of the stable byte
format.

Lock these reasons exactly:

| Runtime condition | Reason text |
| --- | --- |
| integer addition overflow | `integer addition overflow` |
| integer subtraction overflow | `integer subtraction overflow` |
| integer multiplication overflow | `integer multiplication overflow` |
| integer negation overflow | `integer negation overflow` |
| integer division by zero | `integer division by zero` |
| `INT64_MIN / -1` | `integer division overflow` |
| integer remainder by zero | `integer remainder by zero` |
| `INT64_MIN % -1` | `integer remainder overflow` |
| shift count outside `0..=63` | `shift count out of range` |
| overflowing left shift | `integer left shift overflow` |
| float division by either zero | `floating-point division by zero` |
| non-finite float addition | `floating-point addition produced a non-finite result` |
| non-finite float subtraction | `floating-point subtraction produced a non-finite result` |
| non-finite float multiplication | `floating-point multiplication produced a non-finite result` |
| non-finite float division | `floating-point division produced a non-finite result` |
| invalid float-to-int conversion | `float-to-int conversion out of range` |
| failed stdout setup or write | `standard output failure` |

Replace Stage 2's temporary output-failure helper with this common panic path,
using the `FailureOperation::Output` site recorded on the intrinsic. A partial
stdout write may already be observable; do not retry it or report a false
ordinary return value.

An explicit panic uses its temporary `sao2_string` message bytes directly as
the reason, including embedded zero or newline bytes, then appends the same
location suffix. An unhandled Error prints `Error(`, the supported payload, and
`)` as its reason. This milestone supports integer decimal payloads, `true` or
`false` boolean payloads, and raw ASCII character payload bytes. Continue to
reject float and string Error payloads during capability validation; their
required permanent formatting belongs to milestone 8.

An IR `Unreachable` terminator does not use a language failure site. Best-effort
write this exact compiler-invariant diagnostic to `stderr`:

```text
sao2: internal compiler error: reached unreachable IR
```

Then call `abort()`. Do not attach a source filename, function, line, or column,
and do not route the event through the language-panic reason table.

### Tests and completion

Construct IR directly and add exact-C tests for helper definitions, binary64
assertions, helper-call adjacency, metadata field order, filename and function
byte arrays, empty-table sentinels, stable operation codes, defensive identity
checks, and byte-for-byte repeated rendering.

During the required external verification, use the Stage-2 test-only C harness
to compile and execute:

- successful addition, subtraction, multiplication, negation, division, and
  remainder at zero, one, `INT64_MIN`, `INT64_MAX`, and the nearest valid
  operation-specific boundaries;
- bitwise and shift results for positive and negative values, with shift counts
  `0` and `63`;
- failure cases for every locked integer reason, including negative and `64`
  shift counts and both signed-minimum division cases;
- float arithmetic with ordinary finite values, both zero encodings, finite
  subnormal results, and underflow to positive and negative zero;
- non-finite float results from every arithmetic operator and division by both
  positive and negative zero;
- float-to-int at `-0x1p63`, the greatest binary64 value below `0x1p63`, both
  signed zero values, values immediately outside each bound, infinities, and
  NaN; and
- output failure, explicit panic messages, all supported Error payloads, an
  invalid metadata identity, and the compiler-unreachable path.

Snapshot exact `stderr` bytes for every reason and terminal path. Use fixtures
whose filename and function name prove that the selected failure site supplies
the reported function, line, and column. Include embedded zero and newline
bytes in explicit panic and character Error payload cases, and assert process
termination is nonzero without depending on one particular numeric status.

Contributor guidance continues to prohibit compilation, execution, and
formatting during implementation. Native cases run only in the required
external verification, and the production compiler remains on the temporary
resolved-AST emitter.

Exit criterion: every accepted scalar arithmetic operation either produces the
specified result or terminates through its exact IR failure site; all generated
checked operations avoid undefined and implementation-defined signed
arithmetic; runtime diagnostics are byte-stable; and Stage 3 closes every
declaration-only scalar and failure hook introduced by Stage 2.

## Stage 4: Core unions and entry adapters

Extend executable value support to recursively supported scalar unions. A
supported union alternative may contain unit, `int`, `float`, the temporary
string slice, `bool`, `char`, or another recursively supported named or
anonymous union. Do not treat tuples, structs, lists, or maps as supported union
payloads in this stage. Union declarations and function signatures may retain
the broader Stage-1 layout groundwork, but an executable union operation whose
reachable payload graph leaves this subset is a deterministic capability
error.

### Union values and operations

Enable `UnionInject`, `UnionTest`, `UnionPayload`, and union `Switch` during
capability validation. Preserve their function, block, operation, or terminator
context on failure. Continue to reject tuple construction and projection,
tuple equality, hashing, and printing, and every struct or container operation.
Union equality, hashing, and printing also remain later runtime work.

Emit injection in this exact order:

1. assign an all-zero value to the complete destination union;
2. assign the payload to `payload.alternative_<AlternativeId>`; and
3. assign the alternative's one-based physical tag to `tag`.

The tag write publishes the active payload only after its storage is valid.
Zero remains the reserved inactive representation and is never emitted as a
language alternative. Derive field names and physical tags only from IR
identities; tagged and untagged source spelling has no effect on generated C.

Copy unions with ordinary by-value C assignment. The existing generic local,
parameter, result, assignment, direct-call, and recursive-call paths must retain
the complete tag and payload representation without type-specific copying.
Nested unions remain nested C values with independent discriminants and are
never flattened or renumbered relative to their own `AlternativeId` tables.

Emit a union test as a comparison between the materialized union tag and the
selected alternative's one-based tag. Before extracting a payload, compare the
active tag with the requested alternative. A mismatch enters the Stage-3
compiler-invariant path before generated C reads an inactive union member; do
not rely solely on source-flow assumptions to avoid undefined C behavior.

Render an IR union switch as a C `switch` over the materialized tag. Emit one
`case` per alternative in `AlternativeId` order, with each case jumping directly
to its recorded `BlockId`. Multiple cases may jump to the same target. Emit no
fallthrough and no source-derived structure. Tag zero and every unknown tag use
`default` to enter the compiler-invariant path. The IR validator remains
responsible for exhaustive alternative coverage; the generated default handles
corrupt runtime state rather than a source-level `else` arm.

Postfix `?` requires no backend-specific reconstruction. Execute its lowered IR
literally: switch on the source union, extract the selected payload, return a
single success payload directly, reinject multiple success alternatives into
their result union, or reinject `Error` into the enclosing function's result
union before returning. Source and destination unions use their own physical
tags, so Error propagation must assign the destination `AlternativeId` rather
than copying the source tag. In `main`, retain Stage 3's supported
`ErrorPanic` path. Float and string Error payloads may propagate through
ordinary unions but remain capability errors when an `ErrorPanic` would need to
format them.

Tuple declarations and signatures remain available for ABI and dependency
groundwork. Any actual tuple construction, projection, comparison, hashing, or
printing remains a deterministic capability error until milestone 8.

### Host entry adapters

After all SAO2 function definitions, emit exactly one host entry adapter for
the IR function named by `Program::entry`. Select one of the four semantically
validated shapes:

1. no arguments and unit result;
2. no arguments and integer result;
3. `[str]` arguments and unit result; and
4. `[str]` arguments and integer result.

Use `int main(void)` for the two no-argument shapes and `int main(int argc,
char **argv)` for the two borrowed-argument shapes. Call
`sao2_fn_<Program::entry>` exactly once. For a unit result, discard the returned
`sao2_unit` explicitly and return `EXIT_SUCCESS`.

For an integer result, retain it as `int64_t` until it has been checked against
`INT_MIN..=INT_MAX`. Include `<limits.h>` in the fixed standard-header order.
Only then cast to C `int` and return it. If the result is outside the host
`int` range, best-effort write this exact diagnostic to `stderr` and terminate
with `exit(EXIT_FAILURE)`:

```text
sao2: panic: main returned an exit status outside the host int range
```

This is an adapter failure rather than a source operation, so do not invent a
failure site, filename, function, line, or column. The eventual operating
system remains responsible for any platform-specific interpretation of a
representable C `int` returned from `main`.

For `main(args [str])`, scan every byte of `argv[1]` through
`argv[argc - 1]` as `unsigned char` before entering SAO2 code. Empty arguments
are valid; reject the first byte greater than 127. Count arguments from zero in
SAO2 space, excluding the executable name. On failure, best-effort write this
exact adapter diagnostic with the decimal index and terminate through
`exit(EXIT_FAILURE)`:

```text
sao2: panic: command-line argument <index> is not ASCII
```

Do not print the invalid argument bytes. Like exit-status failure, this path has
no source location because the SAO2 entry function has not begun.

After successful validation, construct the visibly temporary borrowed view as
`sao2_args`. Set `count` to `argc > 0 ? argc - 1 : 0` and `values` to
`argc > 0 ? argv + 1 : argv`. This preserves user-argument order, excludes the
executable name, and handles a conforming hosted implementation which supplies
zero arguments. Pass the view by value to the generated SAO2 entry function.

Capability validation permits the adapter and generated function prologue to
copy this parameter into its reserved local, but rejects any IR operation or
terminator which otherwise reads, copies, returns, forwards as a call argument,
indexes, projects, or iterates that local. Scan nested operands and places, not
only top-level operation variants, so no route exposes borrowed `argv` as a
language list. General `[str]` behavior waits for the permanent string and
container runtimes.

Factor the two adapter panics through a small non-returning helper for raw
pre-entry diagnostics. Keep it separate from Stage 3's source-attributed
failure table, ignore secondary `stderr` failures, and always terminate
nonzero. Existing Windows binary stdout handling remains owned by the output
path and is not duplicated in the entry adapter.

### Tests and completion

Construct IR directly and add exact-C and native execution coverage for:

- named and anonymous unions, tagged alternatives with identical payload C
  types, every supported scalar payload, and nested named and anonymous unions;
- injection zeroing, payload-before-tag ordering, union copies, parameters,
  results, assignments, direct calls, and recursive calls;
- positive and negative union tests, guarded payload extraction, inactive or
  unknown tags entering the invariant path, and source-independent identities;
- switches in `AlternativeId` order, distinct and shared targets, nested
  switches, no fallthrough, and the defensive default case;
- postfix `?` with one success alternative, multiple success alternatives,
  success reinjection, Error propagation between differently ordered unions,
  and supported Error panic payloads in `main`;
- deterministic rejection of unions with reachable tuple, struct, or container
  payload operations, plus continued rejection of union printing and
  unsupported Error panic formatting;
- all four entry adapter shapes, exactly one call to the selected IR entry,
  unit success, and integer results at `INT_MIN`, `INT_MAX`, zero, positive, and
  negative representable values;
- both out-of-range integer-result directions with the exact adapter diagnostic;
- zero, one, and multiple ASCII arguments, empty arguments, order-preserving
  borrowed-view construction, and exclusion of the executable name;
- non-ASCII rejection at the first invalid byte with the correct zero-based
  SAO2 argument index and exact diagnostic bytes; and
- deterministic capability errors for every direct and nested use of the
  reserved `args` local.

Use exact-C assertions to prove borrowed argument ordering because accepted
SAO2 code cannot inspect the temporary view. Native tests may prove that valid
ASCII arguments reach an otherwise args-ignoring entry function and that
invalid arguments fail before it runs. Account for platform limitations when
constructing a non-ASCII native `argv`, while retaining unconditional renderer
and helper tests.

Assert byte-for-byte repeated rendering, one final newline, backend-private tag
numbers, exact pre-entry `stderr`, and nonzero failure statuses without relying
on one particular numeric failure code. Generated Stage-4 C now contains its
own host `main`, so native tests no longer append the Stage-2 harness.

Contributor guidance continues to prohibit compilation, execution, and
formatting during implementation. Native cases run only in the required
external verification, and the production compiler remains on the temporary
resolved-AST emitter until Stage 5.

Exit criterion: recursively supported scalar unions execute through guarded
physical tags and payloads without exposing layout to IR; Error propagation
remaps alternatives correctly; every validated entry signature has exactly one
executable adapter; invalid host inputs fail through stable pre-entry
diagnostics; and the backend still does not implement lists, permanent strings,
or tuple value behavior.

## Stage 5: Pipeline replacement and handoff

Activate the IR backend as the sole production C-generation path. This stage
does not add another value representation or language feature; it replaces the
last resolved-AST dependency in the source-to-executable path and proves that
the Stage-1 through Stage-4 backend is a complete milestone-7 handoff.

### Production pipeline cutover

Keep the existing frontend and lowering order:

1. parse the source;
2. perform name and type analysis;
3. perform semantic analysis and validate its handoff;
4. lower to the owned `ir::Program`;
5. validate the completed IR; and
6. emit C from that IR.

At step 6, call the private IR emitter with only `&ir::Program`. Do not pass the
source file, syntax tree, analysis state, semantic result, or a separately
recovered frontend entry signature. Do not query any of those structures to
guide C generation after lowering has completed. `Program::entry`, the entry
function's IR signature, the IR failure-site table, and the IR source metadata
are the authoritative backend inputs for adapter selection and runtime
diagnostics.

Keep the explicit post-lowering `Program::validate` boundary in the compiler
pipeline even though the backend defensively validates its own input. The
pipeline check continues to identify a broken lowering handoff before backend
capability planning; the backend check protects direct callers and tests. Do
not add a second lowering pass, reconstruct IR facts from the frontend, or
special-case production source programs around backend capability validation.

Convert every `CEmissionError` into a compiler diagnostic at the orchestration
boundary while retaining its complete rendered context. Invalid IR and backend
invariants are compiler failures. A valid but post-milestone feature remains a
milestone-7 backend capability failure, not a new parser, name-resolution,
typing, or semantic source error. Preserve the existing failure-category and
exit-status behavior seen by `build` and `run`.

### Output transaction and warning preservation

Complete IR emission into an owned C string before creating the build
directory, opening `program.c`, or changing any filesystem state. If parsing,
analysis, semantic validation, lowering, IR validation, capability validation,
layout planning, or rendering fails:

- return the failure with all semantic warnings accumulated before it;
- do not invoke the host C compiler or the generated program;
- do not create a previously absent build directory; and
- leave an existing `build/program.c` byte-for-byte unchanged.

Only after emission succeeds may the existing filesystem path create the build
directory and write `program.c`. Filesystem failures likewise retain semantic
warnings and their established compiler-diagnostic wording. Keep the generated
filename, build-directory rules, and `CompileOutput { generated_c, warnings }`
contract unchanged; do not return generated text through a second public path.

Warnings remain nonfatal and preserve their current ordering. On an emission
failure, the CLI prints warnings before the compiler diagnostic. On successful
emission, the same warnings travel in `CompileOutput` and are printed before
host compilation. Backend activation must neither duplicate warnings nor turn
them into errors.

The `--show-c` path reads the newly written IR-generated `program.c` through
the existing `CompileOutput` path and prints it unchanged before host
compilation. `build` still compiles that file and reports the executable;
`run` still compiles the same file and returns the generated program's status.
Do not change host-compiler discovery, `SAO2_CC`, child-process argument
construction, toolchain diagnostics, executable naming, stdout behavior, or
program-failure reporting.

### Retire the temporary backend

Remove the production import and invocation of the resolved-AST emitter, then
remove its module registration and implementation once no test or helper uses
it. The repository must contain one C backend, not a dormant alternative whose
capability rules can diverge. Remove the old resolved-AST capability checker,
C renderer, temporary `main` construction, and test-only rendering helpers
with it.

Rewrite compiler tests that intentionally assert the temporary emitter's C
spelling or messages. Delete tests whose only purpose was to prove that typed
lowering still reproduced the old emitter byte for byte. Replace them with
tests of the production IR boundary and observable behavior. Do not copy old
AST traversal, source-name handling, entry-signature inspection, expression
ordering, arithmetic, or diagnostic logic into `compiler`; all such behavior
must remain owned by the frontend, lowering, or IR backend layer which already
defines it.

Update crate and compiler module documentation to describe the active pipeline
as source analysis followed by typed-IR lowering and IR-only C generation.
Remove obsolete dead-code allowances which existed only because the IR backend
was disconnected, while retaining any allowance still justified independently.
Document milestone 8, rather than the deleted emitter, as the owner of
permanent strings, tuple value behavior, and complete printing.

### Compiler-boundary tests

Keep direct IR backend tests as the exhaustive specification of C spelling,
capability classification, physical layouts, helper ordering, and invariants.
At the compiler layer, construct source programs and assert that the completed
frontend/lowering output reaches that same backend. Add or rewrite coverage
for:

- production output containing the canonical IR-backend sections, generated
  SAO2 function, and exactly one host entry adapter;
- byte-for-byte identical `program.c` from repeated compilation of identical
  source, including a second compilation over an existing output file;
- scalar helper calls, direct and recursive SAO2 calls, forward and backward
  CFG edges, branches, loops, and lowered short-circuit control flow;
- checked integer and floating-point operations retaining their failure-site
  metadata and source attribution;
- named and anonymous supported unions, union switches, nested payloads,
  postfix `?`, success reinjection, and Error propagation;
- each of the four validated entry signatures selecting its corresponding
  adapter solely from `Program::entry` and the IR function signature;
- a backend capability failure after successful lowering, with full IR
  context, retained warnings, no new build directory, and an unchanged
  existing `program.c`;
- injected invalid post-lowering IR failing at the validation boundary before
  backend emission, again preserving warnings and output; and
- successful emission followed by a filesystem failure retaining warnings and
  the established filesystem diagnostic.

Use backend-supported programs for successful compiler tests. Programs using
containers, tuple value operations, permanent string behavior, union printing,
or another milestone-8-or-later feature should test the precise capability
boundary, not expect the temporary emitter's former unsupported-subset text.
Frontend errors must still precede lowering and backend checks, and must remain
source diagnostics with their original spans.

### Integration tests and completion

Exercise the public CLI from source file through the host executable. Add or
rewrite end-to-end coverage for:

- deterministic generated C from repeated compilation;
- scalar calls and recursion, branches, loops, and short-circuit expressions;
- checked integer and floating-point success paths and every locked failure
  class exercised by a source program;
- supported union construction, narrowing, switching, and Error propagation;
- all four `main` adapters, including ASCII argument acceptance, pre-entry
  non-ASCII rejection where the host permits constructing such an argument,
  unit success, representable integer statuses, and out-of-range rejection;
- explicit panic, supported Error panic, arithmetic failure, and
  compiler-unreachable diagnostics with exact source filename, function, line,
  and column where applicable;
- exact stdout bytes for strings with escapes and embedded zero bytes,
  integers, booleans, and newlines;
- `--show-c` displaying the IR-generated translation unit without changing the
  subsequent build or run result;
- missing and failing configured C compilers remaining toolchain failures, and
  generated-program nonzero statuses remaining program results; and
- the existing reproducible safe-primitive fuzz programs under the checked
  backend.

Replace obsolete byte-for-byte expectations tied to the temporary emitter with
new canonical IR-backend expectations only where exact C is material to the
compiler boundary. Prefer executable results and exact diagnostics for CLI
tests; the backend's own unit tests remain responsible for comprehensive C
snapshots. A native test may skip only under the repository's ordinary
no-compiler policy. The required milestone verification below fixes `SAO2_CC`,
so its native assertions must execute rather than skip.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. Review the final diff with read-only checks, but leave all
compilation, execution, and formatting to external verification. That
verification must run from a clean intended worktree:

```text
rustc --version
SAO2_CC=cc cargo test
```

Replace `cc` only when another supported compiler is explicitly selected and
available. Confirm Rust is at least 1.90, all unit and integration tests pass,
and native end-to-end assertions actually ran. Also inspect the test output for
unexpected skips and confirm no generated `build/` artifact is committed.

Only after that evidence succeeds, update `ROADMAP.md` in a final handoff
commit: mark milestone 7 complete and milestone 8 current. Update this
document's status consistently or archive/replace it according to the
repository's established current-work practice. Do not declare the milestone
complete merely because the production call was switched.

Exit criterion: production C generation consumes only closed validated IR;
supported scalar and union programs compile and execute with portable checked
semantics; runtime and adapter failures retain their locked diagnostics;
warnings and filesystem transaction boundaries are preserved; no temporary
resolved-AST C backend or assertions remain; `build`, `run`, and `--show-c`
operate through the IR-generated file; the required native verification has
passed without skips; and later runtime milestones can add value
representations without revisiting the frontend or lowering.

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
