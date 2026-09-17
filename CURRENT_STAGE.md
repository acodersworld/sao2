# Current Stage: Exact Trace Plans and Callbacks

Status: current.

This document expands Stage 2 of
[Current Milestone: Garbage Collector](CURRENT_MILESTONE.md). It adds the
compiler plan and generated runtime machinery needed to trace a supplied typed
root precisely. It does not discover program roots, sweep unreachable blocks,
reuse mark results for allocation policy, or trigger collection.

Stage 1 provides the handoff: a non-moving heap of validated allocated and free
blocks, stable packed owner offsets, deterministic struct layout descriptors,
an address-ordered free list, a reserved `mark_epoch` field, and a private
reclamation path. Stage 2 may mark allocated headers but must not reclaim them.

## Outcome

The stage establishes this isolated tracing path:

```text
typed synthetic root
        |
        v
enqueue (owner_ptr, member_ptr, exact struct layout)
        |
        v
deduplicate exact trace item
        |
        +------ heap reference ------> mark complete owner allocation
        |                                  |
        +------ scoped reference ----------+
                                           |
                                           v
                              traverse exact member body
                                           |
                          +----------------+----------------+
                          |                                 |
                 inline/by-value fields             referenced fields
                 traverse synchronously                 enqueue
```

Reference edges enter a work queue and therefore cannot recurse through the C
stack. Inline structs, tuples, and active union payloads may be traversed
synchronously because validated physical layouts prohibit recursive by-value
containment. Cyclic object graphs necessarily cross queued reference edges and
terminate through exact-item deduplication.

A completed trace pass identifies the precise set of reachable heap owners by
their `mark_epoch` values. The same pass traverses live scoped values so they
can lead to heap owners, but it never marks or reclaims scoped storage.

## Preserved contracts

Stage 2 must preserve:

- the two-word packed reference ABI and both arena tags;
- stable owner and member offsets for all live values;
- Stage 1 block spans, free-list ordering, allocation, splitting, coalescing,
  frontier trimming, and failure atomicity;
- the heap header's 48-byte private ABI and all fields other than the permitted
  allocated-block `mark_epoch` update;
- heap body bounds based on requested body size rather than payload capacity;
- the independent scoped bump arena and function-owned scoped marks;
- the existing struct construction, projection, copying, equality, calls, and
  return behavior;
- deterministic layout identities derived from `DefinitionId`;
- source failure attribution and the existing allocation result categories;
  and
- byte-for-byte deterministic C generation for identical compiler input.

The physical layout plan remains the authority for C carriers, field offsets,
inline-versus-referenced storage, sizes, and alignments. Trace planning consumes
that plan; it must not recalculate a competing physical layout.

## Stage boundaries

This stage does not add:

- shadow frames, local-root rewriting, global-root discovery, or stack scans;
- a mark-epoch counter, rollover handling, automatic collection, or sweeping;
- a call to private heap reclamation from the tracer;
- allocation retry, collection thresholds, or GC-driven failure messages;
- a source-visible root, free, trace, or forced-collection operation;
- list, map, or dynamically allocated string traversal;
- a write barrier, moving, compaction, finalization, or weak references; or
- frontend, typed-IR, escape-analysis, or language-semantic changes.

Generated trace machinery is inert in ordinary programs during this stage. A
native probe supplies synthetic roots and a nonzero epoch explicitly. Stage 3
will generate shadow frames; Stage 4 will own collection epochs and connect
root discovery, marking, and sweeping.

## Compiler-owned trace plan

### Plan boundary

Build a `TracePlan` after IR validation and physical layout planning and
before rendering begins. The plan is immutable renderer input alongside
`LayoutPlan` and `AllocationPlan`. Failure to construct or validate it must
return no partial C text.

The plan records:

- whether each `TypeId` contains a reachable struct reference;
- whether each struct body contains outgoing struct references;
- every reference-bearing nominal tuple;
- every reference-bearing nominal or anonymous union;
- each struct layout's optional body traversal callback; and
- deterministic callback dependencies and emission order.

Use compiler identities only: `DefinitionId`, `TypeId`, `FieldId`, and
`AggregateId`. Source names, spans, native addresses, allocation sites, and
hash iteration order are not trace identities.

Keep trace planning backend-owned for this milestone. Typed IR already retains
the type of every local and field, and no GC operation belongs in language IR.

### Reference-bearing rules

Centralize one memoized query for whether a value of a type can carry a struct
reference:

- unit and every primitive, including interned strings, do not carry one;
- a nominal struct value always carries one because its C carrier is
  `sao2_ref`;
- a tuple carries one when any field carries one;
- a union carries one when any alternative payload carries one; and
- lists and maps remain an explicit unsupported trace-planning boundary until
  milestone 11.

Struct-body traversal additionally consults `MemberStorage`:

- a referenced struct field contains a `sao2_ref` and creates a queued edge;
- an inline struct field contains the child body and is traversed inline;
- a tuple or union field is traversed according to its value shape; and
- primitive, unit, and string fields require no trace action.

A referenced struct cycle is therefore immediately known to carry a reference;
it must not be mistaken for a recursive by-value query. Memoization must handle
shared tuple/union subgraphs without duplicating plan entries. Existing layout
validation remains responsible for rejecting impossible recursive inline
containment.

### Validation and order

Validate that:

- every planned struct names an existing `StructLayout`;
- every planned field agrees with the layout's `TypeId`, storage class,
  offset, size, alignment, and nested-layout identity;
- every callback dependency has a planned declaration;
- every aggregate callback names the same C aggregate selected by
  `LayoutPlan`;
- primitive-only aggregates do not receive callbacks;
- containers cannot silently appear in a traceable shape;
- struct layout identities are unique and match their existing descriptors;
  and
- plan entries are unique and deterministically ordered.

Emit forward declarations by identity order. Emit callback definitions in
physical dependency order where convenient, but do not rely on definition
order to break referenced cycles.

## Generated traversal ABI

### Trace context and body callback

Forward-declare the runtime context before layout descriptors:

```c
typedef struct sao2_trace_context sao2_trace_context;
typedef void (*sao2_trace_body_fn)(
    sao2_trace_context *context,
    const unsigned char *body
);
```

Extend `sao2_layout_descriptor` with a nullable body callback. A struct body
with no outgoing references uses `NULL`; it can still be marked as a heap
owner without running a callback. A reference-bearing body names its generated
callback.

The callback receives an already validated pointer to the exact struct body.
It must not rediscover the owner, mark a header, inspect native stack memory,
allocate managed storage, or reclaim anything. Its only job is to traverse
inline/by-value children and enqueue referenced struct values through the
trace context.

Emit callback prototypes before descriptor initializers so descriptors can
contain callback addresses while retaining the existing descriptor section.
Callback pointers are private native metadata and never enter SAO2 values.

### Layout registry

Emit a deterministic registry containing every struct layout descriptor,
ordered by layout identity. When there are no structs, emit a standards-
conforming sentinel representation with a logical count of zero rather than a
zero-length C array.

Provide narrow lookup and membership helpers:

- find the root descriptor recorded by a heap header's layout identity; and
- prove an exact descriptor supplied by generated code belongs to the registry.

The registry validates allocation-root metadata. It does not search from an
interior member back to a source-language field and does not replace the exact
descriptor carried by a trace item.

### Generated callback forms

Use identity-only names and pointer-based traversal:

- `sao2_trace_body_def_N` traverses a private struct body;
- `sao2_trace_value_def_N` traverses a reference-bearing nominal tuple or
  union; and
- `sao2_trace_value_ty_N` traverses a reference-bearing anonymous union.

For a struct reference value, generate or render a small call to the common
enqueue helper with `sao2_layout_def_N`; do not recursively invoke the target
body callback directly.

Struct body callbacks cast the validated byte pointer to the matching
`const sao2_body_def_N *` and visit fields in ascending `FieldId` order:

- enqueue referenced struct fields with the child's exact descriptor;
- call a reference-bearing inline child struct's body callback on its embedded
  address;
- call a tuple or union value callback on its field address; and
- emit no statement for non-reference-bearing fields.

Tuple callbacks visit fields in positional order. Union callbacks switch on
the existing one-based runtime tag, ignore tag zero as inactive zero-safe
storage, visit only the selected payload, and reject a tag greater than the
alternative count through the trace context's invalid-state result. Never
inspect inactive union bytes.

Callbacks must check the context's sticky result before performing further
work. They use native pointers only for the duration of the stopped trace pass
and never store a body pointer in a language value or trace key.

## Exact trace work set

### Item identity

A trace item contains:

- the complete packed `sao2_ref`; and
- a pointer to the exact struct layout descriptor.

Its deduplication key is the full tagged `owner_ptr`, unmodified
`member_ptr`, and descriptor layout identity. The owner tag is part of the
key so equal numeric offsets in the heap and scoped arenas remain distinct.

Do not deduplicate by owner alone. Two references with the same owner but
different member offsets or exact layouts may expose different outgoing
edges. Repeated occurrences of the same exact triple are processed once.

The all-zero reference is an empty edge and succeeds without entering the set.
A partial-zero reference is invalid. Reserved owner tags are invalid.

### Context storage

Define `sao2_trace_context` with:

- a nonzero epoch supplied by its caller;
- a sticky result: success, native scratch exhaustion, or invalid state;
- a growable insertion-ordered array of unique trace items;
- the next array index to drain; and
- an open-addressed membership table keyed by the exact triple.

The unique-item array is also the FIFO work queue. This avoids maintaining two
copies of the work set: successful first insertion appends one item, and the
drain cursor advances through insertion order.

Use ordinary C `malloc`, `calloc`, `realloc`, and `free` for collector
scratch. Never allocate queue or set storage from either SAO2 arena. This adds
no Rust dependency and prevents trace bookkeeping from recursively invoking
managed allocation.

Use checked `size_t` arithmetic, power-of-two table capacities, a fixed hash
mix, and a bounded load factor. An empty hash slot can use owner zero because
valid trace items always have a nonzero owner. Grow required storage before
publishing a new set entry. A failed growth leaves all existing items valid,
sets the sticky exhaustion result, and causes subsequent enqueue operations to
be no-ops.

Context initialization and disposal must be safe for a completely zeroed
context and after partial native allocation failure. Disposal releases all
scratch and clears its pointers, sizes, cursor, epoch, and result.

Stage 2 reports scratch exhaustion only through the probe-facing trace result.
Stage 5 will decide how a production collection failure maps to a
source-attributed allocation failure.

### Enqueue and drain

The enqueue operation:

1. returns immediately when the context already has a failure;
2. accepts all-zero as an empty reference;
3. validates nonzero reference shape and registered exact descriptor;
4. checks the exact-key membership table;
5. grows the queue and table transactionally when required;
6. inserts a new key once; and
7. appends the item to the FIFO array.

The drain operation processes appended items until its cursor reaches the
array length or the context fails. Callbacks may append more items while an
earlier item is being processed. No graph edge is followed through C
recursion.

## Processing one item

### Common exact-member validation

Classify the reference from its tagged owner and resolve the exact member using
the item's descriptor. Require:

- nonzero valid owner and member fields;
- a registered descriptor with nonzero valid alignment;
- member address alignment appropriate for the exact body;
- no offset or end arithmetic overflow; and
- exact body end within the selected arena's live range.

For heap references, retain the existing complete-owner bounds check using the
requested root body size. For scoped references, the headerless scoped arena
can validate its live cursor, address, and alignment but cannot independently
reconstruct the source allocation's complete body bounds. Generated projection
and static typing remain the authority for the owner/member relationship; do
not add scoped headers or a side table in this stage.

If `member_ptr` equals the decoded heap owner offset, require the exact layout
identity to equal the header's root layout identity. For an interior member,
the generated static descriptor supplies its type; runtime validation checks
registry membership, alignment, and owner bounds without searching for a
field path.

The existing mutator-facing body resolver terminates on invariant failure.
Factor or add a non-terminating checked resolver for the isolated trace engine
so malformed probe state becomes the context's invalid result. Ordinary
generated operations may retain their terminating invariant wrapper around the
same checked mechanics.

### Heap item

For a heap reference:

1. recover and validate the allocated owner header through the Stage 1 helper;
2. look up the root descriptor named by the header layout identity;
3. require root descriptor size to match the allocation header and the root
   body address to satisfy the descriptor alignment;
4. validate and resolve the exact member with the supplied descriptor;
5. set the complete owner's `mark_epoch` to the context epoch; and
6. invoke the exact descriptor's callback when it is non-null.

Marking an already marked owner is harmless and does not suppress exact-member
traversal. Exact-item deduplication, rather than the owner mark, decides whether
the callback runs.

### Scoped item

For a scoped reference:

1. validate and resolve the exact live member through the scoped arena;
2. do not read or write a heap header and do not set a mark epoch; and
3. invoke the exact descriptor's callback when it is non-null.

This is required even though scoped storage is not collectible: a scoped root
may contain a reference to a heap object. A scoped-to-scoped cycle terminates
through the same exact work-set deduplication.

Any malformed item sets the context's sticky invalid result. Do not call the
global compiler-invariant terminator inside the isolated engine; Stage 4 can
translate an impossible production trace result at the collection boundary.
A failed pass may have marked owners processed before the failure. It must not
roll those marks back, mutate block structure, or reclaim anything. Stage 4
must sweep only after a fully successful trace; a later epoch makes marks from
an abandoned pass harmless.

## Mark-epoch behavior in this stage

Allow allocated heap headers to contain either zero or a nonzero mark epoch.
Free headers must continue to contain zero. New and reused allocated blocks
still initialize their epoch to zero, and reclamation still clears it.

The trace context requires a caller-supplied nonzero epoch. The native probe
uses explicit small epoch values and inspects headers afterward. Stage 2 does
not own a global current epoch, advance epochs, clear old marks, interpret an
old mark as liveness, sweep, or handle rollover.

Update Stage 1 heap validation accordingly without weakening any other header
or free-list invariant.

## Rendering and runtime integration

Render in this dependency order:

1. fixed value types and forward aggregate/body declarations;
2. complete tuple, union, and struct body definitions;
3. trace-context and callback-type forward declarations;
4. generated body-callback prototypes;
5. extended layout descriptors and deterministic registry;
6. existing diagnostics and scalar runtime;
7. Stage 1 arena and block runtime;
8. trace context, work-set, enqueue, and drain runtime;
9. generated struct, tuple, and union callback definitions;
10. existing copy, formatting, generated function, and host-adapter sections.

Small declaration-only adjustments are allowed where C requires them, but
existing runtime behavior and function ordering should otherwise remain
stable. Emit trace sections only when the program has struct layouts; a
program with no GC-manageable type should not gain unusable callbacks or
nonstandard empty arrays.

Ordinary generated functions do not initialize a trace context or call the
trace engine in this stage. Host adapters continue to initialize and release
only the existing arenas.

## Implementation sequence

Implement Stage 2 in this order:

1. Add the reference-bearing type query and immutable `TracePlan`, including
   container rejection, memoization, validation, and deterministic rendering.
2. Forward-declare the trace ABI, extend layout descriptors with nullable body
   callbacks, and emit the layout registry and lookup helpers.
3. Add exact trace item and context types, sticky results, checked scratch
   lifecycle, and a deterministic FIFO item array.
4. Add the open-addressed exact-key membership table, transactional growth,
   and all-zero/partial-zero handling.
5. Implement heap and scoped item validation, root-descriptor checks, exact
   member resolution, heap marking, and drain semantics.
6. Generate forward declarations and callbacks for struct bodies, tuples, and
   unions, keeping every reference edge queue-based.
7. Relax allocated-header validation for nonzero epochs while retaining zero
   epochs for free and newly allocated blocks.
8. Build the native trace probe from production planner, descriptor, callback,
   block-runtime, and trace-runtime sources.
9. Add ordinary generated-C and source-to-native regressions, then remove
   duplicate type-recursion or trace-expression logic.

Every step must keep ordinary compilation deterministic and executable. Do not
temporarily scan all bytes of a body, trace inactive union payloads, or use the
owner mark as the traversal visited set.

## Test plan

### Direct Rust/backend tests

Add focused tests for:

- reference-bearing classification of primitives, structs, nested tuples,
  named and anonymous unions, and recursive referenced structs;
- distinction between inline struct-body traversal and referenced struct
  enqueue operations;
- rejection of list or map shapes at the explicit milestone boundary;
- unique deterministic plan entries and callback declarations;
- descriptor callback selection, including `NULL` for primitive-only bodies;
- registry order and the standards-conforming zero-struct case;
- field traversal in `FieldId` order and tuple traversal in positional order;
- union tag zero, one-based active alternatives, and invalid-tag handling;
- exact-key comparison using tagged owner, unmodified member, and layout;
- queue insertion order and enqueue-once behavior;
- checked queue/table growth and sticky scratch exhaustion;
- allocated nonzero mark epochs versus mandatory zero free-block epochs;
- absence of shadow frames, root scanning, sweeping, reclaim calls, allocation
  retry, and new source failure operations; and
- byte-for-byte deterministic output.

Direct malformed-plan tests should cover missing layouts, duplicate identities,
wrong field storage or nested identity, absent callback dependencies,
primitive-only callbacks, and container leakage. Runtime-state tests should
cover unregistered descriptors, partial-zero references, reserved tags,
reclaimed owners, wrong root descriptors, misaligned members, owner-bound
overflow, invalid union tags, and zero epochs.

### Native trace probe

Build a generated graph using the production block allocator, layouts, and
callbacks. Supply roots directly to a trace context and assert marked owner
offsets without sweeping.

The probe must cover:

- a primitive-only heap object which is marked without a callback;
- a self-cycle and a multi-object cycle;
- duplicate paths to the same exact item;
- two interior members of one owner which lead to different heap children;
- the same numeric owner/member offsets in heap and scoped arenas;
- an unaligned but valid inline member;
- nested inline structs and reference fields;
- tuples containing references directly and through nested tuples;
- named and anonymous unions for every active reference-bearing alternative;
- tag-zero unions whose payload bytes are ignored;
- inactive union bytes containing a plausible reference which is not traced;
- a scoped root and scoped-to-scoped path leading to a heap child;
- a heap reference reached only through an interior scoped member;
- fresh passes with different explicit epochs;
- graph depth large enough to demonstrate queue traversal rather than native
  recursion over reference edges;
- exact work-set growth and collision handling;
- injected native scratch-allocation failure and safe disposal;
- invalid tags, stale heap owners, wrong root descriptors, out-of-bounds
  members, and invalid union discriminants; and
- an unchanged, valid Stage 1 heap walk and free list after every successful or
  failed trace.

The probe must demonstrate the key precision property directly: marking one
interior member retains its complete owner and traces only that member's
reachable edges, while separately rooting a second member of the same owner
adds that member's distinct reachable edges.

Use the existing supported-compiler discovery and argument-list invocation.
Only a genuinely missing supported compiler may skip native assertions.
Compilation failure, assertion failure, abnormal termination, wrong marks, or
unexpected status is a test failure. Both Windows and POSIX branches require
external verification before completion.

### Public pipeline regressions

Retain the full Stage 1 runtime probe and ordinary struct programs. Generated
trace metadata must not alter output, exit status, allocation placement,
scoped restoration, equality, mutation, diagnostics, or host-adapter cleanup.

Exercise source programs whose struct layouts contain:

- no references;
- direct and recursive referenced fields;
- multiple levels of inline fields;
- tuple fields containing references; and
- active and inactive union fields.

These programs need not invoke collection in Stage 2. They prove the added
plans, descriptors, and callbacks compile alongside the unchanged mutator.

## Completion gate

Stage 2 is complete when:

- every supported reference-bearing shape has one validated deterministic
  trace plan and the required generated callbacks;
- heap headers identify registered root layouts while each trace item retains
  its independent exact member layout;
- exact work items are deduplicated by tagged owner, member, and layout;
- reference cycles are queue-driven and terminate without recursive graph
  traversal;
- heap items mark complete owners and traverse exact members;
- scoped items remain unmarked but can reach heap owners;
- tuples, inline structs, and only active union payloads trace precisely;
- zero references and inactive unions are safe;
- malformed runtime state and scratch exhaustion produce a sticky trace result
  without structural heap mutation, and a failed pass is never sweepable;
- no block is reclaimed and no ordinary program triggers tracing;
- Stage 1 allocation/reuse behavior and all language-visible behavior remain
  unchanged;
- direct and native coverage passes on the available platform matrix; and
- no generated artifacts or new dependency are introduced.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must use Rust 1.90 or newer,
report native skips explicitly, and confirm that the working tree contains
only intended source and documentation changes.
