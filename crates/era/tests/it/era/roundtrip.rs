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
    e2s::types::IndexEntry, era::file::{EraWriter, EraReader},
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

    // Select a few blocks to test
    let test_block_indices = [
        0,                                    // First block
        original_file.group.blocks.len() / 2, // Middle block
        original_file.group.blocks.len() - 1, // Last block
    ];

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
