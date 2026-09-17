# Current Stage: Reclaimable Heap Foundation

Status: current.

This document expands Stage 1 of
[Current Milestone: Garbage Collector](CURRENT_MILESTONE.md). It replaces the
temporary monotonic heap representation with a walkable block heap that can
represent, coalesce, and reuse free storage. It does not trace roots, mark
objects, sweep automatically, or trigger collection.

The stage is deliberately below the language and typed-IR layers. Source-level
struct construction must retain its existing call shape, failure site, packed
reference, observable behavior, and escape-selected lifetime.

## Outcome

At the end of this stage, the reserved heap arena is a contiguous sequence of
validated blocks followed by an unused frontier:

```text
offset 8                                                    heap frontier
   |                                                              |
   v                                                              v
   +----------+---------+----------+---------+-----+----------+----+
   | header A | body A  | header B | free B  | ... | header N | ...|
   +----------+---------+----------+---------+-----+----------+----+
```

Every block can be found by a linear walk, every allocated owner can recover
its header by one checked subtraction, and every free block participates in an
address-ordered free list. Allocation deterministically reuses the first
suitable free block before extending the frontier.

Production programs do not reclaim objects yet because there is no mark/sweep
pass. A native runtime probe invokes the private reclamation mechanics directly
to establish the substrate needed by later stages.

## Preserved contracts

Stage 1 must preserve these milestone-9 contracts:

- `sao2_ref` remains exactly two `uint32_t` fields and eight bytes;
- heap tag zero, scoped tag one, and decoded offset zero retain their meanings;
- `owner_ptr` names the complete allocation body and `member_ptr` may name
  an unaligned inline member;
- a heap header remains immediately before the complete owner body;
- language-visible bodies contain no allocator or collector metadata;
- allocation roots remain eight-byte aligned and zero-sized bodies receive
  distinct identities;
- `sao2_allocate_struct` remains the generated construction boundary and
  `sao2_heap_allocate` remains its private heap-policy boundary;
- heap allocation returns the existing arena result categories;
- source failures remain attributed to the existing `StructAllocation` site;
- the scoped arena retains its independent bump cursor, marks, restoration,
  commitment, zeroing, and failure behavior; and
- normal host-adapter initialization and release remain unchanged.

The compiler's layout plan remains the authority for requested body size and
layout identity. The heap must not infer a language layout from native bytes or
duplicate compiler-side layout calculation.

## Stage boundaries

This stage does not add:

- a trace plan, traversal callback, work queue, or visited set;
- shadow frames, global roots, or native-stack scanning;
- automatic marking, sweeping, or collection triggers;
- a source-visible free operation or forced-collection hook;
- moving, compaction, decommit policy, or changes to packed references;
- lists, maps, dynamic string storage, or container metadata;
- a new dependency or general-purpose allocator; or
- a change to escape analysis, typed IR, lowering, or source semantics.

The header reserves an epoch field for Stage 4, but Stage 1 only initializes
and validates it. A free block is created only by collector-private machinery
used by the native probe; ordinary generated code has no path to reclaim a
block.

## Physical block model

### Header

Grow `sao2_heap_header` into a fixed-size, eight-byte-aligned block header
with the following logical fields:

- total block span, including the header and payload capacity;
- requested language body size;
- root layout identity;
- next-free header offset;
- heap lifetime class;
- allocated/free state;
- mark epoch; and
- an explicitly zeroed reserved field.

A suitable initial C representation is:

```c
typedef struct {
    uint64_t span_size;
    uint64_t body_size;
    uint64_t layout_identity;
    uint64_t next_free;
    uint32_t lifetime;
    uint32_t block_state;
    uint32_t mark_epoch;
    uint32_t reserved;
} sao2_heap_header;
```

The representation is private generated-C ABI, but its size and field meaning
must be locked for the rest of the milestone. Assert that the header size is a
nonzero multiple of eight. Header offsets and spans use `uint64_t`; packed
references are created only after the final body offset is proven encodable in
`uint32_t`.

`span_size` covers the header plus its complete payload capacity. It may be
larger than the minimum span required by `body_size` when a free-block tail is
too small to split. Object bounds and exact-member validation use
`body_size`, never the extra capacity.

Allocated headers have:

- state `allocated`;
- the requested body size and layout identity;
- the heap lifetime class;
- a zero next-free offset;
- mark epoch zero in this stage; and
- a zero reserved field.

Free headers retain only the span needed to walk the heap, their free state,
and the next-free offset. Clear body size, layout identity, lifetime, mark
epoch, and reserved state when reclaiming a block. Layout identity zero is
valid for an allocated layout, so state—not identity—distinguishes a free
block.

### Span calculation

The minimum payload capacity is eight bytes, including for a zero-sized
language body. Compute:

1. `payload = max(body_size, 8)`;
2. round payload upward to eight bytes with checked remainder arithmetic; and
3. add the fixed header size with checked arithmetic.

The smallest representable free block is one header plus one eight-byte
payload unit. Never create a smaller fragment. Reject arithmetic overflow,
arena-capacity overflow, or an unencodable final owner offset before mutating
heap state.

Blocks begin at eight-byte-aligned header offsets. Because the header size and
span are multiples of eight, the root body and following header are also
eight-byte aligned. Blocks exactly tile the initialized range from
`SAO2_ARENA_INITIAL_CURSOR` to the heap frontier; there are no gaps,
footers, side headers, or metadata inside bodies.

## Heap state

Retain the reserved base, logical capacity, page size, committed high-water
mark, and frontier offset. Rename or document the former heap cursor as a
frontier: it is the first byte not represented by a block, not the next
unconditionally available allocation address.

Add a free-list head containing a header offset, with zero as the internal
null link. Free-list offsets are untagged arena-relative offsets and are never
language references. Keep the list in strictly increasing address order.

The initialized heap invariants are:

- the frontier begins at `SAO2_ARENA_INITIAL_CURSOR`;
- the free-list head is zero;
- committed high water is at least every represented block byte;
- a physical walk ends exactly at the frontier;
- every free physical block appears exactly once in the free list;
- no allocated block appears in the free list;
- free-list links strictly increase and remain below the frontier;
- adjacent free blocks do not coexist because reclamation coalesces them; and
- bytes at or beyond the frontier carry no live block identity.

Arena release clears the frontier and free-list head along with the existing
reservation state. Initialization remains transactional across the independent
heap and scoped reservations.

## Allocation algorithm

### Validation and sizing

`sao2_heap_allocate` continues to accept body size, fixed eight-byte
alignment, layout identity, and an output reference. Validate compiler-owned
inputs and calculate the required span before searching or committing.

Do not modify the output reference on failure. An invalid alignment or corrupt
internal state returns or terminates through the existing invariant boundary;
representable capacity exhaustion remains `SAO2_ARENA_EXHAUSTED`.

### Reusing a free block

Search the address-ordered free list from its head and select the first block
whose span is at least the required span. This first-fit rule is deterministic
and keeps Stage 1 independent of future collection policy.

Before mutation, validate the selected header, its predecessor link, span,
physical bounds, and the resulting split arithmetic. Then:

1. unlink the selected free block;
2. split it when the remainder can hold a header plus the minimum payload;
3. insert the remainder at the selected block's former free-list position;
4. otherwise give the complete selected span to the allocation;
5. initialize every allocated-header field;
6. zero the complete allocated payload capacity, including absorbed tail
   bytes; and
7. publish the checked root reference last.

Reusing a committed free block performs no platform commitment and does not
change the frontier or committed high-water mark. A split remainder is already
committed and starts at an aligned offset.

### Extending the frontier

If no free block fits, calculate the new frontier and required committed page
boundary without changing runtime state. Require:

- the header, minimum payload, and complete span fit in logical capacity;
- the root body offset is nonzero and no greater than `UINT32_MAX`;
- the span ends no later than the 4-GiB logical boundary; and
- page rounding does not overflow.

Commit only newly crossed pages. If commitment fails, leave the frontier,
committed high water, free list, output reference, and existing blocks
unchanged. After successful commitment, initialize and zero the new block,
advance the frontier, update committed high water, and publish the root
reference last.

No later step in this path may fail after heap state is published. Arrange
validation and reference construction so all fallible arithmetic precedes the
first header write.

## Private reclamation and coalescing

Add one collector-private transition which turns a validated allocated block
into a free block. It accepts an owner or header offset, not an arbitrary
native pointer. Stage 1 uses it only from the isolated probe; Stage 4 will call
it while sweeping.

Reclamation:

1. validates that the offset names the exact root body of an allocated block;
2. clears allocation-only metadata;
3. inserts the block into the free list in address order;
4. coalesces with a physically adjacent free predecessor;
5. coalesces with a physically adjacent free successor; and
6. trims a resulting free tail from the frontier.

Coalescing adds checked spans and retains the lowest header. Removed headers no
longer represent blocks. If a free block reaches the frontier, unlink it and
move the frontier back to its header offset. Repeat tail trimming if necessary.
Do not lower the committed high-water mark or decommit pages; later frontier
growth may reuse those pages without another platform call.

Reclamation is not a language operation. Do not expose it through typed IR,
generated functions, intrinsics, the CLI, environment variables, or the
ordinary struct-allocation wrapper.

## Walking and validation

Centralize checked block navigation so allocation, reclamation, header lookup,
the native probe, and later sweeping do not implement different arithmetic.
A physical walk starts at the initial cursor and advances by validated
`span_size` until it reaches the frontier exactly.

For every block, validation checks:

- aligned header offset and span;
- minimum span and no arithmetic wrap;
- end no later than the frontier;
- known allocated/free state;
- allocated body size no greater than payload capacity;
- allocated lifetime, next-free, epoch, and reserved invariants;
- free metadata clearing and a valid next-free link; and
- exact final termination at the frontier.

Rewrite `sao2_heap_header_for` around arena offsets rather than unchecked
native-pointer subtraction. Given a heap reference, it must:

1. validate the heap tag and decoded nonzero owner offset;
2. prove the owner is at least one header size into the arena;
3. subtract the header size with checked offset arithmetic;
4. prove the header lies in the represented range and names a physical block
   boundary, using the centralized checked walk or an equivalent validated
   index;
5. validate that it is allocated and that header-plus-header-size equals the
   decoded owner exactly; and
6. validate body size, lifetime, and span before returning its address.

An interior `member_ptr` continues to recover the same owner header through
`owner_ptr`. Existing body resolution uses requested body size for owner
bounds, so absorbed free-block capacity cannot become language-visible.
References to reclaimed blocks are invalid runtime state; Stage 3 root
precision and Stage 4 marking ensure valid programs never present one.

## Runtime factoring

Separate the shared platform commitment operation from allocation policy.
The scoped arena may retain a small bump-allocation helper, but the heap must
no longer call a generic routine which blindly advances its cursor.

Keep the implementation in the generated runtime owned by
`src/c_backend.rs` for this stage. It may be split into smaller Rust string
sections or rendering helpers when that makes the block runtime and native
probe share one source. Do not introduce a second allocator implementation in
tests.

Update the runtime banner to identify the milestone-10 block heap and state
clearly that automatic collection is still pending. Preserve generated section
ordering unless a declaration dependency requires a documented change.

## Implementation sequence

Implement Stage 1 in this order:

1. Define block constants, state values, the expanded header, and compile-time
   alignment assertions. Update direct generated-C expectations.
2. Add checked span, block-offset, and physical-walk helpers without changing
   allocation behavior.
3. Split shared page commitment from scoped bump allocation, keeping the
   scoped path behaviorally unchanged.
4. Implement transactional frontier allocation with the new header and retain
   the existing `sao2_heap_allocate` interface.
5. Add the address-ordered free list and deterministic first-fit allocation,
   including checked splitting and full payload zeroing.
6. Add collector-private reclamation, predecessor/successor coalescing, and
   frontier trimming.
7. Harden header recovery and body resolution against free blocks, malformed
   spans, wrong roots, and capacity-versus-body confusion.
8. Build the native probe from the production runtime source and add reduced-
   capacity and injected-failure coverage.
9. Run ordinary source-to-native regression coverage externally, then remove
   obsolete monotonic-heap comments and duplicate arithmetic.

Each step should leave generated source deterministic and normal programs
working. Do not temporarily route scoped allocation through the block heap.

## Test plan

### Direct Rust/backend tests

Add focused assertions for:

- exact header fields, state constants, alignment assertions, and runtime
  section order;
- checked zero-size, near-limit, and overflow span calculations;
- one centralized physical-block stepping implementation;
- address-ordered first-fit selection and split threshold;
- publication of the output reference only after successful initialization;
- header recovery from a root and from an interior reference's owner;
- body bounds using requested body size rather than payload capacity;
- no collection call, shadow frame, trace callback, or new failure operation;
- unchanged scoped allocation, mark, restore, and zeroing code;
- unchanged generated struct-allocation call sites and source failure IDs; and
- byte-for-byte deterministic emission.

Malformed directly constructed runtime states should be rejected
deterministically: zero or unaligned spans, spans below minimum size, wraps,
unknown states, duplicate or descending free links, free-list cycles, free
headers omitted from the list, allocated headers present in it, adjacent free
blocks, wrong lifetime, nonzero reserved state, and a walk which misses or
passes the frontier.

### Native runtime probe

Assemble a C translation unit from the same fixed value definitions, platform
layer, and arena/block runtime used by production emission. Test-only
configuration may reduce logical capacity and inject reservation or commitment
failure, but those controls must not be reachable through the standard
renderer, CLI, source language, or production environment.

The probe covers:

- initial empty walk and free list;
- distinct zero-sized allocations and aligned roots;
- mixed small and padded body sizes;
- exact layout identity and requested body-size recovery;
- reclaim followed by same-address reuse;
- first-fit choice among differently sized free blocks;
- exact split at the minimum remainder boundary;
- no split one alignment unit below that boundary;
- predecessor-only, successor-only, and two-sided coalescing;
- repeated coalescing and tail trimming back to the initial frontier;
- unchanged committed high water after tail trimming;
- zeroing of the full reused payload, including an absorbed tail;
- stable live owner offsets while unrelated blocks are reclaimed and reused;
- exhaustion with no fitting free block;
- reuse without additional commitment;
- injected commit failure with byte-for-byte unchanged logical heap state;
- invalid or stale owner rejection before body access;
- independent scoped allocation and restoration throughout heap reuse; and
- clean release after a mixture of live and free blocks.

Use the existing supported-compiler discovery and argument-list process
invocation. A genuinely missing supported compiler may skip the native probe;
a C compile failure, assertion failure, signal, or unexpected status must fail
the test. Windows and POSIX branches both require external native verification
before the stage is complete.

### Public pipeline regressions

Retain end-to-end programs which exercise heap and scoped struct construction,
root and interior projection, identity-preserving inline copy, referenced
rebinding, calls, early returns, and all host-adapter shapes. Their output,
status, diagnostics, and generated allocation failure locations must not
change.

Production-size smoke coverage must still reserve the two independent 4-GiB
virtual arenas and commit pages on demand. Generated artifacts remain under
`build/` and are not committed.

## Completion gate

Stage 1 is complete when:

- the heap is a validated contiguous block sequence with deterministic,
  address-ordered first-fit reuse;
- checked splitting, coalescing, and frontier trimming preserve a walkable
  heap under repeated probe reclamation;
- allocated owners retain stable packed offsets and exact header recovery;
- reclaimed storage is zeroed before reuse and never exposes allocator
  metadata as language data;
- exhaustion and commitment failure leave logical heap state unchanged;
- the scoped arena and every existing source-level behavior remain unchanged;
- no tracing, root registration, automatic sweep, or collection trigger has
  entered production behavior;
- direct and native coverage passes on the available platform matrix; and
- the repository contains no generated artifacts or new dependency.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must use Rust 1.90 or newer,
report whether native tests ran or were skipped, and confirm the working tree
contains only intended source and documentation changes.
