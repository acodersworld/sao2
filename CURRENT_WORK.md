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

Status: current.

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

Status: planned.

Connect the proven arena runtime to generated programs. Initially classify
every struct construction as GC lifetime and allocate it monotonically through
the heap-arena interface. This conservative step makes structs executable
before escape analysis can reduce their lifetimes.

Construction must allocate and zero the complete owner body, initialize fields
in typed-IR order, and return a reference whose decoded owner offset equals its
member offset. Packed references must then flow unchanged through locals,
assignments, arguments, returns, tuples, unions, and direct or recursive calls.
Struct equality and inequality use exact reference identity; ordering remains
unsupported.

Initialize both arenas before entering generated SAO2 code and release their
virtual reservations after normal entry completion. No heap object is reclaimed
during this stage, so execution remains safe without a collector or
shadow roots. Generated code must call the heap interface rather than depend on
the temporary monotonic policy.

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
