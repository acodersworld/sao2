use std::ffi::OsString;
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

pub const HELP: &str = "Usage:\n  sao2 build [--show-c] <source>\n  sao2 run [--show-c] <source>\n  sao2 --help\n\nEnvironment:\n  SAO2_CC  Path or name of the host C compiler\n";

#[derive(Debug, Eq, PartialEq)]
pub struct CompileOptions {
    pub source: PathBuf,
    pub show_c: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Build(CompileOptions),
    Run(CompileOptions),
    Help,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Command, Diagnostic> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(Diagnostic::usage("missing command"));
    };

    if command == "--help" || command == "-h" {
        if args.next().is_some() {
            return Err(Diagnostic::usage("help does not accept arguments"));
        }
        return Ok(Command::Help);
    }

    let kind = if command == "build" {
        Command::Build
    } else if command == "run" {
        Command::Run
    } else {
        return Err(Diagnostic::usage(format!(
            "unknown command '{}'",
            command.to_string_lossy()
        )));
    };

    let remaining: Vec<OsString> = args.collect();
    let show_c = remaining.iter().any(|argument| argument == "--show-c");
    let sources: Vec<&OsString> = remaining
        .iter()
        .filter(|argument| *argument != "--show-c")
        .collect();
    let Some(source) = sources.first() else {
        return Err(Diagnostic::usage(format!(
            "command '{}' requires a source path",
            command.to_string_lossy()
        )));
    };
    if sources.len() != 1 || remaining.len() != sources.len() + usize::from(show_c) {
        return Err(Diagnostic::usage(format!(
            "command '{}' accepts one source path and optional --show-c",
            command.to_string_lossy()
        )));
    }

    Ok(kind(CompileOptions {
        source: PathBuf::from(source),
        show_c,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strings(args: &[&str]) -> Result<Command, Diagnostic> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse_strings(&["build", "hello.sao2"]).unwrap(),
            Command::Build(CompileOptions {
                source: PathBuf::from("hello.sao2"),
                show_c: false,
            })
        );
        assert_eq!(
            parse_strings(&["run", "--show-c", "hello"]).unwrap(),
            Command::Run(CompileOptions {
                source: PathBuf::from("hello"),
                show_c: true,
            })
        );
        assert_eq!(parse_strings(&["--help"]).unwrap(), Command::Help);
    }

    #[test]
    fn rejects_invalid_forms() {
        assert!(parse_strings(&[]).is_err());
        assert!(parse_strings(&["compile", "hello.sao2"]).is_err());
        assert!(parse_strings(&["build"]).is_err());
        assert!(parse_strings(&["run", "one", "two"]).is_err());
        assert!(parse_strings(&["build", "--show-c", "--show-c", "one"]).is_err());
        assert!(parse_strings(&["--help", "extra"]).is_err());
    }
}
