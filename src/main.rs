fn main() {
    std::process::exit(sao2::run(std::env::args_os().skip(1)));
}
