//! Command-line entry point for `portez`.

#![forbid(unsafe_code)]

fn main() {
    let code = portez::cli::entry(std::env::args_os().skip(1));
    std::process::exit(code);
}
