// Walking skeleton: milestone 6 constructs and retains validated typed IR while
// the deliberately limited milestone 4 backend remains active until milestone 7.
#[allow(dead_code)]
mod analysis;
mod ast;
#[allow(dead_code)]
mod c_backend;
mod c_emitter;
mod cli;
mod compiler;
mod diagnostic;
mod host_compiler;
#[allow(dead_code)]
mod ir;
mod lexer;
#[allow(dead_code)]
mod lowering;
mod parser;
mod program;
mod semantic;
mod source;

use cli::{Command, HELP};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    let compiled = match compiler::compile(&source) {
        Ok(compiled) => compiled,
        Err(failure) => {
            return report_compile_failure(failure, &mut std::io::stderr());
        }
    };
    finish_build(
        compiled,
        options.show_c,
        requested_run,
        &mut std::io::stderr(),
        host_compiler::compile,
        program::run,
    )
}

fn report_compile_failure<W: Write>(
    failure: compiler::CompileFailure,
    standard_error: &mut W,
) -> i32 {
    if !failure.warnings.is_empty() {
        let _ = writeln!(standard_error, "{}", failure.warnings);
    }
    let _ = writeln!(standard_error, "{}", failure.error);
    failure.exit_code()
}

fn finish_build<W, H, R>(
    compiled: compiler::CompileOutput,
    show_c: bool,
    requested_run: bool,
    standard_error: &mut W,
    compile_c: H,
    run_program: R,
) -> i32
where
    W: Write,
    H: FnOnce(&Path) -> Result<PathBuf, diagnostic::Diagnostic>,
    R: FnOnce(&Path) -> Result<i32, diagnostic::Diagnostic>,
{
    if !compiled.warnings.is_empty() {
        let _ = writeln!(standard_error, "{}", compiled.warnings);
    }
    let generated_c = compiled.generated_c;
    if show_c {
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
    let result = compile_c(&generated_c);
    match result {
        Ok(executable) if requested_run => match run_program(&executable) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, Warnings};
    use crate::source::{SourceFile, Span};
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Captured(Rc<RefCell<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn warnings_are_rendered_before_downstream_compilation_and_do_not_fail() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "fn main() {}".to_owned());
        let mut warnings = Warnings::new();
        warnings.push(Diagnostic::source_warning(
            &source,
            Span::empty(0),
            "synthetic warning",
        ));
        let captured = Rc::new(RefCell::new(Vec::new()));
        let observed = Rc::clone(&captured);
        let mut sink = Captured(captured);
        let status = finish_build(
            compiler::CompileOutput {
                generated_c: PathBuf::from("unused-program.c"),
                warnings,
            },
            false,
            false,
            &mut sink,
            move |_| {
                let rendered = String::from_utf8(observed.borrow().clone()).unwrap();
                assert!(rendered.contains("source warning"));
                assert!(rendered.contains("synthetic warning"));
                Ok(PathBuf::from("unused-program"))
            },
            |_| unreachable!("build must not run the executable"),
        );
        assert_eq!(status, 0);
    }

    #[test]
    fn warnings_are_rendered_before_a_compile_failure_without_changing_its_status() {
        let source = SourceFile::new(PathBuf::from("test.sao2"), "fn main() {}".to_owned());
        let mut warnings = Warnings::new();
        warnings.push(Diagnostic::source_warning(
            &source,
            Span::empty(0),
            "unreachable source",
        ));
        let captured = Rc::new(RefCell::new(Vec::new()));
        let mut sink = Captured(Rc::clone(&captured));
        let status = report_compile_failure(
            compiler::CompileFailure {
                error: compiler::CompileError::Diagnostic(Diagnostic::compiler("backend failed")),
                warnings,
            },
            &mut sink,
        );
        let rendered = String::from_utf8(captured.borrow().clone()).unwrap();
        assert!(
            rendered.find("source warning").unwrap()
                < rendered.find("compiler error").unwrap()
        );
        assert_eq!(status, 1);
    }
}
