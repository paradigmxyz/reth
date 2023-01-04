#[tokio::main]
async fn main() {
    if let Err(err) = reth::cli::run().await {
        eprintln!("Error: {err:?}");
        std::process::exit(1);
    }
}
