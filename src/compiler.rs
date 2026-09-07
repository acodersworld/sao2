use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::{
    ArgumentKind, Declaration, ExpressionKind, FunctionDeclaration, PrimitiveType, Program,
    StatementKind, TypeKind,
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

/// Stable orchestration boundary from loaded SAO2 source to generated C.
/// The AST-to-C subset remains temporary until the typed backend replaces it.
pub fn compile(source: &SourceFile) -> Result<PathBuf, CompileError> {
    compile_into(source, Path::new("build"))
}

fn compile_into(source: &SourceFile, build_directory: &Path) -> Result<PathBuf, CompileError> {
    let program = parser::parse(source).map_err(CompileError::SourceDiagnostics)?;
    let main = validate_main(source, &program)?;
    let bytes = lower_temporary_print_main(source, &program, main)?;
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
    program: &'program Program,
) -> Result<&'program FunctionDeclaration, Diagnostic> {
    let mains: Vec<&FunctionDeclaration> = program
        .declarations
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Function(function)
                if identifier_text(source, function.name.span) == "main" =>
            {
                Some(function)
            }
            _ => None,
        })
        .collect();

    let main = match mains.as_slice() {
        [] => {
            return Err(Diagnostic::source(
                source,
                Span::empty(source.text.len()),
                "executable program requires one 'main' function",
            ));
        }
        [main] => *main,
        [_, duplicate, ..] => {
            return Err(Diagnostic::source(
                source,
                duplicate.name.span,
                "duplicate 'main' function",
            ));
        }
    };

    let parameters_valid = match main.parameters.as_slice() {
        [] => true,
        [parameter] => {
            !parameter.mutable
                && identifier_text(source, parameter.name.span) == "args"
                && matches!(&parameter.ty.kind, TypeKind::List(element) if matches!(element.kind, TypeKind::Primitive(PrimitiveType::Str)))
        }
        _ => false,
    };
    let return_valid = main.return_type.as_ref().is_none_or(|return_type| {
        matches!(return_type.kind, TypeKind::Primitive(PrimitiveType::Int))
    });
    if !parameters_valid || !return_valid {
        return Err(Diagnostic::source(
            source,
            main.name.span,
            "invalid 'main' signature; expected main(), main() int, main(args [str]), or main(args [str]) int",
        ));
    }
    Ok(main)
}

fn lower_temporary_print_main<'program>(
    source: &SourceFile,
    program: &'program Program,
    main: &'program FunctionDeclaration,
) -> Result<&'program [u8], Diagnostic> {
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
    let ExpressionKind::Identifier(identifier) = &callee.kind else {
        return Err(temporary_backend_error(source, main));
    };
    if identifier_text(source, identifier.span) != "print" {
        return Err(temporary_backend_error(source, main));
    }
    let [argument] = arguments.as_slice() else {
        return Err(temporary_backend_error(source, main));
    };
    let ArgumentKind::Positional(argument) = &argument.kind else {
        return Err(temporary_backend_error(source, main));
    };
    let ExpressionKind::String(bytes) = &argument.kind else {
        return Err(temporary_backend_error(source, main));
    };
    Ok(bytes)
}

fn temporary_backend_error(source: &SourceFile, main: &FunctionDeclaration) -> Diagnostic {
    Diagnostic::source(
        source,
        main.body.span,
        "temporary backend supports only fn main() containing one print call with one string literal",
    )
}

fn identifier_text(source: &SourceFile, span: Span) -> &str {
    &source.text[span.start..span.end]
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
            assert!(validate_main(&source, &program).is_ok(), "{text}");
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
            let diagnostic = validate_main(&source, &program).unwrap_err().to_string();
            assert!(diagnostic.contains(expected), "{text}: {diagnostic}");
        }
    }

    #[test]
    fn temporary_backend_rejects_valid_but_unsupported_programs() {
        for text in [
            "fn main() {}",
            "fn main() int { \"value\" }",
            "fn main(args [str]) { print(\"x\"); }",
            "fn main() { println(\"x\"); }",
            "fn main() { print(\"one\", \"two\"); }",
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
