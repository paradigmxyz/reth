//! Genesis block tests for `.era1` files.
//!
//! These tests verify proper decompression and decoding of genesis blocks
//! from different networks.

use alloy_consensus::{BlockBody, Header};
use reth_era::execution_types::CompressedBody;
use reth_ethereum_primitives::TransactionSigned;

use crate::{
    Era1TestDownloader, ERA1_MAINNET_FILES_NAMES, ERA1_SEPOLIA_FILES_NAMES, MAINNET, SEPOLIA,
};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_mainnet_genesis_block_decompression() -> eyre::Result<()> {
    let downloader = Era1TestDownloader::new().await?;

    let file = downloader.open_era1_file(ERA1_MAINNET_FILES_NAMES[0], MAINNET).await?;

    // Genesis and a few early blocks
    let test_blocks = [0, 1, 10, 100];

    for &block_idx in &test_blocks {
        let block = &file.group.blocks[block_idx];
        let block_number = file.group.block_index.starting_number + block_idx as u64;

        println!(
            "Testing block {}, compressed body size: {} bytes",
            block_number,
            block.body.data.len()
        );

        // Test decompression
        let body_data = block.body.decompress()?;
        assert!(!body_data.is_empty(), "Decompressed body should not be empty");
        println!("Successfully decompressed body: {} bytes", body_data.len());

        let decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(&body_data)
                .expect("Failed to decode body");

        // For genesis era blocks, there should be no transactions or ommers
        assert_eq!(
            decoded_body.transactions.len(),
            0,
            "Genesis era block should have no transactions"
        );
        assert_eq!(decoded_body.ommers.len(), 0, "Genesis era block should have no ommers");

        // Check for withdrawals, should be `None` for genesis era blocks
        assert!(decoded_body.withdrawals.is_none(), "Genesis era block should have no withdrawals");

        let header = block.header.decode_header()?;
        assert_eq!(header.number, block_number, "Header should have correct block number");
        println!("Successfully decoded header for block {block_number}");

        // Test total difficulty value
        let td = block.total_difficulty.value;
        println!("Block {block_number} total difficulty: {td}");
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_sepolia_genesis_block_decompression() -> eyre::Result<()> {
    let downloader = Era1TestDownloader::new().await?;

    let file = downloader.open_era1_file(ERA1_SEPOLIA_FILES_NAMES[0], SEPOLIA).await?;

    // Genesis and a few early blocks
    let test_blocks = [0, 1, 10, 100];

    for &block_idx in &test_blocks {
        let block = &file.group.blocks[block_idx];
        let block_number = file.group.block_index.starting_number + block_idx as u64;

        println!(
            "Testing block {}, compressed body size: {} bytes",
            block_number,
            block.body.data.len()
        );

        // Test decompression
        let body_data = block.body.decompress()?;
        assert!(!body_data.is_empty(), "Decompressed body should not be empty");
        println!("Successfully decompressed body: {} bytes", body_data.len());

        let decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(&body_data)
                .expect("Failed to decode body");

        // For genesis era blocks, there should be no transactions or ommers
        assert_eq!(
            decoded_body.transactions.len(),
            0,
            "Genesis era block should have no transactions"
        );
        assert_eq!(decoded_body.ommers.len(), 0, "Genesis era block should have no ommers");

        // Check for withdrawals, should be `None` for genesis era blocks
        assert!(decoded_body.withdrawals.is_none(), "Genesis era block should have no withdrawals");

        let header = block.header.decode_header()?;
        assert_eq!(header.number, block_number, "Header should have correct block number");
        println!("Successfully decoded header for block {block_number}");

        // Test total difficulty value
        let td = block.total_difficulty.value;
        println!("Block {block_number} total difficulty: {td}");
    }

    Ok(())
}
