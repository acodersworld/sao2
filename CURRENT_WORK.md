# Current Work: Struct Layout and Escape Analysis

Status: complete.

This document maps milestone 9 of ROADMAP.md into implementation stages. The
milestone introduces reference-semantic structs, stable references to inline
struct members, the packed arena-reference ABI, and the escape analysis needed
to choose scoped allocation safely.

Milestone 9 establishes the allocation substrate used by milestone 10, but
does not implement garbage collection. Allocations classified as requiring a
garbage-collected lifetime therefore use a visibly temporary monotonic heap
arena and are not reclaimed during this milestone. Proven non-escaping
allocations use a separate scoped arena and are reclaimed when their
allocating function returns.

The implementation must keep the source-to-executable path working at every
stage. The frontend and typed IR already describe struct definitions, member
storage, construction, and field projection; this milestone should extend
those representations only where allocation failure or analysis results need
an explicit compiler-owned representation.

## Allocation architecture

Reserve separate contiguous 4-GiB heap and scoped virtual-address arenas on
supported 64-bit hosts. Decoded offset zero is never allocated in either. A
language struct reference is exactly two 32-bit fields:

- owner_ptr identifies the root of the complete enclosing allocation;
- member_ptr identifies the exact referenced struct slot; and
- decoded owner_ptr equals member_ptr for a reference to the allocation root.

Eight-byte allocation alignment leaves three low bits available in owner_ptr.
Use tag zero for the heap arena and tag one for the scoped arena; reserve the
other tag values. Clearing those bits yields the owner byte offset and the tag
selects the base used to reconstruct both temporary native pointers.
member_ptr remains an untagged byte offset because an interior member, such as
a char field, need not be aligned. This keeps the language reference eight
bytes while allowing the arenas and their allocators to be genuinely
independent.

Give the heap arena a narrow allocation interface that does not expose its
policy to generated code. Milestone 9 can back it with a monotonic allocator.
Milestone 10 may replace that policy with a collector-integrated allocator or
an adapter to an allocator such as jemalloc or mimalloc, provided the adapter
can obtain object storage from the reserved heap arena. The scoped arena
remains a compiler-controlled bump allocator and never shares its free lists,
metadata, or lifetime rules with the heap allocator. Do not add an allocator
dependency in milestone 9 merely to exercise this boundary.

Each function that performs scoped allocation saves the scoped cursor on entry
and restores it on every normal return. Reused scoped bytes are zeroed before
they can contain language values. Physical pages may remain committed after a
rewind; scoped lifetime is defined by cursor ownership, not decommit behavior.

The heap arena is deliberately monotonic in milestone 9. It gives
conservatively escaping objects stable addresses without pretending that the
milestone-10 collector exists. Its allocation metadata must be sufficient for
the future collector to find the owner allocation's size, layout, and lifetime
class, but this milestone must not add marking, sweeping, free lists, shadow
frames, or collection triggers.

## Stage 1: Packed-reference and arena prototype

Status: complete.

Build the arena runtime as an isolated generated-C layer before struct layout
or construction depends on it. Stage 1 fixes the representation, operating-
system, allocator, and lifecycle contracts. It does not relax the backend's
current struct capability boundary or add any source-level allocation.

### Fixed generated-C ABI

Emit a named reference type beside the existing fixed value types:

```c
typedef struct {
    uint32_t owner_ptr;
    uint32_t member_ptr;
} sao2_ref;
```

Follow it with compile-time assertions that the type is exactly eight bytes,
uint32_t is exactly four bytes, size_t can represent 4 GiB, and native pointers
are wide enough for the arena model. Do not use a compiler-specific packing
attribute: two consecutive uint32_t fields must satisfy the ABI naturally, and
the assertions must reject a host where they do not.

Define the reference encoding in one generated-C section:

- allocation roots use eight-byte alignment;
- the owner tag mask is the low three bits;
- tag zero selects the heap arena and tag one selects the scoped arena;
- owner tag values two through seven are invalid and reserved;
- owner offset zero and member offset zero are never allocated in either arena;
- member_ptr is always an untagged byte offset and may be unaligned; and
- a root reference stores the decoded owner offset in member_ptr.

Use uint64_t or size_t for arena sizes, cursors, end positions, and intermediate
arithmetic. Convert to uint32_t only after proving the final offset is no more
than UINT32_MAX. Compute alignment with checked remainder arithmetic rather
than an unchecked add-and-mask expression. A zero-sized body must still consume
one aligned allocation unit so two objects cannot acquire the same identity.

Keep construction and decoding behind helpers instead of open-coding masks in
later renderers. The layer needs operations equivalent to:

- construct a root reference from an arena kind and checked owner offset;
- derive an interior reference by replacing only member_ptr;
- classify a nonzero reference from the tag in owner_ptr;
- resolve the owner using the selected base and decoded owner offset; and
- resolve the member using the same selected base and unmodified member_ptr.

The all-zero reference resolves only as internal null storage. Invalid tags,
offsets outside the selected reservation, and arithmetic overflow are
compiler/runtime invariants, not new language behavior. Helpers must reject
those cases before performing pointer arithmetic. Stage 2 layout code will
add the owner-relative bounds check used when it constructs an interior
reference; the generic Stage 1 resolver has no scoped-object size metadata from
which to rediscover that fact.

### Platform reservation layer

Add generated platform headers and helpers in the existing top-level platform
branches. The generated translation unit owns its feature macros, which must be
defined before system headers.

On Windows:

- obtain page and allocation-granularity information with the Win32 system
  information API;
- reserve each range with VirtualAlloc using MEM_RESERVE and no committed
  writable pages;
- commit page-aligned ranges with VirtualAlloc using MEM_COMMIT and
  PAGE_READWRITE; and
- release a complete reservation with VirtualFree and MEM_RELEASE.

On POSIX hosts:

- obtain the page size through sysconf(_SC_PAGESIZE);
- reserve each range with mmap using PROT_NONE and a private anonymous mapping,
  accepting the platform's MAP_ANONYMOUS or MAP_ANON spelling;
- commit page-aligned ranges with mprotect to add read/write access; and
- release a complete reservation with munmap.

Wrap those calls in a small arena-neutral interface: query the page size,
reserve a range, commit a subrange, and release a range. Platform constants and
native handles must not leak into reference or allocator helpers. Validate that
the page size is nonzero, is a power of two, and divides the production arena
size. Treat a partially initialized runtime transactionally: if scoped
reservation fails after heap reservation succeeds, release the heap reservation
and return to the fully uninitialized state.

The production constants reserve 2^32 bytes independently for the heap and
scoped arenas. Test-only constants may reduce each logical capacity, but the
production generated program must not read arena sizes from environment
variables or expose them as CLI behavior.

### Arena and allocation state

Represent each arena with its base, logical capacity, page size, committed high
water mark, and next allocation cursor. Both cursors begin at offset eight so
decoded offset zero remains unavailable. The arenas grow independently from
low to high addresses; they do not share an extent boundary or allocator
metadata.

Initialization reserves both arenas without committing their complete ranges.
Commit only the page interval newly crossed by a successful allocation. Update
the committed high water mark and allocation cursor only after the platform
commit succeeds. Release must clear all runtime state and be safe after a
partially failed initialization; double initialization remains an invariant.

Use a small internal allocation result enum which distinguishes success,
logical arena exhaustion, physical commit failure, and invalid input. Stage 1
does not create a new FailureOperation because no source construct can allocate
yet. The host adapter reports reservation or initialization failure as a
pre-entry runtime failure without a source location. Stage 3 will translate a
heap allocation result at a struct-construction site into the appropriate
source-attributed panic.

The heap interface accepts body size, eight-byte alignment, and a layout
identity, even though Stage 1 uses only a sentinel prototype layout. Its
temporary implementation advances monotonically and never reuses storage.
Generated code outside the runtime must call this interface and must not gain
access to the heap cursor.

The scoped interface provides:

- a mark containing the current scoped cursor;
- allocation of a zeroed body with a scoped-tagged root reference; and
- checked restoration to an earlier live cursor. Stage 6 associates that mark
  with the function invocation which owns it.

Restoring a mark changes only the live cursor. It does not decommit pages.
Every allocation zeros its complete body before returning, including bytes
reused after rewind, so zero-safe references and union discriminants never
depend on operating-system page freshness.

### Prototype owner metadata

Place fixed-size heap metadata immediately before each heap object's body while
keeping it outside the language-visible layout. Make the header size a multiple
of eight so the body remains aligned. Given a decoded heap owner offset, the
runtime can locate its header by one checked subtraction without a search or a
side table.

The prototype header records only:

- the language-body size;
- the deterministic layout identity supplied to the heap interface;
- the heap lifetime class; and
- collector-owned timestamp/state initialized to zero.

The exact private header may grow in milestone 10, but Stage 1 must establish
that language bodies contain no allocation header or mark field. Scoped bodies
do not need heap headers: the owner tag identifies their lifetime domain, and
their storage is reclaimed by cursor restoration.

The heap interface owns allocation of header plus body and returns an owner
offset for the body. This boundary must remain usable if milestone 10 replaces
the monotonic implementation with a collector-integrated allocator or an
adapter to another heap allocator.

### Backend integration

Keep the implementation in src/c_backend.rs for this stage. Split the arena
runtime from SCALAR_RUNTIME so its platform code, state, and tests have one
clear owner. Extend Renderer::render in this order:

1. emit required feature macros and platform headers before all other headers;
2. emit sao2_ref and its static assertions with the fixed value types;
3. retain the existing aggregate, failure-metadata, writer, scalar, string,
   tuple, and formatting section order;
4. emit the arena platform and allocation runtime after the common invariant
   and diagnostic helpers it may call; and
5. keep generated function prototypes, functions, and the host adapter last.

Initialize the arena runtime in every generated host adapter immediately before
the call into the SAO2 entry function. Do argument validation first so an
invalid host argument does not reserve arenas. On every normal unit or integer
entry return, retain the result in a native temporary, perform the existing
exit-status range validation where applicable, release both arenas, and then
return it. A terminating panic does not unwind or require explicit arena
release; the host process reclaims the reservations.

This lifecycle integration exercises the prototype through the existing
source-to-executable path without enabling struct definitions, aggregates,
projections, or printing. Preserve all four entry-adapter shapes and their
current result and argument behavior.

Do not change typed IR, lowering, semantic analysis, or failure-site tables in
Stage 1. Continue to reject struct definitions in CapabilityValidator before
rendering or filesystem mutation.

### Implementation sequence

Implement Stage 1 in the following order:

1. Add the fixed sao2_ref definition, tag constants, static assertions, and
   pure checked encode/decode helpers. Lock their generated spelling and
   ordering with direct backend tests.
2. Add the Windows and POSIX reservation abstraction and arena state. Implement
   transactional initialization and release before adding allocation.
3. Add checked commitment and monotonic allocation shared mechanics, then the
   independent heap and scoped interfaces. Keep cursors and capacities wide
   until final reference encoding.
4. Add the heap metadata header and owner lookup. Confirm the returned owner
   points at the language body rather than its metadata.
5. Add scoped marks, rewind, and mandatory zeroing on every returned body.
6. Integrate initialization and normal cleanup with each host-adapter shape.
   Update existing exact-C adapter assertions at the same time.
7. Add the standalone native probe and ordinary end-to-end smoke coverage,
   then remove any temporary duplicate masks, pointer arithmetic, or test-only
   production hooks.

### Tests and completion

Direct Rust backend tests should verify:

- sao2_ref field order, exact static assertions, tag constants, and placement
  before any aggregate which may eventually contain a reference;
- distinct Windows and POSIX header and platform-helper branches;
- one centralized implementation of owner masking and base selection;
- member resolution using an unmodified member_ptr rather than clearing its low
  bits;
- arena initialization immediately before entry and release on every normal
  adapter return;
- unchanged struct capability rejection and unchanged observable behavior for
  all currently executable language features;
- deterministic byte-for-byte emission and stable section ordering; and
- absence of a new IR type, operation, failure site, dependency, or source-level
  arena control.

Add a native C probe assembled from the same arena-runtime source used by the
renderer, not a second allocator implementation. Compile and invoke child
processes with argument lists through the repository's existing host-compiler
test support. The probe uses reduced logical capacities and covers:

- sizeof(sao2_ref) == 8 and all-zero null behavior;
- heap and scoped root references with the same decoded offset remaining
  distinct because only owner_ptr carries the arena tag;
- root and unaligned interior-member resolution in both arenas;
- reserved owner tags and arena-range member offsets being rejected before
  pointer arithmetic;
- eight-byte root alignment, unique identity for zero-sized allocations, and
  checked alignment near UINT32_MAX;
- direct heap-header recovery with correct size, layout, lifetime, and zeroed
  collector state;
- independent heap and scoped exhaustion without cross-arena state changes;
- commit-high-water updates only after successful commitment;
- nested scoped marks, exact cursor restoration, repeated address reuse, and
  zeroed bytes on reuse;
- heap references and metadata remaining stable across scoped rewinds; and
- transactional cleanup after an injected second-reservation or commit
  failure.

Retain the established policy for hosts without a supported C compiler. Run a
production-size smoke program through the normal CLI so the active platform
actually reserves and releases both 4-GiB ranges. Windows and POSIX branches
both require external native verification before the stage is complete; one
host family's compile-time branch assertions do not substitute for executing
the other.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification should use Rust 1.90 or newer and an
explicit supported SAO2_CC where needed, confirm that native arena assertions
were not skipped, and confirm no build artifacts are included in the handoff.

Stage 1 is complete when the packed ABI and tag behavior are locked; both
production reservations initialize transactionally and commit on demand; the
heap and scoped allocators have independent state; root and unaligned interior
references round-trip through the correct base; scoped rewind safely reuses
zeroed storage without affecting heap objects; arithmetic and reduced-capacity
exhaustion fail deterministically; existing programs retain their behavior;
and source-level structs remain an explicit later-stage capability boundary.

## Stage 2: Physical struct layouts and allocation metadata

Status: complete.

Plan and emit the physical representation of every struct before enabling any
source-level struct operation. Stage 2 owns layout facts, generated body types,
layout descriptors, and the IR failure-site handoff needed by Stage 3. It does
not allocate a struct, resolve a struct reference, project a member, copy an
inline object, or weaken the capability boundary around those operations.

An otherwise executable program may contain unused struct definitions after
this stage. Struct construction, struct-typed function storage, and struct
projections remain explicit capability errors. This lets the generated C and
its layout assertions exercise the new declarations without accidentally
making a partial struct implementation observable.

### Representation boundary

Keep the language value and allocation body as separate C types:

- every value whose SAO2 type is a struct has C type `sao2_ref`, including
  locals, parameters, results, tuple fields, union payloads, and referenced
  struct fields;
- every struct definition has a private body type named only from its
  `DefinitionId`, for example `sao2_body_def_3`;
- a direct struct field whose `MemberStorage` is `Inline` uses the selected
  struct's private body type instead of `sao2_ref`;
- a direct struct field whose storage is `Referenced` uses `sao2_ref`; and
- all other body fields use the backend's existing physical value
  representation. Tuples and unions remain inline C aggregates and strings
  remain interned-string pointers.

Do not put an owner offset, mark, layout identity, or allocation header in a
body type. An inline body is the same type whether it is the root body or is
embedded at a nonzero offset. The root's heap header remains immediately before
the complete owner body, as established in Stage 1.

Continue using the backend's one-byte C carrier for `sao2_unit`. Its physical
space is an implementation detail and does not change the language's single,
zero-information unit value. All body sizes and offsets describe actual bytes
used by generated C, including that carrier and ordinary C padding.

Use two namespaces in layout planning: value aggregates for tuples and unions,
and struct bodies for struct definitions. A nominal struct encountered in an
ordinary value aggregate is a `sao2_ref` and is therefore not a body-layout
dependency. An inline struct field is a body-layout dependency. Referenced
struct fields deliberately break that dependency. This distinction must be
centralized rather than inferred separately by declaration rendering,
metadata rendering, and later field access.

### Compiler-side layout plan

Extend the existing `LayoutPlanner` into the single owner of physical layout
facts. Its result should contain an ordered aggregate-emission plan plus one
`StructLayout` per struct definition. The exact Rust spelling may vary, but the
plan must make these facts explicit:

- struct `DefinitionId` and layout identity;
- physical body size and alignment;
- fields in `FieldId` order; and
- for each field, its `FieldId`, `TypeId`, `MemberStorage`, byte offset,
  physical size and alignment, and any inline struct layout identity.

Compute sizes and alignments recursively with checked integer operations.
Align an offset using checked remainder arithmetic, add each field size with a
checked addition, and round the final body size up to the maximum field
alignment the same way. Reject a zero alignment, an offset or size that cannot
be represented by the arena ABI, and any calculation overflow. Do not rely on
wrapping Rust arithmetic or on a host C compiler silently choosing a layout
which the compiler did not plan.

The currently supported generated-C ABI has these physical carriers:

- `sao2_unit`, `bool`, and `uint8_t` have size one and alignment one;
- `int64_t` and `double` have size eight and alignment eight;
- `sao2_string` is a native pointer with size eight and alignment eight;
- `sao2_ref` is eight bytes with `uint32_t` alignment; and
- tuple and union size and alignment are computed from their recursively
  planned fields, including the union's `uint32_t` discriminant and aligned C
  union payload.

Emit `_Static_assert` checks for the scalar carrier assumptions. For every
tuple, union, and struct body, also assert its planned `sizeof`, `_Alignof`, and
each materialized field's `offsetof`. These checks are a target-ABI guard, not
a substitute for compiler-side planning. Keep forward declarations separate
from complete definitions so dependency order is deterministic.

Semantic analysis remains responsible for the source diagnostic for an
infinitely recursive tuple or inline-struct definition. The backend must still
detect a by-value cycle in directly constructed or corrupted IR and report a
`BackendInvariant` before rendering. A layout which is acyclic but cannot fit
the packed-offset ABI is a target representability error associated with its
`DefinitionId`, also reported before any C is returned.

### Deterministic identities and descriptors

Assign every struct definition the 64-bit layout identity obtained by a checked
widening of `DefinitionId.index()`. Zero is therefore a valid identity for the
first definition; validity comes from the allocation header and descriptor,
not from a sentinel identity value. Do not hash source names or depend on
traversal order, pointer identity, or an unordered collection. Fields and
descriptors use `DefinitionId`, `FieldId`, and `TypeId` order throughout.

Emit one immutable descriptor for every struct body. It records the layout
identity, body size, body alignment, and an ordered field table. Each field
entry records its byte offset, physical size, storage class, and the identity
of an inline or referenced struct layout when applicable. Use explicit numeric
storage-kind constants; do not encode meaning in generated symbol names.

Descriptors are compiler-owned metadata and are not exposed to SAO2 programs.
Stage 3 passes the root descriptor's identity to `sao2_heap_allocate`, which
stores it in the existing heap header. Stage 4 uses the same planned offsets
for projections. Milestone 10 may extend or pair the descriptors with tracing
callbacks, but must not renumber existing layout identities or reinterpret the
recorded field offsets.

Fields containing tuples or unions retain their `TypeId` in the compiler-side
plan so later tracing can recurse through their value representation. Stage 2
does not add collector traversal, shadow frames, marking, or a generic runtime
descriptor interpreter merely to consume this information.

### Allocation-failure IR handoff

Add `FailureOperation::StructAllocation` now so every struct construction has
the source location needed when Stage 3 begins allocating. Extend the struct
aggregate operation to carry a required `FailureSiteId`; tuple, list, and map
aggregates remain unchanged. Lowering interns the site at the complete struct
constructor expression, after preserving the current left-to-right argument
evaluation and `FieldId` sorting behavior.

IR validation must require that site to name `StructAllocation`, and IR text
rendering must include it so snapshots expose the association. Add the new
operation to the generated failure-operation constants and diagnostic-name
mapping, but emit no allocation call or panic path in Stage 2. The capability
validator continues to reject the struct aggregate before operation rendering.

Arena reservation, initial commitment, and runtime initialization failures
remain pre-entry environment failures with no source site. Once Stage 3 calls
the allocator, logical exhaustion or commitment failure at a constructor uses
its `StructAllocation` site and is a program panic. Invalid allocator input
remains a compiler/runtime invariant.

### Backend integration

Refactor the current blanket `struct definitions` rejection into narrower
capability checks:

- accept and plan struct definitions and their legal field representations;
- continue rejecting a struct type used in a function signature or local until
  Stage 3 enables packed-reference value flow;
- continue rejecting struct aggregate operations until Stage 3; and
- continue rejecting every struct projection or projected assignment until
  Stage 4.

Run capability validation and layout planning in an order which permits unused
struct definitions to be emitted but guarantees that no unsupported operation
reaches `Renderer`. Planning must finish before rendering starts. A failure in
either step returns no partial C text.

Extend `Renderer` to receive the completed immutable layout plan. Emit in this
order:

1. existing feature macros, headers, fixed value types, and ABI assertions;
2. forward declarations for tuple and union value aggregates and private
   struct bodies;
3. complete aggregate and body definitions in dependency order, followed by
   their size, alignment, and offset assertions;
4. immutable struct layout and field descriptors;
5. existing failure metadata, writer, scalar, string, tuple, formatting, and
   arena-runtime sections; and
6. generated functions and the host adapter last.

Make `c_type` return `sao2_ref` for a nominal struct. Add a separate helper for
body-field storage which consults `MemberStorage` and the layout plan; do not
teach general value-type rendering to return a body type. Generated names stay
identity-only and must never contain source identifiers.

Do not change the Stage 1 arena algorithms or header format in this stage. The
prototype header already has the required 64-bit layout-identity field. Do not
add a second header, a side lookup from member address to owner, or allocation
policy to a descriptor.

### Implementation sequence

Implement Stage 2 in the following order:

1. Add `StructAllocation` and the required struct-aggregate failure site across
   lowering, IR validation and rendering, and failure metadata. Keep backend
   execution of the aggregate rejected.
2. Separate value-aggregate and struct-body identities in `LayoutPlanner`, add
   struct roots, and encode inline-versus-referenced dependency edges.
3. Add checked size, alignment, offset, and deterministic layout-identity
   planning. Preserve the existing recursive-aggregate invariant behavior.
4. Split general value C types from struct-body field C types. Emit forward
   declarations, complete private bodies, and compile-time layout assertions.
5. Emit deterministic layout and field descriptors from the same plan; do not
   recalculate offsets in the renderer.
6. Narrow `CapabilityValidator` so unused definitions pass while struct value
   flow, aggregates, and projections retain their intended stage boundaries.
7. Add direct backend and lowering coverage, update exact generated-C
   assertions, and remove any duplicate dependency or offset calculations.

### Tests and completion

Direct Rust tests should verify:

- every struct language value maps to `sao2_ref`, while only an inline struct
  field maps to a private body type;
- mixed primitive, string, tuple, union, inline-struct, and referenced-struct
  fields receive the expected checked offsets, size, alignment, and `offsetof`
  assertions;
- nested inline bodies are emitted before their owners and referenced cycles do
  not create body-definition cycles;
- direct and indirect inline cycles in constructed IR fail before rendering,
  while the existing frontend source diagnostic remains unchanged;
- near-limit alignment and size arithmetic reports deterministic overflow or
  packed-offset representability errors rather than wrapping;
- layout identities are stable by `DefinitionId` and unchanged by source names
  or dependency traversal order;
- descriptor field order follows `FieldId`, including when constructor source
  arguments appear in a different order;
- a struct aggregate has exactly one `StructAllocation` failure site at the
  constructor span, and malformed or mismatched sites fail IR validation;
- unused struct declarations can pass capability validation and produce
  deterministic C, while struct locals, signatures, construction, and
  projections still produce their precise capability errors;
- the Stage 1 header and allocator ABI are byte-for-byte unchanged apart from
  the new generated descriptors and failure-operation table entry; and
- no allocation call, member resolver, copy helper, escape fact, collector
  operation, dependency, or source-level placement control is introduced.

Native verification should compile generated C containing unused layouts whose
fields exercise every supported representation. The generated static
assertions must pass on both supported platform families. Existing executable
programs must retain their output and exit behavior, and a struct constructor
must still stop at the backend capability boundary rather than emit partial C.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification should use Rust 1.90 or newer, report
whether native C assertions were skipped, and leave generated artifacts under
`build/` only.

Stage 2 is complete when all validated struct definitions have deterministic,
checked body layouts and descriptors; generated C independently asserts every
planned carrier, aggregate, body, and field offset; constructor failure sites
are present in IR; unused layouts compile without enabling struct execution;
all executable struct operations remain at their named later-stage capability
boundaries; and Stage 1 reference and arena behavior is unchanged.

## Stage 3: Conservative struct construction and value flow

Status: complete.

Connect struct aggregate operations to the Stage 1 heap interface and the
Stage 2 layout plan. Every construction in this stage receives heap lifetime;
there is no attempt to infer or encode a shorter lifetime. This makes reference
values executable end to end while keeping field projection, field mutation,
escape analysis, and scoped allocation at their later stage boundaries.

The existing IR representation is sufficient. `Aggregate::Struct` already
names its `DefinitionId`, keys every stabilized operand by `FieldId`, and has a
required `StructAllocation` failure site. Frontend lowering currently sorts the
field pairs; rendering should nevertheless select them by identity and follow
the layout's canonical `FieldId` order rather than trusting incidental vector
order in directly constructed IR. `LayoutPlan` already provides the physical
body size, body alignment, layout identity, ordered fields, and private body
type. Do not add allocation policy, a native address, or a heap-versus-scoped
flag to language IR for this conservative stage.

### Construction transaction

Render each struct aggregate as a small block-scoped transaction with private C
temporaries derived only from the destination `LocalId`:

1. request one root allocation through a narrow struct-allocation wrapper;
2. resolve the returned reference's member address and cast it to the private
   body type selected by the aggregate's `DefinitionId`;
3. initialize every field in ascending `FieldId` order from the already
   evaluated IR operands; and
4. publish the completed packed reference to the destination local only after
   all initialization has finished.

The allocation wrapper accepts the immutable root layout descriptor and the
aggregate's `FailureSiteId`. It passes the descriptor size, the fixed
eight-byte root alignment, and the descriptor identity to
`sao2_heap_allocate`. Generated construction code must not read or advance the
heap cursor directly and must not duplicate the header-size calculation.

On `SAO2_ARENA_OK`, require a nonzero heap root whose decoded `owner_ptr`
equals `member_ptr`, whose owner header records the requested body size and
layout identity, and whose body address satisfies the planned alignment. A
violation is a compiler/runtime invariant. Return the packed reference without
storing a native pointer in any language value.

Map `SAO2_ARENA_EXHAUSTED` to a source-attributed panic such as `heap arena
exhausted`, and map `SAO2_ARENA_COMMIT_FAILED` to a distinct source-attributed
panic such as `unable to commit heap storage`. Both paths must validate that
the site names `SAO2_FAILURE_STRUCT_ALLOCATION` and finish through the existing
failure-location machinery. `SAO2_ARENA_INVALID` is an invariant rather than a
language panic because the compiler supplied the size, alignment, and layout
identity. Reservation and initialization failure remains the existing
location-free pre-entry failure.

Tighten IR validation so the struct aggregate's failure-site location equals
the enclosing operation location, matching the checks already applied to
runtime checks, output, built-ins, and terminating panics. This is validation
of the existing field, not a new IR representation.

Allocate into a temporary `sao2_ref` and retain the old destination value until
publication. This preserves aggregate semantics even for directly constructed
IR whose destination is also read by an input operand. A failed allocation
terminates before publication, and no later operation can observe a partially
initialized object.

The monotonic heap leak remains intentional. A panic terminates the process;
construction does not roll back a successfully allocated object if a later
invariant fails. No source operation performed during field initialization can
panic because all source argument expressions and their checks were lowered
before the aggregate operation.

### Field initialization

Stage 1 allocation zeroes the complete physical body before construction sees
it. Preserve that guarantee and then initialize each materialized field exactly
once from its stabilized operand:

- primitive, string, tuple, union, and unit fields use ordinary C value
  assignment into the private body field;
- a referenced struct field copies the operand's `sao2_ref` unchanged; and
- an inline struct field resolves the operand's exact `member_ptr`, treats it
  as the expected private source-body type, and copies that complete body value
  into the embedded destination slot.

The last rule is construction initialization, not Stage 4 field replacement.
The destination allocation and all of its embedded slots are new and cannot
already have observable identities. A whole-body C assignment is therefore
safe here: nested inline bytes are copied into new slots, while packed
references stored in referenced fields remain shared. Do not copy an allocation
header, change the destination owner reference, or preserve the source inline
slot's address.

Use the existing arena-neutral member resolver for the source of an inline
copy. During Stage 3 every source struct reference reachable from valid code is
a root reference, but the generated helper should resolve `member_ptr` rather
than assume it equals the owner offset so Stage 4 interior references can later
reuse the path. Reject all-zero references, reserved tags, unresolved offsets,
or an address which cannot satisfy the expected body alignment as invariants
before dereferencing. Stage 4 remains responsible for the owner-relative bounds
check when it creates a new interior reference.

Do not use `memcpy` as a substitute for the typed body assignment unless the
backend documents and locks the effective-type and aliasing consequences. The
private generated body type already provides the correct recursive copy shape
and lets the host C compiler check the assignment.

Constructor arguments continue to evaluate from left to right in source order
before the aggregate operation. Their later `FieldId` sorting changes only the
order in which stabilized values are written into fresh storage and must not
evaluate an operand again.

### Packed-reference value flow

Remove the capability rejection for struct types in ordinary function results,
parameters, and locals. A struct local is a zero-initialized `sao2_ref`; valid
source control flow assigns a language value before reading it, while the zero
state remains safe for inactive locals and union payloads.

The existing general renderers should then carry references without special
allocation behavior:

- `Copy` and unprojected `Assign` copy both 32-bit fields;
- direct and recursive calls pass and return `sao2_ref` by value;
- tuple construction and copying preserve contained references;
- union injection, payload extraction, switching, copying, argument passing,
  and return preserve contained references; and
- local rebinding changes only the local reference and never copies or mutates
  the referred body.

Extend union capability analysis so a struct is a supported packed-reference
payload and so tuples and nested unions containing structs are executable.
This does not make structs printable: the language's printable-value set still
excludes structs, so a tuple or union with a reachable struct payload remains
rejected by `print` and `println`.

Continue rejecting every `Projection::StructField`, whether used for a read or
write. Tuple projection of a tuple containing a struct is already ordinary
value flow and may produce a copied `sao2_ref`; it does not resolve a struct
body. Container storage and entry-argument materialization retain their current
unrelated capability boundaries.

### Reference identity

Add one generated helper equivalent to:

```c
static inline bool sao2_ref_equal(sao2_ref left, sao2_ref right) {
    return left.owner_ptr == right.owner_ptr
        && left.member_ptr == right.member_ptr;
}
```

Render struct `==` with this helper and `!=` with its negation. Do not compare
only owner offsets, decode native pointers, compare body contents, use `memcmp`,
or rely on C struct equality. Although Stage 3 produces only roots, comparing
both fields locks the correct semantics for Stage 4 interior references.

Extend tuple equality eligibility and generated tuple equality helpers so
struct fields use `sao2_ref_equal` recursively. Structs remain invalid map keys,
so do not add reference hashing or change tuple hash eligibility. Union equality
remains unsupported by the language and needs no helper.

Distinct constructions with identical field values compare unequal. Copies,
assignments, arguments, and returned references compare equal to the reference
from which they came. Equality performs no arena access and remains valid for a
reference after any unrelated allocation.

### Backend and runtime integration

Retain the generated section order established by Stages 1 and 2. Declare the
reference-equality, allocation, failure, and typed-body resolution helpers
before generated functions use them. Keep layout descriptors immutable and use
the descriptor selected by the aggregate `DefinitionId`; do not perform a
runtime search by identity during construction.

Narrow `CapabilityValidator` in these places only:

- `storage_type` and `signature_type` accept nominal structs as `sao2_ref`;
- struct aggregate construction becomes supported;
- union payload checks accept structs and aggregates containing them; and
- binary equality checks accept a struct directly and tuples recursively
  containing structs.

Keep struct projection and projected assignment rejected. Keep struct output,
ordering, numeric operations, hashing, and use as an entry result rejected by
their existing semantic or backend rules. Capability validation must still
finish before rendering so unsupported operations cannot produce partial C.

The host adapter already validates arguments, initializes both arenas before
entry, retains the entry result, and releases both reservations on every normal
exit. Do not add per-function arena initialization or cleanup. All four valid
entry shapes must retain their existing argument, result, exit-status, and
cleanup behavior. A source-attributed allocation panic terminates and needs no
explicit arena release.

Update the arena-runtime stage comment now that source allocation is enabled,
but do not change its heap policy, scoped allocator, reference encoding,
reservation sizes, header layout, or commitment algorithm. No Stage 3 code may
call `sao2_scoped_allocate`, take a scoped mark, or restore the scoped cursor.

### Implementation sequence

Implement Stage 3 in the following order:

1. Add and directly test `sao2_ref_equal`, the source-attributed struct
   allocation failure helper, and the checked wrapper around
   `sao2_heap_allocate`.
2. Add a renderer helper which selects a `StructLayout` by `DefinitionId` and
   emits the allocation transaction without publishing its reference early.
3. Emit ordinary field assignments, referenced-field copies, and typed inline
   body copies from the Stage 2 field plan. Keep operands single-evaluation.
4. Enable struct aggregate construction in `CapabilityValidator` while keeping
   every struct projection rejected.
5. Enable struct locals, parameters, results, direct calls, returns, copies,
   assignments, tuple containment, and union containment.
6. Add reference identity rendering and extend tuple equality recursively
   without changing hash eligibility.
7. Add native end-to-end coverage for construction, value flow, equality,
   nested inline initialization, and allocation failure, then remove temporary
   duplicate layout lookups or reference decoding.

### Tests and completion

Direct IR and backend tests should verify:

- each struct aggregate uses the descriptor for its exact `DefinitionId`, its
  existing `StructAllocation` site, fixed eight-byte allocation alignment, and
  no direct heap-cursor access;
- allocation status handling distinguishes exhaustion, commit failure, and
  invalid compiler input, with only the first two using the source site;
- the returned root is heap-tagged, nonzero, and has equal decoded owner and
  member offsets, while its header records the planned size and identity;
- the body is zero before initialization and every field is written once in
  canonical `FieldId` order from an already stabilized operand, even if a
  directly constructed IR aggregate stores its field pairs out of order;
- referenced fields preserve both reference words and inline fields copy only
  the expected private body, including nested inline and referenced members;
- construction publishes the destination only after initialization and does
  not reevaluate constructor operands;
- struct values copy through locals, rebinding assignments, parameters,
  returns, direct and recursive calls, tuples, named and anonymous unions,
  branches, and loops without decoding or reallocating them;
- reference equality compares both words, aliases compare equal, distinct
  equal-looking allocations compare unequal, and tuple equality recurses into
  reference fields;
- structs and aggregates containing them remain non-printable and non-hashable,
  ordering remains rejected, and every struct projection still reports the
  Stage 4 capability boundary;
- deterministic C emission and existing scalar, string, tuple, union, failure
  metadata, entry adapter, and arena-runtime output remain stable; and
- no scoped allocation, cursor mark, escape summary, collector action, shadow
  frame, copy helper for existing destinations, or dependency is introduced.

Native end-to-end programs should cover:

- a primitive-only struct constructed in `main` and compared with an alias and
  a separately constructed object;
- parameters and results carrying a reference across several calls, including
  a terminating recursive path;
- tuples and each union form carrying references through injection, switch,
  payload extraction, copying, and return;
- an outer body with inline and referenced instances of the same inner type,
  including repeated use of one source reference;
- enough repeated allocation to cross commitment-page boundaries without
  changing earlier references; and
- reduced-capacity exhaustion and injected commitment failure with the exact
  constructor filename, line, column, function, and panic reason.

Retain the established skip policy when no supported C compiler is available.
Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification should use Rust 1.90 or newer, report
native skips explicitly, and leave generated artifacts under `build/` only.

Stage 3 is complete when every struct construction allocates a zeroed,
descriptor-identified heap body; every field representation initializes
correctly; packed references flow unchanged through all already-supported value
paths; exact identity equality works directly and inside tuples; allocation
failures are source-attributed; projections and mutation remain gated for Stage
4; existing programs retain their behavior; and no allocation receives scoped
lifetime or is reclaimed.

## Stage 4: Member access and identity-preserving copy

Status: complete.

Enable the `Projection::StructField` paths already produced by lowering and
validated by typed IR. Stage 4 resolves packed references only while executing
the operation which needs the body, creates stable interior references for
inline struct fields, and implements mutation without replacing an embedded
slot's identity.

The IR representation is sufficient. Every struct projection already records
the receiver `DefinitionId`, `FieldId`, and `MemberStorage`; the surrounding
typed place determines the result type. The Stage 2 layout plan supplies the
private body type, descriptor, field offset, size, alignment, and nested layout
identity. Do not add physical offsets, native pointers, copy operations, or
arena placement to IR.

Stage 4 adds no source-level failure operation. A field access on a valid SAO2
value cannot fail. A zero reference, reserved arena tag, inconsistent layout,
overflowing offset, out-of-range body, or misaligned typed address is therefore
a compiler/runtime invariant rather than a program panic.

### Typed body resolution

Replace the construction-only raw member resolution with one arena-neutral
typed resolver used by construction, projection, and copy helpers. It accepts
a `sao2_ref` and the expected immutable struct layout descriptor and returns a
temporary byte pointer only after checking:

- the reference is nonzero and has a supported owner tag;
- the tag in `owner_ptr` selects the one arena base used for both the decoded
  owner offset and the unmodified member offset, and both are within that
  reservation;
- the untagged owner offset is no greater than the member offset;
- adding the expected body size to the member offset cannot overflow and does
  not exceed the arena's logical capacity;
- the selected address satisfies the expected body alignment; and
- for a heap reference, the owner header exists and the complete expected body
  range lies within the header's root-body range.

For a heap owner, compute its end with checked arithmetic from the decoded
owner offset and header body size. Require the selected range
`[member_ptr, member_ptr + expected.size)` to lie inside that owner range. Also
retain the Stage 1 lifetime and arena invariants. Do not search allocations by
`member_ptr` and do not treat an interior pointer as the owner.

For the scoped tag, which source programs still cannot produce in Stage 4,
validate the typed range against the live scoped cursor as well as the logical
capacity. The compiler-generated interior-reference chain provides the
inductive proof that each child range lies within its typed parent and hence
within the original allocation. Do not add heap-style headers or an allocation
side table to the scoped arena.

Do not infer that a reference is an allocation root merely because decoded
`owner_ptr == member_ptr`. An inline struct in its parent's first field may
have offset zero and therefore the same two packed words as the enclosing root,
while having a different nominal layout. Root-versus-interior interpretation
comes from the compiler's expected layout and projection path. Consequently,
the typed resolver must not require the heap header's root layout identity to
equal the expected layout identity. The Stage 3 allocation transaction remains
the place which validates a newly allocated root's header identity.

Return typed private-body pointers only inside generated helper calls or the
single generated statement which consumes them. Never store a native pointer
in a language local, tuple, union, allocation body, or across a generated call
or allocation. Packed references remain the only persistent struct values.

### Interior-reference construction

Add a narrow helper for projecting an inline struct field. It accepts the
parent reference, parent descriptor, selected field descriptor, and child
descriptor. Before calling the Stage 1 `sao2_ref_interior` primitive, require:

- the parent passes typed body resolution for the parent descriptor;
- the field descriptor denotes inline storage and its nested layout identity,
  size, and alignment agree with the child descriptor;
- the field offset and child size fit wholly within the parent body using
  checked arithmetic;
- adding the field offset to the parent's untagged `member_ptr` is
  representable by `uint32_t`; and
- the resulting child range remains within the selected arena's live range and
  the complete heap owner range when the owner is heap allocated.

The result copies `owner_ptr` exactly and replaces only `member_ptr` with the
absolute arena offset of the embedded field. Do not add the field offset to the
decoded owner offset: nested projection is relative to the current exact
member. Do not mask `member_ptr`, because an embedded body may be unaligned to
eight bytes when its own planned alignment permits that placement.

An offset-zero inline field is valid even though its packed bits can equal the
parent's bits. Values of distinct nominal struct types cannot be compared, and
future tracing distinguishes them with the expected layout identity. Do not
introduce padding merely to force every embedded reference to have distinct
bits.

Referenced struct projection is different: resolve the parent's typed body and
load the `sao2_ref` stored in that field unchanged. Its owner and member both
come from the referenced allocation; the containing object's owner must not be
substituted. A later dereference of that value performs ordinary typed
resolution for the referenced layout.

### General place rendering

Refactor the renderer's current string-only `place` path into a typed projection
walk which can distinguish a read value from an assignment destination. The
exact Rust data structures may vary, but the walk must carry the current
`TypeId`, C expression, and whether the expression represents a packed struct
reference or an ordinary C lvalue.

Process projections in their IR order:

- a tuple projection appends the selected `field_N` to the current inline
  tuple lvalue;
- a struct projection first resolves the current packed reference as the
  projection's declared `DefinitionId`;
- a primitive, string, tuple, union, or unit struct field yields the exact body
  `field_N` lvalue;
- a referenced struct field yields the stored packed reference; and
- an inline struct field yields a packed interior reference constructed from
  the current reference and the planned descriptors.

This must support arbitrary legal mixtures such as a tuple containing a struct
reference, a struct containing a tuple whose member is a struct reference, and
several inline or referenced struct hops. Every inline hop retains the original
owner word; every referenced hop replaces both words with the loaded reference.

Use the same read-place path for operands in copies, calls, binary operations,
aggregates, union operations, intrinsics, checks, and terminators. Resolution
helpers have no language-visible side effects, but lowering's existing
stabilization remains responsible for source evaluation order. Do not evaluate
a source operand or index expression again merely because its place contains
several projections.

Continue rejecting list and map projections at their existing container-stage
capability boundary. Remove the blanket `place projection` rejection only for
validated tuple and struct paths. Capability validation must still complete
before rendering, and malformed projection definitions, field identities,
storage modes, or receiver types remain IR validation errors.

### Ordinary field mutation and rebinding

For an assignment whose final destination is a primitive, string, tuple, union,
unit, or referenced struct field, render the projected body field as an
ordinary C lvalue and perform the existing assignment:

- value fields copy their inline C representation; and
- referenced struct fields copy both words of the source `sao2_ref`, rebinding
  that field without modifying either referenced object.

An unprojected struct-local assignment continues to rebind the local. It must
not invoke a body-copy helper. Likewise, a tuple field which contains a struct
stores a packed reference and follows tuple value semantics; only a struct
field explicitly marked `MemberStorage::Inline` selects identity-preserving
body replacement.

Compound assignment lowering already reads the projected value into a
temporary, performs the checked operation, and emits a final simple `Assign`.
Once read and write place rendering works, compound assignment to eligible
primitive fields needs no separate backend operation. Preserve the existing
failure site and the receiver reference stabilized before evaluation of the
right-hand side.

Semantic analysis remains the sole owner of `var` and transitive mutability
rules. The backend must not add a weaker runtime mutability test or reinterpret
the authorization already reflected in accepted IR.

### Identity-preserving inline replacement

An assignment whose final projection is an inline struct field is not an
ordinary C assignment of `sao2_ref`. Resolve the destination field's interior
reference and the source packed reference as the same expected child layout,
then invoke a deterministic copy helper for that layout. The destination
reference and all references previously obtained for that embedded slot retain
their original packed identity.

Emit one private field-copy helper per struct layout with an identity-only name.
Define helpers in the Stage 2 deterministic dependency order so every inline
dependency precedes its owner; use `DefinitionId` as the deterministic root and
tie-break order, not as a reason to place an owner before its dependency. Each
interface accepts typed destination and source body pointers and copies fields
in ascending `FieldId` order:

- primitive, string, unit, tuple, union, and referenced struct fields use
  ordinary value assignment; and
- inline struct fields recurse into the selected child's copy helper using the
  addresses of the embedded destination and source bodies.

The helper copies language-visible fields only. It never copies a heap header,
owner offset, layout descriptor, mark state, or other allocator metadata. It
does not allocate, panic, construct a new packed reference, or change the
destination slot address. Referenced objects remain shared because their packed
references are copied unchanged.

Handle exact self-assignment with an early typed-pointer equality return. Two
distinct well-typed bodies of the same nominal layout cannot partially overlap:
an inline occurrence of its own layout, directly or indirectly, would be the
recursive layout cycle already rejected by analysis and `LayoutPlanner`.
Therefore distinct source and destination pointers identify disjoint bodies,
and deterministic field-by-field recursion is safe without a potentially
multi-gigabyte native-stack snapshot. Do not add `restrict`, use `memcpy` or
`memmove`, or allocate a temporary body proportional to the layout size.

Use these copy helpers for Stage 3 initialization of inline fields as well, so
fresh construction and later replacement share one recursive definition of
language-visible copying. Construction may still write ordinary fresh fields
directly and continues to publish its root reference only after initialization.

### Aliasing and observable identity

The implementation must preserve these distinctions:

- reading an inline field produces a reference to the existing embedded slot,
  not a new allocation and not the source reference from which it was
  initialized;
- reading a referenced field produces the exact stored reference;
- assigning an inline field changes the values observable through every alias
  to that destination slot but does not change any alias's packed words;
- assigning a referenced field changes which object is reached through the
  containing field, while aliases to the previously referenced object continue
  to reach that object; and
- replacing one inline sibling neither changes another sibling's identity nor
  retargets references stored inside either sibling.

An interior reference may be passed, returned, stored in a tuple or union, and
compared just like a root reference. Stage 3 equality already compares both
packed words, so two reads of the same inline slot compare equal and distinct
nonzero-offset sibling slots compare unequal. Owner equality alone remains
insufficient. Returning an interior reference is safe in Stage 4 because every
allocation still has monotonic heap lifetime.

### Backend and runtime integration

Retain the Stage 1 packed-reference ABI, arena tags, reservations, and heap
header, the Stage 2 descriptors, and the Stage 3 allocation/failure behavior.
Extend the struct runtime section with typed range validation and interior
construction rather than open-coding owner masks or arena selection in each
projected expression.

Generated copy helpers and typed resolver wrappers use identity-only names.
They should be emitted in deterministic layout dependency order before
generated functions. Use `offsetof`-validated Stage 2 field offsets and the
existing descriptors; do not recalculate a second physical layout in place
rendering.

No write barrier is needed because milestone 9 has no collector and its heap is
monotonic. Keep mutation behind generated helpers and field stores so milestone
10 can add any collector integration without changing language IR. Do not add
marking, tracing, sweeping, shadow frames, free lists, or collection triggers.

All source allocations remain heap allocations. Stage 4 must not call the
scoped allocator, save or restore scoped marks, or classify escapes. Arena
initialization and normal host-adapter cleanup remain unchanged.

### Implementation sequence

Implement Stage 4 in the following order:

1. Add the arena-neutral typed body-range resolver and lock heap, future scoped,
   offset-zero interior, overflow, alignment, and owner-range behavior with
   direct runtime tests.
2. Add descriptor-checked inline-reference construction using the current
   `member_ptr` plus the planned field offset. Test nested inline hops before
   enabling general place rendering.
3. Generate deterministic recursive copy helpers and replace Stage 3's direct
   inline construction copy with the appropriate helper call.
4. Refactor read-place rendering to support arbitrary tuple and struct
   projection chains in every operand and terminator position.
5. Refactor assignment destinations into ordinary lvalue stores, referenced
   struct rebinding, and inline struct replacement through copy helpers.
6. Narrow `CapabilityValidator` for tuple/struct projections while retaining
   container, printing, ordering, hashing, escape, and scoped boundaries.
7. Add direct generated-C and native end-to-end coverage, then remove duplicate
   reference decoding, raw offset arithmetic, and construction-only body
   resolution.

### Tests and completion

Direct IR, layout, and backend tests should verify:

- typed resolution rejects zero references, reserved tags, reversed
  owner/member ranges, checked-add overflow, arena overflow, owner-body
  overflow, misalignment, and heap ranges outside the recorded root body;
- offset-zero inline fields resolve successfully without being mistaken for a
  root of the child layout;
- inline projection preserves `owner_ptr`, changes only `member_ptr` by the
  field offset, uses the current member rather than the root as its base, and
  supports unaligned but correctly typed child bodies;
- referenced projection loads both stored words and never substitutes the
  containing owner;
- nested tuple, inline-struct, and referenced-struct paths produce the expected
  type and generated C in reads, calls, aggregates, checks, returns, branches,
  and switch operations;
- ordinary field stores and referenced-field rebinding use exact typed lvalues,
  while an unprojected local assignment remains reference rebinding;
- every inline assignment invokes the copy helper for the selected nominal
  layout, recurses in `FieldId` order, copies referenced fields unchanged, and
  leaves destination addresses and packed references stable;
- self-assignment returns safely and sibling or cross-allocation copies do not
  overwrite the source during recursion;
- compound primitive field assignments retain their existing runtime checks
  and evaluate the receiver and right-hand side in source order;
- Stage 3 construction uses the same recursive helper for inline body values
  without publishing a partially initialized root;
- invalid projection metadata is rejected by IR validation before rendering,
  and list/map projections retain their precise capability errors; and
- no new failure operation, allocation, dependency, scoped-lifetime action,
  escape fact, collector action, or source-visible pointer behavior appears.

Native end-to-end programs should cover:

- reading and writing every primitive and existing inline value representation
  through a struct field;
- inline and referenced fields of the same struct type, demonstrating copy
  versus rebinding behavior;
- references captured for nested inline slots before replacement and observed
  afterward to prove stable destination identity;
- self-assignment, copying between two sibling inline slots, copying between
  allocations, and nested inline copies containing referenced fields;
- a referenced recursive graph, including mutation through several referenced
  hops without recursive physical layout;
- interior references passed to and returned from functions and carried through
  tuples, named and anonymous unions, branches, loops, and equality;
- offset-zero and nonzero nested inline fields, including a minimally aligned
  body at an unaligned absolute member offset; and
- repeated reads and mutations after enough allocations to cross committed-page
  boundaries, proving native addresses are reconstructed rather than retained.

Retain the established native-test skip policy when no supported C compiler is
available. Contributor guidance prohibits compiling, running tests, or
formatting during implementation. External verification should use Rust 1.90
or newer, exercise both platform branches, report native skips explicitly, and
leave generated artifacts under `build/` only.

Stage 4 is complete when every valid struct projection executes; inline
projections produce stable interior references with the original owner;
referenced projections preserve the stored reference; all ordinary field
values can be read and mutated according to existing semantics; inline
replacement recursively copies language-visible fields without changing any
destination slot identity; aliases observe the required copy-versus-rebinding
behavior; Stage 3 construction uses the same copy semantics; and allocation,
escape, scoped-lifetime, and collector behavior remain unchanged.

## Stage 5: Context-insensitive escape summaries

Status: complete.

Add a compiler-owned escape-analysis pass over validated typed IR. The pass
computes context-insensitive parameter summaries and a deterministic lifetime
classification for every struct aggregate operation. Stage 5 does not change
generated allocation calls: the backend continues to allocate every source
object through the monotonic heap until Stage 6 consumes the plan.

Keep this pass independent of physical C layout. It may inspect language types,
nominal definitions, `MemberStorage`, places, projections, operations, and the
control-flow graph, but it must not inspect body sizes, byte offsets, C types,
arena tags, descriptors, or rendered names. A packed root reference and any
interior reference derived from it share one abstract owner provenance.

### Analysis boundary and result

Implement the pass in a dedicated module such as `src/escape.rs`, after typed
IR validation and before C capability validation and layout planning. Its
public crate-level entry point accepts `&ir::Program` and returns either an
`AllocationPlan` or a compiler-invariant error with function, block, and
operation context where available.

Identify an allocation site by its stable IR coordinates:

```text
AllocationId = (FunctionId, BlockId, operation index)
```

Only `OperationKind::Aggregate` containing `Aggregate::Struct` is an
allocation. Do not use source locations as identities because distinct lowered
operations may share a span. Enumerate functions, blocks, and operations in ID
order so repeated analysis produces byte-for-byte equal results.

The plan records:

- one `FunctionSummary` per `FunctionId`, containing one escape bit per
  parameter in signature order and whether the summary was proven or assigned
  conservatively;
- one `AllocationClass` (`Scoped` or `Heap`) for every `AllocationId`; and
- enough per-function indexing to answer whether a function contains any
  scoped allocation without rescanning or changing IR.

An allocation may be classified `Scoped` only when its function was analysed
successfully and no escape constraint reaches its owner origin. Allocations in
unreachable blocks, unresolved functions, recursive dependency regions, or
otherwise conservative functions are `Heap`. The plan must cover every struct
aggregate, including unreachable ones, so Stage 6 never needs a default.

Do not attach summaries or allocation classes to `ir::Program`. Do not add an
allocation-lifetime enum to `Aggregate::Struct`. Keep the analysis result as a
separate immutable compiler artifact passed beside the validated IR.

### Reference-bearing types

Centralize a recursive predicate for whether a value representation can carry
a struct reference:

- a nominal struct is reference-bearing;
- a tuple is reference-bearing when any member is;
- a named or anonymous union is reference-bearing when any payload is;
- unit and primitive values are not;
- a list whose element cannot carry a struct reference and a map whose key and
  value cannot carry one contribute no Stage 5 owner provenance; and
- a list or map whose contents can carry a struct reference is conservatively
  unsupported by the Stage 5 retention model because its executable storage
  belongs to milestone 11.

Use a visiting set for nominal and anonymous aggregate recursion and preserve
the validated type identities. Encountering a reference-bearing list or map, a
malformed recursive value layout, or another representation the pass cannot
model makes the affected function conservative; it must never turn uncertainty
into a scoped classification.

String pointers are not struct-owner origins. They retain their existing
interned lifetime and do not participate in this analysis.

### Abstract owner provenance

Within an analysed function, assign an origin to:

- every reference-bearing parameter, identified by parameter position; and
- every reachable struct allocation, identified by `AllocationId`.

Also use an `ExternalHeap` fact for a reference-bearing value returned by an
already analysed callee when its exact callee-local allocation identity is not
available to the caller. `ExternalHeap` is already safe to retain and is never
classified by the caller.

A local's provenance is a deterministic set of possible origins. A tuple or
union value carries the union of the provenances of its reference-bearing
contents. A struct reference carries the provenance of its complete owner,
not a distinct origin for its current `member_ptr`.

Use may-analysis throughout: joins union facts, assignments add possible
origins rather than relying on a path-specific kill, and loops iterate to a
fixed point. This is intentionally conservative. The result must not depend on
block visitation order.

For place reads, use these transfer rules:

- an unprojected local has the local's current may-provenance;
- tuple projection preserves the subset conservatively carried by its base;
- inline struct projection preserves the exact owner provenance of its base;
- referenced struct projection conservatively uses the base owner's
  provenance as a proxy for objects reachable through that field; and
- a chain of projections applies those rules in order.

The referenced-field proxy may classify the containing owner as escaping even
when only a referenced child escapes. This false positive is accepted. Stores
into that owner create the dependency which also forces any compiler-visible
child allocation to a safe lifetime. If the owner came from a parameter, the
caller's corresponding storage constraints provide that transitive safety.

`ExternalHeap` remains external through copies, projections, tuples, and
unions. If provenance cannot be expressed as parameter, allocation, or known
external heap, abandon the optimistic analysis of that function and use its
conservative result.

### Local value-flow constraints

Compute structurally reachable blocks from the function entry by following
`Jump`, both `Branch` successors, and every `Switch` target. Do not use constant
folding to remove an edge. Operations in unreachable blocks receive heap plans
but do not add call-graph or provenance constraints.

For reachable operations, propagate local provenance as follows:

- `Copy` and an unprojected `Assign` add the operand provenance to the
  destination local;
- tuple construction unions all element provenances into its destination;
- union injection adds payload provenance, and union payload extraction copies
  the union provenance to its destination;
- struct construction gives its destination only the new allocation origin;
  its field operands instead become containment dependencies of that origin;
- ordinary scalar operations, conversions, tests, checks, and string indexing
  produce no owner provenance; and
- a direct call result of reference-bearing type is `ExternalHeap` after the
  callee's parameter effects have been applied.

A call result needs no return-alias component in the summary. If a result can
derive from a callee parameter, returning or otherwise exposing it makes that
parameter's escape bit true, so the caller classifies its argument safely at
the call. If the result derives from a callee-local allocation, that allocation
is classified heap in the callee because it is returned. The caller may
therefore treat the result as already external heap.

Build monotone subset constraints between locals and solve them with a
deterministic work queue. Flow-insensitive reuse of a mutable local may add a
false origin from another path or iteration; it must never remove an origin.
This avoids path explosion while handling branches, joins, and loops safely.

### Containment and lifetime constraints

After local provenance reaches a fixed point, derive owner-containment edges.
An edge from owner `A` to origin `B` means that if `A` requires heap lifetime,
then `B` also requires heap lifetime because a value derived from `B` may be
retained in `A`.

Add such edges for:

- every reference-bearing field operand of a struct construction, from the new
  allocation to each operand origin; and
- every projected assignment into a struct body, from each possible
  destination-owner allocation to each possible source origin.

For an inline struct field, the language copies fields rather than retaining
the source owner itself. Stage 5 may nevertheless add the source owner as a
dependency; that is a documented conservative approximation which avoids a
second field-content points-to analysis. Referenced fields and ref-bearing
tuple or union fields require the dependency directly.

If a projected assignment may target a parameter-owned object or an
`ExternalHeap` object, mark every non-external source origin as escaping
immediately: the destination can outlive the current invocation. This is also
how a function summary records that a parameter stored into another parameter
may escape. A context-insensitive summary intentionally does not specialize
this decision when a particular caller's destination happens to be local.

Unprojected local assignment is rebinding, not containment, and adds only the
local value-flow constraint. Reads and identity comparisons add no lifetime
edge. Mutation of primitive-only fields adds no owner dependency.

Containment cycles are valid. Escape propagation uses a work queue and visits
each origin edge at most once after deduplication.

### Escape sinks

Seed escape propagation from every reachable operation or terminator which can
make a value outlive the current invocation:

- returning a reference-bearing operand, including a tuple, union, root
  reference, or inline interior reference;
- passing an argument to a callee parameter whose proven summary bit is set;
- storing a reference-bearing value into a parameter-owned or external object;
  and
- any operation whose retention behavior is unknown to this stage.

When a seed contains an allocation origin, classify that allocation heap. When
it contains a parameter origin, set that parameter's summary bit. Then follow
containment edges until no new origin escapes. Because inline projections keep
their owner's provenance, returning or passing an interior reference marks the
complete enclosing allocation, not merely the embedded layout.

Known `print`, `println`, scalar checks, comparisons, and non-retaining value
operations are not escape sinks. Panics terminate the process and do not make
their operands escape. Until containers are executable, a list/map aggregate,
indexing path, mutation, iteration, or built-in which can transport struct
provenance makes the containing function conservative rather than relying on
incomplete retention rules. Primitive-only containers and the special `[str]`
entry argument carry no struct owner and do not by themselves poison an
otherwise analysable function.

When intraprocedural analysis encounters such an unknown, stop optimistic
classification for that function: mark every reference-bearing parameter as
escaping, classify every allocation in the function heap, and record a
conservative summary. The function is still resolved for call-graph scheduling,
so its callers may proceed using that conservative summary. Do not leave an
acyclic caller unresolved merely because a callee required this local fallback.

### Direct-call dependency queue

Build the call graph from reachable `OperationKind::Call` operations only.
For every function, record sorted, deduplicated direct callees and direct
callers plus an unresolved count equal to its number of distinct direct
callees. Multiple call sites to one callee are one scheduling edge, although
the intraprocedural analysis still applies the callee summary at every call
site.

Initialize a FIFO queue with all functions whose unresolved count is zero, in
ascending `FunctionId` order. For each dequeued function:

1. analyse it using summaries of its already resolved direct callees;
2. store its parameter summary and allocation classifications;
3. visit its direct callers in ascending `FunctionId` order; and
4. decrement each caller's unresolved count once for this callee, enqueueing the
   caller when the count reaches zero.

Each unique call-graph edge is processed once. Do not recursively traverse call
paths and do not reanalyse a resolved function for different callers or call
sites.

After the queue drains, every unresolved function is recursive or depends
directly or transitively on recursion. Give each reference-bearing parameter in
such a function an escaping summary, classify all of its allocations heap, and
mark the summary conservative. This applies to self recursion, mutually
recursive components, and acyclic callers which depend on them, matching the
roadmap's conservative recursive boundary.

An invalid callee identity is already rejected by IR validation. If a future
IR gains indirect or foreign calls, those calls must use a documented
conservative summary until a stronger contract exists.

### Pipeline and backend handoff

Run escape analysis after the existing post-lowering `ir_program.validate()`
boundary. Convert an analysis invariant into a compiler diagnostic while
preserving semantic warnings and before creating `build/` or writing generated
C.

Pass `&AllocationPlan` beside `&ir::Program` into the C backend. At the backend
boundary, validate that:

- summaries and function plans cover every function exactly once;
- parameter-bit counts match function signatures;
- every struct aggregate has exactly one matching allocation entry;
- no entry names a non-struct operation or invalid IR coordinate; and
- the plan contains no duplicates or omissions.

Stage 5 deliberately ignores `Scoped` when choosing the generated allocator:
both plan classes continue to render the Stage 3 heap call. This locks the
analysis handoff without changing observable execution. Stage 6 becomes the
only place which maps `Scoped` to the scoped allocator and adds function marks.

Backend layout planning must not feed facts back into escape analysis. The
allocation plan may be queried by IR coordinates, never by C symbol, layout
identity, body offset, or source span.

### Implementation sequence

Implement Stage 5 in the following order:

1. Add allocation identities, summaries, plan validation, deterministic result
   rendering for tests, and the reference-bearing type predicate.
2. Compute reachable blocks and the sorted direct-callee/direct-caller graph.
   Lock duplicate-call, diamond, self-recursive, and mutually recursive cases.
3. Add parameter/allocation origins and solve monotone local provenance through
   copies, assignments, projections, tuple and union flow, branches, and loops.
4. Add construction and projected-store containment edges, escape sinks, and
   transitive propagation to allocation classes and parameter bits.
5. Apply resolved callee summaries at every call site and conservatively finish
   all functions left after the dependency queue drains.
6. Integrate the plan into compiler orchestration and the backend boundary while
   retaining heap allocation for both classes.
7. Add regression coverage for diagnostics, warnings, deterministic output, and
   the unchanged generated-C/runtime behavior, then remove duplicate ad hoc
   escape reasoning.

### Tests and completion

Direct analysis tests should verify:

- allocation IDs remain distinct when sites share a source location and are
  stable in function, block, and operation order;
- primitive-only parameters and locals create no origins, while structs and
  nested tuple/union carriers do;
- copies, local rebinding, tuple construction/projection, union
  injection/extraction, inline projections, and branch or loop joins propagate
  all possible owner origins;
- returning a root or any depth of inline interior reference marks its complete
  owner heap and sets the originating parameter bit when applicable;
- referenced-field proxies plus construction and mutation dependencies retain
  compiler-visible child allocations whenever their containing owner escapes;
- storing a local allocation or parameter-derived reference into a parameter or
  external object forces the source heap/escape summary;
- an allocation stored only in another non-escaping local allocation remains
  scoped, including containment cycles confined to one invocation;
- a non-escaping callee parameter permits a caller allocation to remain scoped,
  while an escaping parameter forces it heap at every call site without
  specialization;
- callee-returned local allocations are heap in the callee and appear as
  `ExternalHeap` in the caller without requiring a result-alias summary;
- distinct call sites create one dependency edge but each applies argument
  effects; leaf chains and diamonds resolve in deterministic order;
- self recursion, mutual recursion, and callers depending on recursion receive
  conservative summaries and heap-only allocation plans, while unrelated
  acyclic functions remain analysable;
- unreachable calls do not create scheduling dependencies and unreachable
  allocations receive explicit heap entries;
- unknown reference-bearing container or retention behavior makes the affected
  function conservative rather than scoped, while `[str]` and primitive-only
  containers introduce no owner provenance;
- plan validation rejects missing, duplicate, mistyped, or out-of-range entries
  before rendering; and
- repeated analysis produces identical summaries, allocation classes, and
  diagnostic context without unordered-collection dependence.

Compiler and backend regression tests should verify that analysis failure
preserves warnings and transactional output behavior, a valid plan reaches the
backend intact, and generated C remains unchanged apart from any nonsemantic
test-only assertions needed to lock the handoff. Existing native programs must
retain their output, identity behavior, panic locations, and heap allocation
behavior regardless of whether their plan says `Scoped` or `Heap`.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification should use Rust 1.90 or newer, report
native skips explicitly, and confirm that no Stage 5 program calls the scoped
allocator or restores the scoped cursor.

Stage 5 is complete when every struct aggregate has a deterministic validated
lifetime class; every resolved function has a context-insensitive parameter
summary; direct-callee scheduling processes acyclic dependencies once;
recursive, unreachable, container-dependent, and unknown cases are
conservative; interior escape always marks the complete owner; compiler and
backend boundaries carry the separate plan without changing IR; and generated
programs still allocate every object in the monotonic heap pending Stage 6.

## Stage 6: Scoped arena lifetimes

Status: complete.

Consume the Stage 5 allocation plan in the C backend. A `Scoped` struct
aggregate now allocates from the scoped arena and a `Heap` aggregate retains
the Stage 3 monotonic-heap path. A function containing at least one scoped
site owns one scoped cursor mark for the complete invocation and restores that
mark on every normal return.

This stage changes allocation policy only. Do not refine escape summaries,
attach lifetime information to IR, promote objects dynamically, or introduce
per-block, per-loop, or per-object cleanup. The validated plan is the sole
authority for choosing an arena. Recursive, unreachable, unknown, and
otherwise conservative sites remain heap because Stage 5 already classified
them that way.

### Plan lookup and renderer context

Keep `AllocationPlan` immutable and pass it beside `ir::Program` through
layout planning and rendering. Extend its crate-level query boundary with
deterministic lookups equivalent to:

```text
allocation_class(AllocationId) -> Option<AllocationClass>
function_has_scoped(FunctionId) -> bool
```

The allocation entries are already sorted by `AllocationId`; use that order
for lookup rather than constructing an unordered renderer-side map. Plan
validation remains mandatory before rendering and continues to reject every
missing, duplicate, or invalid site. After successful validation, failure to
find the current struct aggregate is a compiler invariant, never an implicit
heap fallback.

Store `&AllocationPlan` on `Renderer`. When rendering a function, enumerate
blocks and operations with their actual indices and form the exact
`AllocationId { function, block, operation }` for each operation. Pass that
identity, or its already resolved class, into struct-aggregate rendering. Do
not identify a site by destination local, source location, definition, or
emission order: all of those may be shared or reused.

Non-allocation operations do not need a lifetime lookup. Remove the Stage 5
comment and implementation behavior which deliberately discard the plan, but
retain the existing compiler orchestration and backend validation boundary.
No analysis fact may be derived from a C layout or generated name.

### Scoped struct allocation

Retain `sao2_allocate_struct` as the narrow heap-allocation helper so every
`Heap` site and the future collector handoff keep the existing call shape and
metadata behavior. Add a parallel helper with this generated-C boundary:

```c
static void sao2_allocate_scoped_struct(
    const sao2_layout_descriptor *layout,
    size_t site,
    sao2_ref *reference,
    unsigned char **body);
```

The helper calls `sao2_scoped_allocate(layout->size, reference)`. On success
it must verify that the result is a root reference with the scoped tag, a
nonzero aligned owner offset, and equal owner and member offsets. Resolve the
body with `sao2_resolve_body(*reference, layout)` so the existing typed-body
checks also verify the descriptor size, alignment, arena bounds, and current
scoped cursor. A scoped object has no heap header and must never be passed to
`sao2_heap_header_for`.

Map allocator results at the source struct-construction failure site:

- `SAO2_ARENA_EXHAUSTED` reports `StructAllocation` with the reason
  `scoped arena exhausted`;
- `SAO2_ARENA_COMMIT_FAILED` reports `StructAllocation` with the reason
  `unable to commit scoped storage`;
- `SAO2_ARENA_INVALID`, an invalid reference, or a failed postcondition calls
  `sao2_compiler_invariant`; and
- only `SAO2_ARENA_OK` publishes storage to the aggregate renderer.

Keep the existing scoped allocator's cursor, commitment, and zeroing rules.
It reserves at least one aligned slot for a zero-sized body, advances the
cursor only after successful commitment, uses tag one, and zeroes all bytes
which can become language-visible on every allocation, including reused
storage. Restoring a mark does not decommit pages.

For a `Scoped` aggregate, the renderer calls the new helper and then uses the
same field initialization and delayed destination publication as the heap
path. Inline struct operands still use the Stage 4 recursive copy helper;
referenced fields still copy the packed reference. For a `Heap` aggregate,
emit the existing `sao2_allocate_struct` call byte-for-byte. Do not add an
arena branch to generated program control flow when the compiler already
knows the class.

### Function-owned marks

Use `AllocationPlan::function_has_scoped` to decide whether a generated
function needs a mark. If it does, emit exactly one local mark initialized by
`sao2_scoped_mark_current()` at function entry, before the initial jump and
before any operation can allocate. Zero-initializing generated locals and
copying C parameters may occur on either side of the mark because neither
touches an arena; keep one consistent order in emitted C.

The mark belongs to the invocation, not to a block or allocation. All scoped
objects created by that invocation, including allocations repeated by a loop,
remain live until the function returns. A nested callee which has scoped sites
saves and restores its own later mark, leaving the caller's cursor and objects
intact. A callee without scoped sites emits no mark even when it receives a
scoped reference from its caller.

Functions with only heap sites, no struct aggregates, or only unreachable
struct aggregates emit no mark and retain their existing return form. Stage 5
classifies unreachable sites heap, so renderer reachability does not need to
be recomputed here. Recursive functions and acyclic functions made
conservative by a recursive dependency likewise remain heap-only.

The entry adapter continues to initialize both arenas before calling the
source entry function and release them after it returns. Generated source
functions therefore may treat `sao2_scoped_mark_current()` as infallible;
runtime initialization failures remain pre-entry failures.

### Normal return restoration

Every `TerminatorKind::Return` in a function which owns a mark must evaluate
its operand before restoring the cursor. Emit a block-scoped sequence
equivalent to:

```c
{
    <result-type> sao2_return_value = <return-operand>;
    if (!sao2_scoped_restore(sao2_function_mark))
        sao2_compiler_invariant();
    return sao2_return_value;
}
```

Use the function's exact generated result carrier for the temporary. The
compound statement is also valid immediately after a C label and permits the
same temporary name at multiple return terminators. Evaluating first is
required because a primitive, tuple, or union return expression may read a
projected place inside scoped storage even though the returned value itself
does not retain that storage.

Stage 5 guarantees that a returned reference-bearing value cannot contain an
origin allocated in this invocation's scoped arena. Do not attempt a runtime
copy, promotion, tag rewrite, or post-restore reference check. A parameter
which is returned has an escaping summary bit, so every compiler-visible
caller allocation passed to it is already heap. A callee-local allocation
which is returned is likewise already heap.

Emit restoration at every return terminator, including early returns and
returns reached through branches, switches, or loop exits. Jumps, branches,
switch arms, and loop back edges do not restore. `Panic`, `ErrorPanic`, failed
runtime checks, allocation failures, and `Unreachable` terminate the process
and do not restore; milestone 9 has no unwinding protocol. A failed restore is
a compiler/runtime invariant rather than a source failure.

Do not centralize returns by changing typed IR or synthesizing a new CFG exit
block. Keeping cleanup in terminator rendering makes the rule exhaustive for
the current IR and prevents future return sites from silently bypassing it.

### Lifetime and reference invariants

The Stage 4 packed-reference and resolver rules apply unchanged to both arena
classes:

- a scoped root has the scoped tag in `owner_ptr` and its untagged owner offset
  equals `member_ptr`;
- inline projection preserves the tagged complete owner while changing only
  `member_ptr`, so an interior reference has exactly the root's lifetime;
- reference equality compares both packed words, so identical numeric offsets
  in the heap and scoped arenas are distinct references;
- a caller-owned scoped reference may cross a call only through parameters
  whose summaries do not retain it, and the caller's mark encloses the entire
  call; and
- heap or longer-lived objects must never retain a reference to a younger
  scoped owner. Stage 5 construction, mutation, return, and call constraints
  enforce this statically for compiler-visible values.

A scoped object may retain a heap reference or another scoped reference whose
lifetime encloses its own. Scoped objects allocated by one invocation share
that invocation's end mark, so confined containment cycles are reclaimed
together. The backend must not add a dynamic ownership graph, reference
counting, or arena-order check.

Before a restore, `sao2_resolve_body` accepts a valid scoped root or interior
reference only when its complete requested body ends at or below the current
cursor. Immediately after restore, a reference into the discarded suffix is
invalid until storage is reused. Static escape safety, not a generation
counter, prevents stale references from surviving until such reuse. Future
allocations may reuse the same offsets and must observe zeroed storage.

### Implementation sequence

Implement Stage 6 in the following order:

1. Add allocation-class and per-function scoped queries to `AllocationPlan`,
   with boundary tests for first, middle, last, and missing identities.
2. Store the plan on `Renderer`; enumerate operation coordinates during
   function rendering and prove every struct aggregate selects its exact plan
   entry.
3. Add `sao2_allocate_scoped_struct`, its source-attributed exhaustion and
   commitment failures, and its scoped-root and typed-body postconditions.
4. Select the heap or scoped helper at each struct aggregate while preserving
   the common initialization and publication sequence.
5. Emit one entry mark for each `has_scoped` function and no mark for every
   other function.
6. Extend return rendering with a typed pre-restore temporary and checked
   restoration on every normal return; leave all terminating panic paths
   unchanged.
7. Add mixed-lifetime direct and native regression coverage, then update the
   temporary generated-runtime stage comments to describe active scoped
   allocation.

At every step, valid programs must remain renderable. Do not land a state in
which a site emits a scoped allocation without its owning function's mark, or
a mark is emitted without exhaustive normal-return restoration.

### Tests and completion

Direct plan/backend tests should verify:

- two struct aggregates using the same destination local, definition, or
  source location still select their separate coordinate-indexed classes;
- a missing or malformed allocation entry is rejected before renderer output,
  with no heap fallback;
- a mixed function emits the scoped helper only at `Scoped` sites and the
  existing heap helper only at `Heap` sites;
- a function with one or many scoped sites emits one entry mark, while
  heap-only, allocation-free, recursive, conservative, and unreachable-site
  functions emit none;
- loop-contained scoped allocation does not emit a loop-local mark or restore;
- every return in a marked function first captures the value, then checks one
  restore, then returns, including returns in distinct blocks after branches
  and switches;
- an unmarked function retains its direct return, and panic, error-panic, and
  unreachable terminators never emit restoration;
- the scoped helper reports the exact construction failure site and reason for
  exhaustion or commit failure, rejects invalid allocator results as
  invariants, emits no heap header, and resolves through the supplied layout;
- heap allocations retain their layout-bearing header, failure text, root
  checks, and generated call shape; and
- repeated emission from the same program and plan is byte-for-byte
  deterministic.

Generated-C runtime probes should verify:

- nested marks restore in stack order and a callee restore leaves caller-owned
  scoped objects valid;
- restoration rejects marks below the initial cursor or above the current
  cursor;
- a discarded scoped root and inline interior reference fail resolution before
  reuse, while a later allocation can reuse the offset and sees zeroed bytes;
- heap and scoped objects can use the same decoded offset without comparing
  equal or resolving through the wrong arena;
- scoped exhaustion and commitment failure do not advance the cursor or alter
  the heap arena, and heap exhaustion does not alter scoped state; and
- zero-sized and maximally aligned layouts preserve the root encoding and
  cursor alignment rules.

Native source programs should cover a non-escaping local struct, mutation and
reads through a scoped root, nested inline projection and mutation, passing a
root or interior reference through a non-escaping callee, nested allocating
calls, branch joins, loops, and multiple returns. Mixed graphs should show
scoped owners safely retaining heap children and confined scoped children.
Returning an owner or interior reference, storing into a parameter-owned or
heap object, passing to an escaping parameter, and every recursive dependency
case must continue to allocate the complete owner on the heap.

Exercise allocation failure through controlled runtime probes rather than
attempting to consume the production 4-GiB reservation in ordinary source
tests. Preserve existing output, identity, inline-copy, warning, diagnostic,
transactional-write, and host-adapter regressions.

Contributor guidance prohibits compiling, running tests, or formatting during
implementation. External verification should use Rust 1.90 or newer, report
native skips explicitly, and inspect generated C where necessary to confirm
marks, allocator selection, and restoration order.

Stage 6 is complete when every struct aggregate uses its validated plan class;
scoped allocation is source-attributed, typed, zeroed, and header-free; each
function with a scoped site saves exactly one invocation mark; every normal
return evaluates its result before checked restoration; terminating paths do
not pretend to unwind; nested calls and interior references preserve owner
lifetime; heap behavior is unchanged; and external tests demonstrate safe
reuse plus independent heap and scoped failure behavior.

## Stage 7: Integration and garbage-collector handoff

Status: complete.

Close milestone 9 by exercising the complete source-to-executable path and by
making its memory-management handoff explicit. Stages 1 through 6 established
the representation and algorithms; Stage 7 integrates them, fills coverage
gaps, removes obsolete walking-skeleton descriptions, and records the exact
contracts milestone 10 may depend on.

Stage 7 is not a collector implementation. Do not add marking, sweeping,
reclamation, collection triggers, shadow frames, root traversal, write
barriers, or container storage. Do not change language semantics to simplify
tests. If integration exposes a disagreement, follow `DESIGN.md` and
`GRAMMAR.ebnf`; treat any deliberate semantic change as a separate explicit
decision.

### Integration scope

Exercise one coherent pipeline:

```text
source
  -> parsing and semantic analysis
  -> typed IR validation
  -> escape plan
  -> physical layout plan
  -> generated C
  -> host C compiler
  -> native execution
```

The integration suite must cover combinations which isolated stage tests do
not: a single program may contain inline and referenced members, heap and
scoped allocations, root and interior aliases, tuple or union carriers,
mutation, calls, branches, loops, and more than one return. Assert language
output and failure behavior first; inspect generated C only for properties
which are intentionally below the language abstraction, such as arena choice,
mark placement, metadata, or section ordering.

No Stage 7 production path may bypass typed IR, synthesize a second escape
decision, or infer layout from generated C text. Keep `AllocationPlan` and
`LayoutPlan` as distinct compiler-owned artifacts. Generated names, source
names, spans, and destination locals remain unsuitable identities for either
allocation sites or layouts.

Clean up stale stage comments and temporary wording in generated runtime
sections. In particular, the arena-runtime banner must describe active heap
and scoped allocation rather than claiming Stage 4 heap-only behavior. Keep a
comment visibly identifying the monotonic heap as the milestone-9 temporary
policy so it cannot be mistaken for the completed automatic-memory-management
design.

### Source-level struct graph coverage

Add native end-to-end programs in `tests/end_to_end.rs` which use the public
CLI rather than constructing IR directly. Together they must cover:

- primitive, tuple, and union fields embedded in struct bodies;
- multiple levels of inline structs, including an unaligned interior member
  reached beneath an inline parent;
- referenced struct fields which rebind without changing aliases to the old
  object;
- inline struct assignment which recursively copies language-visible fields
  while preserving the destination slot's packed identity;
- aliases to a root and to two distinct inline members of one owner, proving
  equality observes both `owner_ptr` and `member_ptr`;
- a scoped owner containing another confined scoped reference and a heap
  reference, without allowing either scoped owner to reach longer-lived
  storage;
- a root or inline reference passed through a proven non-retaining call and
  used again by the caller after the callee returns;
- a returned root, returned interior reference, projected store into a
  parameter-owned object, and argument to an escaping parameter, each of which
  keeps the complete originating allocation on the heap;
- acyclic direct-call chains which remain analysable and recursive or
  recursion-dependent call graphs which remain conservative;
- allocations inside branches and loops plus early returns, showing that one
  invocation mark encloses all scoped sites and every normal exit restores it;
  and
- mixed printing, comparison, mutation, and final integer or unit entry
  results so struct support does not regress the established host adapter.

Prefer a few readable programs with exact expected output over many nearly
identical fixtures. Use source constructs for observable behavior and retain
focused IR/backend tests for otherwise unobservable allocation-class choices.
Do not expose raw offsets, tags, layout identities, or arena choice through a
new SAO2 intrinsic.

Add negative source cases at the same boundary for illegal mutation,
incompatible construction or assignment, and infinitely recursive inline
layout. These must remain source diagnostics and must not reach escape
analysis, layout planning, C emission, or the host compiler. A recursive graph
which crosses a referenced `&` field remains valid.

### Generated layout and owner-metadata contract

Audit and lock the handoff without freezing private C structure sizes which
milestone 10 is allowed to extend. For every heap allocation, the runtime must
be able to recover from the decoded complete-owner offset:

- the language body size;
- the deterministic root layout identity;
- the heap lifetime class; and
- collector-owned state initialized to zero.

The header precedes only the complete heap allocation root. Inline subobjects
have no header, mark, or independent allocation identity. Scoped objects have
no heap header. Header bytes never contribute to the language-visible body,
field offsets, copy helpers, equality, or formatting.

Keep struct layout identities equal to the checked widening of
`DefinitionId.index()`. They are deterministic compilation-local identities,
not hashes, source names, stable cross-build serialization keys, or native
descriptor addresses. Identity zero remains valid. The heap header records the
layout of the complete allocation root even when a live reference's
`member_ptr` identifies a nested inline struct.

The generated physical descriptor remains the authority for body size,
alignment, ordered field offsets, storage classes, and nested struct layout
identities. The compiler-side `FieldLayout` must continue to retain each
field's `TypeId`, including tuple and union carriers, through rendering. That
typed information is the milestone-10 input for generated traversal code; the
current physical field table is not required to become a generic runtime type
interpreter in Stage 7.

This is the precise tracing attachment point:

1. the heap header identifies the complete owner's layout;
2. the statically typed root or field being traversed supplies the layout of
   the exact value at `member_ptr`;
3. future generated traversal code follows only the active value shape from
   that member, recursively handling inline structs, tuples, unions, and
   referenced fields; and
4. the collector marks the complete owner allocation separately from tracing
   the exact referenced member.

Milestone 10 may extend the descriptor or pair each layout/type with generated
traversal callbacks and a deterministic registry. It must not renumber layout
identities, reinterpret field offsets, place mark bits in struct bodies, or
require a search from `member_ptr` back to an inline parent. Stage 7 should add
tests proving the current plan retains all facts needed to generate those
callbacks later, but it must not emit placeholder callbacks or a partial
tracer.

### Packed-reference and arena contract

Lock the following generated-C invariants with structural assertions and
runtime probes where appropriate:

- `sao2_ref` remains exactly two `uint32_t` words and eight bytes;
- allocation roots are eight-byte aligned, decoded offset zero is reserved,
  and a zero-sized body still consumes a distinct aligned slot;
- tag zero selects the heap arena, tag one selects the scoped arena, and tags
  two through seven are rejected before pointer arithmetic;
- `member_ptr` is never tag-masked and may be unaligned;
- owner and member pointers are reconstructed from the same arena base chosen
  solely from `owner_ptr`;
- root references have equal decoded owner and member offsets, while inline
  projection changes only `member_ptr`;
- equal numeric offsets in different arenas do not compare equal;
- heap owner offsets remain stable for the process lifetime in milestone 9,
  and no language reference stores a native pointer; and
- scoped restoration changes only the scoped live cursor, never heap state,
  reservation bases, committed high-water marks, or packed references owned by
  an enclosing invocation.

Resolution must continue to reject all-zero references, partial-zero
references, reserved tags, offsets outside the selected arena, arithmetic
overflow, wrong heap layout identities, members outside their complete heap
owner, and scoped members beyond the live cursor. These are generated-code or
runtime invariants, not catchable source failures.

Do not add generation counters to packed references in this milestone. A
discarded scoped reference can become numerically reusable after a later
allocation; escape analysis is what prevents it from remaining observable.
Runtime probes may check rejection before reuse and zeroing on reuse, but must
not promise permanent stale-reference detection.

### Heap-policy replacement seam

Keep generated heap sites calling `sao2_allocate_struct` with a layout
descriptor and source failure site. That helper remains the only
struct-construction translation from the arena result into a source panic.
Below it, `sao2_heap_allocate` owns header placement and the current monotonic
cursor policy.

Audit generated C so program operations do not read or modify
`sao2_heap_arena.cursor`, construct heap headers, or call platform reservation
and commitment helpers directly. Milestone 10 must be able to replace the
implementation below the heap-allocation interface and preserve:

- the generated allocation-site call shape;
- allocation failure attribution and reason selection;
- packed references and non-moving owner offsets;
- layout identity and language-body placement;
- the independent scoped allocator, mark stack discipline, and restoration;
  and
- source construction's initialize-before-publication transaction.

Do not add a free operation for heap objects or simulate collection by
rewinding the heap cursor. Do not route scoped allocation through the future
collector seam. Mutation remains expressed through the existing typed stores
and copy helpers; milestone 10 decides whether its collector requires a write
barrier without changing typed IR semantics.

### Zero-safe future roots

Preserve the representations which allow milestone 10 to link a
zero-initialized shadow frame before every local contains a language value:

- `{ owner_ptr: 0, member_ptr: 0 }` is internal null and never an allocation;
- union tag zero is inactive and its payload is ignored;
- generated locals and aggregate temporaries begin zeroed;
- union injection clears the carrier before writing its selected payload and
  nonzero tag; and
- newly allocated and reused scoped bodies are zeroed before publication.

Stage 7 does not generate shadow frames. It verifies only that current value
initialization does not leave a future traversal callback with uninitialized
reference or union state. Strings retain their separate interned lifetime and
must not be mistaken for packed struct references.

The future collector will walk a generated shadow-frame chain, never scan the
native C stack. No Stage 7 helper may register raw addresses of C locals,
depend on conservative stack scanning, or retain temporary native body
pointers across calls which may eventually collect. Current body pointers are
short-lived expression helpers and must not become language values or roots.

### Diagnostics and pipeline preservation

Extend integration coverage without weakening the established failure
boundaries:

- parser, analysis, semantic, lowering, IR, escape, and backend failures do
  not create or overwrite `build/program.c`;
- warnings produced before a later compiler failure are preserved and printed
  before that failure;
- a struct-allocation exhaustion or commit failure is a source-attributed
  runtime panic naming the construction operation and original filename,
  function, line, and column;
- arena reservation failure before source entry remains an environment/runtime
  failure with no fabricated source site;
- a missing or failing host C compiler remains a toolchain diagnostic rather
  than a source diagnostic or program exit;
- a successfully launched program's exit status and stderr remain program
  behavior rather than compiler failure; and
- `--show-c`, `build`, and `run` continue to use the same generated artifact
  and entry adapter for programs with or without command-line arguments.

Continue invoking the C compiler and generated executable with argument lists,
never a constructed shell command. Filenames and build directories containing
spaces must remain supported. Generated artifacts stay under `build/` and are
not committed.

### Determinism and platform verification

Compile the same source more than once and require identical typed IR, escape
plan rendering, layout identities, descriptor order, failure-site indices,
and generated C. Runtime base addresses are intentionally nondeterministic and
must not appear in output or golden files. Do not introduce iteration over an
unordered collection into any rendered artifact.

Keep the platform abstraction limited to reservation, page-size discovery,
commitment, and release. External verification should exercise a supported
POSIX compiler and the Windows branch when available, including feature-macro
and header ordering. A platform which cannot provide the required 64-bit
address model must fail cleanly rather than silently changing the packed ABI.

Use the existing native-test compiler discovery, including `SAO2_CC`. Skip
only assertions which genuinely require a missing supported C compiler; never
turn a generated-C compilation failure, runtime panic, wrong output, or
nonzero program status into a skip. Report native skips explicitly.

### Implementation sequence

Implement Stage 7 in the following order:

1. Inventory the Stage 1 through 6 contracts against `DESIGN.md`,
   `GRAMMAR.ebnf`, and milestone 9 of `ROADMAP.md`; correct stale generated
   comments and tests without changing behavior.
2. Add focused assertions for header semantics, descriptor identities and
   ordering, retained field `TypeId`s, packed-reference decoding, zero-safe
   carriers, and the heap-policy replacement seam.
3. Add source-level end-to-end programs for inline/referenced graphs,
   identity-preserving copy, aliasing, mutation, mixed lifetimes, calls, and
   control-flow restoration.
4. Add focused generated-C probes for invariants which valid source cannot
   observe: invalid tags and offsets, scoped reuse, independent arena failure,
   and header/body separation.
5. Extend transactional compilation, warning, diagnostic-category,
   `--show-c`, entry-adapter, path-with-spaces, and deterministic-output
   regressions to at least one struct-using program.
6. Perform the external Rust 1.90-or-newer and native C verification matrix,
   recording compiler/platform skips and inspecting generated C for both arena
   paths.
7. Only after that verification passes, mark Stage 7 and milestone 9 complete
   and mark milestone 10 current in `ROADMAP.md`. Start milestone 10 from a new
   current-work plan rather than appending collector implementation to this
   document.

Fix integration defects at the layer which owns them. Do not duplicate layout
or escape logic in a test helper merely to make an end-to-end case pass. Any
new runtime probe hook must be test-only and must not create an environment
variable, CLI flag, or source-visible production behavior.

### Tests and completion

The final direct-test matrix must retain all prior stage cases and add:

- descriptor/header consistency for empty, minimally aligned, padded, nested
  inline, referenced, tuple-bearing, and union-bearing struct layouts;
- complete-owner bounds and exact-member resolution at offset zero-adjacent,
  unaligned, last-byte, and overflow boundaries;
- heap/scoped tag separation, invalid reserved tags, internal null, and
  distinct root/interior equality cases;
- exact allocation-plan consumption for mixed classes and conservative
  recursion without a renderer fallback;
- one mark per scoped-allocating invocation and capture-before-restore at every
  normal return shape;
- zeroing after scoped reuse and independence of heap/scoped cursor and
  commitment failures;
- deterministic plan, descriptor, helper, and generated-C order; and
- absence of collector algorithms, shadow frames, native-stack scanning, or
  direct heap-policy access from generated operations.

The final native matrix must demonstrate exact output and status for mixed
struct graphs, root and interior aliasing, inline copy versus referenced
rebinding, non-escaping calls, escaping returns and stores, nested scoped
calls, loops and branches, recursive conservatism, source-attributed
allocation failure, and unchanged primitive/string/tuple/union behavior.

Contributor guidance prohibits compiling, running tests, or formatting while
preparing or implementing this stage. External verification performs those
commands separately, using Rust 1.90 or newer, and reports whether native tests
ran or were skipped because no supported compiler was available.

Stage 7 is complete when the source-to-native integration matrix passes; both
arena paths and mixed struct graphs have direct and native coverage; output,
diagnostics, warnings, artifacts, and failure categories remain stable; the
packed ABI, layout identities, owner metadata, zero-safe representations, and
heap replacement seam are locked; no collector work has leaked into milestone
9; and the milestone-10 team can add precise shadow-frame traversal and
collection without changing source semantics, packed references, scoped
lifetimes, or generated allocation sites.

## Milestone boundaries

The following work remains explicitly outside milestone 9:

- marking, tracing, sweeping, collection epochs, free lists, or collection
  triggers;
- generated shadow frames or precise root traversal;
- reclaiming storage from the temporary heap allocator;
- list and map storage, general iteration, and dynamic string interning;
- native-stack allocation for objects whose packed references are materialized;
  and
- exposing arena placement, raw pointers, allocation metadata, or layout
  identity as language behavior.

The temporary heap-arena leak is an intentional walking-skeleton boundary,
not the final memory model. Milestone 10 replaces it with collection while
preserving the packed references, physical layouts, owner metadata, and escape
decisions established here.
