//! Simple decoding and decompressing tests
//! for mainnet era files

use reth_era::{
    common::file_ops::{StreamReader, StreamWriter},
    era::file::{EraReader, EraWriter},
};
use std::io::Cursor;

use crate::{open_era_test_file, EraTestDownloader, ERA_MAINNET_FILES_NAMES, MAINNET};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_mainnet_era_file_decompression_and_decoding() -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await.expect("Failed to create downloader");

    for &filename in &ERA_MAINNET_FILES_NAMES {
        println!("\nTesting file: {filename}");
        let file = open_era_test_file(filename, &downloader, MAINNET).await?;

        // Handle genesis era separately
        if file.group.is_genesis() {
            // Genesis has no blocks
            assert_eq!(file.group.blocks.len(), 0, "Genesis should have no blocks");
            assert!(file.group.slot_index.is_none(), "Genesis should not have block slot index");

            // Test genesis state decompression
            let state_data = file.group.era_state.decompress()?;
            assert!(!state_data.is_empty(), "Genesis state should decompress to non-empty data");

            // Verify state slot index
            assert_eq!(
                file.group.state_slot_index.slot_count(),
                1,
                "Genesis state index should have count of 1"
            );

            // Test round-trip for genesis era
            let mut buffer = Vec::new();
            {
                let mut writer = EraWriter::new(&mut buffer);
                writer.write_file(&file)?;
            }

            let reader = EraReader::new(Cursor::new(&buffer));
            let read_back_file = reader.read(file.id.network_name.clone())?;

            assert_eq!(file.id.network_name, read_back_file.id.network_name);
            assert_eq!(file.id.start_slot, read_back_file.id.start_slot);
            assert!(read_back_file.group.is_genesis());

            // Verify state data preservation
            assert_eq!(
                file.group.era_state.decompress()?,
                read_back_file.group.era_state.decompress()?,
                "Genesis state data should be identical"
            );
            continue;
        }

        // Non-genesis era - test beacon blocks
        println!(
            "  Non-genesis era with {} beacon blocks, starting at slot {}",
            file.group.blocks.len(),
            file.group.starting_slot()
        );

        // Test beacon block decompression across different positions
        let test_block_indices = [
            0,                           // First block
            file.group.blocks.len() / 2, // Middle block
            file.group.blocks.len() - 1, // Last block
        ];

        for &block_idx in &test_block_indices {
            let block = &file.group.blocks[block_idx];
            let slot = file.group.starting_slot() + block_idx as u64;

            println!(
                "\n  Testing beacon block at slot {}, compressed size: {} bytes",
                slot,
                block.data.len()
            );

            // Test beacon block decompression
            let block_data = block.decompress()?;
            assert!(
                !block_data.is_empty(),
                "Beacon block at slot {slot} decompression should produce non-empty data"
            );
        }

        // Test era state decompression
        let state_data = file.group.era_state.decompress()?;
        assert!(!state_data.is_empty(), "Era state decompression should produce non-empty data");
        println!("    Era state decompressed: {} bytes", state_data.len());

        // Verify slot indices
        if let Some(ref block_slot_index) = file.group.slot_index {
            println!(
                "    Block slot index: starting_slot={}, count={}",
                block_slot_index.starting_slot,
                block_slot_index.slot_count()
            );

            // Check for empty slots
            let empty_slots: Vec<usize> = (0..block_slot_index.slot_count())
                .filter(|&i| !block_slot_index.has_data_at_slot(i))
                .collect();

            if !empty_slots.is_empty() {
                println!(
                    "    Found {} empty slots (first few): {:?}",
                    empty_slots.len(),
                    &empty_slots[..empty_slots.len().min(5)]
                );
            }
        }

        // Test round-trip serialization
        let mut buffer = Vec::new();
        {
            let mut writer = EraWriter::new(&mut buffer);
            writer.write_file(&file)?;
        }

        // Read back from buffer
        let reader = EraReader::new(Cursor::new(&buffer));
        let read_back_file = reader.read(file.id.network_name.clone())?;

        // Verify basic properties are preserved
        assert_eq!(file.id.network_name, read_back_file.id.network_name);
        assert_eq!(file.id.start_slot, read_back_file.id.start_slot);
        assert_eq!(file.id.slot_count, read_back_file.id.slot_count);
        assert_eq!(file.group.blocks.len(), read_back_file.group.blocks.len());

        // Test data preservation for beacon blocks
        for &idx in &test_block_indices {
            let original_block = &file.group.blocks[idx];
            let read_back_block = &read_back_file.group.blocks[idx];
            let slot = file.group.starting_slot() + idx as u64;

            // Test that decompressed data is identical
            assert_eq!(
                original_block.decompress()?,
                read_back_block.decompress()?,
                "Beacon block data should be identical for slot {slot}"
            );
        }

        // Test state data preservation
        assert_eq!(
            file.group.era_state.decompress()?,
            read_back_file.group.era_state.decompress()?,
            "Era state data should be identical"
        );

        // Test slot indices preservation
        if let (Some(original_index), Some(read_index)) =
            (&file.group.slot_index, &read_back_file.group.slot_index)
        {
            assert_eq!(
                original_index.starting_slot, read_index.starting_slot,
                "Block slot index starting slot should match"
            );
            assert_eq!(
                original_index.offsets, read_index.offsets,
                "Block slot index offsets should match"
            );
        }

        assert_eq!(
            file.group.state_slot_index.starting_slot,
            read_back_file.group.state_slot_index.starting_slot,
            "State slot index starting slot should match"
        );
        assert_eq!(
            file.group.state_slot_index.offsets, read_back_file.group.state_slot_index.offsets,
            "State slot index offsets should match"
        );
    }

    Ok(())
}
