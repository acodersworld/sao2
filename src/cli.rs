use std::ffi::OsString;
use std::path::PathBuf;

use crate::diagnostic::Diagnostic;

pub const HELP: &str = "Usage:\n  sao2 build [--show-c] <source>\n  sao2 run [--show-c] <source> [--] [program-argument ...]\n  sao2 --help\n\nEnvironment:\n  SAO2_CC  Path or name of the host C compiler\n\nThe run command forwards program arguments after the source path to main(args [str]).\nUse -- before program arguments when one of them is --show-c.\n";

#[derive(Debug, Eq, PartialEq)]
pub struct CompileOptions {
    pub source: PathBuf,
    pub show_c: bool,
    pub program_arguments: Vec<OsString>,
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

    let is_build = command == "build";
    let kind = if is_build {
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
    let delimiter = remaining.iter().position(|argument| argument == "--");
    let option_end = delimiter.unwrap_or(remaining.len());
    let option_arguments = &remaining[..option_end];
    let show_c_count = option_arguments
        .iter()
        .filter(|argument| *argument == "--show-c")
        .count();
    if show_c_count > 1 {
        return Err(Diagnostic::usage(format!(
            "command '{}' accepts one source path and optional --show-c",
            command.to_string_lossy()
        )));
    }
    let show_c = show_c_count == 1;
    let source_position = option_arguments
        .iter()
        .position(|argument| argument != "--show-c");
    let Some(source_position) = source_position else {
        return Err(Diagnostic::usage(format!(
            "command '{}' requires a source path",
            command.to_string_lossy()
        )));
    };
    let source = &remaining[source_position];
    let mut program_arguments = remaining[source_position + 1..].to_vec();
    if let Some(delimiter) = delimiter {
        if delimiter <= source_position {
            return Err(Diagnostic::usage(format!(
                "command '{}' requires a source path before --",
                command.to_string_lossy()
            )));
        }
        program_arguments = remaining[delimiter + 1..].to_vec();
    } else if !is_build {
        program_arguments.retain(|argument| argument != "--show-c");
    }
    if is_build
        && (delimiter.is_some()
            || option_arguments
                .iter()
                .filter(|argument| *argument != "--show-c")
                .count()
                != 1)
    {
        return Err(Diagnostic::usage(format!(
            "command '{}' accepts one source path and optional --show-c",
            command.to_string_lossy()
        )));
    }
    if is_build {
        program_arguments.clear();
    }

    Ok(kind(CompileOptions {
        source: PathBuf::from(source),
        show_c,
        program_arguments,
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
                program_arguments: Vec::new(),
            })
        );
        assert_eq!(
            parse_strings(&["run", "--show-c", "hello", "one", "two"]).unwrap(),
            Command::Run(CompileOptions {
                source: PathBuf::from("hello"),
                show_c: true,
                program_arguments: vec![OsString::from("one"), OsString::from("two")],
            })
        );
        assert_eq!(
            parse_strings(&["build", "hello", "--show-c"]).unwrap(),
            Command::Build(CompileOptions {
                source: PathBuf::from("hello"),
                show_c: true,
                program_arguments: Vec::new(),
            })
        );
        assert_eq!(
            parse_strings(&["run", "hello", "--", "--show-c"]).unwrap(),
            Command::Run(CompileOptions {
                source: PathBuf::from("hello"),
                show_c: false,
                program_arguments: vec![OsString::from("--show-c")],
            })
        );
        assert_eq!(parse_strings(&["--help"]).unwrap(), Command::Help);
    }

    #[test]
    fn rejects_invalid_forms() {
        assert!(parse_strings(&[]).is_err());
        assert!(parse_strings(&["compile", "hello.sao2"]).is_err());
        assert!(parse_strings(&["build"]).is_err());
        assert!(parse_strings(&["build", "one", "two"]).is_err());
        assert!(parse_strings(&["build", "--show-c", "--show-c", "one"]).is_err());
        assert!(parse_strings(&["run", "--", "hello"]).is_err());
        assert!(parse_strings(&["--help", "extra"]).is_err());
    }
}
