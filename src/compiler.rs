use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::analysis;
use crate::c_backend;
use crate::diagnostic::{Diagnostic, Diagnostics, Warnings};
use crate::ir;
use crate::lowering::{self, LoweringError};
use crate::parser;
use crate::semantic::{self, SemanticResult};
use crate::source::SourceFile;

#[derive(Debug)]
pub(crate) struct CompileOutput {
    pub(crate) generated_c: PathBuf,
    pub(crate) warnings: Warnings,
}

#[derive(Debug)]
pub(crate) struct CompileFailure {
    pub(crate) error: CompileError,
    pub(crate) warnings: Warnings,
}

impl CompileFailure {
    pub(crate) fn exit_code(&self) -> i32 {
        self.error.exit_code()
    }
}

impl fmt::Display for CompileFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

#[derive(Debug)]
pub enum CompileError {
    Diagnostic(Diagnostic),
    SourceDiagnostics(Diagnostics),
}

impl CompileError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Diagnostic(diagnostic) => diagnostic.exit_code(),
            Self::SourceDiagnostics(_) => 1,
        }
    }
}

impl From<Diagnostic> for CompileError {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(diagnostic) => diagnostic.fmt(formatter),
            Self::SourceDiagnostics(diagnostics) => diagnostics.fmt(formatter),
        }
    }
}

/// Stable orchestration boundary from loaded SAO2 source through parsing,
/// analysis, semantic validation, typed-IR lowering, and IR-only C generation.
pub(crate) fn compile(source: &SourceFile) -> Result<CompileOutput, CompileFailure> {
    compile_into(source, Path::new("build"))
}

fn compile_into(
    source: &SourceFile,
    build_directory: &Path,
) -> Result<CompileOutput, CompileFailure> {
    compile_into_with_pipeline(source, build_directory, |_| {}, || Ok(()), |_| {})
}

fn compile_into_with_semantic(
    source: &SourceFile,
    build_directory: &Path,
    inject: impl for<'ast> FnOnce(&mut SemanticResult<'ast>),
) -> Result<CompileOutput, CompileFailure> {
    compile_into_with_pipeline(source, build_directory, inject, || Ok(()), |_| {})
}

fn compile_into_with_pipeline(
    source: &SourceFile,
    build_directory: &Path,
    inject_semantic: impl for<'ast> FnOnce(&mut SemanticResult<'ast>),
    inject_lowering_failure: impl FnOnce() -> Result<(), LoweringError>,
    inject_ir: impl FnOnce(&mut ir::Program),
) -> Result<CompileOutput, CompileFailure> {
    let program = parser::parse(source).map_err(|diagnostics| CompileFailure {
        error: CompileError::SourceDiagnostics(diagnostics),
        warnings: Warnings::new(),
    })?;
    let mut analysis = analysis::analyze(source, &program);
    if !analysis.diagnostics.is_empty() {
        return Err(CompileFailure {
            error: CompileError::SourceDiagnostics(analysis.diagnostics),
            warnings: Warnings::new(),
        });
    }
    let mut semantic = semantic::analyze(&mut analysis);
    inject_semantic(&mut semantic);
    if !semantic.diagnostics.is_empty() {
        return Err(CompileFailure {
            error: CompileError::SourceDiagnostics(semantic.diagnostics),
            warnings: semantic.warnings,
        });
    }
    if let Err(error) = semantic::validate_handoff(&analysis, &semantic) {
        return Err(CompileFailure {
            error: CompileError::Diagnostic(error),
            warnings: semantic.warnings,
        });
    }
    let mut ir_program = match lowering::lower(&analysis, &semantic) {
        Ok(program) => program,
        Err(error) => {
            return Err(CompileFailure {
                error: CompileError::Diagnostic(Diagnostic::compiler(format!(
                    "typed IR lowering boundary failed: {error}"
                ))),
                warnings: semantic.warnings,
            });
        }
    };
    if let Err(error) = inject_lowering_failure() {
        return Err(CompileFailure {
            error: CompileError::Diagnostic(Diagnostic::compiler(format!(
                "typed IR lowering boundary failed: {error}"
            ))),
            warnings: semantic.warnings,
        });
    }
    inject_ir(&mut ir_program);
    if let Err(error) = ir_program.validate() {
        return Err(CompileFailure {
            error: CompileError::Diagnostic(Diagnostic::compiler(format!(
                "post-lowering IR validation boundary failed: {error}"
            ))),
            warnings: semantic.warnings,
        });
    }
    let generated_c = match c_backend::emit(&ir_program) {
        Ok(generated_c) => generated_c,
        Err(error) => {
            return Err(CompileFailure {
                error: CompileError::Diagnostic(Diagnostic::compiler(error.to_string())),
                warnings: semantic.warnings,
            });
        }
    };

    if let Err(error) = fs::create_dir_all(build_directory) {
        return Err(CompileFailure {
            error: CompileError::Diagnostic(Diagnostic::compiler(format!(
                "cannot create build directory '{}': {error}",
                build_directory.display()
            ))),
            warnings: semantic.warnings,
        });
    }
    let output_path = build_directory.join("program.c");
    if let Err(error) = fs::write(&output_path, generated_c) {
        return Err(CompileFailure {
            error: CompileError::Diagnostic(Diagnostic::compiler(format!(
                "cannot write generated C file '{}': {error}",
                output_path.display()
            ))),
            warnings: semantic.warnings,
        });
    }
    drop(ir_program);
    Ok(CompileOutput {
        generated_c: output_path,
        warnings: semantic.warnings,
    })
}

#[cfg(test)]
fn compile_into_with_semantic_result(
    source: &SourceFile,
    build_directory: &Path,
    inject: impl for<'ast> FnOnce(&mut SemanticResult<'ast>),
) -> Result<CompileOutput, CompileFailure> {
    compile_into_with_semantic(source, build_directory, inject)
}

#[cfg(test)]
fn compile_into_with_invariants(
    source: &SourceFile,
    build_directory: &Path,
    inject_lowering_failure: impl FnOnce() -> Result<(), LoweringError>,
    inject_ir: impl FnOnce(&mut ir::Program),
) -> Result<CompileOutput, CompileFailure> {
    compile_into_with_pipeline(
        source,
        build_directory,
        |_| {},
        inject_lowering_failure,
        inject_ir,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Span;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }
    fn temporary_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sao2-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn valid_source_writes_program_c() {
        let source = source("fn main() { print(\"hello\"); }");
        let build_directory = temporary_directory("c-output");
        let output_path = compile_into(&source, &build_directory).unwrap().generated_c;
        let output = fs::read_to_string(&output_path).unwrap();
        assert!(output.starts_with("/* Generated by sao2. */\n"));
        assert!(output.contains("sao2_fn_0"));
        assert_eq!(output.matches("int main(void)").count(), 1);
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn primitive_source_writes_program_c() {
        let source = source(
            "fn main() { initial := 2; var value := initial * 3; value += 1; }",
        );
        let build_directory = temporary_directory("primitive-c-output");
        let output_path = compile_into(&source, &build_directory).unwrap().generated_c;
        let output = fs::read_to_string(&output_path).unwrap();
        assert!(output.contains("int64_t sao2_local_"));
        assert!(output.contains("sao2_check_integer_multiply"));
        assert!(output.contains("sao2_check_integer_add"));
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn repeated_compilation_has_deterministic_ir_backend_output() {
        let source = source("fn main() int { value := 6 * 7; println(value); value }");
        let build_directory = temporary_directory("lowered-identical-c");
        let first = compile_into(&source, &build_directory).unwrap();
        let expected = fs::read_to_string(first.generated_c).unwrap();
        let second = compile_into(&source, &build_directory).unwrap();
        assert_eq!(fs::read_to_string(second.generated_c).unwrap(), expected);
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn output_and_integer_result_write_program_c() {
        let source = source(concat!(
            "fn main() int { var value := 2; value *= 3; ",
            "print(\"value=\"); println(value); println(true); 7 }"
        ));
        let build_directory = temporary_directory("output-and-result");
        let output_path = compile_into(&source, &build_directory).unwrap().generated_c;
        let output = fs::read_to_string(&output_path).unwrap();
        assert!(output.contains("fwrite("));
        assert!(output.contains("fprintf(stdout, \"%\" PRId64"));
        assert!(output.contains("sao2_true_bytes"));
        assert!(output.contains("int64_t sao2_result = sao2_fn_0();"));
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn unsupported_printable_type_returns_backend_capability_diagnostic() {
        let source = source("fn main() { print(1.0); }");
        let build_directory = temporary_directory("unsupported-output");
        let diagnostic = compile_into(&source, &build_directory)
            .unwrap_err()
            .to_string();
        assert!(diagnostic.contains("current C backend"));
        assert!(!build_directory.exists());
    }

    #[test]
    fn analysis_errors_precede_backend_capability_checks() {
        for (text, expected) in [
            (
                "fn helper() { missing; } fn main() { print(\"x\"); }",
                "unknown value 'missing'",
            ),
            (
                "fn main() { print(\"one\", \"two\"); }",
                "print expects exactly one argument",
            ),
            (
                "fn main() { value := []; print(\"x\"); }",
                "empty list requires an expected list type",
            ),
        ] {
            let source = source(text);
            let build_directory = temporary_directory("analysis-error");
            let diagnostic = compile_into(&source, &build_directory)
                .unwrap_err()
                .to_string();
            assert!(diagnostic.contains(expected), "{text}: {diagnostic}");
            assert!(!diagnostic.contains("current C backend"), "{diagnostic}");
            assert!(!build_directory.exists());
        }
    }

    #[test]
    fn repeated_compilation_overwrites_deterministically() {
        let first_source = source("fn main() { print(\"first\"); }");
        let build_directory = temporary_directory("overwrite");
        compile_into(&first_source, &build_directory).unwrap();
        let second_source = source("fn main() { print(\"second\"); }");
        let output_path = compile_into(&second_source, &build_directory)
            .unwrap()
            .generated_c;
        let output = fs::read_to_string(output_path).unwrap();
        assert!(output.contains("sao2_string_data_0"));
        assert!(output.contains("UINT8_C(115), UINT8_C(101)"));
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn unsupported_source_does_not_truncate_existing_output() {
        let build_directory = temporary_directory("preserve-output");
        fs::create_dir(&build_directory).unwrap();
        let output_path = build_directory.join("program.c");
        fs::write(&output_path, "existing generated C").unwrap();

        let source = source("fn main() { print(1.0); }");
        let diagnostic = compile_into(&source, &build_directory)
            .unwrap_err()
            .to_string();

        assert!(diagnostic.contains("current C backend"));
        assert_eq!(fs::read_to_string(output_path).unwrap(), "existing generated C");
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn semantic_errors_precede_backend_validation_and_preserve_output() {
        for (text, expected) in [
            ("type Number(int); fn helper() {}", "requires one 'main'"),
            ("fn main() int {}", "may fall through without returning a value"),
            ("fn main() { 1 }", "unit-returning function cannot return a non-unit value"),
        ] {
            for existing in [false, true] {
                let build_directory = temporary_directory("semantic-error");
                let output_path = build_directory.join("program.c");
                if existing {
                    fs::create_dir(&build_directory).unwrap();
                    fs::write(&output_path, "existing generated C").unwrap();
                }

                let source = source(text);
                let diagnostic = compile_into(&source, &build_directory)
                    .unwrap_err()
                    .to_string();

                assert!(diagnostic.contains(expected), "{diagnostic}");
                assert!(!diagnostic.contains("current C backend"), "{diagnostic}");
                if existing {
                    assert_eq!(
                        fs::read_to_string(&output_path).unwrap(),
                        "existing generated C"
                    );
                    fs::remove_dir_all(&build_directory).unwrap();
                } else {
                    assert!(!build_directory.exists());
                }
            }
        }
    }

    #[test]
    fn mutability_errors_precede_backend_validation_and_preserve_output() {
        for existing in [false, true] {
            let build_directory = temporary_directory("mutability-error");
            let output_path = build_directory.join("program.c");
            if existing {
                fs::create_dir(&build_directory).unwrap();
                fs::write(&output_path, "existing generated C").unwrap();
            }

            let source = source("fn main() { values := [1]; values.append(2); }");
            let diagnostic = compile_into(&source, &build_directory)
                .unwrap_err()
                .to_string();

            assert!(diagnostic.contains("must be declared 'var'"), "{diagnostic}");
            assert!(!diagnostic.contains("current C backend"), "{diagnostic}");
            if existing {
                assert_eq!(
                    fs::read_to_string(&output_path).unwrap(),
                    "existing generated C"
                );
                fs::remove_dir_all(&build_directory).unwrap();
            } else {
                assert!(!build_directory.exists());
            }
        }
    }

    #[test]
    fn name_errors_prevent_semantic_invocation() {
        let source = source("fn main() {} fn main() {}");
        let called = std::cell::Cell::new(false);
        let result = compile_into_with_semantic_result(
            &source,
            &temporary_directory("semantic-not-called"),
            |_| {
                called.set(true);
            },
        );
        assert!(result.is_err());
        assert!(!called.get());
    }

    #[test]
    fn semantic_result_injection_carries_nonfatal_warnings() {
        let source = source("fn main() { print(\"hello\"); }");
        let build_directory = temporary_directory("injected-warning");
        let output = compile_into_with_semantic_result(&source, &build_directory, |result| {
            result.warnings.push(Diagnostic::source_warning(
                &source,
                Span::empty(0),
                "synthetic warning",
            ));
        })
        .unwrap();

        assert!(output.warnings.to_string().contains("synthetic warning"));
        assert!(output.generated_c.exists());
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn handoff_failure_precedes_backend_and_preserves_warnings_and_output() {
        for existing in [false, true] {
            let source = source("fn main() { return; after := (); }");
            let build_directory = temporary_directory("handoff-failure");
            let output_path = build_directory.join("program.c");
            if existing {
                fs::create_dir(&build_directory).unwrap();
                fs::write(&output_path, "existing generated C").unwrap();
            }

            let failure = compile_into_with_semantic_result(
                &source,
                &build_directory,
                |result| result.entry_point = None,
            )
            .unwrap_err();

            assert!(failure.to_string().contains("semantic handoff invariant"));
            assert!(!failure.to_string().contains("current C backend"));
            assert!(failure.warnings.to_string().contains("unreachable source"));
            if existing {
                assert_eq!(fs::read_to_string(&output_path).unwrap(), "existing generated C");
                fs::remove_dir_all(build_directory).unwrap();
            } else {
                assert!(!build_directory.exists());
            }
        }
    }

    #[test]
    fn lowering_invariants_are_compiler_failures_and_preserve_warnings_and_output() {
        let source = source("fn main() int { return 1; after := 2; }");
        let build_directory = temporary_directory("lowering-invariant");
        fs::create_dir(&build_directory).unwrap();
        let output_path = build_directory.join("program.c");
        fs::write(&output_path, "existing generated C").unwrap();

        let failure = compile_into_with_invariants(
            &source,
            &build_directory,
            || Err(LoweringError::Invariant("synthetic fn0 lowering context".to_owned())),
            |_| {},
        )
        .unwrap_err();

        assert!(failure.to_string().contains("compiler error"));
        assert!(failure.to_string().contains("typed IR lowering boundary"));
        assert!(failure.to_string().contains("fn0 lowering context"));
        assert!(failure.warnings.to_string().contains("unreachable source"));
        assert_eq!(fs::read_to_string(&output_path).unwrap(), "existing generated C");
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn post_lowering_validation_invariants_preserve_context_and_do_not_write() {
        let source = source("fn main() int { return 1; after := 2; }");
        let build_directory = temporary_directory("ir-validation-invariant");

        let failure = compile_into_with_invariants(
            &source,
            &build_directory,
            || Ok(()),
            |program| program.functions[0].blocks[0].terminator = None,
        )
        .unwrap_err();

        let rendered = failure.to_string();
        assert!(rendered.contains("compiler error"));
        assert!(rendered.contains("post-lowering IR validation boundary"));
        assert!(rendered.contains("invalid IR in fn0 bb0"));
        assert!(failure.warnings.to_string().contains("unreachable source"));
        assert!(!build_directory.exists());
    }

    #[test]
    fn real_unreachable_warning_is_nonfatal_for_a_supported_program() {
        let source = source("fn main() int { return 1; after := 2; }");
        let build_directory = temporary_directory("real-warning");
        let output = compile_into(&source, &build_directory).unwrap();
        assert!(output.warnings.to_string().contains("unreachable source"));
        assert!(output.generated_c.exists());
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn semantic_and_backend_failures_retain_prior_warnings_without_writing_output() {
        for (text, expected) in [(
            "fn main() { panic(\"stop\"); return 1; }",
            "unit-returning function cannot return a non-unit value",
        )] {
            let source = source(text);
            let build_directory = temporary_directory("warning-failure");
            let failure = compile_into(&source, &build_directory).unwrap_err();
            assert!(failure.warnings.to_string().contains("unreachable source"));
            assert!(failure.to_string().contains(expected), "{failure}");
            assert!(!build_directory.exists());
        }

        let source = source("fn main() { panic(\"stop\"); return 1; }");
        let build_directory = temporary_directory("warning-preserve");
        fs::create_dir(&build_directory).unwrap();
        let output_path = build_directory.join("program.c");
        fs::write(&output_path, "existing generated C").unwrap();
        let failure = compile_into(&source, &build_directory).unwrap_err();
        assert!(failure.warnings.to_string().contains("unreachable source"));
        assert_eq!(fs::read_to_string(&output_path).unwrap(), "existing generated C");
        fs::remove_dir_all(build_directory).unwrap();
    }
}
