# Current Stage: Managed Container Foundation

Status: current.

This document expands Stage 1 of
[Current Milestone: Containers](CURRENT_MILESTONE.md). It establishes the
permanent C carrier, layout planning, value-operation descriptors, precise
roots, variable managed backing layouts, and allocation protocol shared by
lists and maps. It also enables `str.len()` as the first rendered builtin.

List and map execution otherwise remains deliberately gated. Stage 2 enables
lists and entry arguments; Stage 3 enables maps. This stage makes those later
operations an extension of one validated managed-storage system rather than
two unrelated runtimes.

## Outcome

The backend gains a complete planning and runtime path for container storage:

```text
IR types used by program
          |
          v
deterministic container/type closure
          |
          +---- value descriptors
          |       size / alignment / copy / equality / hash / trace
          |
          +---- managed layout descriptors
                  fixed control bodies
                  variable backing bodies
          |
          v
8-byte container carrier in values and canonical shadow slots
          |
          v
managed control allocation ---- rooted before another allocation
          |
          v
managed backing allocation ---- initialize/copy before publication
          |
          v
control callback -> backing callback -> initialized values only
```

Native probes exercise this path with both scalar and reference-bearing
payloads through the production allocator, collector, descriptor registry,
trace queue, sweep, policy, and rollover code. No source list or map operation
is enabled merely to make the probes convenient.

## Preserved contracts

Stage 1 must preserve:

- all `DESIGN.md` and `GRAMMAR.ebnf` container syntax, typing, mutability, and
  lowering already implemented;
- the 8-byte packed `sao2_ref` ABI, arena tags, zero sentinel, stable owner
  offsets, and exact member offsets;
- the 48-byte heap header, deterministic first fit, transactional allocation,
  free-list rebuild, and stationary live objects;
- precise `(owner_ptr, member_ptr, layout)` trace deduplication and the rule
  that owner marking does not broaden exact-member traversal;
- canonical typed shadow-frame storage, zero-safe inactive values, ordinary C
  calls, and allocation as the only collection safe point;
- synchronous policy/pressure collection, classified failure, and safe epoch
  rollover;
- struct physical layouts, escape decisions, scoped lifetime, source failure
  sites, and deterministic generated C; and
- dependency-free compilation and existing platform lifecycle behavior.

Container support must not change the representation of structs, tuples,
unions, primitives, or strings. A container carrier is a packed managed object
reference; decoded control, backing, element, and entry pointers are temporary
runtime implementation values only.

## Stage boundaries

This stage does not enable:

- list or map literals, indexing, assignment, membership, mutation methods,
  length methods, equality operations, or iteration in generated source code;
- `main(args [str])` materialization or access;
- map lookup, hashing tables, insertion-order behavior, or the unresolved map
  edge semantics listed in the milestone document;
- list growth policy or public capacity behavior;
- source-visible container allocation, reservation, capacity, layout, or GC
  controls;
- scoped container placement, host-allocated element storage, or native
  pointers in a language value; or
- printing containers, structural container equality, first-class iterators,
  or any milestone 12 diagnostics redesign.

`str.len()` is the sole newly executable source operation. It is
non-allocating and does not depend on container storage.

## Separate the type questions

The current compiler uses related “carries a reference” queries for several
purposes. Containers make their distinctions observable to compiler
correctness. Introduce clearly named, separately validated queries:

1. **Reference carrier:** whether the value itself is an object reference.
   Structs, lists, and maps are reference carriers regardless of their
   contents.
2. **Contains traceable references:** whether canonical storage of the type
   must be visited by a frame or aggregate callback. Every reference carrier
   is true; tuples and unions recurse through active stored values.
3. **Container contents retain references:** whether storing the type in a
   container can retain a managed object reachable from elsewhere. This
   recurses through tuples and unions and is distinct from the container's own
   object identity.
4. **Map-key hashable:** whether language equality and stable hashing exist for
   the type under the map-key rules.
5. **Value-operation capability:** which generated copy, equality, hash, and
   trace callbacks are valid for a concrete stored type.

Do not use “the element type has references” to decide whether a list carrier
itself is a root. A `[int]` local is still a managed object reference. Do not
make an unrelated struct allocation escape merely because a primitive-only
container type exists in the same function; retention fallback should follow
operations which can actually retain reference-bearing values.

## Backend planning artifacts

### Container closure

Add a backend-owned deterministic container plan derived after IR validation
and before rendering. Its roots are every concrete `Type::List` and
`Type::Map` reachable from:

- function parameters, results, and locals;
- struct fields;
- tuple fields and union alternatives;
- aggregate, builtin, projection, iteration, and check operands; and
- recursively stored element, key, and value types.

Use IR `TypeId` only as an input identity. Sort and deduplicate all planned
container types and callback dependencies so discovery order cannot affect
generated C. Validate that every planned type still has the expected IR shape
and that every referenced value/layout descriptor is in the same closed plan.

The plan records, per concrete type:

- its container kind;
- control-layout key;
- backing-layout roles;
- element or key/value type identities;
- physical value descriptors required by those roles;
- outgoing descriptor dependencies; and
- the generated helper/callback capabilities required now or by later stages.

Map entry and hash-slot behavior stays gated, but the plan must have stable
roles for a map control, ordered-entry backing, and lookup-slot backing so
Stage 3 extends rather than replaces the descriptor model.

### Managed layout identity namespace

Replace the assumption that every heap layout identity is exactly a struct
definition index with one deterministic managed-layout namespace.

Preserve existing struct identities where practical: reserve identities below
`program.definitions.len()` for definition-indexed struct layouts, then assign
container control and backing identities in sorted `(TypeId, role)` order.
Use checked conversion and addition; duplicate, overflowed, missing, or
unordered identities are backend invariants.

The registry remains strictly ordered by numeric identity and contains every
layout which may appear in a heap header. A layout identity is private runtime
metadata, not a source-visible type identifier or stable cross-program ABI.

### Physical carrier planning

Treat both `Type::List` and `Type::Map` as `sao2_ref` in:

- ordinary locals, parameters, and return values;
- tuple and union storage;
- struct fields regardless of the frontend's non-struct storage spelling;
- shadow-frame fields; and
- generated value descriptors.

Their planned size is 8 bytes and their alignment is `_Alignof(sao2_ref)`,
currently 4 bytes. Retain static assertions tying the generated C carrier to
the Rust plan. Zero-initialized container storage is the internal null
reference and contributes no trace edge.

## Runtime body layouts

### Stable controls

Use a fixed-size managed control body for each concrete container type. The
physical C structs may share common prefixes, but their heap descriptors remain
type-specific so callbacks know the exact backing descriptors.

The list control reserves fields for:

- logical length;
- capacity;
- checked iteration-lock count; and
- packed reference to the element backing.

The map control reserves fields for:

- logical length;
- ordered-entry capacity;
- lookup-slot capacity;
- checked iteration-lock count;
- packed reference to the ordered-entry backing; and
- packed reference to the lookup-slot backing.

All fields are zero-safe. An empty container may have zero length/capacity and
zero backing references. Stage 1 validates physical sizes, alignments, offsets,
reserved-zero state, and agreement between a control descriptor and its
concrete type; later stages define growth and map behavior.

### Variable backings

Use a common fixed backing prefix containing at least initialized count and
capacity, followed by aligned fixed-stride storage selected by the concrete
backing descriptor. For a list the stored records are element values. For a
map the ordered-entry role will store aligned key/value records; the lookup-
slot role contains no language references.

The exact body-size equation is planned and checked:

```text
payload_offset = align_up(sizeof(backing_prefix), record_alignment)
body_size      = payload_offset + capacity * record_stride
initialized   <= capacity
```

Every multiplication, alignment, addition, `size_t` conversion, 32-bit arena
offset bound, and body-size comparison is checked before pointer arithmetic or
mutation. Unused capacity is not a language value and is never traced.

Stage 1 may probe ordered map entry records, but it must not settle deletion,
duplicate-key, reinsertion, or lookup-slot policy ahead of the explicit Stage
3 design decisions.

## Generalized managed layout descriptors

The current descriptor assumes `layout.size == header.body_size`. Preserve
that strict rule for fixed struct and control bodies while adding an explicit
variable-backing form.

A managed layout descriptor must state enough to validate a body without
trusting its contents:

- stable identity and layout kind;
- fixed size or variable minimum/payload offset;
- required body alignment;
- record stride and alignment for variable layouts;
- existing inline-field metadata for fixed aggregate containment; and
- an optional trace callback.

Centralize body validation in one runtime helper. For fixed layouts it requires
exact size equality. For variable layouts it requires the prefix, aligned
payload offset, exact stride divisibility, representable derived capacity, and
agreement with the stored backing capacity.

Change the trace-body callback boundary to receive the validated body size (or
an equivalently validated backing view) in addition to the body pointer. Fixed
struct callbacks may ignore it after asserting their exact layout. Variable
callbacks derive bounds only through the descriptor helper and then validate
`initialized <= capacity` before visiting records.

Variable backing layouts are root-only internal allocations. Reject interior
member references into a variable layout: no SAO2 value can identify a backing
slot directly. Existing fixed inline-struct containment and exact interior
references retain their current recursive validation.

Update `sao2_trace_resolve`, `sao2_resolve_body`, heap validation, allocation
helpers, and native probes through the centralized rule. Do not weaken fixed-
layout size, alignment, registry, owner, or member checks to accommodate
backings.

## Generated value-operation descriptors

Add one deterministic value descriptor for each concrete type needed in a
container record. It records:

- physical size and alignment;
- zero-safe copy/initialization behavior;
- optional language equality callback;
- optional stable hash callback for valid key types; and
- optional inline-value trace callback.

Generate only capabilities defined by the language and required by the closed
plan. A missing required capability is a backend invariant; an invalid source
key type remains a frontend diagnostic.

Callbacks follow existing value semantics:

- unit and primitives copy by value;
- strings copy their canonical interned pointer;
- structs, lists, and maps copy their packed reference and compare identity;
- tuples copy and compare their members recursively;
- unions copy their tag and active payload and trace only that payload; and
- no equality or hashing callback compares padding or inactive union bytes.

Map-key hashing exists only for unit, integers, strings, booleans, and valid
immutable tuples composed recursively from them. Use the established string
hash and tuple hash-combine helpers. Equality and hashing must agree, be
deterministic across identical executions, and avoid C object addresses.

Stage 1 directly probes descriptor capabilities but does not yet dispatch
public list membership or map lookup through them.

## Precise tracing and roots

### Type and root plans

Mark every list and map type as containing a traceable reference. Root planning
therefore selects every container-bearing local, parameter, return temporary,
tuple, and union just as it selects struct-bearing values.

Extend root-plan validation so a selected container has a concrete control
layout and a trace expression. Extend aggregate callbacks so nested container
carriers enqueue their type-specific control descriptor. A zero carrier is
ignored by the existing trace queue.

Generalize runtime-section gating from “a struct layout exists” to “a managed
layout or managed allocation exists.” A program whose only managed values are
containers must be able to emit the arena, trace, shadow-frame, collection,
and container descriptor sections without a dummy struct definition.

### Control and backing callbacks

A concrete control callback:

1. validates the fixed body and zero-safe counters;
2. validates nonzero backing references against the expected backing roles;
3. enqueues each published backing using its exact descriptor; and
4. never scans unused or reserved bytes.

A reference-bearing backing callback:

1. validates its variable body and prefix;
2. visits exactly `initialized` logical records in ascending order;
3. invokes the planned value trace callback for each initialized element, key,
   or value; and
4. stops through the existing sticky trace result on any invalid state or
   scratch failure.

Scalar-only backing and lookup-slot layouts use a null trace callback but
remain registered and markable. Enqueueing their packed reference still marks
the complete backing owner live.

The backing mark is separate from the control mark. Replaced backings become
unreachable only after the control publishes a new reference and no other
internal root retains the old one.

## Escape-analysis handoff

Container allocations themselves are always heap-managed and do not become
scoped entries in `AllocationPlan`. That plan continues to classify exact
struct-construction coordinates.

Update escape analysis so:

- a list or map parameter/return is recognized as an object reference even for
  scalar-only contents;
- storing a struct reference or interior reference in a container forces its
  complete owner to heap lifetime;
- list/map literals and mutating operations conservatively model retention
  until a more precise operation model is proven;
- nested tuple/union values propagate retained origins;
- reference-bearing container operations cannot leave a struct allocation
  incorrectly scoped; and
- primitive-only container use does not automatically poison unrelated struct
  allocation decisions.

Stage 1 should establish and validate these queries even though public
container aggregate/mutation emission remains gated. Direct IR/escape tests
must prove safe over-classification where precision is deferred and prohibit
under-classification.

## Managed allocation protocol

Add private allocation helpers which accept a registered managed layout and a
checked body size, call the existing managed heap allocator, validate the
returned owner/layout pair, and return a packed reference plus a temporary
decoded body pointer. They return classified arena results; source failure
translation remains with the eventual list/map operation and its IR failure
site.

Do not enable a source container literal before it has an explicit allocation
failure site and stable reason. Native probes may inspect private results
directly.

The construction protocol for a container with backing is:

1. allocate a zeroed fixed control object;
2. copy its packed reference into a canonical shadow-frame destination;
3. only then allocate a backing, allowing collection to see the control;
4. initialize the new backing prefix and logical records without another
   managed allocation;
5. re-resolve the control body after allocation;
6. publish the backing reference and matching counters last; and
7. expose the already-rooted container result to subsequent operations.

The growth protocol is:

1. validate the rooted control and current published backing;
2. calculate the complete replacement size before mutation;
3. allocate the replacement, which may collect;
4. re-resolve old control/backing references after that safe point;
5. initialize/copy the replacement with no intervening managed allocation;
6. re-resolve the control and atomically publish the new backing/counters; and
7. leave the old backing reachable until publication and collectible after it.

Never retain a decoded pointer, `sao2_heap_header *`, or native array address
across step 3. Failure before publication leaves the old control and backing
unchanged. Stage 1 proves these protocols in the native harness; Stages 2 and 3
attach public operations to them.

## `str.len()` and capability gating

Enable only `BuiltinMethod::StrLen` in the C capability validator and renderer.
Return the immutable interned string's byte length as SAO2 `int`, with a runtime
invariant if the stored length cannot be represented by `int64_t`. Embedded
NUL bytes count as ordinary bytes; no C `strlen` call is permitted.

Continue to reject with precise backend context:

- list and map aggregate construction;
- list/map builtins and index projections;
- container membership and identity operations;
- iteration operations and iteration-lock checks;
- executable `main(args [str])` access; and
- any path whose planned descriptor/helper closure is incomplete.

It is acceptable for container types to have a physical C carrier in otherwise
non-executed signatures, fields, tuples, or unions once their layouts and trace
plans validate. Capability rejection must occur before rendering partial C for
an unsupported operation.

## Rendering order and lifecycle

Render sections in dependency order:

1. scalar carriers, packed reference, aggregate forward declarations, and
   fixed container-control declarations;
2. aggregate/struct/container physical bodies and static assertions;
3. trace declarations, value-operation declarations, and managed-layout
   descriptors in identity order;
4. arena and managed allocation runtime;
5. exact trace runtime plus aggregate, struct, control, and backing callbacks;
6. typed shadow frames and frame callbacks;
7. collection, rollover, and container allocation helpers;
8. scalar/string builtins including `str.len()`;
9. generated functions and host adapter.

Initialization and release continue to own one heap arena and one scoped arena.
Container descriptors are static generated metadata and need no release.
Before arena release the shadow chain is empty and collection inactive; no
container-specific destructor or finalizer runs.

Programs with no managed layouts retain lean deterministic output. Programs
with structs but no containers must remain byte-for-byte unchanged unless a
necessary generalized descriptor signature makes a deliberate, tested change.

## Implementation sequence

Implement Stage 1 in this order:

1. Split the type queries and add direct tests for reference-carrier, trace,
   retention, hashability, and callback-capability distinctions.
2. Extend physical carrier planning and C type rendering for list/map
   `sao2_ref` values while preserving operation capability gates.
3. Add deterministic container closure, managed layout keys/identities, and
   value-operation descriptor planning with full validation.
4. Generalize fixed/variable managed layout descriptors, body validation, and
   trace callback signatures without changing fixed struct behavior.
5. Generate container control/backing declarations, descriptors, static
   assertions, value helpers, and exact trace callbacks.
6. Extend root planning, shadow callbacks, runtime gating, and escape-analysis
   retention for all container carriers.
7. Add private managed control/backing allocation helpers and probe the rooted
   construction and allocate/copy/publish growth protocols.
8. Render `str.len()`, retain all list/map operation rejections, and complete
   deterministic and regression audits.

Every intermediate step must compile ordinary non-container programs through
the established pipeline. Do not add a temporary host-heap container or a
source-visible probing intrinsic.

## Test plan

### Planner and direct Rust tests

Add focused assertions for:

- list/map carriers being 8-byte packed references independent of contents;
- `[int]` and `{int: bool}` being roots despite scalar-only contents;
- recursive tuple/union/container trace closure and cycle-safe planning;
- deterministic sorted container, value, and layout plans;
- stable noncolliding struct/control/backing identities;
- checked identity, size, stride, alignment, capacity, and offset overflow;
- exact fixed control and variable backing body equations;
- value descriptor capability for every primitive and aggregate shape;
- hash capability only for valid map keys;
- no padding/inactive-union dependence in equality or hashing;
- root selection for direct and nested container locals;
- a trace expression for every selected root;
- safe escape classification when structs flow into container operations;
- primitive-only containers not unnecessarily classifying unrelated structs;
- collection/runtime sections emitted without a dummy struct;
- `str.len()` accepted and all list/map execution still rejected with original
  function/block/operation context; and
- identical IR emitting byte-for-byte identical C.

Malformed plans should cover missing roles, duplicate identities, wrong type
shape, unregistered dependencies, invalid callback capability, recursive
by-value layout, zero alignment/stride, and overflowed variable bodies.

### Native managed-layout probe

Build one probe from the production layout, arena, trace, shadow, collection,
and container foundation fragments. Use reduced capacity/epoch settings only
through existing compile-time overrides.

Cover:

- zeroed list and map controls with no backing;
- fixed-size descriptor validation remaining exact;
- variable backings at zero/minimum, ordinary, and maximum practical probe
  capacities;
- rejection of truncated, padded, misaligned, stride-inconsistent, and
  capacity-inconsistent bodies;
- scalar list backing marked without a trace callback;
- struct/container-bearing backing tracing exactly initialized records;
- unused capacity containing hostile nonzero bytes without being traced;
- control callbacks enqueueing only published nonzero backing references;
- container controls and backings surviving policy and pressure collection;
- unreferenced controls and replaced backings being swept and reused;
- nested container descriptors and cycles terminating through exact keys;
- a control rooted before a second managed allocation;
- growth allocation followed by re-resolution and last-step publication;
- allocation, commit, scratch, invalid-state, and rollover paths;
- no sweep after failed trace and no partial publication after failure; and
- a valid heap, free list, root chain, and active guard after every returning
  path.

Probe assertions may use private counters and corruption hooks under existing
test defines. They must not implement a parallel container allocator or tracer.

### Public and regression coverage

Add public end-to-end cases for `str.len()` covering empty, ASCII, escaped, and
embedded-NUL strings with exact output. Existing string indexing and output
behavior must remain unchanged.

Retain explicit build failures for list/map execution during this stage so an
accidental partial implementation cannot silently generate incorrect C. Keep
all milestone 1-10 frontend, IR, escape, backend, native, GC, diagnostic, and
determinism cases unchanged.

Only absence of a supported C compiler may skip native assertions. Generated-C
compile failure, native assertion failure, abnormal termination, wrong output,
or unexpected exit status is a test failure. POSIX and Windows branches both
require external verification before completion.

## Completion gate

Stage 1 is complete when:

- list and map values have a permanent 8-byte packed-reference carrier in all
  supported storage positions;
- deterministic validated plans cover every concrete container, value
  operation, control layout, and backing role needed by the program;
- fixed structs retain exact body validation while variable managed backings
  are validated by checked prefix/stride/capacity rules;
- all container carriers are canonical precise roots regardless of contents;
- generated control and backing callbacks mark owners and traverse exactly the
  initialized reference-bearing values;
- container retention cannot leave a reachable struct incorrectly scoped;
- managed construction and growth probes root controls, re-resolve after safe
  points, and publish replacements transactionally;
- dead controls/backings are reclaimed while live identities remain stationary
  across policy, pressure, and rollover collections;
- `str.len()` executes with byte-length semantics and no `strlen` dependency;
- list/map operations remain explicitly capability-gated for Stages 2-4;
- struct-only and primitive-only generated behavior remains correct and
  deterministic;
- no source feature, host-heap content storage, dependency, or generated
  artifact is introduced beyond this stage; and
- direct and native verification passes on the available platform matrix.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must use Rust 1.90 or newer,
report native skips explicitly, and confirm that only intended source and
documentation changes remain.
