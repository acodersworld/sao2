# Current Stage: Compile-time Diagnostic Model and Rendering

Status: current.

This is Stage 1 of milestone 12, Diagnostics and Hardening. It completes the
compile-time diagnostic data model and presentation contract before later
stages audit runtime panics, toolchain boundaries, fuzzing, and conformance.

The compiler already snapshots one primary source excerpt in each source
diagnostic, sorts diagnostics by primary byte span, independently limits errors
and warnings to 20 entries, and renders warnings before fatal failures. This
stage preserves that behavior while adding related source annotations and
hardening the renderer at source boundaries.

No language syntax or semantics change in this stage. No runtime panic text,
host compiler diagnostic, IR failure-site table, or backend behavior belongs
to this stage except where a regression test proves compile-time errors still
stop before lowering and C emission.

## Completion outcome

Stage 1 is complete when:

- a source diagnostic owns one primary annotation and zero or more labeled
  related annotations without borrowing a `SourceFile`;
- diagnostics without related annotations retain their existing rendering;
- related annotations render deterministically with filename, line, column,
  source line, caret, and a concise relationship label;
- important duplicate-declaration diagnostics point back to the first accepted
  declaration or occurrence;
- source ordering is stable for equal primary spans and is unaffected by
  related-span positions;
- errors and warnings retain independent 20-entry limits;
- lexer, parser, name/type, semantic, and warning diagnostics have stable
  representative rendering tests;
- tabs, CRLF input, empty spans, multiline spans, end-of-line/end-of-file spans,
  Unicode byte boundaries, and unusual filenames render safely; and
- all existing tests plus the new diagnostic end-to-end cases pass without
  formatting files or adding dependencies.

## Fixed diagnostic model

Keep `DiagnosticKind` as the owner of category and exit behavior. Usage, input,
compiler, and program diagnostics remain message-only values. Source errors and
source warnings own source annotations.

Replace the primary-only private snapshot with one reusable private source-
annotation representation containing:

- the owned source path;
- the original half-open byte `Span` used for ordering and test inspection;
- one-based line and display column;
- the rendered source line;
- caret offset and width in display columns; and
- an optional annotation label.

A `Diagnostic` owns one optional primary annotation and an ordered vector of
related annotations. The diagnostic's main message labels the primary
annotation; every related annotation must have its own short relationship
label such as `previous declaration is here` or `first argument is here`.

Annotations snapshot their display data when the diagnostic is constructed.
They do not retain `SourceFile` references and do not look up source text while
formatting. Although v0 accepts one source file, each annotation owns its path
so the representation does not assume that primary and related locations will
always share a file.

Add a small construction API which keeps the common call sites concise:

- `Diagnostic::source` and `Diagnostic::source_warning` continue to create a
  primary-only diagnostic;
- a chainable related-annotation operation accepts a source, span, and label;
- related annotations can only be added to a source error or source warning;
  misuse by compiler internals is rejected by an assertion rather than silently
  dropping the annotation; and
- `primary_span()` retains its present meaning for ordering and existing tests.

Do not expose the renderer's private snapshot through the compiler pipeline.
Frontend phases should continue to create complete `Diagnostic` values rather
than passing a new diagnostic context object through analysis.

## Rendering contract

Preserve the current primary form exactly for a diagnostic with no related
annotations:

```text
sao2: source error: file.sao2:4:4: duplicate function declaration 'work'
  |
4 | fn work() {}
  |    ^^^^
```

Render each related annotation after the primary excerpt, in insertion order,
using the same path/location and excerpt machinery:

```text
  = related: file.sao2:1:4: previous declaration is here
  |
1 | fn work() {}
  |    ^^^^
```

The `related` prefix is presentation, not a new diagnostic kind. A related
annotation does not affect exit status, collection limits, diagnostic ordering,
or whether a warning is fatal.

Use one blank line only between complete diagnostics in a collection, as today.
Do not insert a blank line between a primary excerpt and its related excerpts.
Do not add color, terminal detection, Unicode box drawing, path shortening, or
machine-readable output.

For a span crossing lines, render and underline only the portion on its starting
line. An empty span receives a one-column caret. A span beginning at a newline,
after a CR in CRLF input, or at end of file must still produce a safe one-column
caret at the location selected by the source-location rules.

Tabs continue to expand to four display columns, not variable tab stops. Caret
offset and width must be derived by the same display-width rule as the rendered
line. Unicode remains valid in source files, but spans are byte-oriented; the
renderer must clamp defensive out-of-range or non-character-boundary offsets
without panicking. Frontend-produced spans are still required to be valid and
must not rely on clamping for ordinary operation.

## Related-span integration scope

Add related annotations where the frontend already encounters a later item and
can retain the first item's span without new semantic inference. At minimum,
cover:

- duplicate top-level type declarations;
- duplicate top-level function declarations;
- duplicate function parameter names;
- duplicate named struct members;
- duplicate tagged-union tags and duplicate `Error` alternatives;
- duplicate untagged-union alternatives, using the first source type which
  resolved to the same alternative; and
- duplicate named struct-constructor arguments.

The later occurrence remains the primary error because that is the point the
user must change. The first accepted occurrence is the single related
annotation. With three or more repetitions, each later occurrence points to
the same first occurrence rather than forming a chain.

Keep intrinsic-name reservation errors primary-only because the intrinsic has
no user source declaration. Do not manufacture related spans for missing names,
inferred expectations, synthetic nodes, non-exhaustive switches, or failures
whose counterpart is not already represented by a reliable source span.

Store `(identity, first_span)` while detecting duplicates rather than searching
the source text. For untagged union alternatives, key the first span by the
resolved `TypeId`; for constructor arguments, key by the resolved member index.
Retain current recovery and type-resolution behavior after reporting the
diagnostic.

## Ordering and limits

`Diagnostics` and `Warnings` continue to bound entries as they are pushed, so
later sorting cannot change which first 20 reports survive. Rendering and test
extraction use a stable sort by the primary span only. Equal primary spans
therefore preserve insertion order; related spans never participate in the
sort key.

Keep the error and warning collections separate throughout compilation. A
related annotation is part of its owning entry and consumes no additional
collection slot. Warning rendering remains before any later fatal compiler or
toolchain diagnostic and does not change the eventual exit status.

If helper logic is shared between `Diagnostics` and `Warnings`, keep their
public roles and limits explicit. Do not merge them into a severity-sorted
collection in this stage.

## Source excerpt hardening

Refactor `SourceFile::diagnostic_excerpt` only as needed to produce a complete
annotation snapshot consistently. Keep line starts precomputed from source
bytes and preserve one-based locations.

Add focused source/diagnostic tests for:

- the first byte, an interior empty span, and exact end of file;
- a span ending at and crossing `\n`;
- CRLF boundaries without rendering the carriage return;
- a tab before the caret and a tab inside the underlined span;
- multibyte UTF-8 before and within a byte span;
- an empty final line after a trailing newline;
- a multiline primary or related span;
- defensive offsets beyond the file and inside a UTF-8 code point;
- an empty file; and
- paths containing spaces and punctuation.

The excerpt renderer must not slice at an invalid UTF-8 boundary. Its returned
caret offset and width must always be at least one display column where a caret
is required.

## Frontend diagnostic coverage

Retain focused tests in the module which owns each behavior, then add a small
number of exact-rendering tests at integration boundaries.

Lexer coverage includes an invalid token or literal report with its
focused span and bounded repeated errors. Parser coverage includes one recovered
syntax error and proves later syntax errors remain source ordered. Analysis
coverage includes the duplicate categories above and validates both the primary
and first-occurrence spans. Semantic coverage includes an invalid `main`
signature and one unreachable warning so both fatal and non-fatal paths use the
same renderer.

The public CLI coverage should use a filename containing spaces or punctuation
and assert complete stable stderr for:

- one duplicate declaration with a related annotation;
- multiple recovered source errors in primary source order; and
- an unreachable warning printed before a later fatal failure.

Exact golden strings belong where presentation is the subject. Other frontend
tests should inspect messages and source spans without duplicating the complete
layout everywhere.

## Recovery and pipeline boundaries

Related annotations must not change lexer or parser synchronization, analysis
continuation, the 20-error cutoff, or which phase owns an error. Preserve the
existing rule that name/type diagnostics prevent semantic analysis, lowering,
and C generation, while semantic errors preserve any warnings already found.

Add or retain tests proving:

- malformed syntax produces multiple bounded diagnostics without a Rust panic;
- duplicate declarations do not enter lowering or overwrite an existing
  generated C output;
- warnings are emitted before a frontend, backend, or host-compiler failure;
- a diagnostic with related spans has the same exit code as its primary-only
  equivalent; and
- source diagnostics never acquire compiler/toolchain wording.

Do not expand this stage into Stage 4's arbitrary-input fuzz campaign. Small
table-driven malformed cases are appropriate here only when they validate the
renderer, recovery ordering, or diagnostic limit.

## Implementation batches

### Batch 1: Annotation data model

- Introduce the reusable owned source-annotation snapshot.
- Add ordered related annotations and the chainable construction API.
- Preserve `primary_span`, category, messages, exit codes, and primary-only
  rendering.
- Add unit tests for construction invariants and lifetime independence.

Gate: all current diagnostic tests pass unchanged before related rendering is
enabled at frontend call sites.

### Batch 2: Excerpt and related rendering

- Centralize primary and related excerpt rendering without changing the legacy
  primary layout.
- Implement the fixed related-annotation layout and insertion order.
- Harden line, CRLF, tab, empty, multiline, Unicode-boundary, and EOF handling.
- Lock representative output with exact strings.

Gate: every supported source boundary renders deterministically without panic,
and a primary-only diagnostic remains byte-for-byte compatible.

### Batch 3: Duplicate-origin integration

- Retain the first span in the top-level, parameter, struct member, union, and
  constructor duplicate detectors.
- Attach exactly one useful related annotation to each later duplicate.
- Keep intrinsic collisions and other single-site errors primary-only.
- Verify three-way duplicates consistently reference the first occurrence.

Gate: every in-scope duplicate category has a narrow test for primary span,
related span and label, and unchanged recovery behavior.

### Batch 4: Phase and collection audit

- Add representative lexer, parser, analysis, semantic, and warning rendering
  coverage.
- Verify stable equal-span ordering and independent error/warning saturation.
- Confirm related spans do not affect collection order or limits.
- Exercise warning preservation across later failures.

Gate: all frontend phases follow one presentation contract and the compiler
retains deterministic source ordering.

### Batch 5: Public pipeline and closure

- Add exact CLI stderr coverage with an unusual source filename.
- Prove invalid source stops before C emission and preserves prior artifacts.
- Run the full authorized suite and inspect generated-artifact hygiene.
- Reconcile comments and mark Stage 1 complete only after its completion
  checklist passes.

Gate: the public command boundary exposes the completed compile-time diagnostic
contract without changing successful program behavior.

## Verification

Run without formatting files:

```text
cargo test
cargo run -- --help
```

Use focused `cargo test` filters during the batches, then run the complete suite
for closure. Native C execution is not required for presentation-only cases,
but all existing native tests must continue to pass or explicitly report the
already-supported no-compiler skip.

Manual `build` or `run` smoke tests may supplement the assertions, especially
for inspecting stderr in a terminal, but checked-in automated tests own the
rendering contract.

## Out of scope

Defer the following to later Stage 12 work or post-v0:

- runtime panic provenance and generated failure tables (Stage 2);
- C compiler and executable failure presentation changes (Stage 3);
- arbitrary-input or long-running fuzz campaigns (Stage 4);
- the full design/grammar evidence ledger and sanitizer matrix (Stage 5);
- performance and GC measurement (Stage 6);
- runtime stack traces, suggestions/fix-its, terminal color, source elision,
  multi-line art, JSON output, localization, and warning policy expansion; and
- multi-file/module compilation, even though the annotation model must not
  prevent it.

Contributor guidance authorizes compilation and tests for this stage but
prohibits formatting files. Keep the compiler dependency-free and leave all
generated artifacts under `build/` or test-owned temporary directories.
