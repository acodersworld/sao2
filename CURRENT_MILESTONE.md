# Current Milestone: Garbage Collector

Status: current.

This document maps milestone 10 of `ROADMAP.md` into implementation stages.
The milestone replaces the temporary monotonic heap from milestone 9 with a
precise, non-moving, stop-the-world mark-and-sweep collector. The packed
reference ABI, physical struct layouts, escape decisions, and scoped arena
remain unchanged.

The collector is an implementation detail. It must not add source syntax,
placement controls, finalizers, weak references, or observable allocation
addresses. Every stage must preserve the source-to-executable path, and heap
allocation must continue to report failures at the originating struct
construction site.

## Fixed architecture

Collection is synchronous and may occur only inside the heap-allocation slow
path. Generated functions continue to call one another as ordinary C
functions. A linked shadow-frame chain, rather than the native C stack,
contains every active reference-bearing local and materialized temporary.
Root-bearing locals live directly in typed shadow-frame fields; they are not
mirrored between an ordinary C local and a separate root slot.

Each heap allocation remains stationary for its complete lifetime. Its packed
`owner_ptr` continues to identify the complete allocation, while `member_ptr`
may identify an inline struct within it. Tracing a reference therefore has two
separate effects:

1. retain the heap allocation named by `owner_ptr`; and
2. traverse the statically known value shape at the exact `member_ptr`.

The trace work set is deduplicated by owner, exact member, and referenced
layout. Deduplicating only by owner is incorrect because two live interior
references into one allocation can expose different outgoing references.
Heap headers identify the root allocation layout for validation and sweeping;
they do not replace the exact layout supplied by a root or field traversal.

Scoped allocations are never marked or swept. They must nevertheless be
traversed when reachable from a shadow-frame root, because a live scoped
object may contain references to heap allocations. The escape analysis from
milestone 9 remains responsible for preventing a heap object or longer-lived
scope from retaining a scoped allocation.

Generated traversal follows only reference-bearing parts of a value:

- struct references enqueue their packed reference and the statically known
  struct layout;
- inline struct fields are traversed at their embedded slot;
- tuples recursively traverse their fields;
- unions traverse only the active payload and ignore zero/inactive storage;
- primitives, strings, and unit contain no GC references; and
- lists and maps remain outside this milestone and gain traversal support in
  milestone 11 through an explicit extension point.

The collector has no write barrier. All mutator execution is stopped during a
collection, and the next collection observes completed stores through the
current roots. Generated code must not retain a decoded native body pointer
across a call which can allocate and therefore collect.

The existing deterministic layout identities remain compilation-local
identities. Generated trace plans, descriptors, callbacks, frame fields, and
registries use compiler identities and stable source-independent ordering.
They must not depend on native addresses, hash-map iteration order, or source
identifier spelling.

## Stage 1: Reclaimable heap foundation

Status: complete.

Replace the monotonic heap's private representation with a block model which
can later sweep and reuse storage, without enabling automatic collection yet.
Keep `sao2_heap_allocate` as the sole policy seam used by generated struct
construction.

Extend heap metadata so the runtime can walk every committed block, distinguish
allocated and free blocks, recover the language body and its layout identity,
and represent a mark epoch. Define checked splitting and adjacent coalescing,
with deterministic allocation selection. Reused language bodies must be
zeroed before publication, and neither headers nor free-list links may appear
inside language-visible storage.

Build an early isolated native probe from the same runtime source emitted by
the backend. With reduced arena capacity, exercise block walking, splitting,
coalescing, exact header recovery, stable owner offsets, zeroing on reuse, and
failure atomicity. Test-only probe operations may manufacture block states;
normal generated programs still allocate without collecting in this stage.

Stage 1 is complete when the heap can safely represent and reuse free extents,
the existing allocation call shape and packed references are unchanged, and
ordinary source programs retain milestone-9 behavior.

## Stage 2: Exact trace plans and callbacks

Status: complete.

Add a compiler-owned trace plan derived from typed IR types and the physical
layout plan. It records which value shapes can contain struct references and
generates one deterministic traversal callback for each required struct,
tuple, and union shape. Primitive-only shapes produce no callback.

Add the collector work queue and visited set behind a narrow trace context.
An enqueued item contains a packed reference plus the exact referenced layout.
Processing a heap item marks its complete owner and traverses from its exact
member. Processing a scoped item skips marking but performs the same exact
traversal. Invalid tags, mismatched layouts, out-of-owner members, inactive
union payloads, and malformed header identities remain runtime invariants.

Extend the isolated probe to drive generated callbacks from synthetic roots.
Cover root and unaligned interior references, two distinct members of one
owner, cycles, repeated paths, inline structs, tuples, active and inactive
unions, and a scoped path to a heap child. No native-stack scanning or source-
visible forced-collection operation is added.

Stage 2 is complete when a supplied typed root computes the precise reachable
heap-owner set for cyclic and interior-reference graphs, without yet changing
when production programs collect.

## Stage 3: Precise shadow frames

Status: complete.

Add a root plan for every function after IR validation and escape analysis.
The plan selects all locals whose type can carry a struct reference, including
parameters and compiler-materialized temporaries, and assigns deterministic
fields in a generated function-specific frame. Non-root locals remain ordinary
C locals.

Emit a common frame header containing the previous-frame link and a generated
traversal callback. Each invocation zero-initializes and links its complete
typed frame before copying reference-bearing parameters into it. All reads,
writes, projections, aggregate destinations, and call operands for selected
locals use the frame fields as their canonical storage. Every normal return
captures its result, unlinks the frame, restores any scoped-arena mark, and
then returns in the required order.

The caller keeps arguments rooted until control enters the callee; the callee
roots reference-bearing parameters before any possible allocation. A returned
reference may travel in the native return channel because no collection can
occur between the callee unlink and the caller's assignment. Document and
lock these safe-point assumptions in direct generated-C tests.

Generate a frame callback which visits every selected field using its exact
static type. Link even frames which contain only currently inactive root
storage when required by their plan; zero references and inactive union
payloads must be harmless. Provide an explicit empty global-root hook for
future language features rather than treating interned strings or runtime
metadata as GC roots.

Stage 3 is complete when every active reference-bearing IR local has exactly
one canonical, zero-safe shadow-frame slot and the complete frame chain can be
traversed without inspecting the native stack. Collection is still not
automatically triggered.

## Stage 4: End-to-end mark and sweep

Status: complete.

Connect the shadow-frame chain to the trace engine and implement a complete
collection transaction: advance the collection epoch, visit global and frame
roots, drain exact trace work, sweep every allocated heap block, coalesce dead
extents, and leave live owner offsets unchanged.

Wire the transaction into the heap-allocation slow path. Allocation first uses
available space, then performs one collection and retries before reporting
arena exhaustion. A struct under construction is not published until its body
is initialized; all of its already-evaluated reference-bearing operands remain
rooted in the allocating frame while collection runs.

Use reduced-capacity native programs to prove that unreachable acyclic and
cyclic graphs are reclaimed, live root and interior aliases retain their
owners, distinct interior members are both traced, scoped objects lead to heap
children, and freed extents are reused without moving survivors. Collection
must not alter scoped cursors, packed references, layout identities, or source
failure locations.

Stage 4 is complete when ordinary source programs can allocate beyond the
reduced heap's physical capacity through repeated reclamation, while every
reachable graph remains valid and stationary.

## Stage 5: Collection policy and rollover safety

Status: complete.

Turn the correct collector into a bounded, repeatable runtime policy. Add a
deterministic collection threshold so reclamation is not attempted only after
arena exhaustion, while retaining the collect-and-retry path for allocation
pressure. Policy counters are private runtime state and are not observable by
SAO2 programs.

Define failure behavior for collector work storage, metadata growth, commit
failure, and genuine post-collection arena exhaustion. Preserve the existing
`StructAllocation` source site and distinguish actionable runtime reasons.
Collection must be non-reentrant and must leave the heap walkable after every
failed internal operation; compiler/runtime invariant failures remain distinct
from source-attributed allocation failures.

Handle mark-epoch rollover explicitly. Before reusing an epoch value, clear or
rebase mark state across all allocated blocks so an old mark cannot retain a
dead object. A reduced-width test configuration must force multiple rollovers
and collections. Also stress repeated split/coalesce cycles, highly aliased
graphs, deep graphs without native recursion, and collection immediately
before and after calls and mutations.

Stage 5 is complete when collection remains correct over unbounded logical
cycles, allocation failures are deterministic and transactional, and no
correctness property depends on an epoch never wrapping.

## Stage 6: Integration and milestone closure

Status: current.

The detailed implementation plan is in
[Current Stage: GC Integration and Milestone Closure](CURRENT_STAGE.md).

Exercise the full public pipeline with readable source programs combining
heap and scoped allocations, root and interior aliases, inline and referenced
members, tuples, unions, mutation, branches, loops, direct calls, recursive
calls, and all normal return shapes. Include cyclic graphs whose only roots
change over time and enough allocation pressure to require several
collections.

Audit generated code for the milestone boundaries: no conservative native-
stack scan, moving or compaction, manual free operation, write barrier,
container implementation, source-visible GC control, raw native pointer in a
language value, or collection of scoped storage. Ensure generated body pointers
cannot remain live across allocation safe points.

Retain focused backend and native-probe coverage for facts source programs
cannot observe: descriptor/callback ordering, exact trace keys, header/body
separation, frame link order, zero-safe inactive fields, owner marking versus
member traversal, free-block coalescing, and epoch rollover. Confirm identical
input produces byte-for-byte identical C.

External verification must use Rust 1.90 or newer and supported C compilers on
both platform families when available. It must report native-test skips,
exercise production-size arena reservation, and leave generated artifacts only
under `build/`. Contributor guidance prohibits compiling, running tests, or
formatting while implementing this milestone.

Stage 6 and milestone 10 are complete when cyclic and interior-reference
graphs are reclaimed safely under sustained allocation pressure; all live
values survive every collection; failures and diagnostics preserve their
established categories and source locations; and milestone 11 can add traced
container storage through the documented traversal extension point.

## Milestone boundaries

The following remain outside milestone 10:

- lists, maps, their backing storage, and iteration state;
- changing string interning or placing strings in the traced heap;
- concurrent, incremental, generational, compacting, or moving collection;
- weak references, finalizers, resurrection, and user-observable GC controls;
- native-stack scanning or conservative roots;
- collecting or individually freeing scoped allocations; and
- changing escape-analysis placement decisions except to fix a demonstrated
  correctness defect in milestone 9.
