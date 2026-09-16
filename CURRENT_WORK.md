# Current Work: Struct Layout and Escape Analysis

Status: current.

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

Status: current.

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

Status: planned.

Implement struct projections and mutation using the storage classification
already present in typed IR:

- projecting an inline struct creates an interior packed reference with the
  same owner_ptr and the field slot's member_ptr;
- projecting a referenced struct loads the packed reference stored in that
  field;
- primitive and other inline value fields address the exact body selected by
  member_ptr; and
- nested projections retain the original root owner across every inline hop.

Assignment to a referenced-struct field rebinds that field. Assignment to an
inline-struct field must instead copy language-visible fields into the
existing destination slot so previously created references retain their
identity. Generate deterministic per-layout field-copy helpers, recurse
through nested inline structs, and handle self-assignment and aliased source
and destination safely.

## Stage 5: Context-insensitive escape summaries

Status: planned.

Add a compiler analysis pass independent of physical C layout. For each
function, record its direct callees, direct callers, and unresolved-callee
count. Analyze leaf functions first, then update callers and enqueue them when
their unresolved count reaches zero. Process each direct call edge once.

Each summary records which parameters may escape, including escape through an
inline member. Track provenance through copies, projections, tuple and union
construction, assignments, branches, calls, and returns. If an interior
reference escapes, the complete owner allocation escapes.

A caller uses the callee's summary without specializing it for a call site.
Any function left unresolved after the queue drains is recursive or depends on
recursion and is conservatively treated as escaping. Unknown or otherwise
unprovable flows receive the same treatment.

Produce an allocation plan that classifies an allocation as scoped only when
all paths prove that neither the root nor any inline reference outlives the
allocating invocation. Keep analysis facts compiler-side; do not encode native
addresses or backend layout details in language IR.

## Stage 6: Scoped arena lifetimes

Status: planned.

Apply the escape plan in generated C:

- proven non-escaping objects allocate from the scoped arena;
- all other objects continue to allocate through the heap-arena interface;
- functions that can allocate scoped objects save one cursor mark and restore
  it on every normal return;
- a scoped reference may be passed through non-escaping parameters and may
  identify an inline member; and
- passing a value to an escaping parameter forces its complete owner
  allocation to use the heap arena.

Ensure control-flow lowering cannot bypass restoration on early returns or
branches. Runtime panic paths terminate the program and do not require an
unwind protocol. Stress nested calls, loops, branch joins, multiple returns,
recursive call graphs, interior references, scoped reuse, and independent heap
and scoped exhaustion.

## Stage 7: Integration and garbage-collector handoff

Status: planned.

Complete source-level and generated-C coverage for mixed inline and referenced
struct graphs. Preserve deterministic output, transactional compilation,
source locations, warning behavior, entry adapters, and the distinction
between source failures and toolchain failures.

Lock down the milestone-10 boundary:

- owner metadata identifies allocation size, layout, and lifetime;
- heap objects are non-moving and retain stable owner offsets;
- the heap allocator can be replaced without changing generated allocation
  calls, packed references, or scoped-arena state;
- owner_ptr selects the arena for both offsets and reserved tag values remain
  invalid;
- generated layouts contain enough information for later exact tracing;
- all-zero references and inactive union payloads remain non-traceable;
- distinct interior references retain their member offsets and layout
  identities; and
- no implementation relies on scanning the native C stack or freeing scoped
  storage through the future collector.

Milestone 9 is complete only after external verification covers both production
arena paths, layout edge cases, nested identity-preserving copies, conservative
recursive summaries, safe scoped reuse, and programs that pass references to
inline structs without allowing them to escape. At that point ROADMAP.md may
mark milestone 9 complete and milestone 10 current.

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
