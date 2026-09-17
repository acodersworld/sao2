# Current Milestone: Containers

Status: current.

This document maps milestone 11 of `ROADMAP.md` into implementation stages.
The frontend and typed IR already represent list and map types, literals,
index projections, membership, methods, iteration, and mutation checks. This
milestone replaces the C backend's deliberate container capability boundary
with permanent managed storage and complete public execution.

The milestone must preserve the walking skeleton. Primitive, aggregate,
struct, scoped-allocation, and garbage-collected programs remain executable
while container support advances one coherent runtime slice at a time.

## Fixed architecture

Lists and maps are mutable objects with reference identity. Their language
carrier is the existing 8-byte packed `sao2_ref`; copying, assignment,
parameter passing, tuple/union storage, and return copy this carrier and retain
aliasing. Container equality and inequality compare identity, not contents.

Container allocations are conservatively heap-managed in v0. This is a valid
placement choice because allocation placement is unobservable, and it avoids
putting independently growing shared storage in the function-scoped arena.
Any future scoped-container optimization requires a separate proof and is not
part of this milestone.

Each container has a stable managed control object and one or more replaceable
managed backing allocations. A control object records logical length,
capacity, iteration-lock state, and packed references to its current backing
storage. Growth follows allocate, initialize/copy, then publish: no decoded
body pointer survives the allocation safe point and the old backing remains
reachable until the replacement is published.

Container bodies and backings use the GC heap rather than unconstrained host
`malloc`. The existing 4-GiB arena limit, allocation policy, failure
classification, exact tracing, stationary references, and epoch rollover
therefore apply to user container storage. Host allocation remains limited to
collector scratch and platform/runtime metadata already outside the language
arenas.

Generate deterministic descriptors and typed helpers for each concrete
container type required by the program. Container tracing visits only
initialized elements or live map entries. Lists and maps are roots even when
their element, key, and value types contain no further references; nested
reference-bearing values recursively use the existing exact traversal
machinery.

Lists use contiguous logical index order. Maps use hashed lookup plus an
explicit insertion-order representation; hash-table slot order is never
observable and never defines iteration order. Map keys use the equality and
stable hashing rules already defined for unit, integers, strings, booleans,
and recursively valid immutable tuples.

Iteration locking belongs to the container object, not a particular alias.
Nested iteration is represented by a checked lock count. Structural mutation
through any alias is rejected while the count is nonzero; non-structural value
replacement remains governed by the language design.

Generated operations invoke child processes only through existing argument-
list APIs, add no compiler dependency, and emit deterministic standard C for
the supported 64-bit POSIX and Windows targets.

## Design decisions required before implementation

`DESIGN.md` defines the core semantics but does not yet state every map edge
case needed by the runtime. Before the relevant stage, make explicit decisions
for:

- whether assigning a missing map key inserts it;
- whether replacing an existing value retains that key's insertion position;
- where a removed and later reinserted key appears;
- how duplicate keys in one map literal are resolved and ordered;
- whether `removeKey` on a missing key panics or is a no-op;
- the exact source-attributed reasons for list/map allocation and capacity
  failure; and
- whether replacing an existing map value is permitted during iteration while
  insertion remains forbidden.

Do not infer these rules from a convenient C data structure. Update the design
and corresponding semantic/IR tests as one explicit decision before map
runtime behavior depends on them.

## Stage 1: Managed container foundation

Status: pending.

Define the permanent C carrier, control headers, backing-allocation metadata,
capacity arithmetic, and generated per-type operation descriptors shared by
lists and maps. Descriptors provide the size, alignment, copy, equality, hash
where valid, and trace behavior needed for concrete element/key/value types
without exposing C types to the language.

Extend trace planning and root planning so every list and map value is a
traceable object carrier regardless of whether its contents contain
references. Add container control and backing layouts to the existing layout
registry, including recursive descriptor dependencies. Make the escape
analysis conservative around container retention without confusing “contains
a reference” with “is itself an object reference.”

Provide safe internal managed allocation for a control object followed by its
backing storage. The zero-initialized control must be rooted in canonical frame
storage before a second allocation can collect. Backing growth must keep the
old reference published until a fully initialized replacement is ready.
Failures remain source-attributed and must not publish a partially constructed
language value.

Implement the non-allocating `str.len()` builtin as the first complete builtin
rendering seam, then retain capability rejection for list/map execution until
their operations are enabled by later stages.

Stage 1 is complete when the backend can deterministically plan, render,
allocate, root, and precisely trace probe container controls and backings using
production GC paths, while all existing non-container programs remain
unchanged.

## Stage 2: Complete lists and entry arguments

Status: pending.

Implement list literals, explicitly typed empty lists, length, positive and
negative indexing, indexed replacement, append, removal by index, membership,
identity equality, aliasing, and unbounded geometric growth. Preserve
left-to-right literal evaluation and logical index order. Bounds and allocation
failures use their existing typed IR sites or new explicitly designed literal-
allocation sites.

Element storage must support every v0 storable type: unit, primitives,
interned strings, tuples, unions, structs, lists, and maps. Copy inline values
and packed references according to existing value semantics. Growth and
removal must handle padding without using raw byte comparison as language
equality.

Materialize `main(args [str])` as an ordinary list in original argument order.
Validate ASCII before entering `main`, preserve canonical string equality for
duplicate argument text, root the list during host construction, and release
any host-side argument interning metadata after the generated program and
arena lifecycle are complete.

Keep list iteration behind the capability gate until Stage 4. Stage 2 is
complete when ordinary source programs can construct, pass, return, alias,
mutate, grow, query, and collect lists—including nested and reference-bearing
elements—and can consume command-line arguments.

## Stage 3: Ordered maps

Status: pending.

Resolve and document the outstanding map edge semantics, then implement map
literals, explicitly typed empty maps, lookup, indexed insertion/replacement,
removal by key, length, key membership, identity equality, aliasing, and
unbounded growth.

Use deterministic open-addressed lookup metadata together with explicit
insertion-order entries. Rehashing may change private slot placement but must
not change observable order. Equality and hashing must agree for every valid
key type, including integer extremes, canonical strings, booleans, unit,
nested nominal tuples, and positive/negative floating zero only where floats
occur inside values rather than keys.

Keep partially initialized slots, tombstones, and capacity-only bytes outside
the traced live set. Rehash and compaction use allocate/copy/publish
transactions which remain safe if allocation collects or fails. Updating an
existing key must preserve the design-selected insertion semantics.

Keep map iteration behind the Stage 4 gate. Stage 3 is complete when public
programs can use ordered maps through every non-iteration operation with exact
failure attribution, stable key semantics, GC-safe nested values, and
deterministic generated C.

## Stage 4: Iteration and structural mutation guards

Status: pending.

Render the existing `BeginIteration`, `IterationValue`, `EndIteration`, and
`IterationUnlocked` IR operations. List iteration yields values in index order;
map iteration yields keys in insertion order. The iteration source is
evaluated once and kept rooted for the complete loop.

Implement checked nested lock counts on the shared container object so every
alias observes the same active iterations. Reject append, removal, missing-key
insertion, and any other structural change while locked. Permit or reject
non-structural replacement exactly as recorded in `DESIGN.md`.

Preserve cleanup on fallthrough, `break`, `continue`, and every normal return
from nested loops. Panic still terminates without language-level unwinding.
Counter overflow, underflow, mismatched end operations, and invalid iterator
state are runtime invariants rather than ordinary source outcomes.

Stage 4 is complete when nested and aliased list/map loops preserve their
defined order, every exit balances its lock, and structural mutation reliably
panics at the original source operation without corrupting the container.

## Stage 5: Recursive tracing and growth hardening

Status: pending.

Stress the shared runtime across recursive combinations: lists of lists, maps
of containers, tuples and unions containing containers, containers containing
struct roots and interior references, and cyclic graphs crossing container and
struct boundaries. Confirm exact trace-key behavior and owner retention remain
correct when backing allocations are replaced.

Exercise repeated growth, rehash, removal, tombstone reuse or compaction,
allocation-threshold collection, pressure collection, and epoch rollover.
Collector safe points may occur while constructing literals, appending,
inserting, or growing, but no decoded element, entry, control, or backing
pointer may survive such a point.

Harden arithmetic and failure behavior for capacity growth, byte sizing,
offset representation, arena exhaustion, commit failure, collector scratch
failure, index failure, missing keys, iteration locks, and impossible internal
state. Every recoverable failure is transactional and source-attributed; every
runtime structure remains walkable after failed internal work.

Stage 5 is complete when arbitrary nested and cyclic container graphs remain
precise under sustained mutation and collection, dead backings and containers
are reclaimed, live identities never move, and no correctness property relies
on unused capacity being zero or traced.

## Stage 6: Integration and milestone closure

Status: pending.

Exercise the full public pipeline with readable programs combining lists,
maps, structs, tuples, unions, strings, command-line arguments, calls,
recursion, branches, loops, mutation, membership, indexing, and all supported
return shapes. Include useful algorithms whose working sets grow beyond their
initial capacities and require repeated garbage collections.

Demonstrate reference aliasing, tuple value copying, map insertion order,
negative list indexing, nested iteration, permitted value replacement, and
rejected structural mutation. Confirm container identity equality remains
distinct from structural tuple/key equality.

Retain focused planner, backend, and native probes for details source cannot
observe: descriptor order, physical layouts, hash slots, capacity arithmetic,
root/frame selection, exact tracing, growth publication order, iteration-lock
balance, free-list reuse, failure injection, and epoch rollover. Identical
input must produce byte-for-byte identical C.

External verification uses Rust 1.90 or newer and supported C compilers on
both platform families when available. It reports native skips, covers the
production 4-GiB arena configuration plus reduced stress configurations, and
leaves generated artifacts only under `build/` or test temporary directories.

Stage 6 and milestone 11 are complete when all designed container programs
execute through the public compiler, storage grows without a language-visible
bound other than managed allocation failure, tracing is precise across every
nested shape, iteration semantics are stable, and the implemented language is
capable of unbounded container-based computation.

## Cross-stage verification

Each stage adds tests at the narrowest useful layer:

- semantic and IR tests retain existing type, mutability, map-key, failure-site,
  and cleanup contracts;
- planner tests cover concrete descriptor closure and deterministic ordering;
- backend tests cover carrier/layout selection and generated operation order;
- native probes use production runtime fragments for allocation, hashing,
  tracing, growth, locking, and injected failure facts;
- public end-to-end programs prove observable values, aliases, order, panics,
  arguments, and exit status; and
- regression tests keep every milestone 1-10 program working unchanged.

Native assertions may be skipped only when no supported C compiler is
available. Generated-C compile errors, runtime assertion failures, abnormal
termination, incorrect output, unexpected exit status, or silent capability
fallback are test failures.

## Milestone boundaries

The following remain outside milestone 11:

- printing lists or maps, structural container equality, ordering comparisons,
  sorting, slicing, comprehensions, and iterator values as first-class objects;
- additional collection types such as sets, queues, or user-defined iterators;
- user-selectable hashers, capacity reservation, load factors, or allocation
  placement;
- weak containers, finalizers, concurrent mutation, or thread safety;
- changing ASCII-only strings into a general mutable string type;
- exposing GC, arena, backing-storage, hash-slot, or address details to source;
- FFI, modules, closures, or standard-library APIs beyond the v0 design; and
- diagnostics-wide presentation work belonging to milestone 12.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this milestone. Generated artifacts must not be committed, and
every stage must preserve filenames and byte-oriented source locations for
diagnostics.
