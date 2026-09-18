# Current Stage: Integration and Milestone Closure

Status: complete.

This document expands Stage 6 of
[Current Milestone: Containers](CURRENT_MILESTONE.md). Stages 1-5 completed the
container representation, list and ordered-map operations, entry arguments,
iteration locks, exact recursive tracing, transactional growth, allocator
hardening, failure classification, and epoch recovery. This final stage proves
that those pieces form one coherent public language and closes milestone 11.

Stage 6 should primarily add readable source fixtures, coverage accounting,
public CLI verification, deterministic-output checks, and documentation
cleanup. It must not replace the focused planner, IR, generated-C, and native
runtime probes which already establish private invariants.

## Outcome

At the end of this stage, the repository demonstrates through the public
source-to-executable path that SAO2 programs can:

- use lists and maps with every designed operation and valid stored type;
- pass, return, alias, nest, mutate, grow, iterate, and collect containers;
- combine containers with structs, tuples, unions, strings, entry arguments,
  calls, recursion, branches, switches, `while`, `for`, `break`, `continue`,
  return, and `?` propagation;
- retain value-copy versus reference-identity semantics across those features;
- execute useful algorithms whose working sets grow beyond initial capacity;
- survive repeated production garbage collections while preserving observable
  order and live identities;
- report compile-time, runtime, toolchain, and program-result failures through
  their distinct existing channels; and
- emit identical C for identical source metadata and compiler inputs.

Milestone closure is evidence-driven:

```text
DESIGN.md / GRAMMAR.ebnf / ROADMAP.md
                 |
                 v
       container conformance ledger
          /       |        \
         v        v         v
   focused tests  readable   CLI/platform
   and probes     programs   verification
         \        |         /
          +-------+--------+
                  v
       no uncovered designed behavior
                  |
                  v
     milestone 11 status -> complete
```

## Preserved contracts

Stage 6 must preserve:

- `DESIGN.md` as the semantic authority and `GRAMMAR.ebnf` as the syntax
  authority;
- the 8-byte packed reference ABI, stable managed layouts, stationary objects,
  precise roots, and exact trace keys;
- heap-managed container controls/backings and checked geometric growth;
- list index order and map insertion order;
- descriptor-based value copy, equality, hash, and trace behavior;
- source-attributed container failure operations and stable panic reasons;
- alias-wide iteration locks and balanced normal cleanup;
- entry arguments as an ordinary `[str]` with canonical strings;
- left-to-right evaluation, filenames, byte-oriented source locations, and
  deterministic generated C;
- argument-list child-process invocation and separation of source, compiler,
  toolchain, and program failures;
- dependency-free compilation; and
- all milestone 1-10 and Stage 1-5 tests.

Integration fixes must follow these contracts. Do not weaken a narrow invariant
or introduce a special-case source path merely to make a large fixture pass.

## Stage boundaries

This stage does not add:

- new syntax, types, collection kinds, methods, operators, or iterator forms;
- container printing, structural container equality, ordering, sorting,
  slicing, comprehensions, or capacity APIs;
- general strings beyond the designed ASCII/interned model;
- moving or concurrent GC, weak references, finalizers, or threads;
- dependencies, modules, closures, FFI, or a standard library;
- benchmark targets or performance promises; or
- the diagnostics presentation redesign, fuzzing, sanitizers, and measurement
  work assigned to milestone 12.

If closure reveals an actual semantic omission, stop and record an explicit
design change before implementation. Stage 6 is not permission to infer new
behavior from a convenient test program.

## Container conformance ledger

Build a checked-in test ledger, either as a compact section in the test module
or as comments beside the new fixtures, which maps each designed behavior to
at least one narrow test and one public integration path where useful. Avoid a
Cartesian explosion: use pairwise combinations and reserve native probes for
unobservable implementation facts.

### List behavior

Account for:

- non-empty and context/ascription-typed empty literals;
- left-to-right literal evaluation;
- `len()` at zero, after growth, replacement, and removal;
- positive and negative indexing at both valid boundaries;
- out-of-range reads, replacement, and removal;
- indexed replacement and compound assignment;
- append with spare capacity and repeated geometric growth;
- removal at front, middle, end, and by negative index;
- value membership for primitive, inline aggregate, union, and object identity
  values;
- identity equality distinct from equal contents;
- aliasing through assignment, parameters, results, aggregates, and nested
  containers;
- iteration order, nested iteration, and copied bindings;
- permitted element replacement during iteration; and
- rejected append/removal through direct and indirect aliases while locked.

### Map behavior

Account for:

- non-empty and context/ascription-typed empty literals;
- left-to-right key/value evaluation;
- duplicate literal keys using first position and last value;
- lookup, `len()`, membership, and identity equality;
- insertion versus replacement and compound assignment;
- stable position on replacement;
- removal, missing lookup/removal, and append-position reinsertion;
- repeated entry growth, slot growth, collision handling, and rehash;
- every valid key shape: unit, integer extremes, booleans, canonical strings,
  and nested immutable tuples;
- every v0 representation as a value;
- aliasing through every ordinary storage/call position;
- key iteration in insertion order independently of slot layout;
- permitted existing-value replacement during iteration; and
- rejected insertion/removal through direct and indirect aliases while locked.

### Storage and flow positions

Ensure the combined suite places container carriers and container-containing
values in:

- locals and mutable bindings;
- parameters and return values;
- struct root and interior fields;
- tuple fields;
- named and anonymous union alternatives;
- list elements and map values;
- loop bindings and narrowed union bindings;
- values live across recursive calls and collection safe points; and
- the `main(args [str])` entry parameter.

Account for unit, `int`, `float`, `str`, `bool`, `char`, tuples, unions,
structs, lists, and maps wherever each is valid. Map-key restrictions remain a
separate negative-typing matrix rather than being bypassed for coverage.

## Readable integration programs

Add a small set of named `.sao2` fixtures under `tests/fixtures/`. Prefer
several focused programs with documented expected output over one opaque mega-
fixture. Each should be understandable as a program independently of the
runtime mechanism it stresses.

### Argument frequency table

An entry function accepting `[str]` builds `{str: int}` frequencies in original
first-seen order, using membership, missing-key insertion, existing-value
compound assignment, and map iteration. Run it with zero, unique, duplicate,
and source-literal-equal arguments.

This fixture demonstrates:

- ordinary entry-argument lists;
- static/dynamic string canonical equality and hashing;
- ordered map insertion and replacement;
- string keys and indexed value access;
- argument forwarding through `run`; and
- deterministic observable output.

### Growing graph worklist

Represent an adjacency graph as `{int: [int]}`, traverse it with a growing list
worklist and `{int: bool}` visited map, and compute a stable result. Use a cursor
index rather than relying on an unprovided queue API.

This fixture demonstrates:

- nested map/list values and aliases;
- repeated list and map growth;
- membership, lookup, insertion, and indexing;
- loops, branches, calls, and returned containers;
- graph cycles without traversal nontermination; and
- live nested values across collection pressure.

### Small tag-system interpreter

Represent symbols as integers, production metadata with tuples or tagged
unions, the word with a growing list, and production payloads in an integer-key
map. Execute with a `while` loop and cursor. Include a case whose word grows
beyond its initial capacity and halts with a known result.

This fixture is the milestone's computation witness: finite source plus loops,
branching, and dynamically growing containers can implement an unbounded
abstract machine, subject only to classified managed allocation failure. Keep
the program compact and explanatory; it is not a performance benchmark.

### Recursive ownership graph

Retain one readable fixture which forms cycles crossing structs, lists, maps,
tuples/unions, and interior references, mutates/grows the graph, iterates it,
and prints stable values after allocation pressure. Reuse or split the existing
GC fixture rather than duplicating a second inscrutable stress program.

This fixture demonstrates the public consequence of Stage 5's exact native
probes without exposing collection internals.

## Cross-feature scenarios

The readable fixtures and smaller public tests should collectively cover:

- a function returning a list consumed by a map-building caller;
- a function returning a map stored inside a struct or union;
- recursive calls with live container parameters and results;
- switches/narrowing over unions carrying containers;
- `?` propagation from inside list and map loops, followed by caller-side
  mutation proving cleanup;
- nested iteration over the same and different containers;
- list replacement affecting a future loop binding;
- map value replacement leaving yielded keys unchanged;
- negative list indexing after multiple growth steps;
- tuple structural equality containing container identity members;
- equal-content but nonidentical list/map comparisons;
- command-line strings used as keys and values; and
- abandoned cyclic working sets followed by enough allocation to collect and
  reuse their storage.

Every expected output should express a language rule. Do not assert private
capacities, offsets, hash slots, or collection counts in a public fixture.

## Public CLI pipeline

Exercise the actual command boundary, not only direct compiler functions:

- `--help` reports the supported interface successfully;
- `build path/to/program.sao2` emits generated C under `build/`, invokes a
  supported compiler with argument lists, and produces an executable;
- `run path/to/program.sao2` builds and executes it;
- `run` forwards program arguments in exact order to `main(args [str])`;
- `--show-c` displays the same C used for compilation without changing program
  semantics;
- `SAO2_CC` selects a supported compiler executable without shell parsing;
- filenames containing spaces or punctuation remain single child-process
  arguments; and
- repeated build/run operations replace only their intended generated outputs.

Verify unit and integer entry results, stdout/stderr separation, program exit
status, and cleanup of temporary test directories. Generated artifacts may
remain only in the requested `build/` directory or test-owned temporary paths.

## Failure-channel integration

Retain narrow tests for each failure, then add enough CLI coverage to prove the
outer layers do not collapse them together.

### Source and semantic errors

Cover invalid container syntax/type combinations, heterogeneous literals,
untyped empty literals, invalid map keys, immutable mutation, invalid method
arguments, unsupported printing, and invalid entry signatures. These stop
before C emission or toolchain invocation and preserve source spans.

### Runtime program panics

Cover list bounds, missing map keys, structural mutation during iteration,
container allocation classification, explicit panic, arithmetic failure, and
unhandled `Error` in `main`. Confirm reason, filename, line/column, operation,
nonzero status, and the absence of compiler/toolchain wording.

### Compiler and runtime invariants

Malformed typed IR, impossible descriptor/layout states, and native corruption
probes remain compiler/runtime failures rather than source diagnostics. Public
valid source must never reach these paths.

### Toolchain and program results

A missing or failing configured C compiler remains a toolchain error. A
successfully built program returning a nonzero integer remains a program result,
not a compile failure. Preserve existing output/artifact behavior on failed
rebuilds.

Stage 12 may improve presentation, but Stage 6 must close all classification
and ownership boundaries.

## Capability and stale-stage audit

Audit compiler/backend rejection paths after full container enablement:

- every operation defined for valid v0 container source reaches rendering;
- no valid list/map type shape is rejected by a stale “later stage” gate;
- unsupported container printing and out-of-scope operations remain explicit;
- capability validation still follows typed IR validation;
- no direct/resolved-AST emitter path handles containers;
- no temporary entry-argument or host-backed container representation remains;
- comments naming completed future container work are updated or removed; and
- `unreachable!` cases correspond to validated invariants, not accepted source
  combinations lacking rendering.

Search names and comments as well as behavior. Preserve historical milestone
comments only where they accurately explain an architectural boundary.

## Deterministic generated C

For representative programs containing several concrete container and
aggregate types:

1. Build twice in fresh test directories with identical source metadata and
   compiler inputs.
2. Compare generated C byte-for-byte.
3. Compare descriptor, layout, callback, failure-site, root-frame, and helper
   ordering explicitly in focused tests.
4. Confirm build and run paths generate the same C before native invocation.

Determinism must not depend on hash iteration, allocator addresses, filesystem
enumeration, test order, or which supported C compiler will consume the output.
The filename is intentional diagnostic metadata, so cross-filename byte
identity is not required.

Probe-only macros and reduced stress configuration must not appear in ordinary
production C unless explicitly selected by the native probe harness.

## Platform and compiler matrix

External verification uses Rust 1.90 or newer and exercises, when available:

- a supported POSIX C compiler/runtime path;
- a supported Windows C compiler/runtime path;
- compiler selection through default detection and `SAO2_CC`;
- production 4-GiB virtual arena reservation for public programs; and
- reduced arena, allocation threshold, and epoch limits only in native stress
  probes.

Each available compiler must compile and execute the generated programs; a
compile error, warning promoted by the existing policy, abnormal termination,
wrong output, or wrong status is a failure. A native assertion may skip only
when no supported compiler is available for that environment, and the skip
must be reported rather than silently treated as coverage.

Do not pull milestone 12's sanitizer matrix or performance measurement into
this stage.

## Test organization and cost

Keep the suite useful for ordinary development:

- reuse compiler/native harness helpers instead of spawning shell commands;
- consolidate compiler discovery without hiding per-compiler failures;
- use deterministic bounded workloads which reliably cross growth/collection
  thresholds;
- keep fault injection and private-state assertions in native probes;
- keep public fixtures readable and focused on observable semantics;
- avoid duplicating the full Stage 5 stress matrix in every algorithm; and
- leave all generated artifacts in test temporary directories.

One integration fixture may cover several ledger rows, but every subtle rule
must retain a narrow regression test which localizes failures.

## Implementation batches

Each batch closes a distinct evidence gap while keeping the walking skeleton
green.

### Batch 1: Requirements ledger and stale-boundary audit

- Inventory every container rule from `DESIGN.md`, syntax from `GRAMMAR.ebnf`,
  and outcome from milestone 11.
- Map each rule to existing narrow evidence and identify only genuine gaps.
- Audit capability gates, temporary paths, comments, and unreachable renderer
  cases.
- Add missing semantic/IR/backend tests before broad fixtures mask a gap.

Gate: every designed container behavior has an owner and no accepted operation
depends on an obsolete stage boundary.

### Batch 2: Readable algorithm fixtures

- Add the argument frequency table, growing graph worklist, and compact
  tag-system interpreter.
- Give each fixture explicit arguments, output, and language-rule purpose.
- Factor test harness support only where multiple fixtures genuinely share it.

Gate: useful source programs exercise dynamic lists/maps through the public
pipeline and produce stable, comprehensible results.

### Batch 3: Cross-feature and GC integration

- Extend/rework the recursive ownership fixture rather than duplicating Stage
  5 probes.
- Fill storage/call/control-flow ledger gaps, including returns, narrowing,
  recursion, iteration cleanup, command-line strings, and abandoned cycles.
- Run bounded working sets large enough to trigger production collections.

Gate: containers compose with every v0 language subsystem while live graphs
remain exact under ordinary runtime pressure.

### Batch 4: CLI and failure boundaries

- Exercise build, run, show-C, argument forwarding, compiler selection, unusual
  filenames, exit status, and artifact behavior.
- Complete source/runtime/compiler/toolchain/program failure-channel coverage.
- Confirm every container panic retains its original source operation.

Gate: the public command boundary preserves semantics, diagnostics metadata,
and failure ownership end to end.

### Batch 5: Determinism and external matrix

- Compare generated C across clean repeated builds and build/run paths.
- Exercise available supported C compilers and both platform families in
  external verification.
- Confirm production and reduced-stress configurations stay separated and
  native skips are explicit.

Gate: identical compiler inputs produce identical C which compiles and runs on
every available supported target path.

### Batch 6: Documentation and milestone closeout

- Reconcile `DESIGN.md`, `GRAMMAR.ebnf`, `ROADMAP.md`, milestone/stage docs, and
  implementation terminology.
- Remove stale temporary/future-container commentary without erasing useful
  architectural history.
- Run the complete authorized verification commands and inspect artifact
  hygiene.
- Only after every completion criterion passes, mark Stage 6 and milestone 11
  complete and update the roadmap status/link for the next milestone workflow.

Gate: implementation, tests, and authoritative documents agree, with no
remaining milestone 11 work hidden in milestone 12.

## Verification commands

Final closure runs, without formatting files:

```text
cargo test
cargo run -- --help
cargo run -- build path/to/integration-program.sao2
cargo run -- run path/to/integration-program.sao2
```

Run additional supported C compiler selections through `SAO2_CC` where they
are available. The test suite remains the authoritative aggregate verification;
manual smoke commands supplement rather than replace assertions.

## Completion checklist

Stage 6 and milestone 11 are complete when:

- the conformance ledger covers every list, map, entry-argument, iteration,
  tracing, growth, and failure rule in the authoritative documents;
- readable programs demonstrate ordered maps, growing lists/maps, graph
  traversal, argument processing, and an unbounded abstract-machine pattern;
- all valid v0 stored representations and container key shapes are covered;
- containers compose through locals, aggregates, calls, returns, recursion,
  narrowing, loops, switches, and error propagation;
- identity, aliasing, value copying, ordering, mutation, and iteration behavior
  remain distinct and correct;
- repeated production collections preserve live recursive graphs and reclaim
  abandoned working sets according to existing focused probes;
- every failure remains correctly classified and source-attributed;
- no valid container program reaches an obsolete capability gate or temporary
  implementation path;
- CLI build/run/show-C, arguments, compiler selection, exit status, and
  artifacts behave as designed;
- identical source metadata and compiler inputs emit byte-identical C;
- all available supported platform/compiler paths pass, with only unavailable
  native compilers reported as skips;
- generated artifacts remain outside version control and dependencies remain
  unchanged;
- `cargo test` and the documented smoke commands pass on Rust 1.90 or newer;
- `CURRENT_MILESTONE.md` and `ROADMAP.md` mark milestone 11 complete only after
  the evidence above exists; and
- milestone 12 contains only its diagnostics/hardening remit, not deferred
  container correctness work.

Contributor guidance authorizes compilation and tests for this stage but
prohibits formatting files. Generated artifacts belong only under `build/` or
test temporary directories and must not be committed.
