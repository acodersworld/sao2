use crate::source::SourceFile;
use std::fmt;
use std::path::Path;

#[derive(Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    Usage,
    Input,
    Source,
    Compiler,
    Program,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Diagnostic {
    kind: DiagnosticKind,
    message: String,
}

impl Diagnostic {
    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Usage, message)
    }

    pub fn input(message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Input, message)
    }

    pub fn compiler(message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Compiler, message)
    }

    pub fn program(message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::Program, message)
    }

    pub fn source(source: &SourceFile, offset: usize, message: impl Into<String>) -> Self {
        let (line, column) = line_and_column(&source.text, offset);
        Self::new(
            DiagnosticKind::Source,
            format!(
                "{}:{line}:{column}: {}",
                display_path(&source.path),
                message.into()
            ),
        )
    }

    fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            DiagnosticKind::Usage => 2,
            DiagnosticKind::Input
            | DiagnosticKind::Source
            | DiagnosticKind::Compiler
            | DiagnosticKind::Program => 1,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = match self.kind {
            DiagnosticKind::Usage => "usage error",
            DiagnosticKind::Input => "input error",
            DiagnosticKind::Source => "source error",
            DiagnosticKind::Compiler => "compiler error",
            DiagnosticKind::Program => "program error",
        };
        write!(formatter, "sao2: {category}: {}", self.message)
    }
}

fn line_and_column(text: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(text.len());
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column = text[line_start..offset]
        .chars()
        .fold(1, |column, character| {
            if character == '\t' {
                column + (4 - ((column - 1) % 4))
            } else {
                column + 1
            }
        });
    (line, column)
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn source_locations_are_one_based_and_expand_tabs() {
        let source = SourceFile {
            path: PathBuf::from("test.sao2"),
            text: "first\n\tbad".to_owned(),
        };
        let diagnostic = Diagnostic::source(&source, 7, "expected expression");
        assert_eq!(
            diagnostic.to_string(),
            "sao2: source error: test.sao2:2:5: expected expression"
        );
    }
}
