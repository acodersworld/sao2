use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::analysis::{self, Analysis};
use crate::c_emitter;
use crate::diagnostic::{Diagnostic, Diagnostics, Warnings};
use crate::parser;
use crate::semantic::{self, SemanticResult};
use crate::source::SourceFile;

#[derive(Debug)]
pub(crate) struct CompileOutput {
    pub(crate) generated_c: PathBuf,
    pub(crate) warnings: Warnings,
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
/// name-and-type analysis, and semantic analysis to generated C. The
/// analyzed-AST-to-C subset remains temporary until the typed backend replaces
/// it.
pub(crate) fn compile(source: &SourceFile) -> Result<CompileOutput, CompileError> {
    compile_into(source, Path::new("build"))
}

fn compile_into(
    source: &SourceFile,
    build_directory: &Path,
) -> Result<CompileOutput, CompileError> {
    compile_into_with_semantic(source, build_directory, semantic::analyze)
}

fn compile_into_with_semantic<F>(
    source: &SourceFile,
    build_directory: &Path,
    semantic_analyzer: F,
) -> Result<CompileOutput, CompileError>
where
    F: FnOnce(&mut Analysis<'_, '_>) -> SemanticResult,
{
    let program = parser::parse(source).map_err(CompileError::SourceDiagnostics)?;
    let mut analysis = analysis::analyze(source, &program);
    if !analysis.diagnostics.is_empty() {
        return Err(CompileError::SourceDiagnostics(analysis.diagnostics));
    }
    let semantic = semantic_analyzer(&mut analysis);
    if !semantic.diagnostics.is_empty() {
        return Err(CompileError::SourceDiagnostics(semantic.diagnostics));
    }
    let entry_point = semantic
        .entry_point
        .expect("a diagnostic-free semantic result has a valid entry point");
    let main = analysis.function_signature(entry_point.function_id());
    let generated_c = c_emitter::emit(source, &program, &analysis, main)?;

    fs::create_dir_all(build_directory).map_err(|error| {
        Diagnostic::compiler(format!(
            "cannot create build directory '{}': {error}",
            build_directory.display()
        ))
    })?;
    let output_path = build_directory.join("program.c");
    fs::write(&output_path, generated_c).map_err(|error| {
        Diagnostic::compiler(format!(
            "cannot write generated C file '{}': {error}",
            output_path.display()
        ))
    })?;
    Ok(CompileOutput {
        generated_c: output_path,
        warnings: semantic.warnings,
    })
}

#[cfg(test)]
fn compile_into_with_semantic_result(
    source: &SourceFile,
    build_directory: &Path,
    inject: impl FnOnce(&mut SemanticResult),
) -> Result<CompileOutput, CompileError> {
    compile_into_with_semantic(source, build_directory, |analysis| {
        let mut result = semantic::analyze(analysis);
        inject(&mut result);
        result
    })
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
        assert_eq!(
            fs::read_to_string(&output_path).unwrap(),
            c_emitter::render_print_program(b"hello")
        );
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
        assert!(output.contains("const int64_t sao2_binding_0 = INT64_C(2);"));
        assert!(output.contains(
            "int64_t sao2_binding_1 = (sao2_binding_0 * INT64_C(3));"
        ));
        assert!(output.contains("sao2_binding_1 += INT64_C(1);"));
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
        assert!(output.contains("fwrite(sao2_text_0"));
        assert!(output.contains("printf(\"%\" PRId64 \"\\n\", sao2_binding_0)"));
        assert!(output.contains("fputs((true) ? \"true\\n\" : \"false\\n\""));
        assert!(output.contains("return INT64_C(7);"));
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn unsupported_printable_type_returns_source_diagnostic() {
        let source = source("fn main() { print(1.0); }");
        let build_directory = temporary_directory("unsupported-output");
        let diagnostic = compile_into(&source, &build_directory)
            .unwrap_err()
            .to_string();
        assert!(diagnostic.contains("temporary backend"));
        assert!(!build_directory.exists());
    }

    #[test]
    fn temporary_backend_rejects_valid_but_unsupported_programs() {
        for (text, expected) in [
            (
                "fn main(args [str]) { print(\"x\"); }",
                "does not support parameters",
            ),
            (
                "fn main() int {}",
                "requires integer main to have a final value or unconditional return",
            ),
            ("fn main() { 1 }", "final value in no-value main"),
            ("fn main() { 1; }", "does not support this expression"),
            (
                "fn main() { panic(\"stop\"); }",
                "only direct calls to 'print' and 'println'",
            ),
            (
                "type Number(int); fn main() { print(\"x\"); }",
                "does not support type declaration",
            ),
            (
                "fn helper() {} fn main() { print(\"x\"); }",
                "function declaration other than 'main'",
            ),
        ] {
            let source = source(text);
            let diagnostic = compile_into(&source, &temporary_directory("unsupported"))
                .unwrap_err()
                .to_string();
            assert!(
                diagnostic.contains(expected),
                "{text}: {diagnostic}"
            );
        }
    }

    #[test]
    fn analysis_errors_precede_temporary_backend_limits() {
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
            assert!(!diagnostic.contains("temporary backend"), "{diagnostic}");
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
        assert_eq!(
            fs::read_to_string(output_path).unwrap(),
            c_emitter::render_print_program(b"second")
        );
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

        assert!(diagnostic.contains("temporary backend"));
        assert_eq!(fs::read_to_string(output_path).unwrap(), "existing generated C");
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn semantic_errors_precede_backend_validation_and_preserve_output() {
        for existing in [false, true] {
            let build_directory = temporary_directory("semantic-error");
            let output_path = build_directory.join("program.c");
            if existing {
                fs::create_dir(&build_directory).unwrap();
                fs::write(&output_path, "existing generated C").unwrap();
            }

            let source = source("type Number(int); fn helper() {}");
            let diagnostic = compile_into(&source, &build_directory)
                .unwrap_err()
                .to_string();

            assert!(diagnostic.contains("requires one 'main'"), "{diagnostic}");
            assert!(!diagnostic.contains("temporary backend"), "{diagnostic}");
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
}
