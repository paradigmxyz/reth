pub mod cli;
pub mod commands;

fn main() {
    use cli::Cli;

    let cli = Cli::parse_args();

    if let Err(err) = cli.run() {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
