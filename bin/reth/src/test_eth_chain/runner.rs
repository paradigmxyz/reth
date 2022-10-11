use super::models::Test;
use anyhow::Result;
use std::path::Path;

/// Run one json ethereum blockchain test
pub async fn run_test(path: &Path) -> Result<()> {
    let json_file = std::fs::read(path)?;
    let suits: Test = serde_json::from_reader(&*json_file)?;

    for suit in suits.0 {
        println!("TODO:{:?}", suit.0);
    }
    Ok(())
}
