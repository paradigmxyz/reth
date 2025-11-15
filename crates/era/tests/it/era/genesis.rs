//! Genesis block tests for `era1` files.
//!
//! These tests verify proper decompression and decoding of genesis blocks
//! from different networks.

use crate::{EraTestDownloader, ERA_HOODI_FILES_NAMES, HOODI};

#[tokio::test(flavor = "multi_thread")]
// #[ignore = "download intensive"]
async fn test_hoodi_genesis_era_decompression() -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;

    let file = downloader.open_era_file(ERA_HOODI_FILES_NAMES[0], HOODI).await?;

    // Verify this is genesis era
    assert!(file.group.is_genesis(), "First file should be genesis era");
    assert_eq!(file.group.starting_slot(), 0, "Genesis should start at slot 0");

    // Genesis era has no blocks
    assert_eq!(file.group.blocks.len(), 0, "Genesis era should have no blocks");

    // Genesis should not have block slot index
    assert!(file.group.slot_index.is_none(), "Genesis should not have block slot index");

    // Test state decompression
    let state_data = file.group.era_state.decompress()?;
    assert!(!state_data.is_empty(), "Decompressed state should not be empty");

    // Verify state slot index
    assert_eq!(
        file.group.state_slot_index.slot_count(),
        1,
        "Genesis state index should have count of 1"
    );

    Ok(())
}
