# Current Stage: Precise Shadow Frames

Status: current.

This document expands Stage 3 of
[Current Milestone: Garbage Collector](CURRENT_MILESTONE.md). It generates a
typed shadow frame for every function which can hold a garbage-collected
reference, makes those frame fields the canonical storage for the selected IR
locals, and exposes the active frame chain to the Stage 2 trace engine.

Stage 3 does not collect automatically. A native probe may explicitly trace
the active chain, but ordinary allocation does not enumerate roots, advance
epochs, sweep blocks, or retry after reclamation.

## Outcome

An active call chain has a parallel generated root chain:

```text
native calls                         precise root chain

sao2_fn_0                            sao2_shadow_top
  sao2_fn_1                                 |
    sao2_fn_2                               v
                                  +-------------------+
                                  | frame for fn 2    |
                                  | exact typed roots |
                                  +-------------------+
                                           |
                                           v
                                  +-------------------+
                                  | frame for fn 1    |
                                  | exact typed roots |
                                  +-------------------+
                                           |
                                           v
                                  +-------------------+
                                  | frame for fn 0    |
                                  | exact typed roots |
                                  +-------------------+
```

The native C stack remains responsible for calls, primitive locals, and return
channels. The shadow chain contains only statically known reference-bearing IR
locals and materialized temporaries. Each frame callback visits its fields
through the exact Stage 2 type traversal helpers; it never scans padding,
untyped bytes, or the native stack.

## Preserved contracts

Stage 3 must preserve:

- the packed reference ABI, stable owner/member offsets, and arena tags;
- the Stage 1 block heap, free-list order, allocation policy, and private
  reclamation boundary;
- the Stage 2 trace plan, layout registry, exact-item key, queue, callbacks,
  scratch failure behavior, and heap/scoped traversal distinction;
- the 48-byte heap header and caller-supplied nonzero trace epoch;
- ordinary C function calls and the existing source-level calling convention;
- the escape plan's heap-versus-scoped allocation decision at every site;
- source evaluation order and initialize-before-publication construction;
- one function-owned scoped mark and restoration on every normal return where
  scoped allocations occur;
- entry-point signatures, host adapter behavior, diagnostics, and source
  failure locations; and
- deterministic generated names and byte-for-byte output.

The typed IR remains the authority for every local's type and identity. Stage
3 derives roots from that information without adding GC operations, storage
classes, or physical frame layout to IR.

## Stage boundaries

This stage does not add:

- automatic collection, allocation retry, sweeping, or calls to heap reclaim;
- ownership of a global mark epoch or rollover handling;
- liveness-based root-slot clearing or safe-point-specific live sets;
- native-stack scanning or conservative word interpretation;
- a write barrier, moving, compaction, weak references, or finalization;
- list, map, dynamic string, or iterator roots;
- source-visible root registration or forced collection; or
- frontend, escape-analysis, lowering, or language-semantic changes.

All reference-bearing IR locals are rooted for their complete function
invocation. A slot is zero before its first assignment and may retain its last
value until overwritten or the function returns. This is type-precise and
safe, though it can conservatively retain a dead local until return. The
current IR has no storage-dead operation, and this stage does not invent one.

## Compiler-owned root plan

### Plan boundary

Build an immutable `RootPlan` after `TracePlan` validation and before
rendering. It is backend-owned input alongside `LayoutPlan`,
`AllocationPlan`, and `TracePlan`.

The plan contains one entry per function:

- the `FunctionId`;
- selected root `LocalId` values in ascending local order;
- each selected local's exact `TypeId`; and
- whether the function requires a frame.

A local is selected exactly when the Stage 2
`type_contains_reference[local.ty]` fact is true. This includes source
bindings, parameters, and compiler-materialized temporaries. A function with
no selected local receives no typed frame, frame callback, link, or unlink.

Backend-only native temporaries are not automatically roots. Each such
temporary must instead satisfy the safe-point audit below. In particular, the
temporary packed reference and decoded body pointer used during struct
construction are created only after allocation returns, and no allocation can
occur before the reference is published to its destination frame field.

### Validation

Validate the complete plan before rendering:

- there is exactly one function entry for each IR function;
- function entries are in ascending `FunctionId` order;
- selected locals exist, are unique, and are in ascending `LocalId` order;
- every selected type is marked reference-bearing by `TracePlan`;
- every omitted local is marked non-reference-bearing;
- parameter and temporary origins do not alter selection;
- selected types have an available generated trace expression;
- entry-argument carriers remain non-roots unless a future traced container
  plan explicitly changes that fact; and
- a plan failure produces no partial generated C.

Do not infer roots by inspecting rendered C types or searching emitted text.
Do not use source names, lexical scopes, spans, or allocation classes as root
identities.

## Shadow-frame ABI

### Common header

Emit one common header and callback type whenever at least one function needs a
frame:

```c
typedef struct sao2_shadow_frame sao2_shadow_frame;
typedef void (*sao2_trace_frame_fn)(
    sao2_trace_context *context,
    const sao2_shadow_frame *frame
);

struct sao2_shadow_frame {
    sao2_shadow_frame *previous;
    sao2_trace_frame_fn trace;
};
```

Maintain one process-global `sao2_shadow_top`, initially `NULL`. SAO2 v0 is
single-threaded; this stage does not add thread-local chains or concurrency.
The pointer is runtime metadata, not a language value or a scanned root.

Add narrow link and unlink helpers:

- link requires a non-null frame with a non-null callback, stores the current
  top in `previous`, and publishes the new top last;
- unlink requires the supplied frame to be the current top, restores
  `previous`, and clears the unlinked frame's previous pointer; and
- a non-LIFO unlink is a compiler/runtime invariant.

Neither helper allocates, traces, touches either arena, or fails for valid
generated input.

### Typed function frames

For each function with selected roots, emit an identity-named C struct:

```c
typedef struct {
    sao2_shadow_frame header;
    /* exact C carriers for selected locals, ordered by LocalId */
} sao2_shadow_frame_fn_N;
```

Name fields only from `LocalId`, for example `local_3`. Use the same
`c_type` mapping as ordinary local storage:

- a struct local is `sao2_ref`;
- a tuple or union local uses its generated aggregate carrier; and
- no primitive-only field is emitted.

Assert that `header` is at offset zero and that every root field has the
planned C size and alignment. Do not pack frames or depend on their total size
as a cross-build ABI.

Frame definitions require complete tuple and union carrier definitions, so
emit them after value aggregates and struct bodies but before generated
functions.

### Frame callbacks

Emit one callback per typed frame. It casts the common header pointer back to
the exact function frame after the offset-zero assertion and visits fields in
ascending `LocalId` order:

- a struct field calls `sao2_trace_enqueue` with its exact layout descriptor;
- a reference-bearing tuple calls `sao2_trace_value_def_N`;
- a nominal or anonymous union calls its Stage 2 value callback; and
- no bytewise or conservative fallback exists.

Callbacks observe zero-initialized fields safely. All-zero references are
ignored, tag-zero unions are inactive, and no inactive payload is read. Stop
visiting fields once the trace context has a sticky failure.

The callback takes a `const` frame. Tracing may update heap mark epochs and
native trace scratch, but it must not mutate root slots or native call state.

## Root storage is canonical

### Local declarations

At function entry:

- declare and zero-initialize the complete typed frame when one is planned;
- declare only non-root locals as ordinary zero-initialized C locals; and
- never declare a second ordinary C local for a selected root.

A reference-bearing local's only storage throughout the generated function is
its frame field. There is no mirrored value, dirty bit, spill operation, or
synchronization protocol.

“Mirrored root synchronization” would keep an ordinary C local for generated
operations and a second copy in the shadow frame for tracing. Every write would
then have to update both copies before a possible collection. A stale frame
copy could lose a live object or retain a dead one. Canonical frame storage
removes that failure mode because mutator reads, mutator writes, and tracing
all use the same field.

Centralize local rendering behind a helper which maps:

- selected `LocalId` to `sao2_frame.local_N`; and
- non-selected `LocalId` to `sao2_local_N`.

Thread the current `FunctionId` or validated function root plan explicitly
through operand, place, destination, aggregate, intrinsic, check, and
terminator rendering. Avoid ambient renderer state which could accidentally
use the preceding function's root map.

### Complete renderer audit

Use the centralized local accessor for every occurrence, including:

- copy, unary, binary, conversion, and string-index destinations;
- direct assignment and every projected place root;
- call destinations and arguments;
- tuple construction, field writes, comparison, and projection;
- union zeroing, payload writes, tag writes, tests, extraction, and switches;
- struct construction publication;
- inline-struct replacement and referenced-field rebinding;
- runtime checks, intrinsics, panic operands, and return operands; and
- helper-generated expressions such as inline destination references.

No renderer path may construct `sao2_local_N` directly for a selected local.
Tests should search generated C for both undeclared frame bypasses and
accidental duplicate storage.

Backend-only names such as struct-construction references, decoded body
pointers, output writers, return temporaries, and scoped marks remain distinct
native temporaries. They must never be added to a frame merely because their C
type could contain a reference; their safety follows from the explicit
safe-point rules.

## Function prologue

For a function with roots, emit this logical order:

1. zero-initialize the complete typed frame;
2. set its generated callback;
3. link its common header onto `sao2_shadow_top`;
4. copy each reference-bearing native parameter into its canonical frame
   field;
5. copy each non-root parameter into its ordinary local;
6. capture the scoped-arena mark when the allocation plan requires one; and
7. jump to the IR entry block.

Linking the zeroed frame before copying parameters guarantees that every
published slot is trace-safe. No collection or allocation occurs between
linking and completing parameter copies.

The caller retains every reference-bearing argument in its own linked frame
until control transfers to the callee. A callee parameter may temporarily
exist only in the native argument channel because no safe point occurs before
the callee stores it in its frame.

A function without roots retains the existing prologue and does not touch the
shadow chain. A function with roots but no parameters still links its zeroed
frame. Recursion creates one distinct frame instance per invocation even
though every instance shares the same generated callback.

## Calls, allocations, and safe points

Stage 4 will make heap allocation the only collection safe point. Stage 3 must
make the following contracts true before collection is enabled:

- a caller frame remains linked for the complete callee invocation;
- every reference-bearing call operand is read from canonical rooted storage;
- the callee links its zeroed frame and copies root parameters before any
  operation which can allocate;
- a call destination's old rooted value remains in its slot while the callee
  runs and is overwritten only after return;
- all already evaluated struct-constructor operands remain in rooted IR locals
  while allocation runs;
- the new allocation reference and decoded body pointer are produced only
  after allocation returns;
- no call which can allocate occurs between receiving a new reference and
  publishing it to the destination frame field; and
- no decoded heap or scoped body pointer remains live across a user-function
  call or heap-allocation call.

Existing copy and resolver helpers do not allocate. If any helper gains an
allocation path later, it must first be re-audited as a safe point rather than
silently relying on this stage's proof.

## Normal returns and termination

Every normal return from a framed function uses one block-scoped epilogue:

1. evaluate and capture the return operand in a native temporary of the exact
   result C type;
2. unlink the current shadow frame;
3. restore the function-owned scoped mark when present; and
4. return the captured value.

Capture must occur before unlinking because the operand may live in the frame.
Unlink must occur before scoped restoration so no linked root can name storage
owned by the returning invocation after its cursor is rewound.

A reference-bearing result may travel in the native return channel after the
callee unlinks because there is no allocation or collection between unlink,
optional scoped restoration, native return, and assignment into the caller's
rooted destination. Escape analysis guarantees that a returned struct
reference does not identify callee-owned scoped storage.

Apply the epilogue to every `Return` terminator, including early returns and
functions which also own scoped allocations. A primitive result still uses the
same framed epilogue so link discipline is independent of result type.

Terminating panic and compiler-invariant paths do not unwind frames. They end
the process and may leave the chain linked. Do not introduce cleanup stacks,
`setjmp`, or C unwinding for these paths.

Unframed functions retain their current direct return unless they need scoped
restoration, in which case they retain the existing capture-and-restore shape.

## Root-chain traversal

### Traversal helpers

Add an explicit no-op global-root hook:

```c
static void sao2_trace_global_roots(sao2_trace_context *context);
```

SAO2 currently has no source-level global values. Interned string descriptors,
layout descriptors, failure metadata, arena state, and native argument arrays
are not GC roots.

Add a shadow-root visitor which:

1. preserves the chain unchanged;
2. rejects a null callback or a cycle in otherwise valid frame links;
3. walks from the current top toward the oldest frame;
4. invokes each generated frame callback; and
5. stops immediately when the trace context fails.

Use a non-allocating cycle check, such as tortoise-and-hare, before invoking
callbacks. Generated code owns pointer validity; the runtime need not make
arbitrary forged native pointers safe to dereference.

Root enumeration only enqueues edges. It does not initialize or dispose the
trace context, drain trace work, advance epochs, interpret liveness, or sweep.
The isolated probe performs those phases explicitly; Stage 4 will define the
complete transaction.

### Runtime lifecycle

Arena initialization requires `sao2_shadow_top == NULL`. Normal entry return
requires the chain to be empty before arena release. Add an invariant check in
each host-adapter shape after the SAO2 entry function returns and before
releasing arena reservations.

Arena release clears collector runtime state only after proving there is no
active frame. Do not silently discard a nonempty chain, since that would hide a
generated epilogue defect.

## Rendering order

Preserve existing generated sections and add frame material in this dependency
order:

1. fixed value types and complete aggregate/body definitions;
2. Stage 2 trace declarations, descriptors, and layout registry;
3. common shadow-frame declarations;
4. typed function-frame definitions and callback prototypes;
5. diagnostics, scalar helpers, and Stage 1 arena runtime;
6. Stage 2 trace runtime and value/body callback definitions;
7. shadow-chain helpers and generated frame callback definitions;
8. existing allocation, copy, formatting, and value helpers;
9. generated function prototypes and definitions; and
10. host adapter.

When no function needs a frame, omit typed frame definitions and callbacks.
The no-op global-root hook and empty chain may remain with the trace runtime so
Stage 4 has a uniform integration point, but primitive-only programs with no
struct layouts must remain standards-conforming and free of unusable trace
types.

## Implementation sequence

Implement Stage 3 in this order:

1. Add `FunctionRootPlan`, `RootPlan`, construction from
   `TracePlan::type_contains_reference`, full validation, and deterministic
   direct tests.
2. Add the common shadow-frame ABI, global top pointer, checked link/unlink
   helpers, and empty global-root hook.
3. Emit typed frame definitions, offset/field assertions, callback prototypes,
   and exact generated frame callbacks.
4. Centralize local-name and local-place rendering, then migrate every read,
   write, projection root, destination, and helper-generated expression.
5. Change function declarations and prologues so framed locals exist only in
   zero-initialized canonical frame storage and frames link before parameter
   copies.
6. Unify normal return rendering around capture, unlink, scoped restore, and
   return, preserving the existing unframed cases.
7. Add deterministic global/frame root enumeration and host-adapter empty-chain
   checks without invoking them from ordinary allocation.
8. Audit native temporaries and decoded body pointers against the future
   allocation-only safe-point contract.
9. Extend the isolated native probe to traverse nested generated frames, then
   add source-to-native regression coverage and remove all root-local
   hard-coded names.

Each step must leave the source-to-executable path working. Do not temporarily
mirror selected locals into both frame and ordinary storage.

## Test plan

### Direct Rust/backend tests

Add focused tests for:

- selection of struct, nested tuple, nominal union, anonymous union, parameter,
  binding, and temporary locals;
- omission of unit, primitive, string, entry-argument, and other non-carrying
  locals;
- one ordered root-plan entry per function and unique ordered `LocalId`
  fields;
- no frame for a function without roots;
- one typed frame definition and callback per function with roots;
- header offset zero and exact root-field carrier assertions;
- callback traversal in ascending `LocalId` order;
- zero-safe struct and union frame fields;
- link-before-parameter-copy prologues;
- one distinct frame instance for recursive invocations;
- canonical frame storage across every operation, place, aggregate,
  projection, check, terminator, and helper renderer;
- absence of a duplicate ordinary declaration for every rooted local;
- capture-before-unlink and unlink-before-scoped-restore on every normal return;
- unchanged direct returns for unframed/non-scoped functions;
- caller/callee argument and return no-safepoint ordering;
- host-adapter empty-chain validation before arena release;
- no automatic root enumeration, trace drain, sweep, reclaim, allocation
  retry, epoch ownership, or new source failure operation; and
- deterministic output across repeated emission.

Malformed plan tests should cover missing functions, duplicate or unordered
locals, selected primitive locals, omitted reference-bearing locals, invalid
types, and a callback without the required Stage 2 type helper.

Generated-C structural tests should assert that no textual
`sao2_local_N` occurrence remains for selected local `N`, while unrelated
non-root locals retain their current spelling and initialization.

### Native shadow-frame probe

Build the probe from the production frame declarations, callbacks, trace
runtime, layout registry, and arena runtime. It may explicitly initialize a
trace context, visit globals and frames, and drain work; ordinary generated
programs may not.

Cover:

- an empty chain and no-op global-root enumeration;
- one frame with a root struct and a primitive-only sibling local;
- nested frames whose roots lead to distinct heap owners;
- recursion with several instances of one frame type;
- a struct parameter copied after linking;
- tuple and active-union roots in frame fields;
- a tag-zero union and all-zero struct reference;
- root and unaligned interior references;
- a scoped frame root which reaches a heap child;
- two frames exposing different members of one heap owner;
- overwriting a canonical slot and tracing under a fresh epoch;
- unlinking an inner frame and proving its unique owner is absent from the next
  epoch while outer roots remain;
- LIFO unlink validation and a cycle in valid test frame nodes;
- sticky trace failure stopping later frame callbacks without changing links;
- frame-chain preservation across successful and failed enumeration;
- scoped restoration only after the owning frame is unlinked; and
- a clean empty chain before arena release.

Use current-epoch equality rather than expecting older mark fields to be
cleared. A root absent from a later pass may retain its previous numeric mark
until Stage 4 sweeps or Stage 5 resets epochs.

### Public pipeline regressions

Retain all Stage 1 and Stage 2 probes and run ordinary programs combining:

- struct-valued parameters, results, locals, and temporaries;
- tuple and union carriers containing struct references;
- direct, nested, and recursive calls;
- root and interior aliases;
- inline replacement and referenced rebinding;
- heap and scoped allocations in the same invocation;
- branches, loops, and early returns; and
- unit and integer entry adapters with and without arguments.

Observable output, identity, mutation, failure locations, exit status, scoped
reuse, heap placement, and diagnostics must remain unchanged. Generated trace
and frame machinery remains inactive unless the native probe invokes it.

## Completion gate

Stage 3 is complete when:

- every reference-bearing IR local has exactly one canonical typed frame field;
- every non-reference-bearing local remains ordinary C storage;
- every framed invocation links a completely zero-safe frame before root
  parameter copies and unlinks it on every normal return;
- generated callbacks enumerate each frame field using its exact static type;
- the active chain can be traversed without inspecting the native stack;
- nested calls, recursion, scoped roots, interior roots, tuples, and unions
  produce the expected Stage 2 trace items and heap marks;
- call arguments and return channels are safe under the documented
  allocation-only safe-point model;
- no decoded body pointer or unrooted backend temporary crosses a future safe
  point;
- a normal host return proves the chain is empty before arena release;
- no ordinary allocation triggers tracing or reclaims a block;
- all prior allocation, trace, language, and diagnostic behavior is unchanged;
- direct and native coverage passes on the available platform matrix; and
- no generated artifact or new dependency is introduced.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must use Rust 1.90 or newer,
report native skips explicitly, and confirm that the working tree contains
only intended source and documentation changes.
