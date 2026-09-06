use std::process::ExitCode;

fn main() -> ExitCode {
    sao2::run(std::env::args_os().skip(1))
}
