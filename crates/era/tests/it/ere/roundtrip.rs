//! Roundtrip tests for `.erae` files.
//!
//! These tests download mainnet `.erae` files spread across history (and the merge) from
//! <https://data.ethpandaops.io/erae/mainnet/> and, for a sample of blocks, verify the full
//! lifecycle: read → write the file back and read it again (file roundtrip), then for each
//! component decompress → decode → re-encode → re-compress and confirm nothing is lost.

use alloy_consensus::{BlockBody, Header};
use reth_era::{
    common::file_ops::{EraFileFormat, StreamReader, StreamWriter},
    ere::{
        file::{EreFile, EreReader, EreWriter},
        types::{
            execution::{
                BlockTuple, CompressedBody, CompressedHeader, CompressedSlimReceipts, Proof,
                TotalDifficulty,
            },
            group::{DynamicBlockIndex, EreGroup, EreId},
        },
    },
};
use reth_ethereum_primitives::TransactionSigned;
use std::io::Cursor;

use crate::{EraTestDownloader, MAINNET};

async fn test_ere_file_roundtrip(
    downloader: &EraTestDownloader,
    filename: &str,
    network: &str,
) -> eyre::Result<()> {
    println!("\nTesting roundtrip for file: {filename}");

    let original_file = downloader.open_ere_file(filename, network).await?;

    // Write the whole file to a buffer and read it back.
    let mut buffer = Vec::new();
    {
        let mut writer = EreWriter::new(&mut buffer);
        writer.write_file(&original_file)?;
    }
    let roundtrip_file = EreReader::new(Cursor::new(&buffer)).read(network.to_string())?;

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
    assert_eq!(
        original_file.group.index, roundtrip_file.group.index,
        "Block index should match after roundtrip"
    );

    // Test the first, middle and last block.
    let blocks_len = original_file.group.blocks.len();
    let test_block_indices = [0, blocks_len / 2, blocks_len - 1];

    for &i in &test_block_indices {
        let original = &original_file.group.blocks[i];
        let roundtrip = &roundtrip_file.group.blocks[i];
        let block_number = original_file.group.index.starting_number() + i as u64;

        println!("Testing roundtrip for block {block_number}");

        // --- File roundtrip: compressed bytes / values must be identical ---
        let original_header_data = original.header.decompress()?;
        let original_body_data = original.body.decompress()?;
        assert_eq!(
            original_header_data,
            roundtrip.header.decompress()?,
            "Block {block_number} header data should be identical after roundtrip"
        );
        assert_eq!(
            original_body_data,
            roundtrip.body.decompress()?,
            "Block {block_number} body data should be identical after roundtrip"
        );
        assert_eq!(
            original.receipts.as_ref().map(|r| r.decompress()).transpose()?,
            roundtrip.receipts.as_ref().map(|r| r.decompress()).transpose()?,
            "Block {block_number} receipts should be identical after roundtrip"
        );
        assert_eq!(
            original.total_difficulty.as_ref().map(|d| d.value),
            roundtrip.total_difficulty.as_ref().map(|d| d.value),
            "Block {block_number} total difficulty should be identical after roundtrip"
        );
        assert_eq!(
            original.proof.as_ref().map(|p| &p.data),
            roundtrip.proof.as_ref().map(|p| &p.data),
            "Block {block_number} proof should be identical after roundtrip"
        );

        // --- Decode every component ---
        let original_header = original.header.decode_header()?;
        assert_eq!(
            original_header.number,
            roundtrip.header.decode_header()?.number,
            "Block number should match after roundtrip decoding"
        );

        let original_body = decode_body(&original_body_data)?;
        assert_eq!(
            original_body.transactions.len(),
            decode_body(&roundtrip.body.decompress()?)?.transactions.len(),
            "Transaction count should match after roundtrip"
        );

        let original_receipts =
            original.receipts.as_ref().map(|r| r.decode_receipts()).transpose()?;
        let original_proof = original.proof.as_ref().map(|p| p.decode()).transpose()?;

        // --- Re-encode / re-compress each component and confirm it survives ---
        println!("Testing full re-encoding/re-compression cycle for block {block_number}");

        let recompressed_header = CompressedHeader::from_header(&original_header)?;
        assert_eq!(
            recompressed_header.decompress()?,
            original_header_data,
            "Re-compressed header should match original after full cycle"
        );

        let recompressed_body = CompressedBody::from_body(&original_body)?;
        assert_eq!(
            decode_body(&recompressed_body.decompress()?)?.transactions.len(),
            original_body.transactions.len(),
            "Transaction count should match after re-compression"
        );

        let mut recompressed_block = BlockTuple::new(recompressed_header, recompressed_body);

        if let Some(receipts) = &original_receipts {
            let recompressed = CompressedSlimReceipts::from_receipts(receipts)?;
            assert_eq!(
                &recompressed.decode_receipts()?,
                receipts,
                "Block {block_number} receipts should survive re-encode"
            );
            recompressed_block = recompressed_block.with_receipts(recompressed);
        }
        if let Some(total_difficulty) = &original.total_difficulty {
            recompressed_block = recompressed_block
                .with_total_difficulty(TotalDifficulty::new(total_difficulty.value));
        }
        if let Some((proof_type, ssz_proof)) = &original_proof {
            let recompressed = Proof::encode(*proof_type, ssz_proof)?;
            assert_eq!(
                recompressed.decode()?,
                (*proof_type, ssz_proof.clone()),
                "Block {block_number} proof should survive re-encode"
            );
            recompressed_block = recompressed_block.with_proof(recompressed);
        }

        // Write the single recompressed block to a new file and read it back.
        let component_count = recompressed_block.component_count();
        let index = DynamicBlockIndex::new(
            block_number,
            component_count,
            vec![0; component_count as usize],
        );
        let new_group =
            EreGroup::new(vec![recompressed_block], original_file.group.accumulator.clone(), index);
        let new_file = EreFile::new(new_group, EreId::new(network, block_number, 1));

        let mut recompressed_buffer = Vec::new();
        {
            let mut writer = EreWriter::new(&mut recompressed_buffer);
            writer.write_file(&new_file)?;
        }
        let recompressed_file =
            EreReader::new(Cursor::new(&recompressed_buffer)).read(network.to_string())?;
        let recompressed_first = &recompressed_file.group.blocks[0];

        assert_eq!(
            recompressed_first.header.decode_header()?.number,
            original_header.number,
            "Block number should match after complete re-encoding cycle"
        );
        assert_eq!(
            recompressed_first.header.decompress()?,
            original_header_data,
            "Header data should match after complete re-encoding cycle"
        );

        println!("Block {block_number} full cycle verified successfully 🫡");
    }

    println!("File {filename} roundtrip successful");
    Ok(())
}

/// Decode a decompressed body into a typed [`BlockBody`].
fn decode_body(data: &[u8]) -> eyre::Result<BlockBody<TransactionSigned>> {
    Ok(CompressedBody::decode_body_from_decompressed::<TransactionSigned, Header>(data)?)
}

#[test_case::test_case("mainnet-00000-a6860fef.erae"; "ere_roundtrip_mainnet_0_premerge")]
#[test_case::test_case("mainnet-00500-365ebdb8.erae"; "ere_roundtrip_mainnet_500_premerge")]
#[test_case::test_case("mainnet-01500-8f81d882.erae"; "ere_roundtrip_mainnet_1500_premerge")]
#[test_case::test_case("mainnet-01900-34196a7e.erae"; "ere_roundtrip_mainnet_1900_postmerge")]
#[test_case::test_case("mainnet-02500-73f28496.erae"; "ere_roundtrip_mainnet_2500_postmerge")]
#[test_case::test_case("mainnet-02992-d3f7961d.erae"; "ere_roundtrip_mainnet_2992_postmerge")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "download intensive"]
async fn test_roundtrip_compression_encoding_mainnet(filename: &str) -> eyre::Result<()> {
    let downloader = EraTestDownloader::new().await?;
    test_ere_file_roundtrip(&downloader, filename, MAINNET).await
}
