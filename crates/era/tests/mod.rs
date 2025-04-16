//! Integration tests
//!
//! These tests use the `reth-era-downloader` client to download `.era1` files temporarily
//! and verify that we can correctly read and decompress their data.

use alloy_consensus::Header;
use alloy_primitives::{Bytes, U256};
use reqwest::{Client, Url};
use reth_era::{
    e2s_types::E2sError,
    era1_file::{Era1File, Era1Reader, Era1Writer},
};
use reth_era_downloader::EraClient;
use std::{
    collections::HashMap,
    io::Cursor,
    path::{Path, PathBuf},
    str::FromStr,
    sync::{Arc, Mutex},
};
use tempfile::TempDir;

/// Default nimbus mainnet url
/// for downloading mainnet `.era1` files
const MAINNET_URL: &str = "https://mainnet.era1.nimbus.team/";
const MAINNET: &str = "mainnet";

/// Succint list of mainnet files we want to download
/// from <https://mainnet.era1.nimbus.team/>
/// for testing purposes
const ERA1_MAINNET_FILES_NAMES: [&str; 6] = [
    "mainnet-00000-5ec1ffb8.era1",
    "mainnet-00003-d8b8a40b.era1",
    "mainnet-00151-e322efe1.era1",
    "mainnet-00293-0d6c5812.era1",
    "mainnet-00443-ea71b6f9.era1",
    "mainnet-01367-d7efc68f.era1",
];

/// Utility for downloading `.era1` files for tests
/// in a temporary directory
/// and caching them in memory
#[derive(Debug)]
pub struct Era1TestDownloader {
    /// Temporary directory for storing downloaded files
    temp_dir: TempDir,
    /// Cache mapping file names to their paths
    file_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl Era1TestDownloader {
    /// Create a new downloader instance with a temporary directory
    pub async fn new() -> Result<Self, E2sError> {
        let temp_dir = TempDir::new().map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to create temp directory: {}", e)))
        })?;

        Ok(Self { temp_dir, file_cache: Arc::new(Mutex::new(HashMap::new())) })
    }

    /// Get the temporary directory path
    pub fn temp_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Download a specific .era1 file by name
    pub async fn download_file(&self, filename: &str) -> Result<PathBuf, E2sError> {
        // check cache first
        {
            let cache = self.file_cache.lock().unwrap();
            if let Some(path) = cache.get(filename) {
                return Ok(path.clone());
            }
        }

        // check if the filename is supported
        if !ERA1_MAINNET_FILES_NAMES.contains(&filename) {
            return Err(E2sError::Io(std::io::Error::other(format!(
                "Unknown file: {}. Only the following files are supported: {:?}",
                filename, ERA1_MAINNET_FILES_NAMES
            ))));
        }

        // initialize the client and build url config
        let url = Url::from_str(MAINNET_URL).map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to parse URL: {}", e)))
        })?;

        let folder = self.temp_dir.path().to_owned().into_boxed_path();

        // set up the client
        let client = EraClient::new(Client::new(), url, folder);

        // set up the file list, required before we can download files
        client.fetch_file_list().await.map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to fetch file list: {}", e)))
        })?;

        // create an url for the file
        let file_url = Url::parse(&format!("{}{}", MAINNET_URL, filename)).map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to create URL: {}", e)))
        })?;

        // download the file
        let mut client = client;
        let downloaded_path = client.download_to_file(file_url).await.map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to download file: {}", e)))
        })?;

        // update the cache
        {
            let mut cache = self.file_cache.lock().unwrap();
            cache.insert(filename.to_string(), downloaded_path.to_path_buf());
        }

        Ok(downloaded_path.to_path_buf())
    }

    /// open .era1 file, downloading it if necessary
    pub async fn open_era1_file(
        &self,
        filename: &str,
        network: &str,
    ) -> Result<Era1File, E2sError> {
        let path = self.download_file(filename).await?;
        Era1Reader::open(&path, network)
    }
}

/// Open a test file by name,
/// downloading only if it is necessary
pub async fn open_test_file(
    file_path: &str,
    downloader: &Era1TestDownloader,
    network: &str,
) -> Result<Era1File, E2sError> {
    let filename =
        Path::new(file_path).file_name().and_then(|os_str| os_str.to_str()).ok_or_else(|| {
            E2sError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid file path: {}", file_path),
            ))
        })?;

    downloader.open_era1_file(filename, network).await
}

#[tokio::test(flavor = "multi_thread")]
async fn test_era1_file_decompression_and_decoding() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new().await.expect("Failed to create downloader");

    for &filename in &ERA1_MAINNET_FILES_NAMES {
        println!("\nTesting file: {}", filename);
        let file = open_test_file(filename, &downloader, MAINNET).await?;

        // Test block decompression across different positions in the file
        let test_block_indices = [
            0,                           // First block
            file.group.blocks.len() / 2, // Middle block
            file.group.blocks.len() - 1, // Last block
        ];

        for &block_idx in &test_block_indices {
            let block = &file.group.blocks[block_idx];
            let block_number = file.group.block_index.starting_number + block_idx as u64;

            println!(
                "\n  Testing block {}, compressed body size: {} bytes",
                block_number,
                block.body.data.len()
            );

            // Test header decompression and decoding
            let header_data = block.header.decompress()?;
            assert!(
                !header_data.is_empty(),
                "Block {} header decompression should produce non-empty data",
                block_number
            );

            let header = block.header.decode_header()?;
            assert_eq!(
                header.number, block_number,
                "Decoded header should have correct block number"
            );
            println!("Header decompression and decoding successful");

            // Test body decompression
            let body_data = block.body.decompress()?;
            assert!(
                !body_data.is_empty(),
                "Block {} body decompression should produce non-empty data",
                block_number
            );
            println!("Body decompression successful ({} bytes)", body_data.len());

            // Try body decoding
            match block.body.decode_body::<alloy_primitives::Bytes, alloy_consensus::Header>() {
                Ok(body) => {
                    println!(
                        "Body decoding successful: {} transactions, {} ommers, withdrawals: {}",
                        body.transactions.len(),
                        body.ommers.len(),
                        body.withdrawals.is_some()
                    );
                }
                Err(e) => {
                    println!("Body decoding failed: {}", e);
                }
            }

            // Test receipts decompression
            let receipts_data = block.receipts.decompress()?;
            assert!(
                !receipts_data.is_empty(),
                "Block {} receipts decompression should produce non-empty data",
                block_number
            );
            println!("Receipts decompression successful ({} bytes)", receipts_data.len());

            assert!(
                block.total_difficulty.value > U256::ZERO,
                "Block {} should have non-zero difficulty",
                block_number
            );
            println!("Total difficulty verified: {}", block.total_difficulty.value);
        }

        // Test round-trip serialization
        println!("\n  Testing data preservation roundtrip...");
        let mut buffer = Vec::new();
        {
            let mut writer = Era1Writer::new(&mut buffer);
            writer.write_era1_file(&file)?;
        }

        // Read back from buffer
        let mut reader = Era1Reader::new(Cursor::new(&buffer));
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
            let block_number = file.group.block_index.starting_number + idx as u64;

            // Test that decompressed data is identical
            assert_eq!(
                original_block.header.decompress()?,
                read_back_block.header.decompress()?,
                "Header data should be identical for block {}",
                block_number
            );

            assert_eq!(
                original_block.body.decompress()?,
                read_back_block.body.decompress()?,
                "Body data should be identical for block {}",
                block_number
            );

            assert_eq!(
                original_block.receipts.decompress()?,
                read_back_block.receipts.decompress()?,
                "Receipts data should be identical for block {}",
                block_number
            );

            assert_eq!(
                original_block.total_difficulty.value, read_back_block.total_difficulty.value,
                "Total difficulty should be identical for block {}",
                block_number
            );
        }
    }

    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn test_genesis_block_decompression() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new().await?;

    let file = downloader.open_era1_file("mainnet-00000-5ec1ffb8.era1", MAINNET).await?;

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

        let body = block.body.decode_body::<Bytes, Header>()?;
        println!("Successfully decoded body with {} transactions", body.transactions.len());
        println!("Block has {} ommers/uncles", body.ommers.len());

        // For genesis era blocks, there should be no transactions or ommers
        assert_eq!(body.transactions.len(), 0, "Genesis era block should have no transactions");
        assert_eq!(body.ommers.len(), 0, "Genesis era block should have no ommers");

        // Check for withdrawals, should be `None` for genesis era blocks
        assert!(body.withdrawals.is_none(), "Genesis era block should have no withdrawals");

        let header = block.header.decode_header()?;
        assert_eq!(header.number, block_number, "Header should have correct block number");
        println!("Successfully decoded header for block {}", block_number);

        // Test total difficulty value
        let td = block.total_difficulty.value;
        println!("Block {} total difficulty: {}", block_number, td);
    }

    Ok(())
}
