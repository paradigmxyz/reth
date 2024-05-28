fn main() {
    use devp2p::cli::Cli;

    let cli = Cli::parse_args();

    if let Err(err) = cli.run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
