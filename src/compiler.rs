use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::analysis::{
    self, Analysis, CallableId, CallTarget, EntryPoint, IntrinsicId, LiteralValue, TypeState,
};
use crate::ast::{
    ArgumentKind, Declaration, ExpressionKind, FunctionDeclaration, PrimitiveType, Program,
    StatementKind,
};
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
    let main = main.expect("a diagnostic-free analysis has a valid entry point");
    let bytes = lower_temporary_print_main(source, &program, &analysis, main)?;
    let generated_c = c_emitter::emit_print_program(bytes);

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

fn validate_main<'program>(
    source: &SourceFile,
    analysis: &Analysis<'_, 'program>,
) -> Result<&'program FunctionDeclaration, Diagnostic> {
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
        EntryPoint::Valid(main) => Ok(analysis.function_signature(main).node),
    }
}

fn lower_temporary_print_main<'analysis, 'source, 'program>(
    source: &SourceFile,
    program: &'program Program,
    analysis: &'analysis Analysis<'source, 'program>,
    main: &'program FunctionDeclaration,
) -> Result<&'analysis [u8], Diagnostic> {
    if program.declarations.len() != 1 {
        let unsupported = program
            .declarations
            .iter()
            .find(|declaration| declaration.span() != main.span)
            .map_or(main.span, Declaration::span);
        return Err(Diagnostic::source(
            source,
            unsupported,
            "temporary backend supports only the 'main' function",
        ));
    }
    if !main.parameters.is_empty() || main.return_type.is_some() || main.body.value.is_some() {
        return Err(temporary_backend_error(source, main));
    }
    let [statement] = main.body.statements.as_slice() else {
        return Err(temporary_backend_error(source, main));
    };
    let StatementKind::Expression(expression) = &statement.kind else {
        return Err(temporary_backend_error(source, main));
    };
    let ExpressionKind::Call { callee, arguments } = &expression.kind else {
        return Err(temporary_backend_error(source, main));
    };
    let ExpressionKind::Identifier(_) = &callee.kind else {
        return Err(temporary_backend_error(source, main));
    };
    if !matches!(
        analysis.call_resolution(expression).map(|call| call.target),
        Some(CallTarget::Callable(CallableId::Intrinsic(IntrinsicId::Print)))
    ) {
        return Err(temporary_backend_error(source, main));
    }
    let [argument] = arguments.as_slice() else {
        return Err(temporary_backend_error(source, main));
    };
    let ArgumentKind::Positional(argument) = &argument.kind else {
        return Err(temporary_backend_error(source, main));
    };
    let ExpressionKind::String(_) = &argument.kind else {
        return Err(temporary_backend_error(source, main));
    };
    let str_type = analysis.types.primitive(PrimitiveType::Str);
    if analysis.expression_annotation(expression).map(|annotation| annotation.state)
        != Some(TypeState::NoValue)
        || analysis.expression_annotation(argument).map(|annotation| annotation.state)
            != Some(TypeState::Resolved(str_type))
    {
        return Err(Diagnostic::compiler(
            "temporary backend received an unresolved expression from analysis",
        ));
    }
    match analysis.literal(argument) {
        Some(LiteralValue::String(bytes)) => Ok(bytes),
        _ => Err(Diagnostic::compiler(
            "temporary backend received a string expression without a decoded literal",
        )),
    }
}

fn temporary_backend_error(source: &SourceFile, main: &FunctionDeclaration) -> Diagnostic {
    Diagnostic::source(
        source,
        main.body.span,
        "temporary backend supports only fn main() containing one print call with one string literal",
    )
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
            c_emitter::emit_print_program(b"hello")
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
        for text in [
            "fn main() {}",
            "fn main() int { 1 }",
            "fn main(args [str]) { print(\"x\"); }",
            "fn main() { println(\"x\"); }",
            "fn helper() { value := 1; } fn main() { print(\"x\"); }",
        ] {
            let source = source(text);
            let diagnostic = compile_into(&source, &temporary_directory("unsupported"))
                .unwrap_err()
                .to_string();
            assert!(
                diagnostic.contains("temporary backend"),
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
            c_emitter::emit_print_program(b"second")
        );
        fs::remove_dir_all(build_directory).unwrap();
    }
}
