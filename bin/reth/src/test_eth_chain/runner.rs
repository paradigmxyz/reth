use super::models::Test;
use anyhow::Result;
use std::path::Path;

/// Run one JSON-encoded Ethereum blockchain test at the specified path.
pub async fn run_test(path: &Path) -> Result<()> {
    let json_file = std::fs::read(path)?;
    let suits: Test = serde_json::from_reader(&*json_file)?;

    for suit in suits.0 {
        println!("TODO:{:?}", suit.0);
        let data = suit.1;
        //data.
    }
    Ok(())
}
