use super::models::Test;
use std::path::Path;

/// Run one JSON-encoded Ethereum blockchain test at the specified path.
pub async fn run_test(path: &Path) -> eyre::Result<()> {
    let json_file = std::fs::read(path)?;
    let suits: Test = serde_json::from_reader(&*json_file)?;

    for suit in suits.0 {
        println!("TODO:{:?}", suit.0);
    }
    Ok(())
}
