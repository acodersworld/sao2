use std::path::Path;
use std::ffi::OsString;
use std::process::Command;

use crate::diagnostic::Diagnostic;

/// Runs a compiled SAO2 program directly, forwarding arguments as an argument
/// list. Standard input, output, and error are inherited so the executable
/// behaves as if the user launched it themselves.
pub fn run(executable: &Path, arguments: &[OsString]) -> Result<i32, Diagnostic> {
    let status = Command::new(executable)
        .args(arguments)
        .status()
        .map_err(|error| {
        Diagnostic::program(format!(
            "failed to start executable '{}': {error}",
            executable.display()
        ))
    })?;

    status.code().ok_or_else(|| {
        Diagnostic::program(format!(
            "executable '{}' terminated without an exit code",
            executable.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reports_launch_failures_as_program_errors() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let missing = PathBuf::from(format!("missing-sao2-program-{nonce}"));
        let diagnostic = run(&missing, &[]).unwrap_err().to_string();
        assert!(diagnostic.contains("program error"));
        assert!(diagnostic.contains("failed to start executable"));
    }
}
