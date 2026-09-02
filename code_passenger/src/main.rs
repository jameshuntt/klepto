fn main() {
    if let Err(e) = code_passenger::cli::run() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
