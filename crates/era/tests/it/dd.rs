//! Simple decoding and decompressing tests
//! for mainnet era1 files

use alloy_consensus::{BlockBody, Header};
use alloy_primitives::U256;
use reth_era::{
    common::file_ops::{StreamReader, StreamWriter},
    e2s::types::IndexEntry,
    era1::{
        file::{Era1Reader, Era1Writer},
        types::execution::CompressedBody,
    },
};
use reth_ethereum_primitives::TransactionSigned;
use std::io::Cursor;

use crate::{Era1TestDownloader, MAINNET};

// Helper function to test decompression and decoding for a specific file
async fn test_file_decompression(
    downloader: &Era1TestDownloader,
    filename: &str,
) -> eyre::Result<()> {
    println!("\nTesting file: {filename}");
    let file = downloader.open_era1_file(filename, MAINNET).await?;

    // Test block decompression across different positions in the file
    let test_block_indices = [
        0,                           // First block
        file.group.blocks.len() / 2, // Middle block
        file.group.blocks.len() - 1, // Last block
    ];

    for &block_idx in &test_block_indices {
        let block = &file.group.blocks[block_idx];
        let block_number = file.group.block_index.starting_number() + block_idx as u64;

        println!(
            "\n  Testing block {}, compressed body size: {} bytes",
            block_number,
            block.body.data.len()
        );

        // Test header decompression and decoding
        let header_data = block.header.decompress()?;
        assert!(
            !header_data.is_empty(),
            "Block {block_number} header decompression should produce non-empty data"
        );

        let header = block.header.decode_header()?;
        assert_eq!(header.number, block_number, "Decoded header should have correct block number");
        println!("Header decompression and decoding successful");

        // Test body decompression
        let body_data = block.body.decompress()?;
        assert!(
            !body_data.is_empty(),
            "Block {block_number} body decompression should produce non-empty data"
        );
        println!("Body decompression successful ({} bytes)", body_data.len());

        let decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(&body_data)
                .expect("Failed to decode body");

        println!(
            "Body decoding successful: {} transactions, {} ommers, withdrawals: {}",
            decoded_body.transactions.len(),
            decoded_body.ommers.len(),
            decoded_body.withdrawals.is_some()
        );

        // Test receipts decompression
        let receipts_data = block.receipts.decompress()?;
        assert!(
            !receipts_data.is_empty(),
            "Block {block_number} receipts decompression should produce non-empty data"
        );
        println!("Receipts decompression successful ({} bytes)", receipts_data.len());

        assert!(
            block.total_difficulty.value > U256::ZERO,
            "Block {block_number} should have non-zero difficulty"
        );
        println!("Total difficulty verified: {}", block.total_difficulty.value);
    }

    // Test round-trip serialization
    println!("\n  Testing data preservation roundtrip...");
    let mut buffer = Vec::new();
    {
        let mut writer = Era1Writer::new(&mut buffer);
        writer.write_file(&file)?;
    }

    // Read back from buffer
    let reader = Era1Reader::new(Cursor::new(&buffer));
    let read_back_file = reader.read(file.id.network_name.clone())?;

    // Verify basic properties are preserved
    assert_eq!(file.id.network_name, read_back_file.id.network_name);
    assert_eq!(file.id.start_block, read_back_file.id.start_block);
    assert_eq!(file.group.blocks.len(), read_back_file.group.blocks.len());
    assert_eq!(file.group.accumulator.root, read_back_file.group.accumulator.root);

    // Test data preservation for some blocks
    for &idx in &test_block_indices {
        let original_block = &file.group.blocks[idx];
        let read_back_block = &read_back_file.group.blocks[idx];
        let block_number = file.group.block_index.starting_number() + idx as u64;

        println!("Block {block_number} details:");
        println!("  Header size: {} bytes", original_block.header.data.len());
        println!("  Body size: {} bytes", original_block.body.data.len());
        println!("  Receipts size: {} bytes", original_block.receipts.data.len());

        // Test that decompressed data is identical
        assert_eq!(
            original_block.header.decompress()?,
            read_back_block.header.decompress()?,
            "Header data should be identical for block {block_number}"
        );

        assert_eq!(
            original_block.body.decompress()?,
            read_back_block.body.decompress()?,
            "Body data should be identical for block {block_number}"
        );

        assert_eq!(
            original_block.receipts.decompress()?,
            read_back_block.receipts.decompress()?,
            "Receipts data should be identical for block {block_number}"
        );

        assert_eq!(
            original_block.total_difficulty.value, read_back_block.total_difficulty.value,
            "Total difficulty should be identical for block {block_number}"
        );
    }

    Ok(())
}

#[test_case::test_case("mainnet-00000-5ec1ffb8.era1"; "era_dd_mainnet_0")]
#[test_case::test_case("mainnet-00003-d8b8a40b.era1"; "era_dd_mainnet_3")]
#[test_case::test_case("mainnet-00151-e322efe1.era1"; "era_dd_mainnet_151")]
#[test_case::test_case("mainnet-00293-0d6c5812.era1"; "era_dd_mainnet_293")]
#[test_case::test_case("mainnet-00443-ea71b6f9.era1"; "era_dd_mainnet_443")]
#[test_case::test_case("mainnet-01367-d7efc68f.era1"; "era_dd_mainnet_1367")]
#[test_case::test_case("mainnet-01610-99fdde4b.era1"; "era_dd_mainnet_1610")]
#[test_case::test_case("mainnet-01895-3f81607c.era1"; "era_dd_mainnet_1895")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_mainnet_era1_file_decompression_and_decoding(filename: &str) -> eyre::Result<()> {
    let downloader = Era1TestDownloader::new().await?;
    test_file_decompression(&downloader, filename).await
}
