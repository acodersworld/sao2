# Current Stage: End-to-End Mark and Sweep

Status: current.

This document expands Stage 4 of
[Current Milestone: Garbage Collector](CURRENT_MILESTONE.md). It connects the
Stage 3 shadow-frame chain to the Stage 2 exact tracer, sweeps unmarked Stage 1
heap blocks, and retries a failed heap allocation after one collection.

This is the first stage in which ordinary SAO2 execution can trigger garbage
collection and reclaim unreachable objects. Collection remains an
exhaustion-only policy. Proactive thresholds, permanent failure policy, and
epoch rollover belong to Stage 5.

## Outcome

Heap allocation gains one synchronous slow path:

```text
sao2_heap_allocate(request)
        |
        v
try free list / frontier
        |
        +------ success ------------------------------> publish reference
        |
        +------ commit failure / invalid ------------> return failure
        |
        +------ current heap cannot fit request
                    |
                    v
             begin collection
                    |
             fresh nonzero epoch
                    |
             globals + shadow frames
                    |
             exact trace work drain
                    |
              successful trace?
                 /       \
               no         yes
               |           |
        do not sweep    linear sweep
               |           |
               |       rebuild free list
               |           |
               |       retry once
               |           |
               +-----------+--------------------------> result
```

The mutator is already stopped because collection runs synchronously inside
the allocating C call. Live allocations never move. A heap reference marks its
complete owner while traversal begins at its exact member layout. Scoped
objects are traversed but never swept.

## Preserved contracts

Stage 4 must preserve:

- the packed two-word reference ABI and stable offsets of every live object;
- the Stage 1 physical block format, 48-byte header, body placement, requested
  body bounds, and deterministic first-fit allocator;
- Stage 1 zeroing before reuse and transactional raw-allocation failure;
- the Stage 2 layout registry, exact trace key, callbacks, queue order,
  scratch-allocation boundary, and sticky failure behavior;
- the Stage 3 canonical frame fields, link/unlink order, no-stack-scan rule,
  global-root hook, and native argument/return no-safepoint windows;
- escape-selected scoped allocation, function-owned marks, and cursor restore;
- struct construction's evaluate-first, allocate, initialize, then publish
  transaction;
- existing source failure sites and diagnostic categories; and
- deterministic generated C and ordinary C function calls.

Only heap-block state, free-list structure, collection epoch state, mark
epochs, and collector scratch may change during collection. Language values,
layout descriptors, frame fields, scoped storage, and live allocation bodies
must not be rewritten.

## Stage boundaries

This stage does not add:

- proactive allocation or byte-count thresholds;
- adaptive, generational, incremental, concurrent, compacting, or moving GC;
- mark-epoch rollover or reuse after `UINT32_MAX`;
- a source-visible collection operation, statistic, threshold, or placement
  control;
- finalizers, weak references, resurrection, or write barriers;
- list, map, dynamic string, or iterator tracing;
- liveness-sensitive frame slots beyond Stage 3's invocation-wide roots; or
- frontend, typed-IR, escape-analysis, or language-semantic changes.

Stage 4 uses a visibly temporary epoch-exhaustion boundary: it never wraps a
nonzero epoch back to zero or reuses an old value. Stage 5 replaces that
boundary with a safe rollover procedure.

## Runtime collection state

Add private runtime state:

- the last epoch consumed by a collection attempt;
- a collection-active flag guarding the synchronous transaction; and
- optional last-pass counters used only by native tests, not language behavior.

Initialize the epoch to zero and the active flag to false with arena runtime
initialization. Arena release requires no active collection and resets both
fields after the shadow chain is proven empty.

Define an internal collection result which distinguishes:

- success;
- trace scratch exhaustion;
- invalid heap, root-chain, descriptor, or trace state;
- temporary epoch exhaustion; and
- an impossible reentrant request.

The result remains private generated-C runtime state. It is not a new IR
failure operation or catchable language error.

## Collection transaction

### Entry and epoch consumption

The collection entry point first requires:

- initialized arenas;
- a valid Stage 1 heap walk and free list;
- no collection already active; and
- an available epoch below `UINT32_MAX`.

Set the active guard before root enumeration. Advance and publish the nonzero
epoch before tracing begins. Every started trace attempt consumes its epoch,
including an attempt which later fails.

This ordering is required because Stage 2 may mark some owners before
encountering malformed state or scratch exhaustion. If a later attempt reused
the same epoch, those abandoned partial marks could incorrectly retain an
unreachable object. Consuming the epoch makes every partial mark stale for the
next attempt.

If the current epoch is already `UINT32_MAX`, return the temporary
epoch-exhaustion result without tracing or sweeping. Never wrap to zero.

### Root enumeration and marking

For a new trace context initialized with the consumed epoch:

1. call the no-op-or-future `sao2_trace_global_roots` hook;
2. enumerate `sao2_trace_shadow_roots` from newest frame to oldest;
3. drain the exact trace work queue; and
4. inspect the sticky trace result.

The trace context remains alive until marking has either completed or failed.
Dispose all native scratch on every path.

Stage 2 already provides the marking semantics:

- a heap trace item sets its complete owner's `mark_epoch`;
- an interior reference marks the same complete owner but traverses only the
  exact member;
- two exact members of one owner are independently traversed;
- scoped items enqueue outgoing edges without marking scoped storage;
- cycles terminate through exact-item deduplication; and
- zero references and inactive union payloads contribute no edges.

Do not use the owner mark as a visited set and do not broaden an interior root
into traversal of the complete owner body.

### Failure before sweep

Sweep only when global enumeration, frame enumeration, and trace drain all
complete successfully. On trace scratch exhaustion or invalid trace state:

- dispose the trace context;
- clear the active guard;
- leave every block, free-list link, frontier, body, and scoped cursor
  unchanged;
- retain any partial mark values written before failure; and
- return the corresponding collection result.

Partial marks are harmless because the consumed epoch will not be reused. Do
not attempt mark rollback.

## Linear sweep

### Liveness rule

At a successful pass with epoch `E`:

- an allocated block is live exactly when `header.mark_epoch == E`;
- an allocated block with any other epoch is dead; and
- an existing free block remains free and must have mark epoch zero.

The sweep never examines language bodies or layout callbacks. Header state is
the sole sweep input after successful marking.

### One-pass free-list rebuild

Sweep from `SAO2_ARENA_INITIAL_CURSOR` to the original heap frontier in
physical address order. Rebuild the free list from scratch rather than calling
`sao2_heap_reclaim` once per dead object. Per-object reclaim would repeatedly
validate and search the heap, and coalescing would invalidate a naive next
offset.

Before the first mutation, validate the complete heap and all arithmetic needed
for the walk. Once mutation begins, the pass must contain no allocation or
recoverable failure point.

During the walk:

1. capture each block's original span and next physical offset;
2. keep a marked allocated block in place with its body and owner unchanged;
3. treat an unmarked allocated block and an existing free block as free
   material;
4. accumulate physically adjacent free material into one run;
5. when a live block ends a run, initialize one free header at the run's lowest
   address and append it to the rebuilt address-ordered free list; and
6. continue from the captured next offset, ignoring obsolete interior headers
   inside a coalesced run.

For an intermediate free run:

- its span covers the complete coalesced run;
- body size, layout identity, lifetime, mark epoch, and reserved state are
  cleared;
- its next-free link is filled by the deterministic rebuild; and
- stale body bytes need not be cleared because Stage 1 zeros the complete
  payload before reuse.

For a final free run reaching the original frontier:

- do not add it to the free list;
- clear the run's bytes;
- move the frontier back to the run's first header offset; and
- leave the committed high-water mark unchanged.

The resulting free list is strictly address ordered, contains every
intermediate free block exactly once, contains no allocated block, and has no
adjacent free entries. Validate the finished heap before reporting collection
success; a post-sweep validation failure is a compiler/runtime invariant, not
a recoverable partial collection.

### Sweep statistics

Return or record deterministic private counters useful to probes:

- allocated blocks examined;
- live blocks retained;
- dead blocks reclaimed;
- body or span bytes reclaimed; and
- resulting free blocks.

These counters must not affect policy in Stage 4 and must not be exposed to
SAO2 source. Stage 5 may decide which counters are useful for thresholds.

## Allocation slow path

### Split raw allocation from managed allocation

Rename or factor the existing Stage 1 algorithm as a raw try operation. It:

- searches the address-ordered free list;
- extends the frontier and commits pages when possible;
- never enumerates roots or collects;
- preserves the output reference on every failure; and
- retains all Stage 1 result categories.

Keep `sao2_heap_allocate` as the managed policy seam called by
`sao2_allocate_struct`. Its algorithm is:

1. validate and size the request;
2. attempt raw allocation;
3. return immediately on success, commit failure, or invalid input;
4. if the request can never fit the logical arena or packed-offset ABI, return
   exhaustion without collecting;
5. on current-space exhaustion, perform one collection;
6. retry raw allocation exactly once after successful collection; and
7. publish the new reference only if that retry succeeds.

Use a private temporary reference for both attempts so the caller's output is
unchanged after first-attempt failure, collection failure, or retry failure.

Collection is useful even when total free bytes are sufficient but fragmented:
sweeping may coalesce adjacent dead and free blocks. If live blocks still
prevent a fitting span, the single retry returns exhaustion.

### Result translation

Extend the private arena/allocation result only as needed to represent a
collection failure. Preserve:

- logical post-collection exhaustion as the existing `heap arena exhausted`
  source-attributed panic;
- platform commit failure as `unable to commit heap storage`;
- invalid allocator or trace state as a compiler/runtime invariant; and
- trace scratch or temporary epoch exhaustion as one clearly temporary
  collector-failure reason at the existing `StructAllocation` site.

Do not add a new `FailureOperation`. Stage 5 will refine and harden collector
failure distinctions, rollover, and final reason selection. Stage 4 must still
guarantee that no failed trace is swept and that no failure publishes an
allocation reference.

## Safe-point proof

Heap allocation is the only collection point. The managed slow path may be
entered only from `sao2_allocate_struct`; scoped allocation never collects.

At that point:

- the allocating function's typed frame is linked;
- every reference-bearing IR local and materialized operand is in canonical
  frame storage;
- caller frames remain linked below it;
- the destination field still contains its previous zero or rooted value;
- no decoded body pointer for the new object exists yet; and
- the raw allocator has not mutated the heap when it reports exhaustion.

After collection and retry:

- the new owner is allocated after sweep and therefore needs no mark in the
  just-completed epoch;
- allocation zeroes its complete payload capacity;
- initialization performs no heap allocation or collection;
- referenced operands remain rooted until publication; and
- the packed reference is published to its destination frame field last.

No collection occurs while parameters or return values travel through native C
channels, during field access or copying, while scoped marks restore, or while
a generated callback is tracing.

## Reentrancy and mutator state

Collection is stop-the-world with respect to the single-threaded mutator. The
active flag rejects an impossible nested collection. Stage 2 scratch uses the
host C allocator and cannot call `sao2_heap_allocate`; trace callbacks only
enqueue or traverse existing values; sweep performs no allocation.

Do not invoke source code, output helpers, panic formatting, user functions,
or scoped allocation during the transaction. Translation of a returned
collection failure into the existing source-attributed panic occurs only after
the active guard is cleared and trace scratch is disposed.

## Runtime lifecycle and section order

Update the arena-runtime banner to describe active on-demand collection while
identifying proactive policy and rollover as pending Stage 5 work.

Render declarations and definitions so dependencies remain explicit:

1. value carriers, aggregate/body types, trace declarations, layout registry,
   and typed shadow frames;
2. diagnostics and invariant helpers;
3. Stage 1 platform, arena, block, and raw-allocation runtime;
4. Stage 2 trace runtime and generated body/value callbacks;
5. Stage 3 shadow-chain runtime and generated frame callbacks;
6. collection state, collection transaction, and linear sweep;
7. the managed `sao2_heap_allocate` wrapper;
8. struct allocation/copy and remaining value helpers;
9. generated functions; and
10. host adapter.

Arena initialization resets collection state only after both reservations
succeed. Arena release requires an empty shadow chain and inactive collector,
then clears epoch and collection state with the arenas.

Programs with no heap struct allocation may omit the collection implementation
when dependency-safe, but generated C must remain standard and deterministic
for zero-struct and primitive-only programs.

## Implementation sequence

Implement Stage 4 in this order:

1. Add collection result/state declarations, initialization, release, active
   guard, and temporary checked epoch advancement without changing allocation.
2. Implement a collection entry point which enumerates global and shadow roots,
   drains tracing, consumes failed epochs, disposes scratch on every path, and
   never sweeps a failed trace.
3. Implement and probe the linear sweep and deterministic free-list rebuild,
   including existing free blocks, dead runs, live separators, and tail
   trimming.
4. Refactor the current allocator into raw-try and managed-policy layers while
   preserving raw allocation tests and generated call sites.
5. Wire exhaustion-only collect-and-retry, request-impossibility checks,
   output-reference atomicity, and temporary collection-failure translation.
6. Audit struct construction, shadow frames, native return channels, scoped
   allocation, and decoded pointers against the allocation-only safe point.
7. Extend native probes for complete collection transactions and add reduced-
   capacity generated source programs which require repeated reclamation.
8. Add failure and deterministic-emission regressions, then remove obsolete
   “collection pending” paths without adding Stage 5 policy.

Every intermediate step must preserve ordinary source execution. Do not expose
a temporary manual collection intrinsic to make end-to-end tests easier.

## Test plan

### Direct Rust/backend tests

Add focused assertions for:

- collection state initialization, release, and non-reentrancy;
- epoch consumption before tracing and consumption on failed trace attempts;
- strict refusal to wrap `UINT32_MAX` in this stage;
- global roots before shadow roots before trace drain before sweep;
- trace-context disposal and active-guard clearing on every result;
- no sweep call after scratch exhaustion, invalid roots, or invalid trace;
- liveness comparison against the exact current epoch;
- one-pass physical order and captured-next-offset safety;
- free-list reconstruction in ascending address order;
- merging old free blocks with newly dead neighbors;
- live blocks retaining header, body, mark, and packed owner identity;
- tail trimming without lowering committed high water;
- raw allocation retaining Stage 1 behavior and never collecting;
- managed allocation trying, collecting at most once, and retrying at most
  once;
- impossible requests failing without a destructive collection;
- output references remaining unchanged on every failure path;
- unchanged `StructAllocation` failure sites and no new IR operation;
- no collection from scoped allocation or any non-allocation operation; and
- deterministic generated C.

Malformed runtime tests should cover an invalid heap before collection,
cyclic or null-callback shadow chains, unregistered layouts, invalid exact
members, a post-validation corruption hook, reentrant collection, zero or
exhausted epochs, and scratch failure after partial marking.

### Native collection probe

Build on the production block, trace, shadow, and collection runtimes. Use a
reduced arena capacity and explicit probe assertions, not a second collector.

Cover:

- an empty heap and a heap with no roots;
- all-live, all-dead, and alternating live/dead block sequences;
- existing free blocks before collection;
- predecessor, successor, and two-sided dead/free coalescing;
- a dead tail, several dead tail blocks, and a live final block;
- unchanged committed high water after tail trimming;
- a live root object at the first and last physical block;
- acyclic unreachable graphs;
- self-cycles and multi-object unreachable cycles;
- live cycles reachable from a root or interior root;
- two rooted interior members exposing distinct child graphs;
- a scoped root and scoped chain retaining heap children;
- tuple and active-union roots plus ignored inactive union bytes;
- nested and recursive shadow frames;
- overwriting and unlinking frame roots before a fresh collection;
- stable live owner/member values across multiple collections;
- address-ordered first-fit reuse of swept storage;
- retry success only because collection reclaimed or coalesced space;
- retry failure when all fitting space remains live or fragmented;
- scratch exhaustion after partial marking with no sweep;
- invalid root state with no sweep;
- temporary epoch exhaustion without wrap;
- collection-active reentrancy rejection;
- clean scratch disposal and valid heap/free list after every path; and
- repeated on-demand collections without proactive thresholds.

Assert liveness using the current epoch rather than requiring old mark fields
to be cleared.

### Reduced-capacity generated programs

Compile ordinary generated functions with the test-only arena-capacity
override already supported by the shared runtime. Exercise public language
behavior rather than calling collector helpers directly.

Programs should cover:

- a loop which repeatedly overwrites the only root to heap-classified objects
  and allocates beyond total arena capacity;
- unreachable cycles reclaimed under allocation pressure;
- live cyclic and interior-referenced graphs surviving repeated collections;
- nested calls whose caller and callee frames both contribute roots;
- scoped objects retaining heap children during a collection;
- mutation immediately before allocation changing the traced graph;
- early returns and recursion leaving a balanced shadow chain; and
- exact output, identity, mutation, and exit status after address reuse.

Where source escape analysis would correctly choose scoped allocation, use a
recursive or otherwise conservatively heap-classified source shape rather than
bypassing the allocation plan. Direct IR tests may still force allocation
classes for isolated backend coverage.

### Failure and regression coverage

Retain every Stage 1 block probe, Stage 2 exact-trace probe, Stage 3 frame
probe, and ordinary struct end-to-end case. Confirm:

- source-attributed final exhaustion still names the constructor's filename,
  function, line, and column;
- commit failure remains distinct from logical exhaustion;
- the temporary collector-failure reason uses the same source failure site;
- an invalid collector state remains a compiler/runtime invariant;
- primitive, string, tuple, union, scoped-only, and no-struct programs retain
  their generated behavior;
- host adapters see an empty frame chain and inactive collector before release;
  and
- no generated or native-probe artifact is committed.

Only absence of a supported C compiler may skip native assertions. A generated
C compile failure, runtime assertion, abnormal termination, wrong output, or
unexpected exit status is a test failure. POSIX and Windows branches both
require external native verification before completion.

## Completion gate

Stage 4 is complete when:

- managed heap exhaustion performs one complete synchronous collection and
  retries allocation exactly once;
- roots are enumerated exclusively through generated globals and shadow
  frames, never the native stack;
- every successful trace is followed by a precise sweep and no failed trace is
  ever swept;
- live root and interior references retain stationary complete owners and
  traverse only their exact reachable shapes;
- unreachable acyclic and cyclic objects become coalesced reusable storage;
- scoped storage is never swept but can retain heap children;
- the rebuilt free list and frontier satisfy every Stage 1 invariant;
- reduced-capacity ordinary programs allocate beyond arena capacity through
  repeated reclamation;
- final exhaustion, commit failure, collector failure, and runtime invariants
  preserve their required categories and source attribution;
- epoch values never wrap or repeat in this temporary stage;
- all prior allocation, tracing, frame, language, and diagnostic behavior
  remains correct;
- direct and native verification passes on the available platform matrix; and
- no new dependency, source feature, or generated artifact is introduced.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must use Rust 1.90 or newer,
report native skips explicitly, and confirm that the working tree contains
only intended source and documentation changes.
