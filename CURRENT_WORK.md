# Current Work: First End-to-End Executable

This document expands milestone 1 of `ROADMAP.md`. The immediate objective is
to compile and run the smallest possible SAO2 program through the complete
source-to-C pipeline:

```text
print("hello");
```

Top-level `print` is temporary walking-skeleton syntax. It must remain isolated
from the permanent language grammar and will be removed when real `fn main()`
support reaches the backend.

## Scope

The milestone includes:

- Reading one SAO2 source file
- Recognizing one top-level `print` statement containing one string literal
- Decoding the defined ASCII string escapes
- Producing a minimal C source file
- Invoking an installed C compiler
- Producing an executable
- Running the executable and forwarding its exit status
- Verifying the entire path with automated tests

The milestone does not include functions, static typing, general expressions,
the permanent parser, objects, containers, escape analysis, or garbage
collection.

## Phase 1: Repository and command skeleton

- Establish directories for compiler source, runtime source, tests, fixtures,
  and generated build artifacts.
- Create the compiler executable and argument parser.
- Provide these initial commands:

  ```text
  sao2 build program.sao2
  sao2 run program.sao2
  ```

- Validate input paths and report missing or unreadable files cleanly.
- Keep subprocess invocation independent of shell command construction so paths
  containing spaces are safe.

Exit criterion: both commands accept a path and reach a controlled placeholder
compiler stage.

## Phase 2: Minimal source reader and parser

- Load the source as UTF-8 and retain its filename and byte offsets.
- Recognize only `print`, parentheses, one string literal, a closing semicolon,
  whitespace, and comments.
- Decode `\\`, `\"`, `\'`, `\n`, `\r`, `\t`, `\0`, and `\xNN`.
- Reject non-ASCII decoded string contents.
- Reject trailing tokens and all unsupported constructs with a source-based
  diagnostic.
- Represent the accepted input with a tiny temporary node containing the
  decoded bytes and source span.

Exit criterion: the compiler accepts the example program and precisely rejects
malformed variants.

## Phase 3: Minimal C generation

- Generate one self-contained C translation unit with `main`.
- Store the decoded string as explicit bytes and write it with `fwrite`, avoiding
  C format-string interpretation and preserving embedded zero bytes.
- Return zero after successful output and a nonzero code if output fails.
- Keep generated names independent of user-controlled identifiers.
- Write generated C beneath a dedicated build directory.

Representative output:

```c
#include <stdio.h>

int main(void) {
    static const unsigned char text[] = {104, 101, 108, 108, 111};
    return fwrite(text, 1, sizeof(text), stdout) == sizeof(text) ? 0 : 1;
}
```

Exit criterion: the generated C is deterministic and compiles independently.

## Phase 4: Host C compiler integration

- Detect or configure a supported C compiler.
- Invoke it with an argument list rather than a shell command string.
- Select platform-appropriate executable names and output paths.
- Capture its stdout, stderr, and exit status.
- Report compiler absence and failure as toolchain/compiler errors rather than
  SAO2 source errors.
- Provide an option to retain or display generated C for debugging.

Exit criterion: `sao2 build` creates a runnable native executable from the
example source.

## Phase 5: Run command

- Make `sao2 run` perform the same build pipeline.
- Execute the resulting program without a shell.
- Forward program stdout and stderr.
- Return the program's exit status from the command.
- Keep build failures distinct from program failures.

Exit criterion: `sao2 run` prints `hello` with no added newline and exits with
status zero.

## Phase 6: Automated verification

- Add unit tests for string decoding and minimal syntax errors.
- Add snapshot tests for generated C and diagnostics.
- Add end-to-end tests that compile and execute fixtures.
- Cover empty strings, every escape, embedded zero bytes, comments, malformed
  input, output failure where practical, and paths containing spaces.
- Run the generated executable more than once to confirm deterministic output.

Exit criterion: one command runs all milestone tests reliably on the primary
development platform.

## Phase 7: Walking-skeleton handoff

- Document how later lexer, parser, IR, and backend stages replace each
  temporary component.
- Keep the end-to-end fixture as a permanent regression test.
- Mark temporary top-level `print` parsing clearly so it cannot accidentally
  become part of the public grammar.
- Confirm that the next roadmap milestone can replace the source reader and
  parser without changing C compiler or process-running interfaces.

Exit criterion: milestone 1 is complete and milestone 2 can begin without
breaking the source-to-executable path.

## Definition of done

Milestone 1 is complete when all of the following are true:

- `sao2 build hello.sao2` creates a native executable.
- The executable writes exactly the decoded string bytes.
- `sao2 run hello.sao2` builds and runs it successfully.
- Invalid input produces a SAO2 source diagnostic.
- Missing or failing C toolchains produce a distinct toolchain diagnostic.
- Generated C is inspectable for debugging.
- Automated end-to-end tests pass.
- Temporary syntax and components are clearly identified for later removal.
