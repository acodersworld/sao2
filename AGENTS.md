# SAO2 contributor guidance

SAO2 is a single-file language that compiles to C. Development follows the
walking-skeleton roadmap: keep the source-to-executable path working while
replacing temporary stages with permanent compiler components.

## Authoritative documents

- `DESIGN.md` defines the language semantics.
- `GRAMMAR.ebnf` defines the formal syntax.
- `ROADMAP.md` defines the implementation milestones.
- `CURRENT_WORK.md` defines the active milestone and its phase boundaries.

If implementation and documentation disagree, do not silently invent new
language behavior. Follow the design documents or update them as part of an
explicit design decision.

## Development commands

The compiler requires Rust 1.90 or newer and uses Rust edition 2024.

Do not compile the project, run tests, or format files.

```text
cargo test
cargo run -- --help
cargo run -- build path/to/program.sao2
cargo run -- run path/to/program.sao2
```

Set `SAO2_CC` to a C compiler executable when automatic detection is not
appropriate. Pass `--show-c` to `build` or `run` to print generated C.
Native end-to-end tests run automatically when a supported compiler is on
`PATH` or selected through `SAO2_CC`; only those native assertions are skipped
when no compiler is available.

Keep the compiler dependency-free until a dependency has a clear, documented
benefit. Generated compiler artifacts belong under `build/` and must not be
committed.

## Walking-skeleton constraints

- Keep temporary syntax and implementation stages visibly marked.
- Preserve filenames and byte-oriented source information for diagnostics.
- Invoke child processes with argument lists, never constructed shell commands.
- Keep source errors distinct from compiler/toolchain and program failures.
- Add tests when advancing or replacing a compiler stage.
