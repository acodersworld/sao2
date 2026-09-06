mod c_emitter;
mod cli;
mod compiler;
mod diagnostic;
mod source;
mod temporary_parser;

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
            Ok(source) => match compiler::compile(&source) {
                Ok(generated_path) => {
                    let diagnostic = diagnostic::Diagnostic::compiler(format!(
                        "host C compilation is not implemented yet (generated '{}')",
                        generated_path.display()
                    ));
                    eprintln!("{diagnostic}");
                    diagnostic.exit_code()
                }
                Err(diagnostic) => {
                    eprintln!("{diagnostic}");
                    diagnostic.exit_code()
                }
            },
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
