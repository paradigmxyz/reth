//! Roundtrip tests for `.era` files.
//!
//! These tests verify the full lifecycle of era files by:
//! - Reading files from their original source
//! - Decompressing and decoding their contents
//! - Re-encoding and recompressing the data
//! - Writing the data back to a new file
//! - Confirming that all original data is preserved throughout the process
//!
//!
//! Only a couple of era files are downloaded from `https://mainnet.era.nimbus.team/` for mainnet
//! and `https://hoodi.era.nimbus.team/` for hoodi to keep the tests efficient.

use reth_era::{
    common::file_ops::{EraFileFormat, StreamReader, StreamWriter},
    era::{
        file::{EraFile, EraReader, EraWriter},
        types::{
            consensus::{CompressedBeaconState, CompressedSignedBeaconBlock},
            group::{EraGroup, EraId},
        },
    },
};
use std::io::Cursor;

use crate::{EraTestDownloader, HOODI, MAINNET};

// Helper function to test roundtrip compression/encoding for a specific file
async fn test_era_file_roundtrip(
    downloader: &EraTestDownloader,
    filename: &str,
    network: &str,
) -> eyre::Result<()> {
    println!("\nTesting roundtrip for file: {filename}");

    let original_file = downloader.open_era_file(filename, network).await?;

    if original_file.group.is_genesis() {
        println!("Genesis era detected, using special handling");
        // Genesis has no blocks
        assert_eq!(original_file.group.blocks.len(), 0, "Genesis should have no blocks");
        assert!(
            original_file.group.slot_index.is_none(),
            "Genesis should not have block slot index"
        );

        // Test genesis state decompression
        let state_data = original_file.group.era_state.decompress()?;
        assert!(!state_data.is_empty(), "Genesis state should decompress to non-empty data");
        println!("  Genesis state decompressed: {} bytes", state_data.len());

        // Write to buffer and read back
        let mut buffer = Vec::new();
        {
            let mut writer = EraWriter::new(&mut buffer);
            writer.write_file(&original_file)?;
        }

        let reader = EraReader::new(Cursor::new(&buffer));
        let roundtrip_file = reader.read(network.to_string())?;

        assert_eq!(
            original_file.group.era_state.decompress()?,
            roundtrip_file.group.era_state.decompress()?,
            "Genesis state data should be identical after roundtrip"
        );

        println!("Genesis era verified successfully");
        return Ok(());
    }

    // Write the entire file to a buffer
    let mut buffer = Vec::new();
    {
        let mut writer = EraWriter::new(&mut buffer);
        writer.write_file(&original_file)?;
    }

    // Read back from buffer
    let reader = EraReader::new(Cursor::new(&buffer));
    let roundtrip_file = reader.read(network.to_string())?;

    assert_eq!(
        original_file.id.network_name, roundtrip_file.id.network_name,
        "Network name should match after roundtrip"
    );
    assert_eq!(
        original_file.id.start_slot, roundtrip_file.id.start_slot,
        "Start slot should match after roundtrip"
    );
    assert_eq!(
        original_file.group.blocks.len(),
        roundtrip_file.group.blocks.len(),
        "Block count should match after roundtrip"
    );

    // Select a few blocks to test
    let test_block_indices = [
        0,                                    // First block
        original_file.group.blocks.len() / 2, // Middle block
        original_file.group.blocks.len() - 1, // Last block
    ];

    // Test individual beacon blocks
    for &block_idx in &test_block_indices {
        let original_block = &original_file.group.blocks[block_idx];
        let roundtrip_block = &roundtrip_file.group.blocks[block_idx];
        let slot = original_file.group.starting_slot() + block_idx as u64;

        println!("Testing roundtrip for beacon block at slot {slot}");

        // Test beacon block decompression
        let original_block_data = original_block.decompress()?;
        let roundtrip_block_data = roundtrip_block.decompress()?;

        assert_eq!(
            original_block_data, roundtrip_block_data,
            "Beacon block at slot {slot} data should be identical after roundtrip"
        );

        let recompressed_block = CompressedSignedBeaconBlock::from_ssz(&original_block_data)?;
        let recompressed_block_data = recompressed_block.decompress()?;

        assert_eq!(
            original_block_data, recompressed_block_data,
            "Beacon block at slot {slot} data should be identical after re-compression cycle"
        );

        println!(
            "  Beacon block at slot {slot} re-compression cycle verified: {} bytes",
            recompressed_block_data.len()
        );
    }

    // Test era state decompression
    let original_state_data = original_file.group.era_state.decompress()?;
    let roundtrip_state_data = roundtrip_file.group.era_state.decompress()?;

    assert_eq!(
        original_state_data, roundtrip_state_data,
        "Era state data should be identical after roundtrip"
    );

    let recompressed_state = CompressedBeaconState::from_ssz(&roundtrip_state_data)?;
    let recompressed_state_data = recompressed_state.decompress()?;

    assert_eq!(
        original_state_data, recompressed_state_data,
        "Era state data should be identical after re-compression cycle"
    );

    let recompressed_blocks: Vec<CompressedSignedBeaconBlock> = roundtrip_file
        .group
        .blocks
        .iter()
        .map(|block| {
            let data = block.decompress()?;
            CompressedSignedBeaconBlock::from_ssz(&data)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let new_group = if let Some(ref block_index) = roundtrip_file.group.slot_index {
        EraGroup::with_block_index(
            recompressed_blocks,
            recompressed_state,
            block_index.clone(),
            roundtrip_file.group.state_slot_index.clone(),
        )
    } else {
        EraGroup::new(
            recompressed_blocks,
            recompressed_state,
            roundtrip_file.group.state_slot_index,
        )
    };

    let (start_slot, slot_count) = new_group.slot_range();
    let new_file = EraFile::new(new_group, EraId::new(network, start_slot, slot_count));

    let mut reconstructed_buffer = Vec::new();
    {
        let mut writer = EraWriter::new(&mut reconstructed_buffer);
        writer.write_file(&new_file)?;
    }

    // Read it back and verify
    let reader = EraReader::new(Cursor::new(&reconstructed_buffer));
    let reconstructed_file = reader.read(network.to_string())?;

    assert_eq!(
        original_file.group.blocks.len(),
        reconstructed_file.group.blocks.len(),
        "Block count should match after full reconstruction"
    );

    println!("File {filename} roundtrip successful");
    Ok(())
}

#[test_case::test_case("mainnet-00000-4b363db9.era"; "era_roundtrip_mainnet_0")]
#[test_case::test_case("mainnet-00178-0d0a5290.era"; "era_roundtrip_mainnet_178")]
#[test_case::test_case("mainnet-01070-7616e3e2.era"; "era_roundtrip_mainnet_1070")]
#[test_case::test_case("mainnet-01267-e3ddc749.era"; "era_roundtrip_mainnet_1267")]
#[test_case::test_case("mainnet-01592-d4dc8b98.era"; "era_roundtrip_mainnet_1592")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_roundtrip_compression_encoding_mainnet(filename: &str) -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;
    test_era_file_roundtrip(&downloader, filename, MAINNET).await
}

#[test_case::test_case("hoodi-00000-212f13fc.era"; "era_roundtrip_hoodi_0")]
#[test_case::test_case("hoodi-00021-857e418b.era"; "era_roundtrip_hoodi_21")]
#[test_case::test_case("hoodi-00175-202aaa6d.era"; "era_roundtrip_hoodi_175")]
#[test_case::test_case("hoodi-00201-0d521fc8.era"; "era_roundtrip_hoodi_201")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_roundtrip_compression_encoding_hoodi(filename: &str) -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;

    test_era_file_roundtrip(&downloader, filename, HOODI).await?;

    Ok(())
}
