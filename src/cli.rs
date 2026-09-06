use std::ffi::OsString;
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

pub const HELP: &str = "Usage:\n  sao2 build <source>\n  sao2 run <source>\n  sao2 --help\n";

#[derive(Debug, Eq, PartialEq)]
pub enum Command {
    Build(PathBuf),
    Run(PathBuf),
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

    let Some(source) = args.next() else {
        return Err(Diagnostic::usage(format!(
            "command '{}' requires a source path",
            command.to_string_lossy()
        )));
    };
    if args.next().is_some() {
        return Err(Diagnostic::usage(format!(
            "command '{}' accepts exactly one source path",
            command.to_string_lossy()
        )));
    }

    Ok(kind(PathBuf::from(source)))
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
            Command::Build(PathBuf::from("hello.sao2"))
        );
        assert_eq!(
            parse_strings(&["run", "hello"]).unwrap(),
            Command::Run(PathBuf::from("hello"))
        );
        assert_eq!(parse_strings(&["--help"]).unwrap(), Command::Help);
    }

    #[test]
    fn rejects_invalid_forms() {
        assert!(parse_strings(&[]).is_err());
        assert!(parse_strings(&["compile", "hello.sao2"]).is_err());
        assert!(parse_strings(&["build"]).is_err());
        assert!(parse_strings(&["run", "one", "two"]).is_err());
        assert!(parse_strings(&["--help", "extra"]).is_err());
    }
}
