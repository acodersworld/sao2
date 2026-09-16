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

Status: current.

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

Status: planned.

Teach the C backend to plan physical layouts for every executable struct:

- a language struct value is the packed reference, while a private generated C
  body type contains the actual fields;
- primitive, string, tuple, union, and inline-struct fields occupy inline
  storage according to their existing value representations;
- referenced-struct fields store packed references;
- each inline struct embeds its body at a stable member offset within the
  enclosing allocation; and
- layout planning uses checked sizes, alignments, and offsets and diagnoses
  impossible or cyclic inline layouts before rendering.

Assign deterministic layout identities and emit the metadata needed to locate
and describe an owner allocation. Preserve DefinitionId and FieldId ordering
rather than deriving layout order from unordered collections.

Add a source-attributed allocation failure site for struct construction if the
existing failure-operation representation cannot express it. Arena reservation
or platform setup failures remain compiler/runtime environment failures before
entry; capacity exhaustion caused by a source allocation is a program panic.

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
