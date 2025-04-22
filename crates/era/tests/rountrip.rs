//! Roundtrip tests for `.era1` files.
//!
//! These tests verify the full lifecycle of era files by:
//! - Reading files from their original source
//! - Decompressing and decoding their contents
//! - Re-encoding and recompressing the data
//! - Writing the data back to a new file
//! - Confirming that all original data is preserved throughout the process

use alloy_consensus::{BlockBody, BlockHeader, Header};
use rand::{prelude::IndexedRandom, rng};
use reth_era::{
    e2s_types::E2sError,
    era1_file::{Era1File, Era1Reader, Era1Writer},
    era1_types::{Era1Group, Era1Id},
    execution_types::{BlockTuple, CompressedBody, CompressedHeader, TotalDifficulty},
};
use reth_ethereum_primitives::TransactionSigned;
use std::io::Cursor;

mod common;
use common::{
    Era1TestDownloader, ERA1_MAINNET_FILES_NAMES, ERA1_SEPOLIA_FILES_NAMES, MAINNET, SEPOLIA,
};

// Helper function to test roundtrip compression/encoding for a specific file
async fn test_file_roundtrip(
    downloader: &Era1TestDownloader,
    filename: &str,
    network: &str,
) -> Result<(), E2sError> {
    println!("\nTesting roundtrip for file: {}", filename);

    let original_file = downloader.open_era1_file(filename, network).await?;

    // Select a few blocks to test
    let test_block_indices = [
        0,                                    // First block
        original_file.group.blocks.len() / 2, // Middle block
        original_file.group.blocks.len() - 1, // Last block
    ];

    // Write the entire file to a buffer
    let mut buffer = Vec::new();
    {
        let mut writer = Era1Writer::new(&mut buffer);
        writer.write_era1_file(&original_file)?;
    }

    // Read back from buffer
    let mut reader = Era1Reader::new(Cursor::new(&buffer));
    let roundtrip_file = reader.read(network.to_string())?;

    assert_eq!(
        original_file.id.network_name, roundtrip_file.id.network_name,
        "Network name should match after roundtrip"
    );
    assert_eq!(
        original_file.id.start_block, roundtrip_file.id.start_block,
        "Start block should match after roundtrip"
    );
    assert_eq!(
        original_file.group.blocks.len(),
        roundtrip_file.group.blocks.len(),
        "Block count should match after roundtrip"
    );
    assert_eq!(
        original_file.group.accumulator.root, roundtrip_file.group.accumulator.root,
        "Accumulator root should match after roundtrip"
    );

    // Test individual blocks
    for &block_id in &test_block_indices {
        let original_block = &original_file.group.blocks[block_id];
        let roundtrip_block = &roundtrip_file.group.blocks[block_id];
        let block_number = original_file.group.block_index.starting_number + block_id as u64;

        println!("Testing roundtrip for block {}", block_number);

        // Test header decompression
        let original_header_data = original_block.header.decompress()?;
        let roundtrip_header_data = roundtrip_block.header.decompress()?;
        assert_eq!(
            original_header_data, roundtrip_header_data,
            "Block {} header data should be identical after roundtrip",
            block_number
        );

        // Test body decompression
        let original_body_data = original_block.body.decompress()?;
        let roundtrip_body_data = roundtrip_block.body.decompress()?;
        assert_eq!(
            original_body_data, roundtrip_body_data,
            "Block {} body data should be identical after roundtrip",
            block_number
        );

        // Test receipts decompression
        let original_receipts_data = original_block.receipts.decompress()?;

        let roundtrip_receipts_data = roundtrip_block.receipts.decompress()?;
        assert_eq!(
            original_receipts_data, roundtrip_receipts_data,
            "Block {} receipts data should be identical after roundtrip",
            block_number
        );

        // Test total difficulty preservation
        assert_eq!(
            original_block.total_difficulty.value, roundtrip_block.total_difficulty.value,
            "Block {} total difficulty should be identical after roundtrip",
            block_number
        );
        // Test decoding of header and body to ensure structural integrity
        let original_header = original_block.header.decode_header()?;
        let roundtrip_header = roundtrip_block.header.decode_header()?;
        assert_eq!(
            original_header.number, roundtrip_header.number,
            "Block number should match after roundtrip decoding"
        );
        assert_eq!(
            original_header.mix_hash(),
            roundtrip_header.mix_hash(),
            "Block hash should match after roundtrip decoding"
        );

        // Decode and verify body contents
        let original_decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(
                &original_body_data,
            )
            .expect("Failed to original decode body");

        let roundtrip_decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(
                &original_body_data,
            )
            .expect("Failed to original decode body");

        assert_eq!(
            original_decoded_body.transactions.len(),
            roundtrip_decoded_body.transactions.len(),
            "Transaction count should match after roundtrip between original and roundtrop"
        );

        assert_eq!(
            original_decoded_body.ommers.len(),
            roundtrip_decoded_body.ommers.len(),
            "Ommers count should match after roundtrip"
        );

        // Check withdrawals presence/absence matches
        assert_eq!(
            original_decoded_body.withdrawals.is_some(),
            roundtrip_decoded_body.withdrawals.is_some(),
            "Withdrawals presence should match after roundtrip"
        );

        println!("Block {} roundtrip verified successfully", block_number);

        println!("Testing full re-encoding/re-compression cycle for block {}", block_number);

        let recompressed_header = CompressedHeader::from_header(&original_header)?;
        let recompressed_header_data = recompressed_header.decompress()?;
        assert_eq!(
            original_header_data, recompressed_header_data,
            "Re-compressed header data should match original after full cycle"
        );

        let recompressed_body = CompressedBody::from_body(&original_decoded_body)?;
        let recompressed_body_data = recompressed_body.decompress()?;

        let recompressed_decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(
                &recompressed_body_data,
            )
            .expect("Failed to decode re-compressed body");

        assert_eq!(
            original_decoded_body.transactions.len(),
            recompressed_decoded_body.transactions.len(),
            "Transaction count should match after re-compression"
        );

        let recompressed_block = BlockTuple::new(
            recompressed_header,
            recompressed_body,
            original_block.receipts.clone(), /* reuse original receipts direclty as it not
                                              * possible to decode them */
            TotalDifficulty::new(original_block.total_difficulty.value),
        );

        let mut recompressed_buffer = Vec::new();
        {
            let blocks = vec![recompressed_block];

            let new_group = Era1Group::new(
                blocks,
                original_file.group.accumulator.clone(),
                original_file.group.block_index.clone(),
            );

            let new_file =
                Era1File::new(new_group, Era1Id::new(network, original_file.id.start_block, 1));

            let mut writer = Era1Writer::new(&mut recompressed_buffer);
            writer.write_era1_file(&new_file)?;
        }

        let mut reader = Era1Reader::new(Cursor::new(&recompressed_buffer));
        let recompressed_file = reader.read(network.to_string())?;

        let recompressed_first_block = &recompressed_file.group.blocks[0];
        let recompressed_header = recompressed_first_block.header.decode_header()?;

        assert_eq!(
            original_header.number, recompressed_header.number,
            "Block number should match after complete re-encoding cycle"
        );

        println!(
            "Block {} full re-encoding/re-compression cycle verified successfully 🫡",
            block_number
        );
    }

    println!("File {} roundtrip successful", filename);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_roundtrip_compression_encoding_mainnet() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new().await?;

    let mut rng = rng();

    // pick 3 random files from the mainnet list
    let sample_files: Vec<&str> =
        ERA1_MAINNET_FILES_NAMES.choose_multiple(&mut rng, 4).copied().collect();

    println!("Testing {} randomly selected mainnet files", sample_files.len());

    for &filename in &sample_files {
        test_file_roundtrip(&downloader, filename, MAINNET).await?;
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_roundtrip_compression_encoding_sepolia() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new().await?;

    // Test all Sepolia files
    for &filename in &ERA1_SEPOLIA_FILES_NAMES {
        test_file_roundtrip(&downloader, filename, SEPOLIA).await?;
    }

    Ok(())
}
