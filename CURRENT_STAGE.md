# Current Stage: Recursive Tracing and Growth Hardening

Status: current.

This document expands Stage 5 of
[Current Milestone: Containers](CURRENT_MILESTONE.md). Stages 1-4 completed the
public list/map surface, ordered storage, managed growth, exact callbacks,
iteration, and alias-wide mutation locks. This stage does not add language
features. It hardens those mechanisms under recursive graphs, repeated backing
replacement, collection pressure, arithmetic boundaries, injected failures,
free-list reuse, and collection-epoch rollover.

Stage 5 is the adversarial runtime stage. Public source tests establish that
the language remains observable only through its designed semantics; focused
native probes exercise states and timing which source cannot request directly.

## Outcome

At the end of this stage:

- every valid recursive combination of structs, tuples, unions, lists, and
  maps has a finite deterministic descriptor/trace plan;
- cycles crossing container controls, backings, structs, and interior
  references are retained exactly and traced without recursion leaks;
- repeated list growth and map growth/rehash preserve all live identities while
  superseded backings become reclaimable;
- replacement and removal stop retaining dead values even when the surrounding
  backing remains live;
- allocation policy and pressure collections can occur at every container safe
  point without a decoded pointer surviving;
- capacity and body-size arithmetic fails before overflow or partial mutation;
- heap exhaustion, platform commitment failure, and trace-scratch failure keep
  their stable classification and source attribution;
- failed internal work leaves the published heap graph valid for inspection or
  termination;
- free-list rebuild/coalescing and allocation reuse remain correct after heavy
  container churn; and
- epoch rollover preserves live recursive graphs and makes old mark values
  harmless.

The central invariant is:

```text
canonical roots
      |
      v
stable controls -----> currently published backings
      |                         |
      |                         v
      +-----------------> initialized live values
                                |
                                v
                  exact (owner, member, layout) graph

new backing transaction: rooted privately -> complete -> publish
old backing: published until replacement -> unreachable -> later reclaim
```

## No new language semantics

This stage must not invent behavior to simplify a stress test. `DESIGN.md`
remains authoritative for:

- list index order and map insertion order;
- value copying versus object identity;
- tuple and union equality;
- map key equality and hashing;
- replacement, removal, reinsertion, and duplicate literal rules;
- iteration binding and structural mutation behavior;
- panic reasons and source operations; and
- ASCII/interned string behavior.

If a hardening test exposes disagreement between implementation and design,
fix the implementation unless an explicit design decision is made and recorded
separately. Private capacity, allocation placement, collection timing, backing
identity, and free-list position remain unobservable.

## Preserved contracts

Stage 5 must preserve:

- the 8-byte packed `sao2_ref`, arena tags, zero sentinel, stable owner/member
  offsets, and 4-GiB arena representation;
- fixed struct/list/map control layouts and variable backing descriptors;
- canonical typed shadow frames, map transaction frames, and entry-argument
  roots;
- allocation as the only collection safe point;
- no native pointer surviving a managed allocation;
- exact trace deduplication by owner, member, and layout identity;
- the distinction between marking an owner allocation and traversing one exact
  referenced member;
- initialized-prefix tracing for list elements and ordered map entries;
- non-tracing lookup-slot metadata;
- stationary live allocations and context-insensitive escape placement;
- checked geometric capacity growth and allocate/copy/publish transactions;
- shared iteration locks and balanced normal cleanup;
- source failures remaining distinct from runtime invariants, compiler errors,
  toolchain failures, and program exit status; and
- dependency-free deterministic generated C on supported 64-bit targets.

Hardening may add validation and probe-only instrumentation, but production
semantics and ABI shapes must remain stable.

## Stage boundaries

This stage does not:

- add collection syntax, weak references, finalizers, destructors, or explicit
  user GC controls;
- expose capacity, addresses, hash slots, epochs, allocation statistics, or
  backing identities to source;
- introduce moving/compacting GC, concurrent collection, or thread safety;
- change the 4-GiB packed-reference model;
- add new container operations or iterator forms;
- redesign diagnostics presentation; or
- replace Stage 6's readable end-to-end algorithms, full conformance sweep,
  deterministic-output closure, and external platform verification.

Stage 5 tests mechanisms at their narrowest useful layer. Stage 6 demonstrates
the completed language as one integrated system.

## Hardening threat model

Exercise each invariant against four independent pressures:

1. **Shape pressure:** deep nesting, cycles, repeated references, multiple
   interior members of one owner, inactive unions, and zero-sized values.
2. **Mutation pressure:** repeated growth, replacement, removal, reinsertion,
   rehash, iteration locks, and aliasing.
3. **Allocator pressure:** policy collection, capacity pressure, fragmentation,
   free-list reuse, commitment boundaries, and impossible-size requests.
4. **Collector pressure:** trace worklist growth, trace-key collisions, scratch
   allocation failure, repeated collections, and epoch rollover.

Test combinations, not merely each axis in isolation. The dangerous cases are
transitions—for example, a cyclic value copied to a new backing immediately
before pressure collection, or two distinct interior members of one owner
reached through old and new container paths.

## Recursive descriptor closure

Validate the backend's deterministic closure over every concrete type reachable
from container elements, map keys/values, aggregate fields, function storage,
and nested container types.

The planner must:

- insert a type into its visited set before following recursive dependencies;
- distinguish legal reference cycles from illegal by-value layout cycles;
- plan each concrete container control/backing exactly once;
- plan each required value copy/equality/hash/trace capability exactly once;
- retain stable managed-layout identities independent of discovery order;
- emit declarations/prototypes in an order valid for mutually recursive trace
  callbacks; and
- reject an impossible descriptor dependency as a compiler invariant with
  concrete type context rather than recursing indefinitely.

Cover legal graphs such as:

- a struct containing a list of its own nominal type;
- mutually referential structs connected through lists and maps;
- a union alternative containing a container whose values lead back to the
  union's owning struct;
- tuples/unions which contain several container carriers;
- lists of maps whose values are lists; and
- maps whose values are structs containing interior referenced members.

The language's map-key restrictions still prevent container/object reference
cycles through keys. Keys participate in descriptor closure only through valid
unit, integer, string, boolean, and immutable tuple shapes.

## Recursive graph matrix

Construct graphs which isolate each traversal edge:

- frame root to container control;
- container control to current backing;
- list backing to element value;
- map ordered-entry backing to value;
- tuple/union value to active contained carrier;
- struct carrier to exact root or interior body;
- interior struct member to another managed object; and
- managed object back to an earlier container, closing a cycle.

Required combinations include:

- list -> list -> struct -> original list;
- map -> value struct -> list -> original map;
- list -> active union -> map -> value -> original list;
- two containers retaining different inline members of one owner allocation;
- many aliases retaining one control through different aggregate paths;
- duplicate references to one exact member/layout key; and
- one owner reached at multiple member offsets and layout identities.

For every graph, separately prove live retention and dead reclamation by
dropping selected roots or replacing/removing selected edges.

## Exact trace-key behavior

The collector deduplicates traversal by:

```text
(owner_ptr, member_ptr, layout_identity)
```

Stage 5 must prove all three components matter:

- repeated identical keys are visited once and still retain the complete
  reachable subgraph;
- identical owners with different member offsets are traversed independently;
- an owner mark does not suppress later exact-member traversal;
- the same member cannot be accepted under an incompatible unregistered or
  non-containing layout;
- heap and scoped owner tags resolve through the correct arena; and
- cycles terminate because repeated exact keys are deduplicated, not because
  traversal silently drops valid edges.

Use deliberate trace-hash collisions in native probes. Collision behavior must
fall back to full key equality and remain deterministic; hash-table slot order
must not affect which objects are marked.

## Initialized values and unused capacity

Container callbacks may inspect only published, initialized language values.
Harden the agreement among control length, backing prefix, and capacity:

- a list's initialized element count equals published logical length;
- a map's initialized ordered-entry count equals published logical length;
- lookup-slot occupied metadata agrees with map length but carries no trace
  edges;
- zero-capacity controls have zero backing references;
- spare capacity is never traced or compared as a value;
- a removed/replaced final slot is cleared sufficiently to be zero-safe but
  correctness does not depend on spare bytes remaining zero forever; and
- inactive union storage is not traversed.

Native probes should poison unused capacity and inactive payload bytes with
nonzero patterns which resemble packed references. Collection must ignore them
while retaining every value inside the initialized prefix.

During probe-only partial construction, advance `initialized` only after the
complete record is copy-valid. A collection between record publications must
trace exactly the prefix already declared initialized.

## Allocation safe-point audit

Create a single audited inventory of every generated operation which can reach
managed allocation:

- struct allocation;
- list control/backing construction;
- list append growth;
- map control and dual-backing construction;
- missing-key map insertion and dual-backing growth;
- entry-argument list construction; and
- any runtime path which invokes collection for allocation policy or pressure.

For each site, document and test:

- which source/canonical operands must be rooted before the call;
- which published old references remain the authoritative graph;
- which transient new references are rooted by a typed transaction frame;
- which decoded pointers are discarded before the call;
- which pointers/references are re-resolved after it;
- the exact initialized prefix visible if collection runs; and
- the final publication order.

No descriptor copy, equality, hash, lookup, iteration-value, replacement, or
removal helper may unexpectedly allocate. If a future implementation changes
that fact, it requires a new safe-point design rather than relying on native
locals.

## Backing replacement transactions

### Lists

On append growth, the receiver control and appended value remain canonical
roots. Collection may occur before the new backing allocation succeeds. After
return, re-resolve the control and old backing, copy exactly the old logical
prefix, append the new value, and publish backing/capacity/length in the defined
order. The old backing stays control-reachable until publication.

### Maps

On construction or growth, the typed transaction frame roots the new ordered
entry backing across allocation of lookup slots. Only after both backings are
complete and mutually consistent may the stable control publish the pair.
Old entry and slot backings remain published together until replacement.

### Failure and reclamation

Any checked arithmetic or allocation failure occurs before publication and
must not alter the old control. Once publication succeeds, no root may retain a
superseded backing merely because a scratch field or temporary was not cleared.
The next eligible collection must be able to reclaim old backings while keeping
the stable control and new pair stationary.

Probe collection immediately before allocation, between map backing
allocations, immediately after initialization, immediately after publication,
and after transaction-frame unlinking.

## Replacement, removal, and reachability

Non-growing mutation has no collection safe point, but it changes the graph
seen by the next collection:

- list replacement must stop retaining the previous element;
- list removal must stop retaining the removed element and the cleared tail;
- map value replacement must stop retaining the previous value;
- map removal/compaction must stop retaining the removed value and cleared
  final record;
- map lookup rebuild must not introduce trace edges; and
- non-structural replacement during iteration must preserve lock count, length,
  order, and backing identity.

Retain aliases to both old and new objects in selected probes to distinguish
“edge removed from container” from “object necessarily dead.” Then drop the
independent alias and verify later reclamation.

## Free-list and stationary-identity hardening

Repeated container growth produces many dead variable-size backings. Exercise
the production sweep and allocator until it must:

- reclaim isolated controls and backings;
- coalesce adjacent dead spans;
- preserve live blocks between free runs;
- rebuild a sorted, non-overlapping free list;
- split reusable runs without losing alignment or header boundaries;
- fall back to the frontier only when no free run fits;
- reuse reclaimed storage for different compatible layout identities; and
- retain every live `sao2_ref` owner/member offset unchanged.

Inspect production GC statistics in native probes: blocks examined, live blocks
retained, dead blocks reclaimed, reclaimed span bytes, and resulting free block
count. Statistics are test evidence, not a source API.

After reuse, old dead references must never be traced or dereferenced by a
valid program. A newly allocated object at a reclaimed offset is a distinct
language object even if its packed carrier later reuses the same bits after the
old object became unreachable; no live comparison can observe both identities.

## Capacity and size arithmetic

Centralize and exhaustively test all growth equations at boundary values:

- zero, one, minimum capacity, and every power-of-two transition;
- list doubling and map entry doubling;
- map slot load-factor multiplication and next-power-of-two selection;
- backing prefix alignment and payload offset;
- record stride multiplication;
- payload plus record bytes;
- heap header/span alignment;
- conversions among `uint64_t`, `uint32_t` packed offsets, `size_t`, and
  language `int64_t` length; and
- arena frontier/free-run addition.

Every multiplication, addition, rounding, and conversion must be checked before
mutation or pointer arithmetic. Impossible requests fail as heap exhaustion
without collecting an otherwise valid heap. Loops which seek a larger capacity
must terminate on overflow rather than wrapping or repeating.

Include zero-sized unit language values, maximum-alignment records, padded
tuples/unions, and map entry records whose key/value alignment differs.

## Failure matrix

Exercise each recoverable allocator result at every source-attributed container
allocation category:

| Runtime result | Stable reason | Source operations |
| --- | --- | --- |
| logical/size exhaustion | `heap arena exhausted` | struct/list/map construction, list append, map insert |
| platform commitment failure | `unable to commit heap storage` | the same allocating operations |
| trace scratch exhaustion | `unable to allocate garbage collector work storage` | the allocation which triggered collection |

Also retain exact non-allocation failures:

- `list index out of range` for read/replacement/removal;
- `map key not found` for lookup and removal;
- `container structurally modified during iteration` for structural mutation;
  and
- existing arithmetic/index failure operations outside containers.

Verify operation code, filename, byte-derived line/column, reason, stderr form,
and nonzero termination. A failure must not be relabeled according to an
internal helper such as backing allocation, hashing, rehash, or collection.

Invalid layout identities, corrupt body sizes, broken prefixes, impossible slot
indices, trace-key/layout mismatch, epoch corruption, lock underflow/overflow,
and invalid free-list structure remain compiler/runtime invariants. Native
probes should make each fail closed rather than become a source panic or memory
access.

## Probe-only fault injection

Add narrowly guarded runtime-probe seams only where existing hooks cannot
reliably select a failure boundary. Suitable probe controls include:

- fail-next platform commit;
- fail-next trace scratch allocation;
- force policy or pressure collection on the next managed allocation;
- reduce arena capacity and allocation threshold at compile time;
- reduce the GC epoch limit; and
- pause/check transaction state between map backing allocations and before
  publication.

All such controls must be excluded from ordinary emitted C unless the existing
probe macro is defined. They must not alter production layout, source behavior,
allocation order, or deterministic output.

Prefer observing production helpers over maintaining a second test allocator.

## Collector scratch failure and recovery

The trace worklist and deduplication table use host metadata. Force failures
while growing each structure and verify:

- collection reports scratch exhaustion rather than sweeping a partial mark;
- no heap block is reclaimed from an incomplete trace;
- temporary host allocations are disposed;
- `sao2_gc_active` and context state are reset;
- allocation returns `SAO2_ARENA_COLLECTION_FAILED` to the source operation;
- the published heap and free list remain valid; and
- a later collection without injection can succeed and reclaim genuinely dead
  blocks.

Trace scratch remains the only container-related host allocation beyond
platform/runtime metadata. It must never become language container storage.

## Epoch rollover

Compile native probes with a small positive epoch limit and repeatedly collect
across rollover. At the boundary:

1. Validate the complete heap before resetting marks.
2. Clear stale mark epochs on every allocated block without changing free
   blocks, headers, layout identities, bodies, or references.
3. Reset the global epoch and begin the next mark at one.
4. Trace all current roots and sweep normally.

Exercise rollover while live roots include recursive container cycles,
interior references, superseded backing candidates, active iteration locks,
and typed map transaction roots. Objects live across rollover remain stationary;
dead objects are reclaimed according to the new mark, not retained by a stale
numeric coincidence.

## Implementation batches

Each batch tightens one class of invariant without expanding the language.

### Batch 1: Descriptor and graph closure

- Add planner tests for mutually recursive legal shapes and illegal by-value
  cycles.
- Harden deterministic descriptor/callback closure and declaration ordering.
- Build native graph fixtures covering all control/backing/value/interior edges.

Gate: every valid recursive type graph plans finitely and emits stable exact
callbacks.

### Batch 2: Exact tracing and initialized-prefix poisoning

- Stress trace-key deduplication, collisions, shared owners, distinct interior
  members, cycles, active unions, and repeated aliases.
- Poison unused capacity, inactive payloads, and lookup metadata.
- Collect during probe-controlled partial initialization.

Gate: only exact initialized language values affect reachability.

### Batch 3: Growth transactions and dead-edge reclamation

- Audit every container allocation safe point and publication order.
- Force collection across list growth and both map transaction allocations.
- Exercise replacement/removal reachability and reclaim superseded backings.

Gate: new storage is rooted until publication, old storage until replacement,
and neither is retained afterward without a real edge.

### Batch 4: Capacity, fragmentation, and reuse

- Exhaustively test checked capacity/body/span arithmetic boundaries.
- Drive repeated growth/compaction through reduced arenas.
- Validate sweep coalescing, free-list ordering/splitting, statistics, reuse,
  and stationary live identities.

Gate: churn cannot overflow arithmetic, corrupt heap walks, or strand reusable
managed storage.

### Batch 5: Failure classification and recovery

- Add minimal probe-only fault controls where required.
- Inject exhaustion, commitment, and trace-scratch failures at each allocating
  container operation.
- Validate invariant failures separately and prove successful collection after
  aborted scratch work.

Gate: every recoverable failure is transactional, exactly attributed, and
leaves production runtime state valid.

### Batch 6: Epoch and sustained-pressure hardening

- Combine recursive graphs, mutation, iteration, repeated policy/pressure
  collection, free-list reuse, and a small epoch limit.
- Run deterministic native stress sequences with explicit expected statistics.
- Retain focused public regressions without taking over Stage 6 integration.

Gate: the runtime remains precise and stationary through sustained churn and
multiple epoch rollovers.

## Verification map

### Planner and backend tests

Cover:

- recursive container/value descriptor closure;
- deterministic managed identities and callback ordering;
- correct trace capability through tuple/union/struct/container combinations;
- exact transaction-frame fields and callback layouts;
- audited allocation/re-resolution/publication ordering;
- checked arithmetic helper emission; and
- absence of probe controls from production output.

### Native runtime probes

Cover:

- every recursive graph edge and cycle shape;
- trace-key collisions and multi-member shared owners;
- poisoned unused capacity and inactive unions;
- collection during construction/growth transactions;
- replacement/removal dropping the correct edges;
- dead control and backing reclamation;
- fragmentation, coalescing, split reuse, and live-reference stationarity;
- impossible capacity/body requests without spurious collection;
- commitment and scratch failure injection;
- recovery after incomplete mark work;
- iteration locks during body allocation; and
- repeated epoch rollover.

### Focused public programs

Use compact source programs to prove observable consequences:

- cyclic struct/container graphs remain usable after heavy allocation;
- nested list/map/tuple/union values survive repeated growth;
- removed and replaced objects become irrelevant while live aliases remain
  valid;
- iteration order and mutation rules survive natural collection pressure; and
- observable output remains independent of private backing replacement and
  free-list reuse.

Keep larger algorithms and full-feature combinations for Stage 6.

### Regression coverage

Retain all Stage 1-4 and milestone 1-10 tests. Generated-C compilation failure,
abnormal native termination, invalid heap statistics, wrong output, unexpected
panic text/location, nondeterministic output, or a silent runtime fallback is a
failure. Native assertions may skip only when no supported C compiler exists.

## Completion checklist

Stage 5 is complete when:

- recursive valid type graphs plan finitely and deterministically;
- exact tracing retains every live root/member/layout path and terminates on
  cycles;
- unused capacity, inactive storage, and lookup metadata never create edges;
- list and map transactions remain safe under collection at every allocation
  boundary;
- replacement, removal, and publication make obsolete edges/backings
  reclaimable;
- repeated churn preserves stationary live references and produces a valid,
  reusable free list;
- all capacity, alignment, body-size, span, offset, and conversion arithmetic is
  checked before mutation;
- exhaustion, commitment, and scratch failures retain exact classification and
  source attribution;
- incomplete collection work never sweeps and the collector can recover;
- epoch rollover preserves live recursive graphs and reclaims dead ones;
- probe-only controls are absent from normal generated C;
- generated C remains byte-for-byte deterministic;
- all Stage 1-4 and earlier walking-skeleton behavior remains unchanged; and
- the Stage 5 test matrix passes externally on Rust 1.90 or newer and available
  supported C compilers.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. Generated artifacts belong only under `build/` or
test temporary directories and must not be committed.
