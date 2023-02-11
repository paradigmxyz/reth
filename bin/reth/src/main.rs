fn main() {
    if let Err(err) = reth::cli::run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
