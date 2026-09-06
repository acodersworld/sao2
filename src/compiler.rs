use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;
use crate::temporary_parser;

/// Temporary walking-skeleton boundary. Phase 3 replaces the diagnostic with C
/// generation while Phase 2's temporary parser remains in place.
pub fn compile(source: &SourceFile) -> Diagnostic {
    let statement = match temporary_parser::parse(source) {
        Ok(statement) => statement,
        Err(diagnostic) => return diagnostic,
    };
    let _ = (&statement.bytes, statement.span);
    Diagnostic::compiler(format!(
        "C generation is not implemented yet (parsed '{}')",
        source.path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn valid_source_reaches_c_generation_placeholder() {
        let source = SourceFile {
            path: PathBuf::from("hello.sao2"),
            text: "print(\"hello\");".to_owned(),
        };

        let diagnostic = compile(&source).to_string();
        assert!(diagnostic.contains("C generation is not implemented yet"));
        assert!(diagnostic.contains("hello.sao2"));
    }

    #[test]
    fn malformed_source_returns_source_diagnostic() {
        let source = SourceFile {
            path: PathBuf::from("bad.sao2"),
            text: "print(123);".to_owned(),
        };

        let diagnostic = compile(&source).to_string();
        assert!(diagnostic.contains("bad.sao2:1:7"));
        assert!(diagnostic.contains("expected string literal"));
    }
}
