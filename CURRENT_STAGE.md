# Current Stage: Complete Lists and Entry Arguments

Status: current.

This document expands Stage 2 of
[Current Milestone: Containers](CURRENT_MILESTONE.md). Stage 1 established the
permanent packed container carrier, stable managed control objects, variable
managed backings, value-operation descriptors, exact tracing, and allocation
seams. This stage makes lists fully executable through those seams and replaces
the temporary entry-argument view with an ordinary managed `[str]`.

Map execution and all container iteration remain capability-gated. Stage 3
uses the same storage architecture for maps; Stage 4 renders iteration and its
complete lock lifecycle.

## Outcome

At the end of this stage, source programs can:

- construct non-empty and explicitly typed empty lists;
- store every v0 value representation in a list;
- copy list references through locals, parameters, results, tuples, unions,
  structs, and other lists without losing identity or aliasing;
- read and replace elements using positive and negative indices;
- call `len()`, `append(value)`, and `removeIndex(index)`;
- test list membership using the element type's language equality;
- compare lists by object identity;
- grow lists geometrically through managed backing allocations; and
- receive `main(args [str])` as a normal list of canonical interned strings in
  original command-line order.

The public path remains a walking skeleton:

```text
source list operation
        |
        v
typed IR + exact failure site
        |
        v
list capability validation
        |
        v
typed generated helper
        |
        +---- resolve stable control
        +---- resolve current backing
        +---- copy / compare / trace through value descriptor
        |
        v
managed allocation, collection, and source-attributed panic
```

## Preserved contracts

Stage 2 must preserve:

- the source semantics and evaluation order in `DESIGN.md` and syntax in
  `GRAMMAR.ebnf`;
- the 8-byte `sao2_ref` value carrier and the rule that decoded native pointers
  never live across a managed allocation;
- the Stage 1 list control layout: length, capacity, iteration lock count, and
  current element-backing reference;
- the common variable-backing prefix and descriptor-provided payload offset,
  stride, copy, equality, and trace operations;
- zero-safe canonical shadow-frame slots and exact root traversal;
- stationary references, exact member tracing, collection policy, allocation
  failure classification, and epoch rollover;
- value copying for inline tuples and unions, and reference copying for
  structs, lists, maps, and strings;
- left-to-right operand lowering and deterministic generated C;
- source errors remaining distinct from compiler, toolchain, and program
  failures; and
- unchanged execution for every program accepted before this stage.

List storage remains private. Source code cannot observe capacity, backing
identity, growth count, layout, collection, or unused bytes.

## Stage boundaries

This stage does not enable:

- map literals, lookup, insertion, replacement, removal, membership, length,
  equality, or map entry-argument behavior;
- `BeginIteration`, `IterationValue`, or `EndIteration` for either container;
- a source-visible iterator, capacity, reserve, shrink, slice, sort, or deep
  copy operation;
- printing lists or structural list equality;
- scoped list allocation or host-allocated language element storage;
- shrinking a backing after removal; or
- the diagnostics-wide presentation changes assigned to milestone 12.

The existing `IterationUnlocked` check is rendered only for list structural
mutation. This is required because lowering already pairs it with `append` and
`removeIndex`; the lock count remains zero in ordinary Stage 2 source programs
until Stage 4 enables iteration.

## Decisions to record before runtime behavior depends on them

### List allocation failures

Add `FailureOperation::ListAllocation` for list aggregate construction. Append
it to the stable generated failure-operation numbering; do not renumber the
existing operations. Both non-empty literals and explicitly typed empty lists
carry this site because an empty list still allocates its stable control.

Use the existing managed-allocation reason classes at the literal site:

- `heap arena exhausted` for an unrepresentable capacity, body size, or logical
  heap exhaustion;
- `unable to commit heap storage` for platform commitment failure; and
- `unable to allocate garbage collector work storage` for collector scratch
  failure.

Growth attributes those same reasons to the existing `ListAppend` site. Index
normalization failure uses `list index out of range` at the existing
`ListIndex` or `ListRemoveIndex` site. A nonzero iteration lock uses
`container structurally modified during iteration` at the append or removal
site. Invalid descriptors, corrupt lengths, impossible lock state, or broken
publication invariants are compiler/runtime invariant failures rather than
source panics.

### Equality needed by membership

List identity equality is fully specified. Membership must call the element
descriptor's language equality, so primitives compare by value, tuples compare
recursively, and struct/list/map members compare by identity. Before enabling
membership for a union element, make the currently implicit union-equality rule
explicit in `DESIGN.md` and cover it in semantic tests; do not derive a new
rule merely from the C representation. This clarification must not block union
storage, copying, or tracing.

### Private growth policy

Use a deterministic private policy:

- an empty list has capacity zero and a zero backing reference;
- the first required backing capacity is four elements;
- a larger literal rounds its element count up to the next power of two;
- append doubles capacity until it satisfies the required length; and
- removal does not shrink.

Every step uses checked arithmetic and is capped by `INT64_MAX`, `size_t`, the
32-bit packed arena offset, and the concrete backing descriptor's maximum body
size. Exhausting any cap is reported as managed heap exhaustion. Capacity is
not language-visible, so this policy can later be tuned without changing list
semantics, but tests may assert it at the native-helper layer.

## IR and validation contract

Change the list aggregate to carry allocation attribution:

```text
Aggregate::List {
    ty,
    elements,
    failure: FailureSiteId,
}
```

Lowering interns `ListAllocation` at the complete literal or typed-empty-list
span after evaluating and stabilizing element expressions. Element expressions
remain left-to-right, and their earlier effects or failures remain earlier than
the allocation operation.

IR validation must require:

- `ty` is the exact list type;
- every element operand has its declared element type;
- the failure site belongs to `ListAllocation` and the containing function;
- list index projections carry `ListIndex` sites and an `int` index local;
- list append and removal carry their matching failure sites and immediately
  follow the existing receiver-equivalent `IterationUnlocked` check;
- `len` has no failure site or arguments;
- append receives exactly one element value;
- removal receives exactly one `int` index;
- membership operands are an element and its exact list type; and
- list equality operands have the same list type.

Update textual IR rendering and all visitors without conflating the new site
with struct allocation. Tests should prove malformed combinations are rejected
and that existing numeric, string-index, struct-allocation, and output operation
codes remain stable.

## Capability boundary

Enable the smallest coherent list slice in backend capability validation:

- list aggregate construction;
- list-index projections in reads and assignment destinations;
- `ListLen`, `ListAppend`, and `ListRemoveIndex`;
- `value in list`;
- `==` and `!=` for list carriers;
- list-side `IterationUnlocked`; and
- list values inside executable tuples and unions, including recursive shapes.

Continue to reject every map operation and every begin/value/end iteration
operation with the existing capability error. A map carrier may still be
stored, copied, traced, and compared as an element once such a value reaches a
list through an already valid program path; Stage 2 must not manufacture a
public map-construction path to test this.

Replace capability predicates which call all container payloads unsupported.
The relevant question for tuple/union execution is whether each value has a
valid C representation and required operation descriptor, not whether it is a
container. Keep printing capability separate so enabling list values does not
make containers printable.

## Typed list helper surface

Generate deterministic helpers for each concrete list type in the Stage 1
container plan. Helpers may share untyped checked arithmetic and reference
resolution routines, but all element access must be tied to the concrete value
descriptor and backing layout.

The generated surface should cover:

- construction from zero or more already-stabilized operands;
- checked control and backing resolution;
- index normalization;
- immediate element load and store;
- length;
- append with optional growth;
- removal and stable left shift;
- membership scan; and
- identity comparison through the packed carrier.

Each resolver validates before pointer arithmetic:

- the reference is nonzero and resolves to the expected list-control layout;
- `length <= capacity` and both fit the language integer domain;
- capacity zero agrees with a zero backing;
- nonzero capacity has the exact planned backing layout;
- the backing prefix's capacity agrees with the control;
- initialized element count agrees with logical length; and
- the computed payload and final element lie within the validated body size.

A decoded control, backing, or element pointer is a short-lived C temporary.
It must be discarded before any helper which can allocate or collect.

## List construction

Construction receives element operands which lowering has already stabilized in
canonical locals. Render it as one publication transaction:

1. Zero the canonical destination carrier.
2. Allocate the fixed list control directly into that destination.
3. The destination's shadow-frame slot is now the collection root; do not keep
   the only reference in a native C temporary.
4. Leave the zeroed control logically empty while computing capacity.
5. For a non-empty literal, allocate the typed backing. Collection may occur,
   so re-resolve the control from the canonical destination afterward.
6. Copy elements in source order through the value descriptor. Advance the
   backing prefix's initialized count only after each destination slot contains
   a complete value.
7. Publish backing reference and capacity, then publish logical length last.

No allocation occurs during descriptor copy. If this stops being true in a
future stage, this protocol must be redesigned rather than retaining decoded
pointers across a safe point. A failed second allocation leaves only an
unobservable empty control and terminates through the literal's failure site;
no partially constructed list becomes a language value.

Zero-sized unit elements still have logical slots and initialized count. Their
descriptor defines a nonzero physical stride suitable for address progression,
while value copy and equality implement unit semantics. Padding is initialized
as required for deterministic C output but is never compared as language data.

## Indexing and replacement

Use one checked normalization rule for reads, replacement, and removal. Avoid
negating `INT64_MIN`:

```text
nonnegative: normalized = index, valid when normalized < length
negative:    magnitude = -(index + 1) + 1 in unsigned arithmetic
             valid when magnitude <= length
             normalized = length - magnitude
```

An index expression is evaluated exactly once by lowering. Bounds checking uses
the operation's existing source site. `-1` selects the last item; `-length`
selects the first; both `length` and values less than `-length` panic.

Generated place rendering needs two distinct paths:

- a read copies the selected value immediately into its destination; and
- a final list-index assignment copies the already-stabilized right-hand value
  immediately into the selected slot.

For deeper projections, consume the selected slot immediately while resolving
the next inline member or copied object reference. Do not return a native slot
pointer into general expression rendering or preserve it across a call or
allocation. Preserve the operation order represented by IR for assignment
targets and right-hand operands.

Replacement is non-structural: it does not change length, capacity, backing,
or lock count. It therefore does not use `IterationUnlocked`. Copy through the
element descriptor so tuple/union padding is not treated as a language value
and reference carriers retain their exact packed representation.

## Length, membership, and identity

`len()` resolves and validates the control, then returns logical length as
`int64_t`. Capacity rules guarantee that conversion is representable. It does
not allocate or carry a failure site.

Membership resolves the list once, walks exactly `[0, length)` in logical order,
and invokes the concrete element equality operation. It ignores unused
capacity. It returns on the first match and performs no allocation. Equality
callbacks must recurse by language value rules rather than `memcmp`, including
for padding-bearing tuples and unions and identity-bearing object members.

List `==` and `!=` compare the complete packed `sao2_ref` identity. They do not
resolve or traverse either list and do not compare controls, lengths, elements,
or backing references.

## Append and geometric growth

Append first checks the shared control's iteration-lock count. With spare
capacity it:

1. resolves the current backing;
2. copies the argument into slot `length`;
3. advances initialized count; and
4. publishes the new logical length last.

When growth is required it performs an allocate/copy/publish transaction:

1. Compute the new capacity with the checked private growth policy.
2. Keep the receiver and append argument in canonical frame storage.
3. Allocate a new typed backing into a canonical backing-scratch root. This may
   collect.
4. Re-resolve the receiver control and old backing from packed references.
5. Copy the old logical elements in index order, advancing only the new
   backing's initialized count.
6. Copy the appended value into the next slot.
7. Re-resolve the control if any helper boundary could have invalidated native
   pointers.
8. Publish the new backing and capacity, then publish the new length last.

The old backing stays reachable through the control until publication and can
be reclaimed by a later collection. Never use host `realloc`, mutate the old
capacity early, or expose a half-filled new backing. An allocation or capacity
failure leaves the original list unchanged and panics at the append site.

## Removal

`removeIndex` checks the shared iteration lock, normalizes and bounds-checks the
index, then shifts later values one position toward the front in logical order.
Use descriptor copy or generated typed assignment, never raw byte equality or
an overlap-unsafe bulk copy. Because destination indices are lower than source
indices, forward copying is safe.

After the shift, zero the vacated final physical slot to restore the internal
zero-safe state, reduce the backing initialized count, and publish the shorter
logical length. Removal keeps the existing backing and capacity. It returns
unit and never allocates, so a bounds failure leaves the list unchanged.

## Tracing and collection invariants

The Stage 1 control callback visits the current backing reference. The typed
backing callback must trace exactly the initialized logical prefix using the
element descriptor. Stage 2 must demonstrate that:

- unused capacity is never traced;
- a removed final slot cannot retain an object accidentally;
- replacement changes the reachable graph immediately;
- a new backing is rooted during growth before it is published;
- the old backing remains rooted until publication completes;
- nested lists and lists containing tuple/union/struct references recurse with
  the exact existing `(owner, member, layout)` keys;
- aliases preserve one shared control and observe the same mutations; and
- unreachable controls and superseded backings are reclaimed independently.

The source language cannot force collection at a chosen instruction, so native
probes must inject policy, pressure, and allocation failures at each growth and
construction boundary.

## Entry arguments as an ordinary list

Remove the special `sao2_args { count, values }` signature and local-storage
exception. The generated SAO2 entry function accepts the same `sao2_ref`
carrier used by every other `[str]` parameter, and its normal shadow-frame copy
and trace plan apply without a special operation-use rejection.

The host adapter constructs arguments in this order:

1. Exclude the executable name and preserve the remaining `argv` order.
2. Validate every byte of every argument as ASCII before entering generated
   `main`; report the existing indexed pre-entry panic for invalid input.
3. Initialize the managed runtime and link a generated, zeroed host-adapter
   shadow frame containing one `[str]` carrier.
4. Allocate the list control directly into that rooted carrier.
5. Allocate its known-size backing through the ordinary `[str]` descriptor.
6. Intern each argument and copy its canonical string reference into the next
   initialized slot.
7. Publish the completed backing, capacity, and length using the ordinary list
   construction invariants.
8. Call generated `main` with the packed list carrier while the adapter frame
   remains linked.
9. After `main` returns, unlink the adapter frame, release the arenas, and free
   host-side dynamic interning metadata.

Even zero arguments produce a distinct ordinary empty list control. The
adapter frame may be linked before its carrier becomes nonzero, so there is no
gap between allocation return and root publication. Generated `main` then
copies the carrier into its own canonical parameter slot before any body
allocation.

### Canonical argument strings

Argument strings must participate in the same canonical equality as source
string literals:

- emit a deterministic registry of the program's static interned strings;
- compute the existing stable string hash for each ASCII argument;
- resolve equal bytes to the static object when one exists;
- otherwise reuse one dynamic intern record for duplicate argument text; and
- allocate dynamic intern records and byte copies only as host runtime metadata,
  never as list backing storage.

Track dynamic records in an adapter-owned pool and release them only after the
generated program has returned and arena teardown can no longer trace argument
values. Host allocation failure during this pre-entry work reports a stable
pre-entry panic and never enters generated `main`. Managed allocation failures
during argument-list construction retain their allocator classification but do
not claim a source location, because no SAO2 expression allocated the list.
Every pre-entry failure unlinks any adapter frame, releases initialized arenas,
and frees the dynamic intern pool before returning failure to the host.

## Implementation batches

Each batch leaves the accepted subset coherent and retains capability rejection
for work assigned to a later batch.

### Batch 1: IR attribution and capability shape

- Add `ListAllocation`, the aggregate failure field, lowering, textual IR, and
  validation.
- Generalize executable tuple/union carrier checks for list-containing values.
- Define list-only capability cases while keeping rendering rejected until the
  corresponding helper exists.
- Lock stable failure codes and malformed-IR tests.

Gate: list IR is fully attributed and deterministic; unsupported generated C
cannot slip through a broad container allowance.

### Batch 2: Construction, length, and identity

- Generate typed list resolution and construction helpers.
- Enable non-empty and typed-empty literals.
- Enable `len`, list identity equality, aliasing, parameter passing, and return.
- Prove scalar, zero-sized, inline aggregate, and reference element layouts.

Gate: fixed-capacity lists are ordinary managed objects with correct identity,
roots, construction order, and literal failure behavior.

### Batch 3: Index places and replacement

- Implement overflow-safe negative normalization.
- Render list-index reads, final replacements, and deeper projections.
- Add exact source panics and left-to-right/order regression tests.

Gate: all valid index forms read and replace correctly, and every invalid form
panics without mutation at its original source operation.

### Batch 4: Append, removal, and membership

- Render list-side mutation checks.
- Implement spare-capacity append, geometric growth, stable removal, and
  descriptor-based membership.
- Clarify union equality before enabling union-element membership.

Gate: mutation is transactional, aliases observe it, capacity is unobservable,
and all supported element equalities match `DESIGN.md`.

### Batch 5: Entry arguments

- Replace the special entry ABI with an ordinary `[str]` carrier.
- Add the adapter shadow root, known-size list construction, static/dynamic
  interning bridge, duplicate canonicalization, cleanup, and pre-entry errors.
- Exercise zero, duplicate, literal-equal, ordered, and invalid-ASCII arguments.

Gate: `main(args [str])` uses no special list semantics after adapter
construction and leaves no host metadata live after runtime teardown.

### Batch 6: Collection and integration hardening

- Stress nested/reference-bearing elements, repeated growth, replaced and
  removed references, dead backing reclamation, pressure collection, and epoch
  rollover.
- Inject each construction and growth failure boundary.
- Confirm deterministic output and unchanged milestone 1-10 behavior.

Gate: list programs remain exact under collection and failure, and the only
remaining container capability boundaries are maps and iteration.

## Verification map

### Semantic and lowering tests

Cover:

- typed empty-list requirements and element homogeneity;
- mutability requirements for replacement, append, and removal;
- membership and list identity operator typing;
- left-to-right element and argument evaluation;
- stable `ListAllocation`, `ListIndex`, `ListAppend`, and `ListRemoveIndex`
  attribution; and
- ordinary `[str]` entry signature validation.

### IR and planner tests

Cover:

- list aggregate failure-site validation;
- concrete list descriptor closure through functions, tuples, unions, structs,
  nested lists, and map-containing storage shapes;
- value copy/equality/trace capability selection;
- deterministic helper and managed-layout ordering; and
- rejection of map execution and all iteration operations.

### Generated-C tests

Assert operation order, not incidental whitespace:

- destination rooting precedes backing allocation;
- operands are copied before publication;
- no decoded pointer crosses an allocation call;
- growth copies before changing the control;
- length is the final published construction/append fact;
- removal clears the vacated slot;
- reads and stores normalize an index once; and
- entry arguments use `sao2_ref`, not `sao2_args` or raw `argv` access in the
  generated SAO2 function.

### Native runtime probes

Use the production runtime fragments for:

- empty, minimum, multi-growth, and maximum representable capacities;
- unit, primitive, string, tuple, union, struct, list, and map carriers as
  physical element records;
- exact backing tracing before and after append, replacement, and removal;
- collection during literal construction and growth;
- old-backing reclamation after publication;
- aliased controls and identity comparison;
- lock rejection with an injected nonzero lock count;
- allocator, commitment, scratch, and arithmetic failure injection; and
- epoch rollover with nested lists live.

### Public end-to-end programs

Cover observable behavior with programs that use:

- empty and non-empty literals;
- positive indices, every valid negative boundary, and both invalid sides;
- replacement through aliases;
- enough appends for repeated growth;
- removal at front, middle, end, and negative indices;
- membership for representative value and identity element types;
- lists passed, returned, nested, stored in aggregates, and selected from
  unions;
- reference-bearing elements surviving collections;
- zero, one, duplicate, ordered, and source-literal-equal command-line
  arguments; and
- an invalid non-ASCII argument with no entry-body effects.

Keep iteration programs rejected until Stage 4 and map-operation programs
rejected until Stage 3. Native assertions may skip only when no supported C
compiler is available.

## Completion checklist

Stage 2 is complete when:

- every enabled list operation uses Stage 1 managed controls and backings;
- list construction and growth are rooted, transactional, and precisely
  source-attributed;
- element storage works for every v0 representation without raw-byte language
  equality;
- positive and negative indexing, replacement, length, append, removal,
  membership, identity, aliasing, passing, and return match `DESIGN.md`;
- repeated geometric growth has no language-visible bound before classified
  managed allocation failure;
- tracing visits exactly live elements and reclaims dead controls and backings;
- `main(args [str])` receives an ordinary rooted list of canonical strings and
  all adapter metadata is released;
- maps and iteration remain explicit capability errors;
- generated C remains byte-for-byte deterministic;
- the Stage 2 test matrix passes externally on Rust 1.90 or newer and available
  supported C compilers; and
- all earlier walking-skeleton programs continue to work unchanged.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. Generated artifacts belong only under `build/` or
test temporary directories and must not be committed.
