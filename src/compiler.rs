use crate::diagnostic::Diagnostic;
use crate::source::SourceFile;

/// Temporary Phase 1 boundary. Phase 2 replaces this with minimal parsing.
pub fn compile(source: &SourceFile) -> Diagnostic {
    let _ = &source.text;
    Diagnostic::compiler(format!(
        "compilation stage is not implemented yet (loaded '{}')",
        source.path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn valid_source_reaches_placeholder_stage() {
        let source = SourceFile {
            path: PathBuf::from("hello.sao2"),
            text: "print(\"hello\");".to_owned(),
        };

        let diagnostic = compile(&source).to_string();
        assert!(diagnostic.contains("compilation stage is not implemented yet"));
        assert!(diagnostic.contains("hello.sao2"));
    }
}
