# Current Work: Runtime Value Foundations

Status: current.

This document gives a high-level view of milestone 8 of `ROADMAP.md`. The
milestone replaces the remaining temporary string and output support with the
first permanent runtime value layer, and completes the value behavior of
strings and tuples. The source-to-executable path must remain working
throughout, with generated C continuing to consume only validated typed IR.

The milestone has three closely related outcomes:

- every string value is an immutable reference to a canonical interned ASCII
  byte sequence;
- tuples can be constructed, copied, projected, compared, and hashed according
  to their recursive value semantics; and
- `print`, `println`, explicit `panic`, and unhandled `Error` formatting work
  for every value category assigned to this milestone.

## Stage 1: String foundations

Replace the backend's borrowed `{ bytes, length }` value with the permanent
inline reference required by `DESIGN.md`, canonicalize the closed set of string
literals, and implement string equality, ordering, indexing, and hashing.
Complete type-directed formatting remains Stage 3.

At this point every executable SAO2 string originates as a literal. Actual use
of the entry `[str]` parameter remains unsupported until milestone 11, so this
stage does not need a heap allocator or a dynamically growing intern table.
Intern the closed literal set while generating C, but choose a representation
that a later runtime interner can return without changing the language value or
its copy semantics.

### Representation contract

Represent a SAO2 string as a pointer to an immutable descriptor with an
explicit byte pointer and length. Emit the equivalent of:

```c
typedef struct sao2_interned_string {
    const unsigned char *bytes;
    size_t length;
    uint64_t hash;
} sao2_interned_string;

typedef const sao2_interned_string *sao2_string;
```

The exact physical representation remains private to generated C, but these
properties are required:

- copying, assigning, passing, returning, or storing `sao2_string` copies only
  the canonical reference;
- a valid language string is never a null reference; zero remains available
  only as backend-internal uninitialized local storage;
- descriptors and their bytes are immutable and live for the entire generated
  program;
- length, comparison, output, and panic paths never depend on a terminating
  zero byte; and
- equal byte sequences resolve to the same descriptor address and hash.

Do not add a string pointer, literal identity, descriptor identity, or
intern-table operation to the typed IR. `ConstantValue::String` remains owned
bytes, and the backend derives canonical runtime identity solely from those
bytes. Future runtime-created strings must look up the generated literal set as
well as previously created runtime strings before returning a descriptor, so a
runtime value equal to a literal eventually receives that literal's canonical
identity.

Use deterministic 64-bit FNV-1a over the exact byte sequence for string hashes,
with offset basis `14695981039346656037` and prime `1099511628211`. Unsigned
64-bit wraparound is part of this private runtime contract. The empty string
therefore retains the offset basis. Compute literal hashes in the backend and
emit them with the descriptors; later runtime interning must use the same
algorithm. Hash values remain backend-private and are not added to the IR or
made observable as a language operation.

### Deterministic literal pool

Retain one pool entry for each distinct literal byte sequence in the validated
IR. Discover literals by the existing deterministic traversal: function table
order, block table order, operation order, terminator last, and operand order
within each IR node. The first occurrence assigns the pool index; later equal
constants reuse it. Do not use pointer identity, a randomly seeded collection,
or source spelling to determine pool order.

For every pool entry, emit:

1. a `static const unsigned char` backing array containing the exact bytes; and
2. a `static const sao2_interned_string` descriptor pointing to that array and
   recording the logical length and hash.

Continue to give the empty string a one-byte backing array so the generated C
does not rely on zero-length arrays, while recording a logical length of zero.
Embedded zero bytes remain ordinary data. Use stable pool-index-derived C names
for both objects, and render integer byte values rather than C string literals.

Render the complete descriptor type before any runtime helper dereferences a
string. Render literal backing arrays and descriptors before generated SAO2
function definitions. A string constant operand becomes the address of its
canonical descriptor rather than a compound slice value. Repeated emission of
the same IR must remain byte-for-byte identical.

### Comparison, indexing, and hashing

Lift the capability rejection for validated string binary comparisons. Render
`==` and `!=` as canonical descriptor identity comparisons. Render `<`, `<=`,
`>`, and `>=` through one generated helper which compares the common byte
prefix with `memcmp` and then compares lengths without subtraction. This gives
lexicographic ASCII ordering, distinguishes prefixes correctly, and treats an
embedded zero byte as data.

Implement `OperationKind::StringIndex` through a generated helper returning
`uint8_t`. The helper accepts the materialized string, signed `int64_t` index,
and `FailureSiteId`. Nonnegative indices count from the start. Negative indices
use their unsigned mathematical magnitude and count from the end; do not negate
`INT64_MIN` in signed arithmetic. Reject an empty string, a nonnegative index at
or beyond the length, or a negative magnitude greater than the length with the
reason `string index out of range`. Validate that the site records
`SAO2_FAILURE_STRING_INDEX` before reporting the existing source-attributed
panic.

Keep hashing separate from language operations. Add a small Rust backend helper
for the fixed FNV-1a calculation and use it when planning literal descriptors.
The generated program need not contain an unused byte-hashing function while
no runtime string producer exists. Stage 2 tuple hash helpers consume the
cached descriptor hash; milestone 11 must add the equivalent generated-runtime
calculation before it creates strings dynamically.

### Backend migration

Change the renderer and generated helpers as one representation migration:

- replace the current `sao2_string` slice typedef with the descriptor and
  immutable-reference typedefs;
- preserve deterministic byte collection, but have the renderer plan canonical
  literal objects rather than only backing arrays;
- render every `ConstantValue::String` as its descriptor address;
- update temporary string output to read `value->bytes` and `value->length`;
- update explicit panic to read the same descriptor fields;
- route string binary operations through descriptor identity or the lexical
  comparison helper as appropriate;
- render `StringIndex` as one call to the checked indexing helper;
- allow ordinary C pointer copies to carry strings through locals, direct and
  recursive calls, returns, and supported union injection and extraction; and
- remove every compound `(sao2_string){bytes, length}` construction once no
  generated path consumes it.

Avoid evaluating a rendered string operand more than once in newly introduced
helper calls. Current IR operands are constants or materialized locals, but the
backend should retain that invariant instead of relying on C expression
evaluation details.

Keep the existing source-to-executable walking skeleton operational during the
migration. String, integer, and boolean output, zero-argument `println`,
explicit string panic, scalar operations, unions, and all four entry adapters
must retain their milestone-7 behavior. Existing output failures and explicit
panics keep their original `FailureSiteId` and source-attributed diagnostics.

### Capability and ownership boundaries

Stage 1 changes no frontend or IR semantics. It removes only the backend
capability errors for string comparison and indexing. Continue to reject:

- string `len`;
- `Error(str)` panic formatting;
- tuple construction, projection, equality, hashing, and printing;
- actual use of the reserved entry `[str]` parameter; and
- structs, lists, maps, iteration, and other later runtime features.

The borrowed host `sao2_args` adapter remains separate from `sao2_string` and
continues to validate ASCII before entering the SAO2 function. Do not intern
`argv` in this stage, add general list storage, or introduce cleanup behavior
for storage that has static program lifetime.

Update stale backend comments which describe the active backend as milestone 7.
Change the user-facing prefix once to `unsupported by the current C backend`
rather than embedding a milestone or stage number that would require churn at
every boundary. Keep unsupported-feature diagnostics contextual and
deterministic. Capability validation must still finish before rendering so a
rejected program cannot produce a partial translation unit or filesystem side
effects.

### Tests and completion

Extend direct backend tests to cover:

- the immutable descriptor and pointer typedefs, their placement before
  dereferencing helpers, and the absence of the old slice typedef;
- one backing array and descriptor per distinct byte sequence, including
  repeated literals in different functions and operand positions;
- deterministic first-occurrence pool order and byte-for-byte repeated output;
- empty strings, all ASCII boundary bytes, escape-derived bytes, and embedded
  zero bytes with exact logical lengths and fixed hash vectors;
- descriptor-address operands in copies, calls, returns, union payloads,
  temporary output, and explicit panic;
- string locals retaining zero initialization without treating null as a
  language value;
- equality and inequality for repeated and distinct literals, plus every
  ordering operator over equal strings, unequal bytes, prefixes, empty strings,
  and embedded zero bytes;
- indexing at zero, the final position, `-1`, and the negative length, plus
  exact failures for both adjacent bounds, empty strings, and `INT64_MIN`; and
- continued capability errors for string `len`, `Error(str)` formatting, and
  actual entry-argument use.

Add source-to-executable cases for repeated equal literals, all comparisons,
successful positive and negative indexing, strings flowing through functions
and supported unions, exact stdout bytes, and explicit and index panic messages
with source locations. A backend-native harness may additionally compare
descriptor addresses and cached hashes: equal literals must share both, while
unequal literals must have distinct descriptors and match their fixed hash
vectors without assuming that hashes can never collide. Retain the established
native-test skip policy when no supported C compiler is available.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification for the completed stage must exercise
the new native cases with `SAO2_CC` selecting a supported compiler and confirm
that no native assertion was skipped.

Exit criterion: every executable string value is a reference to one immutable
canonical descriptor; distinct occurrences of equal literal bytes share that
descriptor and cached hash; equality, lexical ordering, and checked positive
and negative indexing execute with permanent semantics; output and explicit
panic use the new representation without observable regressions; the old
borrowed-slice construction is absent; and the representation is ready for
Stage 2 tuple hashing and future runtime interning without changing the typed
IR.

## Stage 2: Tuple value behavior

Enable tuple construction and tuple-field projection in the C backend. Preserve
the IR's left-to-right evaluation order and the language's immutable,
by-value semantics: assignment, arguments, returns, and union payload copies
copy the tuple value, while any references in future tuple members will remain
shared.

Generate or select structural equality and hashing helpers from the closed IR
type graph. Both operations recurse through nested tuples and strings and must
agree for all valid map-key tuples. Helpers should be deterministic, avoid
source-derived C identifiers, and be emitted only from validated type
information. This lays the value-semantic foundation required by milestone 11
without adding maps or other containers now.

Tuple support also removes the milestone-7 restriction on tuples nested in
otherwise supported unions. Recursive copying must therefore work through both
tuple and union boundaries wherever the frontend and IR already accept the
type, and the same type traversal becomes available to Stage 3 printing.

Exit criterion: tuple construction, projection, assignment, parameter passing,
returns, equality, and hashing work recursively through supported tuple fields
and union payloads while preserving by-value behavior.

## Stage 3: Complete formatting and handoff

Replace the temporary output cases with a type-directed formatting layer shared
by ordinary output and panic paths. Implement the `DESIGN.md` behavior for:

- unit, integers, finite floats, booleans, characters, and strings;
- recursively printable tuples;
- named and anonymous, tagged and untagged unions, including `Error`; and
- zero-argument `println` and newline handling.

Float output must use the shortest decimal form that round-trips exactly.
Strings and characters write their bytes without quotes, and all output must
continue to handle embedded zero bytes and report write failures through the
existing panic machinery.

Explicit `panic` consumes the permanent string representation. An unhandled
`Error` can now format every permitted primitive payload, including `float` and
`str`, while preserving the existing failure-site filename, line, column, and
function reporting. Formatting must not introduce a second, inconsistent path
for stdout, stderr, or runtime failure metadata.

Before locking exact output snapshots, resolve and document any presentation
detail not already fixed by `DESIGN.md`, especially the textual form of printed
tuple values. Do not silently make a backend-only language decision.

### Integration and verification

Remove milestone-7 capability rejections and temporary helpers only after
their permanent replacements are executable end to end. Keep direct backend
coverage as the detailed specification of generated runtime support, and add
source-to-executable coverage for each newly accepted operation.

Retain exact-byte tests for embedded zeros and newlines, recursive tuple and
union cases, string canonicalization, equality/hash agreement, shortest
round-tripping floats, output failures, explicit panic, and every permitted
unhandled-`Error` payload. Repeated emission must remain byte-for-byte
deterministic. Confirm that compiler failures still occur before filesystem or
toolchain side effects and preserve accumulated warnings.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. Final verification is therefore external to this work plan and
must use Rust 1.90 or newer and run the complete suite with a supported C
compiler selected so native assertions do not skip.

Exit criterion: `print`, `println`, explicit `panic`, and unhandled `Error`
format every milestone-8 value through the permanent type-directed path with
exact output bytes and unchanged source-attributed failure behavior; the
production pipeline accepts the complete milestone-8 subset; no temporary
milestone-7 string or output implementation remains; and external verification
passes with native end-to-end assertions enabled.

## Completion criteria

Milestone 8 is complete when source programs using supported primitive,
string, tuple, and union values compile and execute with the semantics above;
all printable values use the permanent formatting path; all strings use the
permanent interned representation; tuple equality and hashing are structural
and consistent; panic diagnostics retain their source attribution; and no
temporary milestone-7 string or output implementation remains.

At handoff, update `ROADMAP.md` to mark milestone 8 complete and milestone 9
current only after the required external verification succeeds.

## Boundaries

- Do not implement struct layout, escape analysis, arena references, garbage
  collection, or shadow frames; those begin in milestones 9 and 10.
- Do not implement lists, maps, iteration, general `[str]` entry-argument use,
  or string `len`; those remain with milestone 11.
- Do not change language syntax or silently define unspecified observable
  formatting. Update the authoritative design explicitly when a decision is
  required.
- Do not move runtime representation details into the typed IR or repeat
  frontend name resolution, typing, mutability, or flow analysis in the
  backend.
- Preserve the public CLI, output transaction, warning propagation, generated-C
  determinism, argument-based child process invocation, and the distinction
  between source, compiler/toolchain, and program failures.
