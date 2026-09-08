// Walking skeleton: milestone 3 phases 1-5 establish names, scopes, and expression types;
// phase 7 connects the result to the compiler pipeline.
#[allow(dead_code)]
mod analysis;
mod ast;
mod c_emitter;
mod cli;
mod compiler;
mod diagnostic;
mod host_compiler;
mod lexer;
mod parser;
mod program;
mod source;

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
    let source = match source::SourceFile::load(&options.source) {
        Ok(source) => source,
        Err(diagnostic) => return report_error(&diagnostic, diagnostic.exit_code()),
    };
    let generated_c = match compiler::compile(&source) {
        Ok(generated_c) => generated_c,
        Err(error) => return report_error(&error, error.exit_code()),
    };
    if options.show_c {
        match std::fs::read_to_string(&generated_c) {
            Ok(text) => print!("{text}"),
            Err(error) => {
                let diagnostic = diagnostic::Diagnostic::compiler(format!(
                    "cannot display generated C file '{}': {error}",
                    generated_c.display()
                ));
                return report_error(&diagnostic, diagnostic.exit_code());
            }
        }
    }
    let result = host_compiler::compile(&generated_c);
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

fn report_error(error: &impl std::fmt::Display, exit_code: i32) -> i32 {
    eprintln!("{error}");
    exit_code
}
