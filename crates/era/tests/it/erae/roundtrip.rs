//! Roundtrip tests for `.erae` files.
//!
//! These tests verify the full lifecycle of era files by:
//! - Reading files from their original source
//! - Decompressing and decoding their contents
//! - Re-encoding and recompressing the data
//! - Writing the data back to a new file
//! - Confirming that all original data is preserved throughout the process
//!
//! Only a couple of erae files are downloaded from <https://data.ethpandaops.io/erae/mainnet/>
//! and <https://data.ethpandaops.io/erae/sepolia/> to keep the tests efficient.

use alloy_consensus::{BlockBody, BlockHeader, Header};
use reth_era::{
    common::file_ops::{EraFileFormat, StreamReader, StreamWriter},
    erae::{
        file::{EraEFile, EraEReader, EraEWriter},
        types::{
            execution::{
                BlockTuple, CompressedBody, CompressedHeader, CompressedSlimReceipts, Proof,
                ProofType, SlimReceipt, TotalDifficulty,
            },
            group::{EraEGroup, EraEId},
        },
    },
};
use reth_ethereum_primitives::TransactionSigned;
use std::io::Cursor;

use crate::{EraTestDownloader, MAINNET, SEPOLIA};

// Helper function to test roundtrip compression/encoding for a specific file
async fn test_erae_file_roundtrip(
    downloader: &EraTestDownloader,
    filename: &str,
    network: &str,
) -> eyre::Result<()> {
    println!("\nTesting roundtrip for file: {filename}");

    let original_file = downloader.open_erae_file(filename, network).await?;

    // Select a few blocks to test
    let test_block_indices = [
        0,                                    // First block
        original_file.group.blocks.len() / 2, // Middle block
        original_file.group.blocks.len() - 1, // Last block
    ];

    // Write the entire file to a buffer
    let mut buffer = Vec::new();
    {
        let mut writer = EraEWriter::new(&mut buffer);
        writer.write_file(&original_file)?;
    }

    // Read back from buffer
    let reader = EraEReader::new(Cursor::new(&buffer));
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
        original_file.group.accumulator, roundtrip_file.group.accumulator,
        "Accumulator should match after roundtrip"
    );

    // Test individual blocks
    for &block_id in &test_block_indices {
        let original_block = &original_file.group.blocks[block_id];
        let roundtrip_block = &roundtrip_file.group.blocks[block_id];
        let block_number = original_file.group.block_index.starting_number() + block_id as u64;

        println!("Testing roundtrip for block {block_number}");

        // Test header decompression
        let original_header_data = original_block.header.decompress()?;
        let roundtrip_header_data = roundtrip_block.header.decompress()?;
        assert_eq!(
            original_header_data, roundtrip_header_data,
            "Block {block_number} header data should be identical after roundtrip"
        );

        // Test body decompression
        let original_body_data = original_block.body.decompress()?;
        let roundtrip_body_data = roundtrip_block.body.decompress()?;
        assert_eq!(
            original_body_data, roundtrip_body_data,
            "Block {block_number} body data should be identical after roundtrip"
        );

        // Test receipts decompression
        let original_receipts_data = original_block.receipts.decompress()?;

        let roundtrip_receipts_data = roundtrip_block.receipts.decompress()?;
        assert_eq!(
            original_receipts_data, roundtrip_receipts_data,
            "Block {block_number} receipts data should be identical after roundtrip"
        );

        // Test total difficulty preservation
        assert_eq!(
            original_block.total_difficulty.value, roundtrip_block.total_difficulty.value,
            "Block {block_number} total difficulty should be identical after roundtrip",
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
            .expect("Failed to decode original body");

        let roundtrip_decoded_body: BlockBody<TransactionSigned> =
            CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(
                &roundtrip_body_data,
            )
            .expect("Failed to original decode body");

        assert_eq!(
            original_decoded_body.transactions.len(),
            roundtrip_decoded_body.transactions.len(),
            "Transaction count should match after roundtrip between original and roundtrip"
        );

        assert_eq!(
            original_decoded_body.ommers.len(),
            roundtrip_decoded_body.ommers.len(),
            "Ommers count should match after roundtrip"
        );

        // `SlimReceipt` uses `Eip658Value`, so it decodes both pre-Byzantium post-state-root
        // receipts and post-Byzantium status receipts — no fork-dependent guard needed.
        let original_receipts_decoded: Vec<SlimReceipt> = original_block.receipts.decode()?;
        let roundtrip_receipts_decoded: Vec<SlimReceipt> = roundtrip_block.receipts.decode()?;

        assert_eq!(
            original_receipts_decoded, roundtrip_receipts_decoded,
            "Block {block_number} receipts should match after roundtrip"
        );

        // Full decode -> encode -> decode cycle
        let re_encoded = CompressedSlimReceipts::from_receipts(&original_receipts_decoded)?;
        let re_decoded: Vec<SlimReceipt> = re_encoded.decode_receipts()?;
        assert_eq!(
            original_receipts_decoded, re_decoded,
            "Block {block_number} receipts should survive decode/encode/decode cycle"
        );

        println!(
            "Block {block_number}: decoded and re-encoded {} slim receipts",
            original_receipts_decoded.len()
        );

        // Check withdrawals presence/absence matches
        assert_eq!(
            original_decoded_body.withdrawals.is_some(),
            roundtrip_decoded_body.withdrawals.is_some(),
            "Withdrawals presence should match after roundtrip"
        );

        println!("Block {block_number} roundtrip verified successfully");

        println!("Testing full re-encoding/re-compression cycle for block {block_number}");

        // Re-encode and re-compress the header
        let recompressed_header = CompressedHeader::from_header(&original_header)?;
        let recompressed_header_data = recompressed_header.decompress()?;
        assert_eq!(
            original_header_data, recompressed_header_data,
            "Re-compressed header data should match original after full cycle"
        );

        // Re-encode and re-compress the body
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

        // Re-encode receipts: decompress -> re-compress and verify
        let re_encoded_receipts = CompressedSlimReceipts::from_rlp(&original_receipts_data)?;
        let re_decoded_receipts_data = re_encoded_receipts.decompress()?;
        assert_eq!(
            original_receipts_data, re_decoded_receipts_data,
            "Receipts should survive decode/encode cycle"
        );

        let recompressed_block = BlockTuple::new(
            recompressed_header,
            recompressed_body,
            re_encoded_receipts,
            TotalDifficulty::new(original_block.total_difficulty.value),
        );

        let mut recompressed_buffer = Vec::new();
        {
            let blocks = vec![recompressed_block];

            let new_group = EraEGroup::new(
                blocks,
                original_file.group.accumulator.clone(),
                original_file.group.block_index.clone(),
            );

            let new_file =
                EraEFile::new(new_group, EraEId::new(network, original_file.id.start_block, 1));

            let mut writer = EraEWriter::new(&mut recompressed_buffer);
            writer.write_file(&new_file)?;
        }

        let reader = EraEReader::new(Cursor::new(&recompressed_buffer));
        let recompressed_file = reader.read(network.to_string())?;

        let recompressed_first_block = &recompressed_file.group.blocks[0];
        let recompressed_header = recompressed_first_block.header.decode_header()?;

        assert_eq!(
            original_header.number, recompressed_header.number,
            "Block number should match after complete re-encoding cycle"
        );

        println!(
            "Block {block_number} full re-encoding/re-compression cycle verified successfully 🫡"
        );
    }

    println!("File {filename} roundtrip successful");
    Ok(())
}

#[test_case::test_case("mainnet-00000-a6860fef.erae"; "erae_roundtrip_mainnet_0")]
#[test_case::test_case("mainnet-00151-126e861b.erae"; "erae_roundtrip_mainnet_151")]
#[test_case::test_case("mainnet-01367-fa634aec.erae"; "erae_roundtrip_mainnet_1367")]
#[test_case::test_case("mainnet-01895-4373f22f.erae"; "erae_roundtrip_mainnet_1895")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_roundtrip_compression_encoding_mainnet(filename: &str) -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;
    test_erae_file_roundtrip(&downloader, filename, MAINNET).await
}

/// Download one real erae file and verify slim receipt and proof decoding
/// against actual mainnet data (era 1895 has plenty of transactions/receipts).
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_slim_receipts_and_proofs_from_real_file() -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;
    let file = downloader.open_erae_file("mainnet-01895-4373f22f.erae", MAINNET).await?;

    let block_count = file.group.blocks.len();
    println!("File has {block_count} blocks");

    // --- Slim receipts ---
    // Test a few blocks across the file
    let test_indices = [0, block_count / 4, block_count / 2, block_count - 1];
    for &idx in &test_indices {
        let block = &file.group.blocks[idx];
        let block_number = file.group.block_index.starting_number() + idx as u64;

        // Decompress slim receipts
        let decompressed = block.receipts.decompress()?;
        assert!(!decompressed.is_empty(), "Block {block_number} receipts should not be empty");

        // Decode as slim receipts (no bloom)
        let receipts: Vec<SlimReceipt> = block.receipts.decode()?;
        println!(
            "Block {block_number}: {} receipts, cumulative_gas={}",
            receipts.len(),
            receipts.last().map(|r| r.cumulative_gas_used).unwrap_or(0)
        );

        // Verify receipt fields are sane
        let mut prev_cumulative = 0;
        for (i, receipt) in receipts.iter().enumerate() {
            assert!(
                receipt.cumulative_gas_used >= prev_cumulative,
                "Block {block_number} receipt {i}: cumulative gas should be non-decreasing"
            );
            prev_cumulative = receipt.cumulative_gas_used;

            // Verify log addresses are non-zero if logs exist
            for log in &receipt.logs {
                assert!(
                    !log.address.is_zero() || log.data.topics().is_empty(),
                    "Block {block_number} receipt {i}: log with zero address should have no topics"
                );
            }
        }

        // Re-encode and verify roundtrip
        let recompressed = CompressedSlimReceipts::from_receipts(&receipts)?;
        let re_decoded: Vec<SlimReceipt> = recompressed.decode_receipts()?;
        assert_eq!(receipts, re_decoded, "Receipts should survive decode/encode/decode cycle");
    }

    // --- Proof encode/decode unit check ---
    for proof_type in [
        ProofType::BlockProofHistoricalHashesAccumulator,
        ProofType::BlockProofHistoricalRoots,
        ProofType::BlockProofHistoricalSummariesCapella,
        ProofType::BlockProofHistoricalSummariesDeneb,
    ] {
        let ssz_data = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let proof = Proof::encode(proof_type, &ssz_data)?;
        let entry = proof.to_entry();
        let recovered = Proof::from_entry(&entry)?;
        let (decoded_type, decoded_ssz) = recovered.decode()?;
        assert_eq!(decoded_type, proof_type);
        assert_eq!(decoded_ssz, ssz_data);
    }

    println!("Slim receipts and proof tests passed");
    Ok(())
}

#[test_case::test_case("sepolia-00000-8e3e7dc9.erae"; "erae_roundtrip_sepolia_0")]
#[test_case::test_case("sepolia-00074-3d475575.erae"; "erae_roundtrip_sepolia_74")]
#[test_case::test_case("sepolia-00173-b0373953.erae"; "erae_roundtrip_sepolia_173")]
#[test_case::test_case("sepolia-00182-959852e5.erae"; "erae_roundtrip_sepolia_182")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_roundtrip_compression_encoding_sepolia(filename: &str) -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;

    test_erae_file_roundtrip(&downloader, filename, SEPOLIA).await?;

    Ok(())
}
