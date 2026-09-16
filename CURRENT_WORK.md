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

Each function that performs scoped allocation saves the upper cursor on entry
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

Build an isolated generated-C prototype before production struct lowering
depends on the representation. This stage settles the platform and allocator
contracts that would otherwise make every later stage speculative.

The prototype must:

- define the packed reference as two uint32_t fields and prove with generated
  static assertions that its size is exactly eight bytes;
- reserve and release separate 4-GiB heap and scoped arenas on supported
  64-bit Windows and POSIX hosts without initially committing either range;
- reserve decoded offset zero in both arenas and treat the all-zero reference
  only as internal null storage;
- commit physical storage on demand at each arena's page granularity;
- encode the heap and scoped tags in owner_ptr, reject reserved tags, and
  reconstruct both owner and member pointers from the selected base;
- align allocations without overflowing or truncating an offset;
- implement a monotonic heap allocator behind the heap-arena interface and
  a separate scoped bump allocator;
- save and rewind scoped allocation marks, zeroing storage before reuse;
- reject any individual size or aligned position that cannot fit its arena's
  packed-offset model; and
- keep arena-reservation failure distinct from source-attributed allocation
  exhaustion.

Keep operating-system calls behind narrow runtime helpers selected by existing
generated-C platform conditionals. Use explicit test-only arena-size overrides
so independent exhaustion, alignment, and rewind behavior can be exercised
without consuming the production address space.

Prototype the minimum owner metadata shape alongside heap allocation.
Metadata may live outside language object bodies, but lookup from owner_ptr
must be direct and deterministic. Reserve only fields justified by milestone 9 or the
documented milestone-10 handoff: allocation size, layout identity, lifetime
class, and collector-owned state that can remain zero for now. Demonstrate
that the future trace identity can distinguish equal owners with different
member offsets and layouts; do not implement traversal.

Stage 1 is complete when external verification demonstrates:

- the reference ABI is exactly eight bytes and all-zero is never allocatable;
- root and nested interior references, including unaligned members, round-trip
  through tag and offset conversion in both arenas;
- allocation roots preserve eight-byte alignment without imposing it on
  interior members;
- heap and scoped allocations use independent reservations and allocator state;
- scoped rewind permits safe zeroed reuse while heap offsets remain stable;
- overflow and deliberately small-arena exhaustion fail deterministically; and
- the reservation and commit strategy works on each host family supported by
  the compiler.

Do not connect source-level struct operations to the prototype in this stage.
That integration begins only after the ABI and arena invariants are fixed.

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
