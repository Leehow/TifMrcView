//! `tmv`: TifMrcView Pro processing from the command line (see `cli`).
fn main() {
    std::process::exit(tifmrcview_lib::cli::main(std::env::args().skip(1).collect()));
}
