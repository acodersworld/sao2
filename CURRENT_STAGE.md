# Current Stage: Iteration and Structural Mutation Guards

Status: current.

This document expands Stage 4 of
[Current Milestone: Containers](CURRENT_MILESTONE.md). Stages 1-3 established
managed list and ordered-map storage, exact roots and tracing, transactional
growth, alias-preserving identity, and complete non-iteration operations. This
stage renders the existing iteration IR and makes the control objects' shared
lock counts enforce iteration safety through every alias.

List iteration yields values in logical index order. Map iteration yields keys
in insertion order. No iterator object is added to the language or runtime:
lowering already stabilizes the container once, snapshots its length, advances
an integer index, and emits explicit begin/value/end operations.

## Outcome

At the end of this stage, source programs can:

- iterate over empty and non-empty lists and maps;
- observe list values in index order and map keys in insertion order;
- nest iteration over different containers or the same container;
- use `break`, `continue`, ordinary fallthrough, `return`, and `?` propagation
  without leaking iteration locks;
- replace list elements and existing map values while iterating;
- retain copied loop-binding values according to ordinary value/reference
  semantics; and
- receive a source-attributed panic when any alias attempts a structural
  mutation of an actively iterated container.

The control flow remains explicit:

```text
evaluate iterable once into canonical rooted temporary
                         |
                         v
                 BeginIteration
                 lock_count += 1
                         |
                         v
                  snapshot len()
                         |
             +-----------+-----------+
             | index < snapshot       |
             v                        |
       IterationValue                 |
       execute body                   |
       advance index -----------------+
             |
             v
       cleanup block
       EndIteration
       lock_count -= 1
             |
             v
            exit
```

Nested iterations add nested lock ownership to the same shared counter. The
counter records active loop scopes, not aliases or backing references.

## Preserved contracts

Stage 4 must preserve:

- `DESIGN.md` iteration order, value semantics, map insertion semantics, and
  structural-mutation rules;
- the existing `for binding in container` syntax and static binding types;
- left-to-right, exactly-once evaluation of the iterable expression;
- the packed `sao2_ref` carrier and canonical shadow-frame rooting;
- stable controls, replaceable managed backings, exact tracing, and stationary
  references;
- the Stage 1 control layouts and their `uint64_t lock_count` fields;
- Stage 2 list replacement, append, removal, growth, and index behavior;
- Stage 3 ordered-map lookup, replacement, insertion, removal, growth, and
  compaction behavior;
- source attribution for rejected structural mutation at the attempted
  mutation, never at the loop header;
- panic-as-process-termination without language-level unwinding;
- deterministic generated C and unchanged execution for all previously
  accepted programs; and
- dependency-free compilation and the existing platform runtime lifecycle.

Iteration introduces no allocation safe point by itself. Allocations in the
loop body continue to use the ordinary managed runtime.

## Stage boundaries

This stage does not add:

- first-class iterator values, iterator protocols, ranges, enumeration, or
  user-defined iterables;
- iteration over strings, tuples, structs, or any type other than lists/maps;
- mutable loop bindings that write through to a container slot;
- entry-pair or value iteration for maps—maps yield keys only;
- reverse iteration or a source-visible order/capacity API;
- exception unwinding, deferred cleanup, destructors, or recovery from panic;
- concurrent iteration, mutation, or thread safety; or
- the broad recursive stress and diagnostics work assigned to Stages 5-6 and
  milestone 12.

No container capability gate remains after this stage, but the later hardening
and integration stages are still required before milestone 11 closes.

## Iteration semantics to make explicit

The existing design implies these rules; record any wording needed in
`DESIGN.md` before tests depend on subtle cases:

1. The iterable expression is evaluated exactly once before its lock is
   acquired and remains rooted for the entire loop.
2. The loop length is snapshotted after locking. Structural mutation is
   forbidden, so the snapshot remains equal to the live logical length.
3. A list binding receives the value currently stored at its index when that
   iteration begins. Replacing a not-yet-visited element changes the value a
   later iteration observes.
4. A map binding receives the key at the corresponding insertion-order entry.
   Existing-value replacement cannot change yielded keys or order.
5. The binding is an ordinary copy: inline values copy, while object carriers
   preserve identity and aliasing.
6. List indexed replacement and existing-key map value replacement are
   non-structural and permitted while locked.
7. List append/removal and map insertion/removal are structural and rejected
   through every alias while any iteration is active.
8. Nested iteration, including over the same object, is permitted and increments
   the shared checked lock count once per active loop scope.
9. `continue` retains the current loop's lock; `break`, fallthrough, normal
   return, and error propagation release every loop scope they exit.
10. Explicit panic and unhandled-error panic terminate the process and do not
    perform language-level lock cleanup.

Replacement of the currently bound slot does not retroactively alter an inline
binding value already copied for the body. A binding that is itself an object
reference continues to alias that object under the ordinary reference rules.

## Existing IR contract

Render the existing operations without introducing a runtime iterator value:

```text
BeginIteration { iterable }
IterationValue { destination, iterable, index }
EndIteration { iterable }
```

The iterable must be the same unprojected, stabilized temporary operand at all
three operation kinds. Its type determines behavior:

- `[T]` makes `IterationValue` produce `T`; and
- `{K: V}` makes `IterationValue` produce `K`.

The index remains `int`. Begin and end return no value and have no source
failure site. Counter overflow, underflow, inactive value access, changed
length, invalid index, wrong layout, and mismatched cleanup are compiler/runtime
invariants, not user-triggerable language failures.

Keep the existing lowering shape:

- `BeginIteration` precedes the length snapshot and control-flow header;
- `IterationValue` occurs at the start of each body execution;
- the advance block performs checked integer increment;
- normal exhaustion and `break` converge on the cleanup block;
- `continue` targets the advance block without ending iteration;
- return and `?` propagation emit active cleanup operations from innermost to
  outermost before their terminator; and
- the normal cleanup block emits one `EndIteration` before entering the exit.

Do not lower iteration into repeated public indexing calls with artificial
source bounds failures. Iteration state which contradicts valid lowering is an
invariant.

## Control-flow validation

The current type checks are insufficient: individually well-typed begin/end
operations could still leak or underflow a lock. Add a function-level forward
dataflow validation over reachable basic blocks.

The abstract state is an ordered stack of active iterable operands. Transfer
rules are:

- `BeginIteration` requires an unprojected container temporary, verifies that
  it is not already invalidated by a write, and pushes it;
- `IterationValue` requires its iterable to equal the top active operand and
  validates its destination/index types;
- `EndIteration` requires exact equality with the top operand and pops it;
- a direct overwrite of the carrier local for any active iterable is invalid
  IR; projected mutation still follows the structural rules below;
- ordinary operations preserve the stack;
- jumps, branches, and switch edges propagate the complete stack; and
- every reachable join requires exactly the same ordered stack from all
  predecessors.

Normal `Return` requires an empty stack. `Panic`, `ErrorPanic`, and
`Unreachable` may terminate with active iterations because execution cannot
continue. Backedges must converge on the same state; placing a begin operation
inside its own backedge therefore fails rather than increasing abstract depth
without bound.

Reject, with full function/block/operation context:

- end without begin;
- end of the wrong operand or in the wrong nesting order;
- iteration value without the matching active loop;
- overwriting the stabilized iterable local while active;
- a normal exit with remaining locks;
- unequal active stacks at a control-flow join; and
- a cleanup which is reachable both locked and unlocked.

This validation proves compiler-generated balance independently of runtime
checks. It does not attempt alias analysis: exact stabilized operands establish
static ownership, while the control object's counter establishes dynamic alias
enforcement.

## Capability boundary

Enable:

- `BeginIteration`, `IterationValue`, and `EndIteration` for lists and maps;
- iteration operands and results nested through ordinary tuple, union, struct,
  list, and map storage;
- existing `IterationUnlocked` checks for list append/removal and map removal;
  and
- the conditional lock check already performed by missing-key map insertion.

Keep non-structural replacement free of an unconditional lock check. A map
store must look up the key first: replacement proceeds while locked, whereas a
missing-key insertion panics before allocation or mutation.

Printing containers and all out-of-scope language features remain rejected by
their existing independent capability checks.

## Typed lock helpers

Generate begin/end helpers for each concrete list and map type. Both helpers
resolve and validate the stable control afresh; neither retains a decoded
pointer after returning.

Begin performs:

1. Resolve the carrier with the exact control descriptor.
2. Validate the complete published container shape.
3. Reject `UINT64_MAX` as counter overflow via compiler invariant.
4. Increment `lock_count` exactly once.

End performs:

1. Resolve and validate the same control.
2. Reject zero as counter underflow via compiler invariant.
3. Decrement `lock_count` exactly once.

The increment/decrement is not a source-level arithmetic operation and has no
failure site. The runtime is single-threaded; ordinary checked reads/writes are
sufficient and no atomic ABI is introduced.

Nested loops over aliases resolve to the same stable control and therefore
share the count. Iterating distinct objects, even of the same concrete type,
updates distinct controls.

## Iteration value helpers

Generate a typed, non-allocating value helper for each concrete container type.
It must:

- resolve the control on every call;
- require `lock_count > 0`;
- require a nonnegative index representable as `uint64_t` and strictly below
  live length; the surrounding validated loop condition establishes the same
  bound against the snapshot;
- resolve the currently published backing with its exact descriptor;
- copy the selected language value immediately into the canonical destination;
  and
- discard every decoded pointer before returning to the loop body.

For a list, select `records[index].value` from the element backing and copy via
the element descriptor. For a map, select `records[index].key` from the dense
ordered-entry backing and copy via the key descriptor. Never derive map order
from lookup slots or repeat a hash lookup during iteration.

The helper must not call the public list-index or map-lookup panic path. If the
index or published shape is invalid while locked, call the compiler-invariant
path because valid lowering and mutation guards make that state impossible.

## Snapshot and mutation visibility

Lowering continues to call the ordinary typed `len()` immediately after begin
and stores the result in an `int` temporary. Since every structural mutation
consults the shared control, live length and the snapshot cannot diverge during
valid execution.

Non-structural operations remain observable according to their timing:

- replacing a future list element changes the later binding copy;
- replacing the current or earlier list element does not change a binding copy
  already produced;
- replacing a map value does not change any yielded key;
- mutating an object reached through a list element binding is ordinary alias
  mutation and does not change list structure; and
- assigning through an alias uses the same rules as assigning through the
  expression named in the loop header.

No cached native backing pointer or entry pointer spans execution of the body.
This remains true even though structural mutation is locked, because body calls
may allocate and collect unrelated objects.

## Structural mutation enforcement

Use the existing source-attributed paths:

- list `append` checks before either spare-capacity write or growth;
- list `removeIndex` checks before index normalization or shifting;
- map `removeKey` uses its adjacent `IterationUnlocked` check and validates
  again at the typed mutation boundary as already required;
- map indexed assignment first looks up the key, permits existing-value
  replacement, and checks before missing-key insertion; and
- operations reached through aliases resolve the same control counter.

The stable panic reason remains
`container structurally modified during iteration`. Its failure operation and
source location remain those of the attempted append, removal, or insertion.
The failed operation must leave length, capacities, backings, entries, slots,
initialized counts, and values unchanged.

Checks should remain at typed mutation helpers even when an adjacent IR check
exists. IR validation proves compiler sequencing; helper validation protects
the runtime boundary and conditional map-insertion case.

## Cleanup behavior

Cleanup is lexical and explicit, not an unwind mechanism.

### Fallthrough and exhaustion

The false header edge enters the loop's cleanup block, ends exactly that
iteration, then reaches the loop exit. Empty containers follow this path after
beginning and snapshotting, so even a zero-iteration loop balances its lock.

### Break

`break` jumps to the current loop's cleanup block. In nested loops it ends only
the innermost loop named by the existing semantic target. The outer lock stays
active until its own cleanup.

### Continue

`continue` jumps to the advance block. It must not emit an end/begin pair or
temporarily unlock the container between iterations.

### Return and error propagation

A normal `return` and a propagating `?` emit `EndIteration` for every exited
active loop from innermost to outermost before returning. The return value or
error payload is evaluated and stabilized before cleanup, preserving source
evaluation order and keeping structural checks active during that evaluation.

### Panic

Explicit panic, unhandled-error panic in `main`, and runtime panic terminate the
process. They do not run end operations, and no subsequent code can observe the
remaining counts. Do not add `setjmp`, host unwinding, or cleanup callbacks.

## Rooting and garbage collection

The stabilized iterable is a canonical container temporary and therefore a
precise shadow-frame root for the full loop. The loop binding is an ordinary
canonical local and is traced whenever its type contains references.

Body allocations may collect on every iteration. Collection must retain:

- the iterable control through its stabilized carrier;
- the published current backing(s) through the control callback;
- any current binding references through the function frame; and
- independently live aliases through their normal roots.

Iteration helpers re-resolve packed references after the body and never depend
on a pointer retained from a prior iteration. The lock count is scalar control
metadata and adds no trace edge.

## Implementation batches

Each batch leaves the generated subset coherent and testable.

### Batch 1: Semantic clarification and IR balance validation

- Make replacement visibility and nested-lock rules explicit in `DESIGN.md` if
  required.
- Add active-iteration stack dataflow validation across the function CFG.
- Require stabilized unprojected temporary operands and reject active-local
  writes, mismatched joins, and unbalanced normal exits.
- Expand malformed-IR and lowering-cleanup tests.

Gate: accepted IR has statically balanced, properly nested iteration scopes on
every normal reachable path.

### Batch 2: List iteration runtime

- Generate typed list begin/end/value helpers.
- Enable list iteration capability and render its three IR operations.
- Cover empty, single, multi-value, nested, aliased, and reference-bearing list
  loops.

Gate: lists iterate in index order with exact roots and balanced shared locks.

### Batch 3: Map iteration runtime

- Generate typed map begin/end/value helpers over ordered entries.
- Enable map iteration capability without consulting lookup-slot order.
- Cover collisions, replacements, removals/reinsertions before iteration,
  nested keys, and aliases.

Gate: maps yield keys in permanent insertion order independently of hash-table
layout.

### Batch 4: Cleanup exits

- Exercise exhaustion, empty loops, `break`, `continue`, nested exits, ordinary
  return, and `?` propagation.
- Verify reverse-order cleanup for nested loops and no cleanup for terminating
  panic paths.
- Demonstrate that mutation succeeds after every normal exited scope.

Gate: no normal source control-flow path leaks, duplicates, or prematurely
releases a lock.

### Batch 5: Mutation matrix

- Prove list replacement and existing map-value replacement remain permitted.
- Reject list append/removal and map insertion/removal through direct and
  aliased references.
- Exercise nested counts and helper calls which receive aliases.
- Confirm failed mutations are unchanged and report the attempted operation.

Gate: structural classification is consistent across every public mutation
path and every alias.

### Batch 6: Collection and integration hardening

- Force body allocations and collections while iterables and bindings remain
  live.
- Exercise nested/reference-bearing containers, calls, recursion, branches,
  switches, and epoch rollover during loops.
- Confirm deterministic C and all Stage 1-3 regressions.

Gate: iteration stays exact under collection and the complete public container
surface is executable.

## Verification map

### Semantic and lowering tests

Cover:

- list element and map key binding types;
- iterable evaluation exactly once;
- binding scope and shadowing;
- fallthrough, empty, break, continue, return, and `?` cleanup shapes;
- nested loops over same and different containers;
- reverse-order active cleanup emission; and
- mutation authorization remaining distinct from runtime lock rejection.

### IR validation tests

Cover:

- non-container iteration operands and wrong destination/index types;
- projected or non-temporary iterable operands;
- iteration value before begin or for a non-top iterable;
- wrong-order, wrong-operand, duplicate, and missing ends;
- active iterable-local overwrite;
- locked/unlocked join disagreement;
- stable backedge convergence;
- normal return with active scopes; and
- permitted panic termination with active scopes.

### Generated-C tests

Assert meaningful ordering:

- begin precedes the length snapshot;
- value loading occurs before body code;
- continue reaches advance without end;
- exhaustion and break pass through end;
- nested return cleanup is inner-to-outer;
- each value helper resolves control and backing afresh;
- map iteration indexes ordered entries, never lookup slots; and
- no decoded pointer spans body execution or an allocation.

### Native runtime probes

Use production helpers for:

- zero, one, nested, and near-maximum lock counts;
- begin overflow and end underflow invariants;
- nested aliases to the same control;
- invalid index, inactive value access, and corrupted published shape;
- typed list values and map keys across all supported representations;
- structural mutation rejection without partial writes;
- permitted non-structural replacement while locked;
- collection with locked controls and live binding references; and
- epoch rollover during repeated loop-body allocation.

### Public end-to-end programs

Cover observable behavior with programs that use:

- list order, map insertion order, empty loops, and nested loops;
- `break` and `continue` in conditional and nested bodies;
- early return and `?` propagation followed by caller-side mutation;
- same-container nested iteration;
- list replacement affecting a future binding;
- map value replacement without changing yielded keys;
- structural mutation attempted directly, through an alias, and through a
  called function;
- containers and object references as list elements; and
- tuple keys, command-line string keys, branches, switches, calls, and
  collection pressure inside loop bodies.

Native assertions may skip only when no supported C compiler is available.

## Completion checklist

Stage 4 is complete when:

- list values iterate in index order and map keys in insertion order;
- each iterable expression is evaluated once and rooted for the whole loop;
- begin/end update the stable shared checked lock count exactly once per scope;
- CFG validation rejects every unbalanced normal iteration path;
- fallthrough, empty loops, break, continue, return, and propagation have the
  specified cleanup behavior;
- nested same/different-container loops balance in strict lexical order;
- non-structural replacement remains permitted and has defined visibility;
- every structural mutation path rejects all aliases transactionally at its
  original source operation;
- body allocations and collections retain iterables and binding values exactly;
- all container execution capability gates are removed while unrelated gates
  remain intact;
- generated C remains byte-for-byte deterministic;
- all Stage 1-3 and earlier walking-skeleton programs remain unchanged; and
- the Stage 4 test matrix passes externally on Rust 1.90 or newer and available
  supported C compilers.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. Generated artifacts belong only under `build/` or
test temporary directories and must not be committed.
