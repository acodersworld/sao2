mod cli;
mod compiler;
mod diagnostic;
mod source;

use std::ffi::OsString;
use std::process::ExitCode;

use cli::{Command, HELP};

pub fn run(args: impl IntoIterator<Item = OsString>) -> ExitCode {
    match cli::parse(args) {
        Ok(Command::Help) => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Ok(Command::Build(path) | Command::Run(path)) => match source::SourceFile::load(&path) {
            Ok(source) => {
                let diagnostic = compiler::compile(&source);
                eprintln!("{diagnostic}");
                diagnostic.exit_code()
            }
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                diagnostic.exit_code()
            }
        },
        Err(diagnostic) => {
            eprintln!("{diagnostic}\n\n{HELP}");
            diagnostic.exit_code()
        }
    }
}
