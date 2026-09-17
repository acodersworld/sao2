# Current Stage: Collection Policy and Rollover Safety

Status: current.

This document expands Stage 5 of
[Current Milestone: Garbage Collector](CURRENT_MILESTONE.md). It turns Stage
4's exhaustion-only collector into a deterministic repeated policy, replaces
the temporary epoch-exhaustion boundary with safe epoch reuse, and gives each
allocation and collection failure a stable outcome.

Stage 4 already supplies the complete correctness path: precise global and
shadow roots, exact iterative tracing, a stationary linear sweep, free-list
rebuild, and one collect-and-retry allocation slow path. Stage 5 changes when
that path runs and how it behaves indefinitely; it does not change what is
live.

## Outcome

Each managed heap allocation follows one bounded decision tree:

```text
validate and size request
          |
          v
allocation debt at threshold?
     /                 \
   yes                  no
    |                    |
collect for policy   raw allocation attempt
    |                    |
    |              success / hard failure
    |                    |
    |              current-space exhaustion
    |                    |
    |                collect for pressure
    |                    |
    +----------+---------+
               |
      collection successful?
          /           \
        no             yes
        |               |
 classified failure   raw allocation attempt
                        |
                 charge successful span
                        |
                  publish reference
```

There is at most one collection and at most two raw allocation attempts in a
single managed allocation call. A policy collection happens before its first
raw attempt; a pressure collection happens only after an exhaustion result.
If a policy collection just completed and the following raw attempt is
exhausted, a second collection cannot discover a different graph and is not
run.

Collection chooses a fresh nonzero mark epoch. When the configured final epoch
has already been consumed, it first validates the heap and clears every
allocated block's mark epoch before reusing epoch one. This rollover pass does
not move, reclaim, allocate, enumerate roots, or inspect object bodies.

## Preserved contracts

Stage 5 must preserve:

- the packed two-word reference ABI and stable owner/member offsets;
- the 48-byte heap header and all Stage 1 block, frontier, free-list, commit,
  zeroing, first-fit, and output-atomicity invariants;
- the Stage 2 exact trace key, iterative queue, callback ordering, scratch
  boundary, and sticky trace failure;
- the Stage 3 canonical shadow slots, root-chain order, zero-safe inactive
  fields, global-root hook, and no native-stack scan;
- the Stage 4 stop-the-world transaction, successful-trace-before-sweep rule,
  stationary survivors, linear coalescing sweep, and tail trimming;
- scoped allocation, cursor restoration, and traversal of scoped-to-heap
  edges without sweeping scoped storage;
- construction's evaluate, root, allocate, initialize, then publish order;
- source-attributed `StructAllocation` failures and compiler/runtime
  invariants as distinct categories; and
- deterministic generated C with no compiler dependency.

Policy state and rollover may change heap-header mark epochs and private
counters. They must not change source values, layouts, frame contents, scoped
cursors, live bodies, packed references, or allocation placement except where
an earlier policy collection makes dead storage available sooner.

## Stage boundaries

This stage does not add:

- adaptive thresholds, heap-growth heuristics, pause targets, or tuning based
  on elapsed time, addresses, platform page size, or host allocator behavior;
- generational, incremental, concurrent, compacting, or moving collection;
- source-visible collection, threshold, statistics, placement, or manual-free
  controls;
- finalizers, weak references, resurrection, or write barriers;
- container storage or tracing for lists, maps, dynamic strings, or iterators;
- liveness-sensitive root slots or a change to the allocation safe point;
- shrinking or decommitting the reserved arena after collection; or
- frontend, IR, type-system, escape-analysis, or language-semantic changes.

Stage 6 owns broad public-pipeline integration and milestone closure. Stage 5
adds focused end-to-end programs where they prove policy, rollover, or failure
behavior, but does not repeat every language-feature combination.

## Deterministic collection policy

### Allocation debt

Add a private unsigned 64-bit allocation-debt counter and a fixed byte
threshold. The production default is one mebibyte:

```text
SAO2_GC_ALLOCATION_THRESHOLD = 1,048,576 bytes
```

Allow a compile-time override for native and reduced-capacity tests. The
configured threshold must be a positive value representable by `uint64_t`.
It is a generated-runtime setting, not a compiler flag or source feature.

Debt measures successful managed heap allocation traffic since the last
successful collection. Charge the physical block span actually assigned to
the allocation, including its header and any unusable remainder absorbed from
a free block. Reuse of swept storage is charged in the same way as frontier
growth. Scoped allocation, collector scratch, failed allocation attempts,
committed pages, live bytes, and direct probe-only raw allocation are not
charged.

Use saturating addition so policy correctness never depends on counter wrap.
Once saturated, debt remains due until a successful collection resets it.

### Trigger rule

At entry to managed heap allocation, validate the request and determine its
required span before consulting policy. An impossible request fails directly
without causing a collection.

Run a policy collection before the raw attempt exactly when existing debt is
greater than or equal to the configured threshold. Do not include the current
request in that decision. This deliberately permits one allocation to cross
the threshold and makes the next allocation the safe point which collects. It
also avoids collecting an empty heap merely because the first request is
larger than the threshold.

A successful collection resets debt to zero whether or not it reclaims any
blocks. This prevents an all-live heap from collecting on every subsequent
allocation. A failed collection leaves debt unchanged and the requested
allocation unpublished.

If policy is not due:

1. attempt raw allocation once;
2. on success, charge its assigned span and publish the candidate;
3. on commit failure or invalid state, preserve that result without
   collecting; and
4. on current-space exhaustion, collect once, reset debt on success, retry raw
   allocation once, and charge only a successful retry.

If policy is due:

1. collect once before raw allocation;
2. on failure, return the classified collection result without a raw attempt;
3. on success, reset debt and attempt raw allocation once; and
4. return that raw result without a second collection.

The managed wrapper continues to use a private candidate reference. The
caller's output and construction destination remain unchanged unless a raw
attempt succeeds.

### Raw allocator accounting seam

Keep raw allocation free of policy and collection. Extend its private result
path to report the physical span assigned on success, or recover that span
from the newly allocated header before publication. The accounting value must
be validated and must not introduce a fallible step after allocator mutation.

Direct block probes may continue to exercise raw allocation independently.
Production struct allocation must pass only through the managed wrapper, so
all production heap traffic is charged exactly once.

## Epoch rollover

### Configurable epoch limit

Keep header marks and the trace-context epoch as `uint32_t`. Add a private
compile-time epoch limit whose production default is `UINT32_MAX` and whose
test override may select a small positive value. Reject zero and values above
`UINT32_MAX` at C translation time.

The limit changes only how soon the rollover algorithm runs. It must not
change trace, sweep, or liveness semantics. Reduced-limit tests must exercise
the same production rollover code rather than a probe-only reset helper.

### Preparing the next epoch

Replace `SAO2_COLLECTION_EPOCH_EXHAUSTED` with a preparation routine used by
every collection attempt:

1. reject reentrancy and validate initialized runtime, heap walk, free list,
   and epoch-limit state;
2. set the collection-active guard;
3. if the current epoch equals the configured limit, validate the complete
   physical heap before mutation;
4. walk every physical block without allocation, clearing `mark_epoch` on
   allocated blocks and confirming free blocks already have zero marks;
5. set the current epoch to zero only after the clearing walk completes;
6. increment to the next nonzero epoch and publish it before tracing; and
7. perform ordinary global-root, shadow-root, trace, and sweep work.

The heap walk and all offset arithmetic must be validated before the first mark
is cleared. After mutation starts, rollover has no allocation or recoverable
failure point. A post-pass inconsistency is a compiler/runtime invariant.

Rollover clearing is safe even if the following trace fails: it changes only
old liveness stamps. The failed trace's partial marks use the newly consumed
epoch, and the next attempt either uses a different epoch or performs another
complete clear before reusing one. This remains correct even with a test limit
of one.

Free blocks retain epoch zero. Rollover does not rebuild the free list, alter
the frontier, clear bodies, update sweep statistics, or reset allocation debt.
Only a fully successful collection resets debt.

### Failed attempts and reuse

Every trace attempt still consumes its prepared epoch, including scratch or
trace failure. Never decrement the epoch or reuse it without the clearing
pass. Never sweep after a failed trace. Partial marks may remain until a later
trace or rollover because sweep compares only against the current successful
epoch.

## Stable result model

### Internal collection outcomes

Keep collection results private and make them exhaustive:

- success;
- trace work-storage exhaustion;
- invalid runtime, heap, descriptor, root, or trace state; and
- reentrant collection.

Epoch exhaustion is no longer an outcome. Rollover is an allocation-free part
of epoch preparation. Trace item growth and exact-key table growth share the
work-storage exhaustion result because both use collector scratch and have the
same source-level remedy.

The active guard must be cleared and trace scratch disposed on every return
after activation. Invalid preconditions detected before activation make no
state change. Invalid state and reentrancy are compiler/runtime invariants,
not recoverable source failures.

### Allocation outcomes and diagnostics

Keep the arena/allocation boundary explicit enough to distinguish:

- logical exhaustion after the permitted collection path:
  `heap arena exhausted`;
- platform backing-store failure:
  `unable to commit heap storage`;
- collector trace work-storage exhaustion:
  `unable to allocate garbage collector work storage`; and
- invalid or reentrant internal state: `sao2_compiler_invariant()`.

All recoverable heap-allocation reasons use the existing
`StructAllocation` failure site, filename, function, line, and column. Do not
add a new `FailureOperation` or expose the collector result to SAO2 code.

A policy-triggered collection failure and a pressure-triggered collection
failure translate identically. The allocation reference, destination slot,
heap block structure, frontier, free list, and debt remain transactional. A
failed trace may change only consumed epoch state, partial mark epochs, and
temporary test statistics; it never sweeps.

Commit failure is returned only by a raw frontier allocation attempt. It does
not trigger collection afterward because it is a platform failure rather than
evidence that tracing can make a requested span fit. Post-collection logical
exhaustion remains distinct from commit failure.

## Policy and collection state

The private runtime state consists of:

- current collection epoch;
- collection-active guard;
- saturating allocation debt;
- last successful or attempted sweep statistics already present in Stage 4;
  and
- optional probe-only collection count and trigger kind under an existing
  runtime-probe define.

Arena initialization sets epoch and debt to zero and the active guard false.
Release requires an inactive collector and empty shadow chain, then clears all
policy state. A reinitialization starts with no inherited debt or epoch.

Production behavior must not branch on statistics or probe-only state. The
fixed threshold, charged spans, allocation results, and root graph completely
determine when collection occurs.

## Safe-point and reentrancy audit

Both proactive and pressure collection occur inside the same managed struct
allocation safe point established by Stages 3 and 4. Before either trigger:

- the allocating function's frame is linked;
- every live reference-bearing local and materialized operand is in canonical
  frame storage;
- caller frames remain linked;
- no new reference or decoded pointer has been published; and
- struct initializer operands remain rooted across collection.

The policy check must stay after request validation but before a successful
raw allocation. Never collect immediately after raw success: the new object
has not yet been initialized or safely published as a traceable root.

Collection remains synchronous and non-reentrant. Trace scratch uses the host
C allocator, callbacks only inspect existing values and enqueue trace items,
rollover and sweep allocate nothing, and no generated source function runs
during collection. Translate failures only after scratch disposal and active-
guard clearing.

## Runtime rendering and lifecycle

Update the arena-runtime banner to describe deterministic proactive and
pressure collection with safe epoch rollover. Remove Stage 4's temporary
comments and `EPOCH_EXHAUSTED` translation.

Keep runtime dependency order explicit:

1. value carriers, body types, trace declarations, layouts, and typed frames;
2. diagnostics and invariant helpers;
3. arena state, policy counters, block helpers, and raw allocation;
4. exact trace runtime and generated trace callbacks;
5. shadow-chain runtime and generated frame callbacks;
6. rollover preparation, collection, sweep, and policy helpers;
7. managed allocation and source-failure translation;
8. generated functions and host adapter.

Primitive-only, scoped-only, and struct-free programs may continue to omit
collection machinery where dependency-safe. Any generated program containing
managed heap allocation must emit one coherent policy, rollover, trace, and
sweep runtime.

## Implementation sequence

Implement Stage 5 in this order:

1. Add threshold and epoch-limit configuration checks plus allocation-debt
   state, initialization, release, and saturating accounting helpers.
2. Extend raw-allocation success accounting without changing placement,
   mutation order, failure atomicity, or direct Stage 1 behavior.
3. Refactor managed allocation into the one-collection decision tree, retaining
   pressure collection and adding pre-allocation policy collection.
4. Add the validated allocation-free rollover clearing pass and replace the
   epoch-exhaustion branch with reusable epoch preparation.
5. Refine collection/allocation result translation and stable
   `StructAllocation` reasons while preserving invariant boundaries.
6. Update runtime comments and direct emission assertions; remove obsolete
   Stage 4 temporary paths.
7. Extend the production native probe for trigger order, accounting,
   failures, and reduced-limit rollover.
8. Add reduced-capacity generated programs and long stress cases, then audit
   deterministic emission and all earlier GC regressions.

Every intermediate change must preserve the source-to-executable walking
skeleton. Do not introduce a manual collection intrinsic or a second test-only
collector.

## Test plan

### Direct Rust and emission tests

Add focused assertions for:

- production and overridden threshold/epoch-limit constants;
- rejection of zero thresholds and invalid epoch limits by generated C;
- policy state initialization, release, and clean reinitialization;
- physical-span charging for frontier, exact-fit, split, and absorbed-
  remainder allocations;
- saturating debt addition and reset only after successful collection;
- no charge for failed, scoped, scratch, or raw probe-only allocation;
- no policy collection before the first threshold-crossing allocation;
- policy collection before the next raw attempt once debt is due;
- pressure collection only after raw exhaustion;
- no more than one collection in one managed allocation call;
- no more than two raw attempts, with two only on the pressure path;
- impossible requests failing without collection;
- commit failure bypassing collection and retaining its result;
- caller output unchanged on policy, pressure, commit, and retry failure;
- epoch preparation choosing consecutive nonzero epochs below the limit;
- a validated full mark clear before epoch one is reused;
- free blocks retaining zero marks through rollover;
- failed traces consuming epochs and never sweeping;
- rollover followed by failed trace remaining safe at limit one;
- classified scratch exhaustion at the original source failure site;
- invalid and reentrant states remaining compiler/runtime invariants;
- removal of epoch-exhaustion output and temporary Stage 4 comments; and
- byte-for-byte deterministic C emission.

Retain every Stage 1 allocator, Stage 2 tracer, Stage 3 shadow-frame, and Stage
4 collection assertion. Update tests which intentionally call raw allocation
so their accounting expectations remain explicit.

### Native policy and rollover probe

Use the production arena, allocator, tracer, frame chain, rollover, collector,
and sweep. Override only arena capacity, threshold, and epoch limit.

Cover:

- debt just below, exactly at, and saturated above the threshold;
- a crossing allocation succeeding without immediate collection, followed by
  collection before the next allocation;
- successful policy collection with no reclaimed blocks resetting debt;
- policy collection reclaiming dead runs before exhaustion;
- ordinary pressure collection when policy is not yet due;
- pressure retry success and genuine post-collection exhaustion;
- a due-policy collection followed by exhaustion without a redundant second
  collection;
- exact charge values across free-list split, exact-fit, and whole-block use;
- repeated allocate/reclaim/split/coalesce cycles with a stable heap walk;
- repeated rollover with limits one, two, and a small non-power-of-two value;
- live owners and interior aliases surviving every rollover;
- dead objects bearing marks from the epoch being reused;
- partial marks from scratch failure immediately before rollover;
- partial marks from scratch failure immediately after rollover;
- all-live, all-dead, cyclic, highly aliased, and deep iterative graphs;
- collection immediately before and after calls and graph mutation;
- trace item and exact-key growth failure with no sweep;
- injected commit failure distinct from collection scratch failure;
- unchanged references, frontier, and free list after each failed path;
- balanced active guard and scratch ownership on every return; and
- deterministic collection count and trigger kind for an identical sequence.

Assert heap validity after every operation where the runtime can safely return.
Deep-graph coverage must demonstrate iterative work-queue traversal rather
than native recursion.

### Reduced-capacity generated programs

Compile ordinary SAO2 programs against small test-only arena, threshold, and
epoch-limit overrides. Exercise:

- enough successful allocations to cross the threshold well before arena
  exhaustion;
- repeated proactive collections with an all-live working set;
- repeated reclamation and address reuse with a changing root set;
- live cycles, unreachable cycles, and distinct interior aliases across many
  forced rollovers;
- nested and recursive calls contributing simultaneous frame roots;
- scoped objects retaining heap children during proactive collection;
- graph mutation immediately before an allocation which triggers policy; and
- exact output, identity, mutations, and exit status after repeated rollover.

Use source shapes which escape analysis genuinely classifies as heap storage.
Direct IR or native probes may force allocation classes only for isolated
backend facts that public source cannot select.

### Failure and platform coverage

Confirm that:

- final logical exhaustion reports `heap arena exhausted` at the constructor;
- commit injection reports `unable to commit heap storage` at the same site;
- collector scratch injection reports
  `unable to allocate garbage collector work storage` at that site;
- malformed heap, roots, layouts, reentrancy, and impossible post-mutation
  states take the invariant path;
- failed proactive and pressure collection publish no reference and perform no
  sweep;
- primitive, tuple, union, string, scoped-only, and no-struct programs retain
  prior behavior; and
- no native-probe or generated artifact is committed.

Only absence of a supported C compiler may skip native assertions. POSIX and
Windows branches both require external verification before stage completion.

## Completion gate

Stage 5 is complete when:

- successful heap allocation traffic deterministically triggers collection
  before exhaustion using the fixed debt policy;
- pressure collection and retry remain available when policy is not yet due;
- each allocation performs no more than one collection and publishes only a
  successfully allocated reference;
- debt accounting is exact, saturating, private, and reset only by successful
  collection;
- every epoch value can be reused indefinitely only after all old allocated
  marks have been safely cleared;
- reduced-limit production paths survive many rollovers, including failed
  trace attempts, without retaining dead objects or reclaiming live ones;
- collection scratch failure, commit failure, logical exhaustion, and
  compiler/runtime invariants have deterministic distinct behavior;
- no failed trace sweeps and every recoverable allocation failure remains
  transactional and source-attributed;
- repeated split/coalesce, aliased, cyclic, deep, call-boundary, and mutation
  stress leaves the heap and shadow chain valid;
- all earlier allocation, exact-trace, frame, collection, language, and
  diagnostic behavior remains correct;
- generated C is deterministic and adds no dependency or source feature; and
- external native verification passes on the required platform matrix.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must use Rust 1.90 or newer,
report native skips explicitly, and confirm that only intended source and
documentation changes remain.
