# Current Stage: GC Integration and Milestone Closure

Status: current.

This document expands Stage 6 of
[Current Milestone: Garbage Collector](CURRENT_MILESTONE.md). It closes
milestone 10 by exercising the completed collector through the public language
pipeline, auditing generated C against the design boundaries, retaining
low-level proofs which source programs cannot observe, and documenting the
tracing seam needed by milestone 11 containers.

Stages 1 through 5 have already implemented the permanent runtime pieces:
reclaimable heap blocks, exact layout tracing, typed shadow frames,
mark-and-sweep collection, deterministic allocation policy, classified
failures, and safe epoch rollover. Stage 6 is integration and hardening. It
adds no new garbage-collection algorithm and no new language behavior unless
the audit exposes a specific discrepancy with `DESIGN.md`.

## Outcome

Milestone closure requires one continuous argument from source to reclaimed
storage:

```text
readable SAO2 program
        |
        v
parse -> analyse -> lower -> escape plan -> trace/root plans
        |
        v
deterministic generated C
        |
        +---- scoped object --------> function-owned bump lifetime
        |
        +---- escaping object ------> managed heap allocation
                                           |
                              policy or pressure safe point
                                           |
                               globals + typed frame chain
                                           |
                              exact owner/member traversal
                                           |
                                stationary mark and sweep
                                           |
                         live identity preserved / dead span reused
```

Public tests prove language-visible values, alias identity, mutation, calls,
returns, and failures. Backend and native probes prove internal properties such
as block boundaries, mark keys, frame order, trigger kind, free-list shape, and
epoch reuse. Neither layer substitutes for the other.

## Preserved contracts

Stage 6 must preserve all completed milestone contracts:

- 8-byte packed references with arena-tagged owner offsets and exact member
  offsets, never native pointers as language values;
- independent 4-GiB virtual heap and scoped arenas with on-demand commitment;
- stable addresses for every live heap and scoped object;
- escape-selected scoped lifetime and conservative heap placement whenever
  safety is not proven;
- exact typed tracing from generated globals and shadow-frame fields without
  scanning the native C stack;
- allocation-owner marking combined with exact-member traversal and exact-key
  deduplication;
- synchronous, non-moving, non-reentrant mark-and-sweep at managed heap
  allocation safe points only;
- deterministic proactive allocation debt plus pressure collect-and-retry;
- safe repeated epoch reuse, including failed trace attempts;
- transactional construction and allocation failure with existing source
  locations and diagnostic categories; and
- deterministic, dependency-free generated C.

Fix implementation defects discovered by integration, but do not silently
change language semantics. If a test expectation conflicts with `DESIGN.md` or
`GRAMMAR.ebnf`, follow the authoritative document or record an explicit design
change before modifying behavior.

## Stage boundaries

This stage does not add:

- lists, maps, container backing storage, iterators, or their trace callbacks;
- dynamic or heap-managed strings;
- conservative native-stack scanning or native-pointer language values;
- incremental, concurrent, generational, compacting, or moving collection;
- weak references, finalizers, resurrection, write barriers, or pinning;
- manual free, source-visible GC controls, thresholds, or statistics;
- collection or individual freeing of scoped allocations;
- liveness-sensitive frame slots, root push/pop instructions, or a changed
  collection safe point;
- new escape-analysis optimizations or context sensitivity; or
- broad refactoring unrelated to a demonstrated milestone-10 defect.

Container syntax and semantic analysis may already exist, but backend
capability rejection remains in force until milestone 11 implements storage,
operations, and traversal together.

## Public integration suite

### Test shape

Add a small set of readable end-to-end programs under the existing public
compiler test path. They must enter through SAO2 source and the normal
parse/analyse/lower/emit/host-compile/run pipeline. Do not build their IR by
hand and do not call collector helpers from the source program.

Keep each fixture focused enough that a wrong output identifies the broken
relationship. Prefer two or three complementary programs over one opaque
mega-program. Comments and names should explain which references remain live
and which graph is intentionally abandoned.

Use exact stdout and exit-status assertions. A supported C compiler is
required for these native end-to-end cases under the existing skip policy;
compile failure, abnormal termination, wrong bytes, or wrong status is a test
failure.

### Integrated graph program

Cover these language-visible shapes together:

- a heap-classified outer struct with two inline struct members and at least
  one referenced member;
- aliases to both distinct inline members of the same allocation;
- a root reference and an interior reference surviving several collections;
- replacement of one inline member while an alias to its stable slot remains;
- rebinding of a referenced member while an older separately rooted object
  remains valid;
- tuples and active union alternatives carrying struct references;
- inactive union payload storage contributing no root;
- branches and loops which change the currently reachable graph;
- mutation immediately before an allocation which can trigger collection;
- direct calls whose caller and callee frames both hold live roots; and
- unit, primitive, tuple, union, root-struct, and interior-struct function
  return shapes where the current language permits them.

Assert both values and identities. An interior alias must continue to compare
equal to the same embedded slot after inline assignment, while aliases to
distinct slots compare unequal. Live referenced children must preserve their
mutations across calls and collection.

### Cycles and changing roots

Construct cycles through recursive relationships whose recursion crosses a
referenced representation, as required by `DESIGN.md`. Include:

- a live self-cycle;
- a live multi-object cycle;
- a helper which creates a cycle and returns no reference, so unlinking its
  frame makes the complete cycle unreachable;
- a loop which repeatedly replaces its only long-lived graph root; and
- later allocation pressure sufficient to reclaim abandoned cycles and reuse
  their storage.

Do not claim that a value is unreachable merely because its last textual use
has passed. Invocation-wide frame fields may conservatively retain it until
the field is overwritten or the function returns. Structure the program so
unreachability follows from an overwritten canonical slot or an unlinked
helper frame.

Because raw addresses and collection counts are not language-visible, public
assertions should prove that live cycles survive and sustained allocation
finishes with exact output. Free-list reuse and dead-cycle reclamation are
confirmed by the native layer.

### Heap and scoped lifetimes together

Exercise escape analysis through ordinary source:

- a plainly local object which remains scoped;
- a local inline child passed to a non-escaping helper;
- an object returned directly and therefore heap-classified;
- an interior child returned from an enclosing object, forcing the complete
  owner to heap lifetime;
- an argument passed to a callee whose summary allows scoped placement;
- an argument passed through a recursive or unresolved call relation and
  therefore conservatively heap-classified; and
- a scoped object which contains or otherwise traces a reference to a heap
  child while collection occurs.

The program must demonstrate that scoped storage is restored only at its
owning function return, is never swept, and can keep heap children alive.
Backend plan assertions may confirm selected allocation classes, but the same
shapes must also execute through public source.

### Sustained production policy

At least one ordinary generated program must use the production arena size,
production threshold, and production epoch width. Allocate enough short-lived
heap-classified objects to cross the one-mebibyte policy threshold several
times while retaining a small live working set.

Choose the iteration count from a conservative upper bound on generated block
span so the source remains stable if harmless layout padding changes. Keep the
run bounded and fast; it should prove several proactive collections, not
approach the 4-GiB logical capacity.

The test must not override runtime constants, manually invoke collection, or
infer collection from timing. Exact completion and survival of deliberately
rooted values provide the public proof. Existing reduced-capacity and
reduced-epoch native probes continue to prove pressure and rollover paths.

## Function-boundary matrix

Audit ordinary C call handoff for every supported traceable return shape:

| Shape | Callee before return | Native handoff | Caller before next allocation |
| --- | --- | --- | --- |
| root struct | materialize return, unlink | packed `sao2_ref` | store in canonical frame field |
| interior struct | retain complete owner, unlink | packed `sao2_ref` | store exact owner/member pair |
| tuple with refs | materialize tuple, unlink | inline C value | copy into canonical frame field |
| union with ref payload | materialize active union, unlink | tagged inline C value | copy tag and payload into frame |
| unit/primitive | no traceable return root | scalar C value | ordinary local is sufficient |

No collection may occur after a callee unlinks and before the caller stores a
traceable return, because collection occurs only inside a later managed heap
allocation. Likewise, arguments are copied into the callee's zero-initialized
frame and the frame is linked before an operation can allocate.

Add generated-C assertions for this ordering and public programs for direct,
nested, recursive, early-return, and branch-return paths. Include a caller
root which must remain live while the callee allocates and a returned reference
which must remain live when the caller's next operation allocates.

## Decoded-pointer lifetime audit

Packed references may outlive safe points; decoded native body pointers may
not. Audit every generated declaration and expression which can hold an
`unsigned char *`, a typed body pointer, or an arena-base-derived address.

The allowed pattern for construction is:

1. evaluate all initializer operands into canonical locals;
2. call managed allocation, which may collect;
3. decode the newly returned reference;
4. initialize fields using only non-collecting helpers;
5. publish the packed reference to its canonical destination; and
6. let temporary body pointers leave lexical scope before any later operation.

Field reads, writes, projections, inline copies, equality, formatting, and
trace callbacks must either decode for one non-allocating expression/helper or
keep their native pointer within a region containing no managed allocation.
No raw body pointer may be stored in a language carrier, shadow frame, runtime
descriptor, static variable, or across an ordinary generated function call
which might allocate.

Review `render_struct_aggregate`, place/projection rendering, inline-copy
helpers, `sao2_resolve_body`, generated trace callbacks, and function return
cleanup. Add narrow emission assertions around ordering; avoid brittle
whole-function string snapshots unless byte-for-byte output itself is the
property being tested.

## Generated-code boundary audit

### Required structure

For representative mixed-lifetime programs, verify generated C contains:

- one descriptor and exact trace callback per traceable layout;
- typed frame declarations and callbacks for functions with traceable locals;
- zero initialization before link and argument copies after local creation;
- LIFO link/unlink on every normal return path;
- the explicit global-root hook followed by shadow-root enumeration;
- exact `(owner_ptr, member_ptr, layout)` trace deduplication;
- owner-header marking and exact-member callback dispatch;
- scoped traversal without scoped sweeping;
- deterministic policy and pressure triggers;
- rollover mark clearing before epoch reuse;
- linear sweep, coalescing, and address-ordered free-list rebuild; and
- arena release only after the entry frame chain is empty and collection is
  inactive.

### Forbidden structure

Audit source and representative output for the absence of:

- native stack-range discovery or conservative word scanning;
- rewriting live owner/member offsets, object movement, or compaction;
- source-visible manual free or collection entry points;
- write barriers, generations, finalizers, or weak-reference machinery;
- sweeping or individually reclaiming scoped blocks;
- raw native pointers in `sao2_ref` or generated language-value carriers;
- container backing allocation or list/map trace callbacks;
- a collector call from any operation other than managed heap allocation; and
- decoded construction pointers remaining live across another allocation.

Use structural assertions tied to function names, types, and call order. A
simple forbidden-word search is insufficient where standard C headers or
comments may legitimately contain the same vocabulary.

## Internal proof retention

Do not replace focused probes with broad source tests. Retain and, where an
integration defect reveals a missing case, extend coverage for:

- 48-byte header/body separation and requested-body bounds;
- packed owner tags, zero sentinel, alignment, and 32-bit offset limits;
- deterministic first-fit split, coalesce, tail trim, and free-list order;
- descriptor identity and callback dependency order;
- exact trace keys for two interior members sharing one owner;
- inactive unions, zero references, tuple payloads, and scoped-to-heap edges;
- newest-to-oldest frame enumeration and zero-safe inactive fields;
- owner marking distinct from exact-member traversal;
- iterative deep/cyclic traversal without native recursion;
- no sweep on scratch or trace failure;
- policy versus pressure trigger count and one-collection bound;
- commit, scratch, exhaustion, invalid-state, and reentrant results;
- epoch rollover at limits one, two, and a small non-power-of-two value;
- repeated split/coalesce and failed-trace rollover stress; and
- balanced scratch ownership, active guard, frame chain, and arena lifecycle.

Probe code must use the production runtime fragments rather than a parallel
allocator, tracer, or collector. Probe-only mutation and counters remain under
their existing test defines and must not affect production branches.

## Diagnostics and failure integration

Exercise failure paths at the highest practical layer:

- public source confirms a normal `StructAllocation` failure retains filename,
  function, line, column, operation category, and exact stable reason;
- native injection distinguishes heap commit failure from collector
  work-storage exhaustion;
- reduced capacity confirms genuine post-collection heap exhaustion;
- malformed runtime state and impossible reentrancy remain
  compiler/runtime invariants rather than source panics; and
- every failure leaves the caller's destination unpublished and all walkable
  runtime structures valid where recovery is defined.

Do not add source syntax solely to induce commit or collector-scratch failure.
Those are host/runtime conditions and belong in native probes.

## Determinism and capability boundaries

Emit identical C twice from the same validated program and compare bytes. Do
this for a representative integrated GC program, not only a primitive fixture.
Also confirm stable descriptor order, frame-field order, callback order,
failure-site indices, and layout identities.

Programs with no structs, only scoped structs, mixed lifetimes, and heap
lifetimes must each retain their intended runtime sections. Capability checks
must continue to reject executable container operations before C emission,
without partially generating container runtime code.

Generated artifacts belong under the test temporary directory or `build/`.
No emitted C, object file, executable, probe file, or captured output is
committed.

## Milestone 11 traversal handoff

Document and verify the extension seam without implementing containers:

1. a future container carrier becomes traceable in the type-carrying query so
   canonical frame planning includes container locals;
2. its backing storage receives a generated or runtime layout descriptor and
   stable identity;
3. value tracing enqueues the backing allocation from the container carrier;
4. the backing callback traces only initialized elements or entries using
   their exact element/key/value layouts;
5. the existing `(owner, member, layout)` visited set terminates cycles through
   containers and structs;
6. the existing allocation-owner mark retains complete backing storage;
7. container mutation remains safe under stop-the-world allocation points; and
8. any genuine static mutable container root is added through the explicit
   global-root hook rather than native-stack discovery.

The handoff must identify the relevant planner, renderer, descriptor, root,
and callback extension points in code comments or maintained documentation.
It must not predeclare an ABI which milestone 11 has not designed, nor weaken
the current backend rejection of container execution.

## Implementation sequence

Implement Stage 6 in this order:

1. Inventory existing public, backend, and native coverage against the stage
   matrix; preserve strong focused tests and list only genuine gaps.
2. Add readable public integration programs for aliases, mixed layouts,
   calls/returns, changing roots, cycles, mutation, and heap/scoped lifetimes.
3. Add the sustained production-threshold program and exact observable
   assertions without exposing GC controls.
4. Audit call handoff, cleanup, and decoded-pointer lifetimes; add structural
   emission tests and fix only demonstrated violations.
5. Audit required and forbidden generated-runtime boundaries plus capability
   gating and deterministic emission.
6. Extend production-runtime probes only for internal facts still unproved by
   the Stage 1-5 suite or exposed by an integration defect.
7. Verify diagnostic classification and transactional failure behavior at
   public or native level as appropriate.
8. Document the milestone-11 traversal seam, remove obsolete milestone-10
   temporary wording, and perform the closure audit.

Keep the source-to-executable walking skeleton working after each step. Do not
create a special source-visible collection intrinsic or duplicate runtime path
to simplify tests.

## Verification matrix

The completed stage must cover:

| Layer | Primary evidence |
| --- | --- |
| frontend and lowering | readable source reaches expected allocation, trace, and root plans |
| public native execution | exact output/status across repeated real collections |
| generated-C structure | safe ordering, typed roots, exact callbacks, no forbidden mechanisms |
| allocator probe | block walk, placement, coalescing, reuse, capacity, commit behavior |
| trace/frame probe | exact keys, owner/member semantics, frame order, scratch failure |
| collection probe | sweep transaction, policy/pressure, rollover, failure atomicity |
| determinism | identical source produces byte-for-byte identical C |
| platform lifecycle | reserve, commit, execute, frame-empty release on POSIX and Windows |

Only absence of a supported C compiler may skip native assertions. Every skip
must be explicit. A present compiler producing invalid C, an abnormal program,
a failed assertion, wrong output, or wrong exit status is a failure.

External completion verification must use Rust 1.90 or newer. Exercise the
production 4-GiB virtual reservations as well as reduced test configurations
on both platform families when available. Confirm virtual reservation does not
imply eager physical commitment.

## Documentation and closure

Before declaring the stage complete:

- reconcile runtime comments with the final Stage 5 policy and rollover;
- ensure `DESIGN.md` still describes the implemented packed-reference,
  lifetime, frame, trace, and sweep model exactly;
- ensure `GRAMMAR.ebnf` has not acquired GC-specific syntax;
- confirm `ROADMAP.md` milestone 10 outcomes are satisfied without importing
  milestone 11 container work;
- retain the milestone boundaries in `CURRENT_MILESTONE.md`;
- record any platform verification which cannot be performed locally; and
- inspect the working tree for generated artifacts and unrelated changes.

Mark Stage 6 and roadmap milestone 10 complete only after all in-scope evidence
passes. Starting milestone 11 and replacing the current milestone document is
a separate transition after this stage closes.

## Completion gate

Stage 6 and milestone 10 are complete when:

- readable public programs combine heap and scoped allocation, root and
  interior aliases, inline and referenced members, tuples, unions, mutation,
  control flow, calls, recursion, and supported return shapes;
- sustained production allocation triggers several collections while all
  deliberately reachable values and identities remain correct;
- abandoned acyclic and cyclic graphs are demonstrably reclaimed and their
  storage reusable without moving survivors;
- root changes across frames, calls, returns, branches, loops, and mutations
  are precisely reflected at the next allocation safe point;
- no decoded body pointer or unrooted native handoff crosses a possible
  collection;
- generated code contains every required precise-GC mechanism and none of the
  milestone's forbidden mechanisms;
- allocation, trace, frame, sweep, policy, rollover, failure, and lifecycle
  probes retain their focused guarantees;
- logical exhaustion, commit failure, collector scratch failure, source panic,
  and compiler/runtime invariants preserve their categories and locations;
- representative integrated programs emit byte-for-byte deterministic C;
- production and reduced configurations pass supported POSIX and Windows
  verification with skips reported explicitly;
- the container traversal extension point is documented without implementing
  container runtime behavior;
- no dependency, source-visible GC feature, or generated artifact is added;
  and
- authoritative documentation and implementation agree at milestone closure.

Contributor guidance prohibits compiling, running tests, or formatting while
implementing this stage. External verification must perform those steps and
report the exact platform/compiler coverage used for closure.
