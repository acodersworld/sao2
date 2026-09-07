use std::fs;
use std::path::{Path, PathBuf};

use crate::c_emitter;
use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;
use crate::temporary_parser;

/// Stable orchestration boundary from loaded SAO2 source to generated C.
/// Frontend and backend implementations behind this function are temporary.
pub fn compile(source: &SourceFile) -> Result<PathBuf, Diagnostic> {
    compile_into(source, Path::new("build"))
}

fn compile_into(source: &SourceFile, build_directory: &Path) -> Result<PathBuf, Diagnostic> {
    let statement = temporary_parser::parse(source)?;
    let generated_c = c_emitter::emit_print_program(&statement.bytes);

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("sao2-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn valid_source_writes_program_c() {
        let source = SourceFile {
            path: PathBuf::from("hello.sao2"),
            text: "print(\"hello\");".to_owned(),
        };
        let build_directory = temporary_directory("c-output");

        let output_path = compile_into(&source, &build_directory).unwrap();
        assert_eq!(output_path, build_directory.join("program.c"));
        assert_eq!(
            fs::read_to_string(&output_path).unwrap(),
            c_emitter::emit_print_program(b"hello")
        );

        fs::remove_dir_all(build_directory).unwrap();
    }

    #[test]
    fn malformed_source_returns_source_diagnostic() {
        let source = SourceFile {
            path: PathBuf::from("bad.sao2"),
            text: "print(123);".to_owned(),
        };

        let build_directory = temporary_directory("malformed");
        let diagnostic = compile_into(&source, &build_directory)
            .unwrap_err()
            .to_string();
        assert!(diagnostic.contains("bad.sao2:1:7"));
        assert!(diagnostic.contains("expected string literal"));
        assert!(!build_directory.exists());
    }

    #[test]
    fn repeated_compilation_overwrites_deterministically() {
        let source = SourceFile {
            path: PathBuf::from("hello.sao2"),
            text: "print(\"first\");".to_owned(),
        };
        let build_directory = temporary_directory("overwrite");
        compile_into(&source, &build_directory).unwrap();

        let source = SourceFile {
            path: PathBuf::from("hello.sao2"),
            text: "print(\"second\");".to_owned(),
        };
        let output_path = compile_into(&source, &build_directory).unwrap();
        assert_eq!(
            fs::read_to_string(output_path).unwrap(),
            c_emitter::emit_print_program(b"second")
        );

        fs::remove_dir_all(build_directory).unwrap();
    }
}
