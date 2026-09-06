use std::fmt;
use std::process::ExitCode;

#[derive(Debug, Eq, PartialEq)]
pub enum DiagnosticKind {
    Usage,
    Input,
    Compiler,
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

    fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self.kind {
            DiagnosticKind::Usage => ExitCode::from(2),
            DiagnosticKind::Input | DiagnosticKind::Compiler => ExitCode::FAILURE,
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let category = match self.kind {
            DiagnosticKind::Usage => "usage error",
            DiagnosticKind::Input => "input error",
            DiagnosticKind::Compiler => "compiler error",
        };
        write!(formatter, "sao2: {category}: {}", self.message)
    }
}
