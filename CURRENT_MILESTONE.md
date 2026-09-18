# Current Milestone: Diagnostics and Hardening

Status: current.

This document maps milestone 12 of `ROADMAP.md` into implementation stages.
It is the final implementation milestone in the v0 roadmap. Completing it
closes the remaining diagnostics, robustness, conformance, and measurement
gaps; the remaining roadmap item is then the v0 release gate rather than a
further feature milestone.

The compiler already has source spans, bounded error and warning collections,
typed runtime failure sites, distinct outer failure categories, deterministic
generated C, focused runtime probes, and broad end-to-end coverage. This
milestone completes and audits those mechanisms. It does not redesign the
language or add post-v0 features.

The walking skeleton remains executable throughout. Each stage must preserve
all accepted v0 programs, keep invalid source out of C emission, and retain
the distinction between source, compiler/toolchain, runtime-panic, and program
result channels.

## Fixed quality contract

Source diagnostics retain the original filename and half-open byte spans.
Rendered line and column numbers are one-based, tabs occupy four display
columns, primary and related annotations are deterministic, and errors and
warnings retain separate 20-entry limits. Recovery must make progress and may
not turn malformed input into a compiler panic.

Every source-attributed runtime panic names its reason, exact SAO2 filename,
line, column, and function without exposing a generated-C location. Failures
which occur before entry to a SAO2 function remain explicitly classified as
pre-entry runtime failures rather than receiving invented source provenance.
Internal runtime invariants remain visibly distinct from language panics.

Generated-C compilation is a compiler/toolchain failure, never a source
error. Starting or observing a built executable is a program boundary.
Subprocesses continue to use argument lists rather than shell commands, and
diagnostic output must tolerate non-UTF-8 host output without losing the
failure classification.

Robustness tests are deterministic and bounded by default. A reported fuzz
failure must include enough input or seed information to reproduce it. Native
and sanitizer checks may skip only when their required compiler or facility
is unavailable, and the unavailable coverage must be explicit.

No performance threshold becomes part of the language semantics. Measurements
record reproducible workloads and counters so regressions can be investigated;
they do not introduce a benchmark dependency or a timing-sensitive ordinary
test.

## Stage 1: Compile-time diagnostic model and rendering

Status: complete.

The detailed implementation plan is in
[Current Stage: Compile-time Diagnostic Model and Rendering](CURRENT_STAGE.md).

Extend the diagnostic representation from one primary span to a primary span
plus ordered related spans. Use related annotations for high-value conflicts
such as duplicate declarations, prior definitions, or incompatible sites,
without mechanically attaching noise to every message.

Audit diagnostic ordering, equal-span stability, tab and empty-span rendering,
line boundaries, end-of-file spans, unusual filenames, independent warning
and error limits, and warning-before-fatal output. Preserve the existing
recovery boundaries and ensure every frontend phase reports focused source
spans owned by the original source file.

Stage 1 is complete when representative lexer, parser, name, type, semantic,
and warning diagnostics have stable golden rendering, useful related context
where the compiler already knows it, and no regression in bounded recovery.

## Stage 2: Runtime panic provenance

Status: pending.

Audit every fallible typed-IR operation and every generated runtime helper
against the failure-site table. Complete source operation and function mapping
for arithmetic, conversions, indexing, allocation, containers, iteration,
I/O, explicit `panic`, and unhandled `Error` in `main`.

Make the runtime presentation contract uniform while keeping pre-entry
failures and internal invariants honest about their lack of a SAO2 operation.
Validate filenames with spaces or punctuation, nested calls, multiple failure
sites on one line, and all supported entry adapters. Generated C locations and
host addresses must not leak into language panic output.

Stage 2 is complete when every reachable language panic has an end-to-end
witness for its exact reason and SAO2 provenance, while pre-entry and invariant
failures remain separately recognizable.

## Stage 3: Failure boundaries and toolchain hardening

Status: pending.

Exercise and tighten the complete command boundary: input loading, frontend
failure, backend invariant failure, generated-C creation, C compiler discovery
and invocation, executable creation, program launch, program exit, and
`--show-c`. Preserve warnings, stdout/stderr ownership, exit status, and prior
artifact guarantees on every path.

Make missing, unlaunchable, failing, or falsely successful C compilers
actionable compiler diagnostics. Preserve captured host stdout and stderr and
distinguish a program's ordinary nonzero integer result from failure to start
or abnormal termination. Keep paths and arguments lossless at subprocess
boundaries.

Stage 3 is complete when the public CLI has deterministic tests for every
failure class and no user/source failure can be mistaken for a generated-C,
toolchain, or program failure.

## Stage 4: Malformed-input corpus and frontend fuzzing

Status: pending.

Add dependency-free, deterministic lexer and parser fuzzing over arbitrary
bytes and structured token mutations. Include truncation, delimiter damage,
operator ambiguity, invalid literals, comments, whitespace, and long recovery
sequences. Retain minimized readable malformed fixtures for important bugs and
grammar boundaries.

The harness must prove termination, bounded diagnostics, valid source spans,
stable reruns, and the absence of Rust panics for all generated inputs. Valid
generated fragments remain covered so hardening cannot simply reject difficult
syntax. Optional extended runs may use externally supplied seeds or iteration
counts without making the default suite expensive.

Stage 4 is complete when lexer/parser fuzz campaigns are reproducible in the
ordinary test harness, malformed grammar families have durable regression
fixtures, and failures print a directly reusable reproducer.

## Stage 5: V0 conformance and sanitizer matrix

Status: pending.

Build a requirements ledger for every normative rule in `DESIGN.md` and every
production in `GRAMMAR.ebnf`. Map each item to a focused unit, frontend, IR,
backend, native, or public end-to-end witness; add tests for genuine gaps and
record deliberately out-of-scope behavior without treating it as conformance.

Compile and run representative generated programs with the available C
compiler families and their address/undefined-behavior sanitizers. Include
primitive checks, value copying, structs and interior references, GC, recursive
container graphs, growth, iteration locks, panics, and all entry adapters.
Keep sanitizer flags out of ordinary generated programs and isolate compiler-
specific capability detection in the test harness.

Stage 5 is complete when every v0 design and grammar rule has an evidence
owner, the complete normal suite passes, and each available sanitizer/compiler
combination passes its representative native matrix with explicit skips for
unavailable combinations.

## Stage 6: Measurement, integration, and v0 closure

Status: pending.

Add stable instrumentation or harnesses for compiler phase time, generated-C
and executable build time, managed allocation and reclamation counts, peak
live storage, collection count, and collection work. Measure small, growing,
and GC-heavy public fixtures without changing their language-visible behavior.
Document the environment and workload alongside results rather than enforcing
fragile wall-clock limits.

Run the full diagnostic, malformed-input, conformance, native, sanitizer, CLI,
and deterministic-output suites as one release audit. Remove stale temporary-
stage wording and capability gates, confirm generated artifacts stay under
`build/` or test-owned temporary directories, and record any intentionally
unsupported post-v0 behavior.

Stage 6 and milestone 12 are complete when the evidence ledger has no
unresolved v0 rule, no known correctness defect remains, all available release
checks pass, measurements are reproducible, and `ROADMAP.md` can mark the v0
release gate complete.

## Cross-stage verification

Each stage adds evidence at the narrowest useful layer:

- diagnostic unit tests own rendering, ordering, limits, and related spans;
- lexer, parser, analysis, and semantic tests own recovery and source subjects;
- IR and backend tests own failure-site completeness and deterministic tables;
- native probes own runtime invariants, memory behavior, and injected failures;
- public end-to-end tests own CLI classification and observable panic text;
- fuzz tests own arbitrary-input termination and reproducibility; and
- the conformance ledger points to these tests instead of duplicating them.

`cargo test` remains the ordinary regression command. Optional compiler,
sanitizer, extended-fuzz, and measurement modes must be discoverable, bounded
when used in automation, and explicit when skipped.

## Milestone boundaries

The following remain outside milestone 12 and v0:

- new syntax, types, intrinsics, containers, modules, closures, FFI, or a
  standard library beyond `DESIGN.md`;
- runtime stack traces, interactive diagnostics, machine-readable diagnostic
  formats, localization, or terminal color;
- Unicode source/string semantics beyond the designed ASCII string model;
- an optimizing backend, incremental compilation, caching, or parallel builds;
- a general-purpose fuzzing dependency or hosted fuzzing service;
- hard performance guarantees or platform behavior not promised by the design;
- concurrent or moving garbage collection; and
- packaging, distribution channels, or post-v0 compatibility policy.

Contributor guidance authorizes compilation and tests while continuing to
prohibit formatting files. The milestone must remain dependency-free unless a
separate explicit decision documents a clear benefit.
