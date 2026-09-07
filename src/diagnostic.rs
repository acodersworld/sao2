use crate::source::{SourceFile, Span};
use std::fmt;
use std::path::PathBuf;

pub const MAX_SOURCE_ERRORS: usize = 20;

#[derive(Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    Usage,
    Input,
    Source,
    Compiler,
    Program,
}

#[derive(Debug, Eq, PartialEq)]
struct PrimarySpan {
    path: PathBuf,
    span: Span,
    line: usize,
    column: usize,
    source_line: String,
    caret_width: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Diagnostic {
    kind: DiagnosticKind,
    message: String,
    primary: Option<PrimarySpan>,
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

    pub fn source(source: &SourceFile, span: Span, message: impl Into<String>) -> Self {
        let (line, column, source_line, caret_width) = source.diagnostic_excerpt(span);
        Self {
            kind: DiagnosticKind::Source,
            message: message.into(),
            primary: Some(PrimarySpan {
                path: source.path.clone(),
                span,
                line,
                column,
                source_line,
                caret_width,
            }),
        }
    }

    fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            primary: None,
        }
    }

    pub fn primary_span(&self) -> Option<Span> {
        self.primary.as_ref().map(|primary| primary.span)
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
        write!(formatter, "sao2: {category}: ")?;
        if let Some(primary) = &self.primary {
            writeln!(
                formatter,
                "{}:{}:{}: {}",
                primary.path.display(),
                primary.line,
                primary.column,
                self.message
            )?;
            let gutter_width = primary.line.to_string().len();
            writeln!(formatter, "{:gutter_width$} |", "")?;
            writeln!(
                formatter,
                "{:>gutter_width$} | {}",
                primary.line, primary.source_line
            )?;
            write!(
                formatter,
                "{:gutter_width$} | {}{}",
                "",
                " ".repeat(primary.column - 1),
                "^".repeat(primary.caret_width)
            )
        } else {
            write!(formatter, "{}", self.message)
        }
    }
}

/// A bounded collection used by recovering frontend stages.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Diagnostics {
    entries: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, diagnostic: Diagnostic) {
        if self.entries.len() < MAX_SOURCE_ERRORS {
            self.entries.push(diagnostic);
        }
    }
    pub fn is_full(&self) -> bool {
        self.entries.len() == MAX_SOURCE_ERRORS
    }
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    #[cfg(test)]
    pub fn into_sorted(mut self) -> Vec<Diagnostic> {
        self.entries
            .sort_by_key(|diagnostic| diagnostic.primary_span().map(|span| span.start));
        self.entries
    }
}

impl fmt::Display for Diagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut diagnostics: Vec<&Diagnostic> = self.entries.iter().collect();
        diagnostics.sort_by_key(|diagnostic| diagnostic.primary_span().map(|span| span.start));
        for (index, diagnostic) in diagnostics.into_iter().enumerate() {
            if index > 0 {
                writeln!(formatter)?;
            }
            write!(formatter, "{diagnostic}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(text: &str) -> SourceFile {
        SourceFile::new(PathBuf::from("test.sao2"), text.to_owned())
    }

    #[test]
    fn source_diagnostic_renders_location_line_and_span_caret() {
        let source = source("first\n\tbad input");
        let diagnostic = Diagnostic::source(&source, Span::new(7, 10), "expected expression");
        assert_eq!(diagnostic.primary_span(), Some(Span::new(7, 10)));
        assert_eq!(
            diagnostic.to_string(),
            "sao2: source error: test.sao2:2:5: expected expression\n  |\n2 |     bad input\n  |     ^^^"
        );
    }

    #[test]
    fn diagnostics_are_bounded_and_sorted_by_primary_span() {
        let source = source("0123456789");
        let mut diagnostics = Diagnostics::new();
        for offset in (0..25).rev() {
            diagnostics.push(Diagnostic::source(
                &source,
                Span::empty(offset.min(10)),
                format!("error {offset}"),
            ));
        }
        assert!(diagnostics.is_full());
        assert_eq!(diagnostics.len(), MAX_SOURCE_ERRORS);
        let diagnostics = diagnostics.into_sorted();
        assert!(
            diagnostics
                .windows(2)
                .all(|pair| pair[0].primary_span().unwrap().start
                    <= pair[1].primary_span().unwrap().start)
        );
    }

    #[test]
    fn preserves_non_source_categories() {
        assert_eq!(Diagnostic::usage("bad command").exit_code(), 2);
        assert_eq!(Diagnostic::input("bad input").exit_code(), 1);
        assert_eq!(Diagnostic::compiler("bug").exit_code(), 1);
        assert_eq!(Diagnostic::program("failed").exit_code(), 1);
    }

    #[test]
    fn diagnostic_collection_renders_in_source_order() {
        let source = source("one two");
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(Diagnostic::source(&source, Span::new(4, 7), "second"));
        diagnostics.push(Diagnostic::source(&source, Span::new(0, 3), "first"));

        let rendered = diagnostics.to_string();
        assert!(rendered.find("first").unwrap() < rendered.find("second").unwrap());
    }
}
