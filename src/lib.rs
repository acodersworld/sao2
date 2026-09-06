mod c_emitter;
mod cli;
mod compiler;
mod diagnostic;
mod host_compiler;
mod program;
mod source;
mod temporary_parser;

use cli::{Command, HELP};
use std::ffi::OsString;

pub fn run(args: impl IntoIterator<Item = OsString>) -> i32 {
    match cli::parse(args) {
        Ok(Command::Help) => {
            print!("{HELP}");
            0
        }
        Ok(Command::Build(options)) => build(options, false),
        Ok(Command::Run(options)) => build(options, true),
        Err(diagnostic) => {
            eprintln!("{diagnostic}\n\n{HELP}");
            diagnostic.exit_code()
        }
    }
}

fn build(options: cli::CompileOptions, requested_run: bool) -> i32 {
    let result = source::SourceFile::load(&options.source)
        .and_then(|source| compiler::compile(&source))
        .and_then(|generated_c| {
            if options.show_c {
                match std::fs::read_to_string(&generated_c) {
                    Ok(text) => print!("{text}"),
                    Err(error) => {
                        return Err(diagnostic::Diagnostic::compiler(format!(
                            "cannot display generated C file '{}': {error}",
                            generated_c.display()
                        )));
                    }
                }
            }
            host_compiler::compile(&generated_c)
        });

    match result {
        Ok(executable) if requested_run => match program::run(&executable) {
            Ok(exit_code) => exit_code,
            Err(diagnostic) => {
                eprintln!("{diagnostic}");
                diagnostic.exit_code()
            }
        },
        Ok(executable) => {
            println!("built {}", executable.display());
            0
        }
        Err(diagnostic) => {
            eprintln!("{diagnostic}");
            diagnostic.exit_code()
        }
    }
}
