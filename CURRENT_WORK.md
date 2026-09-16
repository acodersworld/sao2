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

Status: complete.

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

Replace the renderer's bare `Vec<Vec<u8>>` with a small backend-only literal
record containing owned bytes and the computed hash. Keep lookup by byte
equality; the hash is cached output data, not the deduplication authority. A
simple linear first-occurrence lookup is sufficient for this milestone and
preserves the current ordering without introducing another dependency or a
second identity map.

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

Use one helper contract:

```c
static int sao2_string_compare(sao2_string left, sao2_string right);
```

Return a negative value, zero, or a positive value rather than normalizing to
`-1`, `0`, and `1`. Compare `min(left->length, right->length)` bytes. If
`memcmp` is nonzero, return its result; otherwise compare the lengths with
branches. Map the four IR ordering operators to comparison-with-zero in
`binary_expression`. Identity equality must not call this helper.

Implement `OperationKind::StringIndex` through a generated helper returning
`uint8_t`. The helper accepts the materialized string, signed `int64_t` index,
and `FailureSiteId`. Nonnegative indices count from the start. Negative indices
use their unsigned mathematical magnitude and count from the end; do not negate
`INT64_MIN` in signed arithmetic. Reject an empty string, a nonnegative index at
or beyond the length, or a negative magnitude greater than the length with the
reason `string index out of range`. Validate that the site records
`SAO2_FAILURE_STRING_INDEX` before reporting the existing source-attributed
panic.

Use this generated helper contract:

```c
static uint8_t sao2_string_index(
    sao2_string value,
    int64_t index,
    size_t site
);
```

For a nonnegative index, convert it to an unsigned magnitude, require it to be
strictly less than `value->length`, and use it as the byte position. For a
negative index, obtain the magnitude with the existing
`sao2_int_magnitude` helper, require it to be no greater than the length, and
use `value->length - magnitude`. Perform the bounds comparison before the final
`size_t` conversion. On failure call `sao2_fail` with the string-index failure
operation and reason; on success return `value->bytes[position]`. This handles
`INT64_MIN` without signed overflow and makes `-1` select the last byte.

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

### Implementation sequence and code ownership

Keep the change inside `src/c_backend.rs` apart from compiler and end-to-end
test expectations. No AST, analysis, semantic, lowering, or IR shape change is
needed: those layers already accept and lower every Stage-1 operation.

Implement in this order so each step has one clear representation owner:

1. Add a private literal record and `string_hash(&[u8]) -> u64`. Change
   `collect_strings` and `Renderer::strings` to retain canonical bytes plus the
   fixed hash while preserving first occurrence. Add direct Rust hash-vector
   tests before using the value in generated C.
2. Change the fixed helper types emitted by `Renderer::render`, then extend
   `render_string_data` to emit each backing array followed immediately by its
   immutable descriptor using `UINT64_C(hash)`. Change `operand` to render the
   descriptor address.
3. Migrate the existing consumers in `render_intrinsic` and `SCALAR_RUNTIME`'s
   `sao2_panic` from field access on a slice value to field access through the
   descriptor pointer. Update existing exact-C assertions at the same time and
   prove no `(sao2_string){...}` construction remains.
4. Add `sao2_string_compare` to the generated scalar runtime after the string
   descriptor is fully defined. Relax `CapabilityValidator::operation` for
   validated string `Binary` operations, and make `binary_expression` dispatch
   string equality to pointer comparison and string ordering to the helper.
5. Add `sao2_string_index` after the common failure helpers it calls. Relax the
   capability case for `OperationKind::StringIndex`, render the destination
   assignment in `render_operation`, and remove that variant from the
   renderer's unreachable unsupported group. Preserve the operand order
   `string`, `index`, then failure-site index in the emitted call.
6. Replace the combined backend rejection fixture's string-index portion with
   positive rendering and failure-helper coverage while retaining its other
   capability assertions. Update compiler assertions to the stable `current C
   backend` diagnostic prefix, then add focused source and native cases rather
   than duplicating all exact-C assertions at the compiler boundary.

Keep generated section ordering stable: headers and fixed types, aggregate
declarations and definitions, runtime metadata, scalar runtime helpers, string
literal arrays and descriptors, prototypes, functions, and the host adapter.
The descriptor definition must precede `SCALAR_RUNTIME`; literal objects need
only precede generated SAO2 functions. Do not split string support into a
second Rust module or introduce a general runtime abstraction before another
value family needs it.

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

Run the required verification outside this implementation session from the
intended worktree:

```text
rustc --version
SAO2_CC=cc cargo test
```

Replace `cc` only with another explicitly selected supported compiler. Confirm
Rust is at least 1.90, the complete suite passes, native cases ran rather than
skipped, repeated generated C is identical, and no generated `build/` artifact
is included in the stage handoff.

Exit criterion: every executable string value is a reference to one immutable
canonical descriptor; distinct occurrences of equal literal bytes share that
descriptor and cached hash; equality, lexical ordering, and checked positive
and negative indexing execute with permanent semantics; output and explicit
panic use the new representation without observable regressions; the old
borrowed-slice construction is absent; and the representation is ready for
Stage 2 tuple hashing and future runtime interning without changing the typed
IR.

## Stage 2: Tuple value behavior

Status: complete.

Enable the tuple operations already present in validated IR: construction,
field reads, ordinary value copies, equality, and inequality. Add deterministic
structural hashing for the tuple types permitted as map keys, and allow tuples
to participate in the existing supported union value graph. Printing remains
Stage 3.

No frontend, lowering, or IR shape change is expected. Lowering already
stabilizes tuple constructor arguments from left to right, represents
construction as `Aggregate::Tuple`, and represents field access as a
`Projection::TupleField` chain. Stage 2 is a backend capability, rendering, and
generated-helper change.

### Supported tuple boundary

Retain the existing nominal C struct layout `sao2_def_<DefinitionId>` and
`field_<FieldId>` member names. A supported tuple may recursively contain unit,
any primitive, another supported tuple, or a supported named or anonymous
union. Struct, list, and map fields remain rejected by the existing storage
boundary. Inline recursive tuple layouts remain invalid IR or backend layout
invariants rather than a runtime case.

Tuple values remain immutable and are passed by value. C struct assignment is
the implementation of a language tuple copy for locals, assignments, function
arguments, returns, and union payloads. Never use a C pointer as the language
representation merely to avoid a copy. Future object references stored in a
tuple will themselves be copied reference values, so this representation does
not imply a deep copy.

Only tuple-field reads become supported projections. A projected assignment
destination must remain a capability error even for a handcrafted valid IR
program, because tuple fields are immutable. Struct-field, list-index, and
map-index projections remain later features. Permit any validated chain made
entirely of `TupleField` projections on an operand, including nested tuple
reads and reads used by calls, binary operations, intrinsics, union injection,
returns, and terminators.

### Construction and projection rendering

Render a tuple aggregate as explicit statements against its destination local:

```c
sao2_local_N = (sao2_def_D){0};
sao2_local_N.field_0 = operand_0;
sao2_local_N.field_1 = operand_1;
```

Emit field assignments in stored element order. Do not place all operands in a
single C initializer whose evaluation order is unspecified. Lowering has
already materialized source side effects from left to right, but keeping one IR
operand per C statement preserves the backend's established evaluation-order
discipline and makes nested aggregate output predictable. The zero assignment
also gives padding and any future inactive representation bytes a deterministic
state without making those bytes part of equality or hashing.

Add a renderer for places which starts with `sao2_local_<LocalId>` and appends
`.field_<FieldId>` for every tuple projection. `operand` must use this renderer
for `Operand::Copy` rather than discarding projections. Keep physical field
selection identity-derived; do not recover source member spelling or a numeric
suffix from source text.

Update renderer-side operand type recovery to walk the same projection chain.
This is required when a projected field reaches type-directed paths such as
string comparison, output, union injection, or `Error` handling. Reuse the
validated definition and field identities; do not query frontend analysis or
infer a type from emitted C spelling.

### Structural equality

Generate an identity-named helper for each recursively equatable tuple
definition, for example:

```c
static inline bool sao2_tuple_equal_def_0(
    sao2_def_0 left,
    sao2_def_0 right
);
```

Emit helpers after all aggregate definitions and in the existing aggregate
dependency order, so a nested tuple helper is defined before a helper which
calls it. `static inline` avoids unused-function warnings for tuple definitions
which are stored but not compared.

Compare fields structurally in declaration order:

- unit is always equal;
- `int`, `float`, `bool`, and `char` use their established scalar equality;
- `str` uses canonical descriptor identity from Stage 1; and
- a nested tuple calls its identity-derived equality helper.

Combine field results with short-circuiting `&&`. Values are already
materialized, so short-circuiting changes no source evaluation order. Do not
use `memcmp` on a tuple struct: padding is not a language value, and string
references must compare by canonical identity rather than pointer bytes hidden
inside a wider object.

The IR does not define equality for unions, so a tuple containing a union is a
valid value but is not recursively equatable. Lists, maps, and structs remain
outside this backend stage. Mirror the IR equality boundary rather than
inventing equality for those fields. Render tuple `==` as the helper call and
tuple `!=` as its logical negation; no ordering operator is added for tuples.

### Structural hashing

Generate a hash helper only for each recursively valid map-key tuple: its fields
must be unit, `int`, `str`, `bool`, or another valid map-key tuple. `float`,
`char`, unions, structs, lists, and maps are not hashable tuple fields under
`DESIGN.md`, even when some of them support equality.

Use the contract:

```c
static inline uint64_t sao2_tuple_hash_def_0(sao2_def_0 value);
```

Start with the Stage-1 FNV-1a offset basis. Fold each field in declaration order
through a shared `sao2_hash_combine(state, value)` helper which feeds the eight
low-to-high bytes of the `uint64_t` component value through the same FNV-1a
prime. Derive component values as follows:

- unit uses `UINT64_C(0)`;
- `int` uses `sao2_int_to_bits`;
- `bool` uses `UINT64_C(0)` or `UINT64_C(1)`;
- `str` uses the descriptor's cached Stage-1 hash; and
- a nested tuple calls its tuple hash helper.

The explicit low-to-high byte order makes hashes independent of host
endianness. Hash collisions are permitted; equality implies equal hashes, but
unequal tuples need not have unequal hashes. Tuple nominal identity need not be
mixed into the value because map key types are statically fixed and distinct
nominal tuple types are never compared within one map.

Hashing is not yet an IR operation. Emit the `static inline` helpers as the
stable handoff required by milestone 11, and exercise them with backend-native
harnesses. Do not add maps, a public hash intrinsic, or physical helper names to
the IR.

### Tuples in unions

Extend the backend's recursive union capability classification so a nominal
tuple is supported when every field is itself a supported value. Preserve the
existing visiting guard and reject any reachable struct or container. This
allows tuple payload injection, extraction, switching, propagation, copying,
calls, and returns without changing union tags or payload layout.

A tuple may itself contain a supported union and may be copied regardless of
whether it is equatable or hashable. Equality and hashing eligibility are
separate recursive classifications and must not be used as the general storage
or union-value capability test.

### Capability changes and backend ownership

Replace the blanket projection rejection with an operand-aware check:

- read operands may contain only tuple-field projections;
- assignment destinations must remain unprojected in this stage; and
- any struct, list, or map projection retains the contextual `place projection`
  capability error.

Accept only `Aggregate::Tuple` in `OperationKind::Aggregate`; keep struct, list,
and map aggregates as precise capability failures. Accept tuple `Equal` and
`NotEqual` binary operations and continue to reject other non-scalar binary
operations. Expand supported union values recursively through tuples, but do
not enable tuple printing in `output_operand` until Stage 3.

Keep all physical behavior in `src/c_backend.rs`. `Program::validate` remains
the authority for aggregate arity, field types, projection identities, and
legal equality operators. The backend capability pass decides only whether the
validated operation belongs to this stage, and rendering continues to consume
only the owned IR.

### Implementation sequence

Implement Stage 2 in this order:

1. Add backend predicates for supported tuple values, recursively equatable
   tuples, and recursively hashable map-key tuples. Keep their purposes
   separate and traverse identities deterministically with cycle guards.
2. Replace the blanket projected-place capability check with read-versus-write
   classification. Add renderer `place` and projected `operand_type` support,
   then convert the existing tuple-projection rejection fixture into positive
   nested-projection coverage while retaining later projection failures.
3. Accept and render `Aggregate::Tuple` as zero initialization followed by
   ordered field assignments. Remove only the tuple variant from the aggregate
   capability rejection and renderer unreachable path.
4. Emit equality helpers in dependency order and dispatch nominal tuple
   `Equal` and `NotEqual` from `binary_expression`. Retain ordinary C struct
   assignment for all other tuple copies.
5. Emit the common hash combiner and hash helpers for exactly the recursively
   valid map-key tuple definitions. Add fixed vectors before treating the
   helpers as the milestone-11 handoff.
6. Extend `supports_union_value` through tuple fields and cover tuple payloads
   in named and anonymous unions. Then update compiler and end-to-end cases for
   the newly executable source subset without duplicating backend snapshots.

Render tuple equality and hash helpers after the Stage-1 string literal arrays
and descriptors and before SAO2 function prototypes. At that point all
aggregate types and scalar helpers such as `sao2_int_to_bits` are already
defined. Preserve the existing header, metadata, scalar-runtime, function, and
host-adapter ordering around that new deterministic section.

### Tests and completion

Extend direct backend tests to cover:

- single-field, mixed primitive, nested, and union-containing tuple layouts;
- construction field order, zero initialization, and source-independent C
  identifiers;
- single and chained field reads in copies, comparisons, calls, output,
  returns, union payloads, and terminators;
- tuple copies through locals, assignments, parameters, direct and recursive
  calls, returns, and union injection and extraction;
- equality and inequality for every supported primitive field, positive and
  negative zero floats, canonical strings, nested tuples, and differences in
  each field position;
- absence of equality helpers for tuple definitions containing unions or other
  non-equatable fields;
- fixed hash vectors for unit, signed integers including `INT64_MIN`, booleans,
  embedded-zero strings, field order, and nested valid-key tuples;
- equal tuples producing equal hashes, without asserting that unequal tuples
  cannot collide;
- absence of hash helpers for tuples containing `float`, `char`, unions, or
  other invalid map-key fields;
- named and anonymous unions carrying tuples, including nested tuple/union
  graphs, switches, and Error propagation around unaffected alternatives;
- deterministic helper dependency order and byte-for-byte repeated emission;
  and
- continued rejection of projected assignment destinations, non-tuple
  projections, tuple printing, struct/list/map aggregates, and reachable
  unsupported union payloads.

Add source-to-executable coverage for tuple construction and field reads,
nested tuples, equality and inequality, parameter and return copies, recursion,
and tuple payload narrowing through unions. Use a backend-native harness for
hash helpers because hashing is not yet a source-level operation. Prefer
observable results at compiler and CLI layers; keep comprehensive generated-C
spelling assertions in backend tests.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification for the completed stage must run from
the intended worktree with Rust 1.90 or newer and a supported C compiler forced
so native assertions do not skip:

```text
rustc --version
SAO2_CC=cc cargo test
```

Confirm all tests pass, native tuple and hash harnesses ran, repeated emission
is identical, no unexpected compiler warnings were introduced, and no
generated `build/` artifact is included in the handoff.

Exit criterion: tuple construction and nested field reads execute from source;
ordinary copies preserve immutable by-value behavior across locals, calls,
returns, and unions; structural equality follows field semantics without
observing padding; every valid map-key tuple has a deterministic structural
hash consistent with equality; tuple-bearing supported unions execute without
changing their tag model; later projections, containers, structs, and tuple
printing remain explicit capability boundaries; and no frontend or IR
representation change was required.

## Stage 3: Complete formatting and handoff

Status: current.

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
