use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::analysis::{self, Analysis, EntryPoint, FunctionId};
use crate::c_emitter;
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::parser;
use crate::source::{SourceFile, Span};

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

/// Stable orchestration boundary from loaded SAO2 source through parsing and
/// name-and-type analysis to generated C. The analyzed-AST-to-C subset remains
/// temporary until the typed backend replaces it.
pub fn compile(source: &SourceFile) -> Result<PathBuf, CompileError> {
    compile_into(source, Path::new("build"))
}

fn compile_into(source: &SourceFile, build_directory: &Path) -> Result<PathBuf, CompileError> {
    let program = parser::parse(source).map_err(CompileError::SourceDiagnostics)?;
    let mut analysis = analysis::analyze(source, &program);
    let main = match validate_main(source, &analysis) {
        Ok(main) => Some(main),
        Err(diagnostic) => {
            analysis.diagnostics.push(diagnostic);
            None
        }
    };
    if !analysis.diagnostics.is_empty() {
        return Err(CompileError::SourceDiagnostics(analysis.diagnostics));
    }
    let main = analysis.function_signature(
        main.expect("a diagnostic-free analysis has a valid entry point"),
    );
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
    Ok(output_path)
}

fn validate_main(
    source: &SourceFile,
    analysis: &Analysis<'_, '_>,
) -> Result<FunctionId, Diagnostic> {
    match analysis.entry_point {
        EntryPoint::Missing => Err(Diagnostic::source(
            source,
            Span::empty(source.text.len()),
            "executable program requires one 'main' function",
        )),
        EntryPoint::Duplicate { duplicate, .. } => Err(Diagnostic::source(
            source,
            analysis.function_signature(duplicate).node.name.span,
            "duplicate 'main' function",
        )),
        EntryPoint::Invalid(main) => Err(Diagnostic::source(
            source,
            analysis.function_signature(main).node.name.span,
            "invalid 'main' signature; expected main(), main() int, main(args [str]), or main(args [str]) int",
        )),
        EntryPoint::Valid(main) => Ok(main),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let output_path = compile_into(&source, &build_directory).unwrap();
        assert_eq!(
            fs::read_to_string(&output_path).unwrap(),
            c_emitter::render_print_program(b"hello")
        );
        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn malformed_source_returns_source_diagnostic() {
        let source = source("fn main() { print(123); }");
        let build_directory = temporary_directory("malformed");
        let diagnostic = compile_into(&source, &build_directory)
            .unwrap_err()
            .to_string();
        assert!(diagnostic.contains("temporary backend"));
        assert!(!build_directory.exists());
    }

    #[test]
    fn validates_all_four_main_signatures() {
        for text in [
            "fn main() {}",
            "fn main() int {}",
            "fn main(args [str]) {}",
            "fn main(args [str]) int {}",
        ] {
            let source = source(text);
            let program = parser::parse(&source).unwrap();
            let analysis = analysis::analyze(&source, &program);
            assert!(validate_main(&source, &analysis).is_ok(), "{text}");
        }
    }

    #[test]
    fn rejects_missing_duplicate_and_invalid_main() {
        for (text, expected) in [
            ("fn helper() {}", "requires one 'main'"),
            ("fn main() {} fn main() {}", "duplicate 'main'"),
            ("fn main(value str) {}", "invalid 'main' signature"),
            ("fn main(var args [str]) {}", "invalid 'main' signature"),
            ("fn main() str {}", "invalid 'main' signature"),
        ] {
            let source = source(text);
            let program = parser::parse(&source).unwrap();
            let analysis = analysis::analyze(&source, &program);
            let diagnostic = validate_main(&source, &analysis).unwrap_err().to_string();
            assert!(diagnostic.contains(expected), "{text}: {diagnostic}");
        }
    }

    #[test]
    fn temporary_backend_rejects_valid_but_unsupported_programs() {
        for (text, expected) in [
            ("fn main() {}", "exactly one print statement"),
            ("fn main() int { 1 }", "does not support this type"),
            (
                "fn main(args [str]) { print(\"x\"); }",
                "does not support parameters",
            ),
            (
                "fn main() { println(\"x\"); }",
                "only a direct call to 'print'",
            ),
            (
                "fn main() { value := 1; }",
                "does not support this statement",
            ),
            ("fn main() { 1; }", "does not support this expression"),
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
        let output_path = compile_into(&second_source, &build_directory).unwrap();
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

        let source = source("fn main() { println(\"unsupported\"); }");
        let diagnostic = compile_into(&source, &build_directory)
            .unwrap_err()
            .to_string();

        assert!(diagnostic.contains("temporary backend"));
        assert_eq!(fs::read_to_string(output_path).unwrap(), "existing generated C");
        fs::remove_dir_all(build_directory).unwrap();
    }
}
