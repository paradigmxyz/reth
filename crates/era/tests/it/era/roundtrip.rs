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

use consensus_types::{
    BeaconState, ChainSpec, ForkVersionDecode, MainnetEthSpec, SignedBeaconBlock,
};
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
use ssz::Encode;
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
    let is_mainnet = network == MAINNET;

    if original_file.group.is_genesis() {
        println!("Genesis era detected, using special handling");
        assert_eq!(original_file.group.blocks.len(), 0, "Genesis should have no blocks");
        assert!(
            original_file.group.slot_index.is_none(),
            "Genesis should not have block slot index"
        );

        let state_data = original_file.group.era_state.decompress()?;
        println!("  Genesis state decompressed: {} bytes", state_data.len());

        // Verify ssz decode/encode only on mainnet, tesnets specs are not supported :'(
        if is_mainnet {
            let spec = ChainSpec::mainnet();
            let beacon_state = BeaconState::<MainnetEthSpec>::from_ssz_bytes(&state_data, &spec)
                .map_err(|e| eyre::eyre!("Failed to decode genesis state: {e:?}"))?;

            // Verify ssz roundtrip
            let re_encoded = beacon_state.as_ssz_bytes();
            assert_eq!(
                state_data, re_encoded,
                "Genesis state SSZ should be identical after decode/encode"
            );
        }

        // File roundtrip test
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

    // non genesis start
    let original_state_data = original_file.group.era_state.decompress()?;

    let fork_name = if is_mainnet {
        let spec = ChainSpec::mainnet();
        let beacon_state =
            BeaconState::<MainnetEthSpec>::from_ssz_bytes(&original_state_data, &spec)
                .map_err(|e| eyre::eyre!("Failed to decode beacon state: {e:?}"))?;

        let fork = beacon_state.fork_name_unchecked();

        // Verify ssz roundtrip
        let re_encoded = beacon_state.as_ssz_bytes();
        assert_eq!(
            original_state_data, re_encoded,
            "State SSZ should be identical after decode/encode"
        );

        Some(fork)
    } else {
        None
    };

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

        let original_block_data = original_block.decompress()?;
        let roundtrip_block_data = roundtrip_block.decompress()?;

        // Verify file roundtrip preserves data
        assert_eq!(
            original_block_data, roundtrip_block_data,
            "Block {block_idx} data should be identical after file roundtrip"
        );

        // Verify ssz decode/encode roundtrip, still only for mainnet only
        if let Some(fork) = fork_name {
            let signed_block = SignedBeaconBlock::<MainnetEthSpec>::from_ssz_bytes_by_fork(
                &original_block_data,
                fork,
            )
            .map_err(|e| eyre::eyre!("Failed to decode block {block_idx}: {e:?}"))?;

            let slot = signed_block.message().slot();

            // Verify SSZ roundtrip
            let re_encoded = signed_block.as_ssz_bytes();
            assert_eq!(
                original_block_data, re_encoded,
                "Block at slot {slot} SSZ should be identical after decode/encode"
            );
        }

        // Verify compression roundtrip
        let recompressed_block = CompressedSignedBeaconBlock::from_ssz(&original_block_data)?;
        let recompressed_block_data = recompressed_block.decompress()?;

        assert_eq!(
            original_block_data, recompressed_block_data,
            "Block {block_idx} should be identical after re-compression cycle"
        );
    }

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

    let reader = EraReader::new(Cursor::new(&reconstructed_buffer));
    let reconstructed_file = reader.read(network.to_string())?;

    assert_eq!(
        original_file.group.blocks.len(),
        reconstructed_file.group.blocks.len(),
        "Block count should match after full reconstruction"
    );

    // Verify all reconstructed blocks match
    for (idx, (orig, recon)) in
        original_file.group.blocks.iter().zip(reconstructed_file.group.blocks.iter()).enumerate()
    {
        assert_eq!(
            orig.decompress()?,
            recon.decompress()?,
            "Block {idx} should match after full reconstruction"
        );
    }

    // Verify reconstructed state matches
    assert_eq!(
        original_state_data,
        reconstructed_file.group.era_state.decompress()?,
        "State should match after full reconstruction"
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
    test_era_file_roundtrip(&downloader, filename, HOODI).await
}
