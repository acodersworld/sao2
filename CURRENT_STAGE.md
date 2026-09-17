# Current Stage: Ordered Maps

Status: current.

This document expands Stage 3 of
[Current Milestone: Containers](CURRENT_MILESTONE.md). Stages 1 and 2 established
the packed container carrier, managed control and backing descriptors, exact
tracing, typed value operations, transactional list growth, container identity,
and executable container places. This stage completes every non-iteration map
operation using that architecture.

Map storage combines a dense insertion-order entry backing with a separate
open-addressed lookup backing. The entry backing alone defines observable
order; lookup slots are private acceleration metadata. List behavior remains
unchanged, and `BeginIteration`, `IterationValue`, and `EndIteration` remain
capability-gated until Stage 4.

## Outcome

At the end of this stage, source programs can:

- construct non-empty and explicitly typed empty maps;
- use every valid v0 map-key type with stable equality and hashing;
- store every v0 value representation as a map value;
- read values by key and assign through map-index places;
- insert a missing key or replace an existing value through indexed assignment;
- call `len()` and `removeKey(key)`;
- test key membership with `in`;
- compare maps by object identity;
- pass, return, alias, nest, mutate, grow, and collect maps; and
- retain deterministic insertion order through replacement, removal, growth,
  and rehashing.

The runtime path is:

```text
map control
    |
    +---- ordered-entry backing: [key, value] in insertion order
    |          ^
    |          | entry index
    |          |
    +---- lookup-slot backing: open-addressed entry_index + 1
                    ^
                    |
              stable key hash/equality
```

No language operation observes slot order, capacity, probe distance, backing
identity, or rehash count.

## Preserved contracts

Stage 3 must preserve:

- all syntax, typing, evaluation order, mutability, and map-key restrictions in
  `DESIGN.md` and `GRAMMAR.ebnf`;
- the 8-byte `sao2_ref` carrier and reference-identity equality for maps;
- the Stage 1 48-byte map control layout: length, entry capacity, slot
  capacity, iteration-lock count, ordered-entry reference, and lookup-slot
  reference;
- the common variable-backing prefix and deterministic managed-layout identity
  namespace;
- the rule that no decoded control, entry, slot, key, or value pointer survives
  a managed allocation;
- canonical shadow-frame roots and exact traversal of initialized language
  values only;
- descriptor-based copy, equality, hash, and trace operations without raw-byte
  language equality;
- Stage 2 list behavior, entry arguments, union equality, and source failure
  attribution;
- left-to-right operand lowering and byte-for-byte deterministic generated C;
  and
- unchanged execution for every program accepted before this stage.

Keys remain immutable values. Map values retain their ordinary semantics:
inline tuples and unions copy by value, while structs, lists, maps, and strings
copy their carriers and preserve identity or canonicalization.

## Stage boundaries

This stage does not enable:

- list or map iteration;
- source-visible iterators, entry pairs, keys, values, capacity, reservation,
  load factor, hash values, or rehash controls;
- printing maps or structural map equality;
- floating-point, character, union, struct, list, or map keys;
- scoped map controls or host-allocated entry/slot storage;
- automatic backing shrink after removal; or
- diagnostics-wide rendering work assigned to milestone 12.

Stage 4 will expose insertion order through map iteration. Stage 3 must make
that future behavior inevitable from the storage invariants rather than adding
order later.

## Map semantics to record in `DESIGN.md`

Before enabling public map execution, update `DESIGN.md` and semantic/IR tests
with these explicit decisions:

1. Assigning `table[key] = value` inserts when the key is absent.
2. Assigning an existing key replaces only its value and retains its original
   insertion position.
3. Removing and later reinserting a key places it at the end of insertion
   order.
4. A map literal evaluates every key then value from left to right. The first
   occurrence of an equal key establishes its position; the last occurrence
   supplies its final value.
5. `removeKey` on a missing key panics with `map key not found`.
6. Reading a missing key, including a compound assignment's initial read,
   panics with `map key not found`.
7. Replacing the value of an existing key is non-structural and is permitted
   during iteration. Inserting a missing key and removing a key are structural
   and panic while the shared lock count is nonzero.

These rules are independent of open addressing. A future lookup strategy must
preserve them.

## Failure attribution

Append new stable failure operations without renumbering existing codes:

- `MapAllocation` for non-empty and explicitly typed empty map literals; and
- `MapInsert` for a final indexed assignment which may insert and allocate.

Map aggregate IR gains a required allocation site:

```text
Aggregate::Map {
    ty,
    entries,
    failure: FailureSiteId,
}
```

Final map-index assignment carries a `MapInsert` site. Read projections and
deeper assignment projections retain `MapIndex`, because they require an
existing value. Compound assignment reads first and therefore reports a
missing key at its `MapIndex` site before its final replacement.

Use the existing managed-allocation reasons at `MapAllocation` or `MapInsert`:

- `heap arena exhausted` for logical exhaustion or unrepresentable entry,
  slot, or body capacity;
- `unable to commit heap storage` for platform commitment failure; and
- `unable to allocate garbage collector work storage` for collector scratch
  failure.

Missing lookup and removal use `map key not found` at `MapIndex` and
`MapRemoveKey` respectively. Structural mutation under a nonzero lock uses
`container structurally modified during iteration` at `MapInsert` or
`MapRemoveKey`. Invalid references, descriptors, slot encodings, probe cycles,
prefix counts, capacity relationships, or publication states are compiler or
runtime invariants rather than source panics.

## IR and validation contract

Lowering must preserve the already-defined source evaluation order:

- a map literal evaluates key 0, value 0, key 1, value 1, and so on before its
  aggregate allocation;
- a lookup evaluates and stabilizes the receiver before the key;
- an indexed assignment evaluates according to the existing assignment IR
  order and stabilizes every value needed across a possible insertion
  allocation; and
- a method call evaluates and stabilizes its receiver before its argument.

IR validation must require:

- a map aggregate's `ty` is the exact map type;
- every key and value operand matches that type;
- the aggregate failure site is `MapAllocation` in the containing function;
- map read and nested-place projections use `MapIndex` with an exact key local;
- a final map-index assignment uses `MapInsert`;
- `removeKey` has one exact key argument, its `MapRemoveKey` site, and the
  existing receiver-equivalent `IterationUnlocked` pre-check;
- `MapLen` has no argument or failure site;
- membership operands are an exact key and map type; and
- map equality operands have the same map type.

Textual IR rendering and all place/operand visitors must preserve the access
mode and failure site. Malformed IR must not be able to use an insertion site
for a read, bypass the removal pre-check, or apply map operations to a list.

## Capability boundary

Enable exactly the non-iteration map surface:

- map aggregate construction;
- map-index reads and indexed assignment destinations;
- `MapLen` and `MapRemoveKey`;
- key membership;
- map `==` and `!=` identity comparison;
- map-side removal mutation checks; and
- map carriers in all already-executable storage, call, tuple, union, struct,
  list, and map-value positions.

Keep `BeginIteration`, `IterationValue`, and `EndIteration` rejected for both
lists and maps. Indexed insertion performs its conditional lock check inside
the typed helper because only an absent key makes the operation structural;
an unconditional IR pre-check would incorrectly reject permitted value
replacement during iteration.

Printing remains a separate capability and must continue to reject containers.

## Stable key equality and hashing

Valid map keys are exactly:

- unit;
- `int`;
- interned `str`;
- `bool`; and
- immutable nominal tuples composed recursively only of valid map-key types.

Align frontend diagnostics and backend validation with this complete set,
including unit. No runtime fallback accepts a statically invalid key.

Use the Stage 1 value descriptors for both equality and hashing:

- unit has one equality class and one fixed hash;
- integers hash a stable 64-bit representation through specified unsigned
  mixing, including `INT64_MIN` and `INT64_MAX`;
- booleans use distinct fixed values;
- strings use their stable byte-derived hash and compare canonical/value-equal;
  and
- tuples combine member hashes in declaration order and compare members
  structurally.

Hashing must not depend on native addresses, padding, host endianness, C signed
overflow, randomized seeds, discovery order, or backing location. Equal keys
must always have equal hashes. Unequal keys may collide and must still be
distinguished by descriptor equality. Floating-point zero normalization is
irrelevant to keys because floats are not valid key components; floats stored
as values retain their existing equality behavior.

## Physical representation

### Ordered entries

The ordered-entry backing is a dense prefix of typed records:

```text
record 0: key, value   <- oldest live insertion
record 1: key, value
...
record length - 1     <- newest live insertion
```

The backing prefix's initialized count equals the number of complete live
records and therefore equals map length whenever the backing is published.
Only this prefix is traced. Unused capacity contains no language values.

Replacing a value changes its record in place. Removing an entry shifts later
records left and clears the vacated final record. Reinsertion appends a new
record. Growth copies records in this same order.

### Lookup slots

The lookup backing is an array of `uint64_t` slots with deterministic linear
probing:

- zero means empty;
- a nonzero slot stores `entry_index + 1`;
- initial probe position is `hash & (slot_capacity - 1)`; and
- collisions advance by one with power-of-two wraparound.

No persistent tombstone state is needed. Removal compacts the ordered entries
and rebuilds the complete lookup table in place from the remaining entries.
The lookup prefix's initialized count records occupied slots and equals map
length; the entire capacity is validated as metadata but never traced as
language values.

Every occupied slot must identify one distinct entry below length, every live
entry must be named by exactly one slot, and probing that entry's key must reach
that slot before an empty slot. Duplicate, missing, or out-of-range indices are
runtime invariants.

## Capacity policy

Use deterministic private capacities:

- an empty map has zero entry and slot capacity and zero backing references;
- the first entry backing has capacity four;
- entry capacity is a power of two and doubles geometrically;
- the first slot backing has capacity eight;
- slot capacity is a power of two and always keeps load at or below one half;
- literal capacities are computed from the number of source entries as a safe
  upper bound before duplicate keys are resolved; and
- removal does not shrink either backing.

Check required counts, doubling, `2 * required`, alignment, stride, body size,
`size_t`, `INT64_MAX`, and packed 32-bit arena offsets before mutation or
pointer arithmetic. A full probe cycle is impossible in a valid map at the
chosen load factor and is an invariant if encountered.

## Typed map helper surface

Generate deterministic helpers for each concrete map type:

- checked control and dual-backing resolution;
- stable key hash and equality dispatch;
- lookup returning found/not-found plus entry and slot positions;
- entry and slot capacity calculation;
- slot insertion and complete table rebuild;
- literal construction;
- value load and existing-value store;
- insert-or-replace indexed assignment;
- length;
- key membership;
- removal and order-preserving compaction; and
- identity equality through the packed carrier.

Each resolver validates:

- a nonzero carrier with the exact map-control descriptor;
- `length <= entry_capacity` and the one-half slot load bound;
- zero capacities agree with zero backing references;
- nonzero references use the exact ordered-entry and lookup-slot layouts;
- backing-prefix capacities agree with the control;
- entry initialized count and slot occupied count equal length;
- every computed payload and record lies inside its validated body; and
- the slot-to-entry bijection and probe reachability hold where full validation
  is required.

Hot lookup need not perform an avoidable quadratic whole-table audit on every
access. Centralize structural validation at publication and tracing boundaries,
while lookup still checks every slot value it consumes and terminates after at
most `slot_capacity` probes.

## Temporary roots for dual allocation

Map construction and growth may allocate two independent managed backings. The
first new backing must survive collection triggered by allocation of the
second; a native C local is not a root.

Generate a typed transient map-transaction shadow frame containing new entry
and slot carriers, with a callback using their exact managed layouts. Link and
zero it before the first backing allocation, write allocation results directly
to its fields, and unlink it only after both references have been published or
the helper returns without them. The receiver or destination control remains in
the caller's canonical rooted storage.

Do not add conservative C-stack scanning, a global untyped root, or early
publication into an observable existing control. No decoded pointer survives
either allocation.

## Literal construction

Render a map literal as one transaction:

1. Stabilize all key/value operands in source order before allocation.
2. Zero the canonical destination carrier.
3. Allocate the fixed map control directly into that rooted destination.
4. For a non-empty literal, link the typed transaction frame.
5. Compute entry and slot capacities from the source entry count.
6. Allocate both backings into transaction-frame fields, re-resolving after
   each allocation.
7. Process stabilized pairs in source order. Insert a new equal class at the
   end, or replace the existing record's value without moving it.
8. Advance initialized/occupied counts only after complete records and slots
   are written.
9. Publish both backing references and capacities into the control, then
   publish final unique length last.
10. Clear and unlink the transaction frame.

An empty literal allocates only its stable control. A failure before final
publication terminates at `MapAllocation`; no partially constructed map becomes
a language value. Hashing, equality, and descriptor copying do not allocate.

## Lookup, membership, and length

Lookup computes the key hash once, follows the deterministic probe sequence,
and compares only entries referenced by occupied slots. A match returns the
entry index. An empty slot proves absence. Reads copy the value immediately
through its descriptor and panic on absence.

Membership uses the same lookup and returns the found flag without accessing
or comparing the value. `len()` returns validated logical length as `int64_t`.
Neither operation allocates.

Map `==` and `!=` compare the complete packed `sao2_ref`. They do not resolve
controls or compare keys, values, lengths, order, capacities, or slots.

## Indexed assignment

A final `table[key] = value` is insert-or-replace:

- if the key exists, copy the stabilized value into its existing record and
  preserve every structural field and insertion position;
- if the key is absent and the lock count is nonzero, panic at `MapInsert`
  without mutation; and
- if the key is absent and capacity is available, append the entry, install its
  slot, advance backing counts, and publish length last.

When either backing must grow, prepare a full replacement pair under the typed
transaction frame:

1. Compute checked target capacities.
2. Allocate both new backings before retaining decoded pointers.
3. Re-resolve the control and old backings.
4. Copy old entries in insertion order and append the new key/value.
5. Build new slots exclusively from the new ordered entries.
6. Publish both references and capacities, then publish length last.
7. Clear and unlink the transaction frame.

The old pair remains reachable from the control until publication and can be
reclaimed later. Any allocation failure leaves the original map unchanged and
panics at `MapInsert`. Replacing an existing value never allocates and remains
permitted under an iteration lock.

Deeper places such as `table[key].field` require an existing key and use
`MapIndex`; they never synthesize a value. Compound assignment likewise reads
the old value first, so it cannot insert a missing key.

## Removal and compaction

`removeKey` performs the existing shared-lock pre-check, then looks up the key.
Absence panics before mutation. On success:

1. Shift later ordered entries left through descriptor copies.
2. Clear the vacated final key/value record.
3. Reduce the entry initialized count.
4. Zero all lookup slots and rebuild them from the compacted entry prefix.
5. Set the lookup occupied count to the shorter length.
6. Publish control length last.

Removal does not allocate or shrink, so no safe point occurs during temporary
inconsistency. Reinserting the removed key later appends it at the end. Padding
and inactive union storage are cleared for zero safety but never compared or
hashed as language data.

## Tracing and collection invariants

The map control callback traces both published backings. The ordered-entry
callback traces exactly its initialized prefix, recursively visiting key and
value descriptors. The lookup backing has no trace callback because it contains
only integer metadata.

Stage 3 must demonstrate that:

- unused entry capacity and all slot metadata retain no language objects;
- replacement updates reachability without changing identity or order;
- removal stops retaining the removed key and value;
- new backings remain rooted between the two allocation safe points;
- old backings remain reachable until the replacement pair is ready;
- failed construction or growth does not publish either partial backing;
- nested maps, maps in values, and values containing lists, structs, tuples,
  unions, or interior references trace exactly;
- aliases share one control and observe all mutations; and
- dead controls and superseded entry/slot backings are reclaimed independently.

Native probes must force collection between the two backing allocations and at
each policy/pressure boundary; ordinary source cannot select those safe points.

## Implementation batches

Each batch leaves a coherent accepted subset and keeps iteration explicitly
rejected.

### Batch 1: Semantic decisions and IR attribution

- Record the seven map edge rules in `DESIGN.md`.
- Add `MapAllocation`, `MapInsert`, aggregate failure attribution, and
  access-mode-aware map projections.
- Align key validation and diagnostics, including unit and nested tuples.
- Extend textual IR, visitors, validation, and stable operation codes.

Gate: every map operation has unambiguous semantics and exact failure
attribution before generated behavior is enabled.

### Batch 2: Hashing, probing, and validation

- Finalize stable hashes for every valid key shape.
- Generate typed lookup, slot insertion, table rebuild, and corruption checks.
- Lock slot encoding, load factor, capacity arithmetic, and deterministic probe
  behavior with planner and native tests.

Gate: equal keys share hashes, collisions remain correct, and lookup metadata
is deterministic and non-traceable.

### Batch 3: Construction, length, membership, and identity

- Add the typed transaction shadow frame and dual-backing construction.
- Enable empty/non-empty literals and duplicate-key resolution.
- Enable `len`, membership, identity equality, aliasing, passing, and return.

Gate: fixed-capacity maps are ordinary managed objects with stable insertion
order, exact roots, and source-attributed literal failure.

### Batch 4: Indexed reads and value replacement

- Render map-index reads and deeper place projections.
- Enable replacement of existing values without order or structural changes.
- Cover missing reads, compound assignment, padding-bearing values, aliases,
  and exact panic sites.

Gate: existing-key access is complete and replacement is demonstrably
non-structural.

### Batch 5: Insertion, growth, removal, and compaction

- Enable missing-key indexed insertion with conditional lock checking.
- Implement transactional dual-backing growth and rehash.
- Implement removal, dense order compaction, lookup rebuild, and reinsertion
  order.

Gate: maps grow without a language-visible bound, failures leave the old map
unchanged, and every mutation preserves the chosen order rules.

### Batch 6: GC and integration hardening

- Stress nested/reference-bearing values, collisions, duplicates, repeated
  growth/removal/reinsertion, pressure collection, and epoch rollover.
- Inject failures before, between, and after backing allocations.
- Confirm deterministic C, list regressions, and explicit iteration rejection.

Gate: all non-iteration map programs remain exact under mutation, collection,
and failure, leaving only iteration for Stage 4.

## Verification map

### Design, semantic, and lowering tests

Cover:

- every valid and invalid key type, including unit and recursive tuples;
- mutability requirements for indexed assignment and removal;
- insert-versus-replace classification;
- literal key/value evaluation order and duplicate semantics;
- removal-missing and lookup-missing behavior;
- compound assignment requiring an existing key;
- map identity and membership typing; and
- stable `MapAllocation`, `MapInsert`, `MapIndex`, and `MapRemoveKey` sites.

### IR and planner tests

Cover:

- aggregate and projection access-mode validation;
- required removal pre-checks and conditional insertion locking;
- concrete map descriptor closure through all storage shapes;
- key equality/hash and value copy/trace capabilities;
- entry/slot layout identity and deterministic ordering; and
- rejection of all iteration operations.

### Generated-C tests

Assert meaningful operation order:

- destination rooting before literal backing allocation;
- transaction-frame linking before the first of two allocations;
- re-resolution after each allocation;
- entry copying before slot construction;
- both replacement backings ready before control publication;
- logical length published last;
- existing replacement contains no allocation or order mutation;
- removal clears the final record before publishing shorter length; and
- no decoded pointer crosses a managed allocation.

### Native runtime probes

Use production runtime fragments for:

- every valid key hash, integer extremes, canonical strings, unit, booleans,
  nested tuples, and deliberate collision chains;
- empty, minimum, repeated-growth, and maximum representable capacities;
- slot wraparound, absence termination, and entry-slot bijection checks;
- duplicate literals and value replacement;
- removal at front, middle, and end followed by rebuild and reinsertion;
- all v0 physical value representations;
- exact tracing before and after replacement, removal, and growth;
- collection between entry and slot allocation;
- old-pair reclamation after publication;
- injected lock counts and allocator failure classes; and
- epoch rollover with nested maps live.

### Public end-to-end programs

Cover observable behavior with programs that use:

- empty and non-empty literals;
- duplicate equal keys with first position and last value;
- lookup, missing lookup, membership, length, and identity;
- insertion, replacement, removal, missing removal, and reinsertion;
- collisions which do not alter source behavior;
- enough insertions for repeated growth and rehash;
- aliases, parameters, results, structs, tuples, unions, lists, nested maps,
  and reference-bearing values;
- command-line strings as keys and values; and
- collection pressure while live maps retain nested objects.

Map and list iteration programs remain capability errors until Stage 4. Native
assertions may skip only when no supported C compiler is available.

## Completion checklist

Stage 3 is complete when:

- the map edge rules are authoritative in `DESIGN.md` and covered by tests;
- every enabled map operation uses the Stage 1 managed control and dual
  backings;
- valid keys have stable equality-compatible hashes and collision-safe lookup;
- insertion order survives replacement, removal, reinsertion, growth, and
  rehash exactly as designed;
- construction and growth root both transient backings and publish
  transactionally;
- lookup, assignment, removal, length, membership, identity, aliasing, passing,
  return, and nesting work for all valid type shapes;
- maps grow without a language-visible limit before classified managed
  allocation failure;
- tracing visits exactly live entries and reclaims dead controls and backings;
- lists and all milestone 1-10 programs remain unchanged;
- iteration remains an explicit capability error;
- generated C is byte-for-byte deterministic; and
- the Stage 3 test matrix passes externally on Rust 1.90 or newer and available
  supported C compilers.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. Generated artifacts belong only under `build/` or
test temporary directories and must not be committed.
