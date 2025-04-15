//! Integration tests
//!
//! These tests use the `reth-era-downloader` client to download `.era1` files
//! temporarily and verify that we can correctly read and decompress their data.

use alloy_primitives::{B256, U256};
use reqwest::{Client, Url};
use reth_era::{
    e2s_types::E2sError,
    era1_file::{Era1File, Era1Reader, Era1Writer},
    execution_types::MAX_BLOCKS_PER_ERA1,
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
use tokio::runtime::Runtime;

/// Default nimbus mainnet url
/// for downloading mainnet `.era1` files
const MAINNET_URL: &str = "https://mainnet.era1.nimbus.team/";

/// Succint list of mainnet files we want to download
/// from <https://mainnet.era1.nimbus.team/>
/// for testing purposes
const ERA1_FILES_NAMES: [&str; 3] =
    ["mainnet-00000-5ec1ffb8.era1", "mainnet-00003-d8b8a40b.era1", "mainnet-00151-e322efe1.era1"];

/// Helper for downloading `.era1` files for tests
/// in a temporary directory
/// and caching them in memory
#[derive(Debug)]
pub struct Era1TestDownloader {
    temp_dir: TempDir,
    runtime: Runtime,
    file_cache: Arc<Mutex<HashMap<String, PathBuf>>>,
}

impl Era1TestDownloader {
    /// Create a new downloader instance with a temporary directory
    pub fn new() -> Result<Self, E2sError> {
        let runtime = Runtime::new().map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to create tokio runtime: {}", e)))
        })?;

        let temp_dir = TempDir::new().map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to create temp directory: {}", e)))
        })?;

        Ok(Self { temp_dir, runtime, file_cache: Arc::new(Mutex::new(HashMap::new())) })
    }

    /// Get the temporary directory path
    pub fn temp_dir(&self) -> &Path {
        self.temp_dir.path()
    }

    /// Download a specific .era1 file by name
    pub fn download_file(&self, filename: &str) -> Result<PathBuf, E2sError> {
        // check cache first
        {
            let cache = self.file_cache.lock().unwrap();
            if let Some(path) = cache.get(filename) {
                return Ok(path.clone());
            }
        }

        // check if the filename is supported
        if !ERA1_FILES_NAMES.contains(&filename) {
            return Err(E2sError::Io(std::io::Error::other(format!(
                "Unknown file: {}. Only the following files are supported: {:?}",
                filename, ERA1_FILES_NAMES
            ))));
        }

        // initialize the client and build url config
        let url = Url::from_str(MAINNET_URL).map_err(|e| {
            E2sError::Io(std::io::Error::other(format!("Failed to parse URL: {}", e)))
        })?;

        let folder = self.temp_dir.path().to_owned().into_boxed_path();

        let downloaded_path = self.runtime.block_on(async {
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
            let path = client.download_to_file(file_url).await.map_err(|e| {
                E2sError::Io(std::io::Error::other(format!("Failed to download file: {}", e)))
            })?;

            Ok::<_, E2sError>(path)
        })?;

        // update the cache
        {
            let mut cache = self.file_cache.lock().unwrap();
            cache.insert(filename.to_string(), downloaded_path.to_path_buf());
        }

        Ok(downloaded_path.to_path_buf())
    }

    /// open .era1 file, downloading it if necessary
    pub fn open_era1_file(&self, filename: &str) -> Result<Era1File, E2sError> {
        let path = self.download_file(filename)?;
        Era1Reader::open(&path, "mainnet")
    }
}

/// Open a test file by name,
/// downloading only if it is necessary
pub fn open_test_file(
    file_path: &str,
    downloader: &Era1TestDownloader,
) -> Result<Era1File, E2sError> {
    let filename =
        Path::new(file_path).file_name().and_then(|os_str| os_str.to_str()).ok_or_else(|| {
            E2sError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid file path: {}", file_path),
            ))
        })?;

    downloader.open_era1_file(filename)
}

#[test]
fn test_decompression_on_original_files() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new().expect("Failed to create downloader");

    for &filename in &ERA1_FILES_NAMES {
        println!("Testing decompression on: {}", filename);
        let original_file = open_test_file(filename, &downloader)?;

        // Test block decompression across different positions in the file
        let test_block_indices = [
            0,                                    // First block
            original_file.group.blocks.len() / 2, // Middle block
            original_file.group.blocks.len() - 1, // Last block
        ];

        for &block_idx in &test_block_indices {
            let block = &original_file.group.blocks[block_idx];
            let block_number = original_file.group.block_index.starting_number + block_idx as u64;

            // Test header decompression
            let header_data = block.header.decompress()?;
            assert!(
                !header_data.is_empty(),
                "Block {} header decompression should produce non-empty data",
                block_number
            );

            // Test header decoding
            let header = block.header.decode_header()?;
            assert_eq!(
                header.number, block_number,
                "Decoded header should have correct block number"
            );

            // Test body decompression
            let body_data = block.body.decompress()?;
            assert!(
                !body_data.is_empty(),
                "Block {} body decompression should produce non-empty data",
                block_number
            );

            // Test receipts decompression
            let receipts_data = block.receipts.decompress()?;
            assert!(
                !receipts_data.is_empty(),
                "Block {} receipts decompression should produce non-empty data",
                block_number
            );

            // Verify difficulty data
            assert!(
                block.total_difficulty.value > U256::ZERO,
                "Block {} should have non-zero difficulty",
                block_number
            );

            println!("Block {} decompression successful", block_number);
        }

        println!("All blocks successfully decompressed for {}", filename);
    }

    Ok(())
}

#[test]
fn test_era1_file_genesis_cases() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new()?;
    let genesis_file = downloader.open_era1_file(ERA1_FILES_NAMES[0])?;

    println!("Testing genesis file: {:?}", genesis_file);

    // For genesis file, verify special properties
    assert_eq!(
        genesis_file.group.block_index.starting_number, 0,
        "Genesis should start at block 0"
    );
    assert_eq!(
        genesis_file.group.blocks.len(),
        MAX_BLOCKS_PER_ERA1,
        "Genesis should have MAX_BLOCKS"
    );
    assert!(genesis_file.contains_block(0), "Genesis should contain block 0");
    assert!(genesis_file.contains_block(8191), "Genesis should contain block 8191");
    assert!(!genesis_file.contains_block(8192), "Genesis should not contain block 8192");

    // genesis-specific data
    if let Some(genesis_block) = genesis_file.get_block_by_number(0) {
        let header = genesis_block.header.decode_header()?;
        assert_eq!(header.number, 0, "Genesis block number should be 0");

        // verify empty parent hash
        assert_eq!(header.parent_hash, B256::ZERO, "Genesis block should have zero parent hash");

        // verify difficulty
        let td = genesis_block.total_difficulty.value;
        assert!(td > U256::ZERO, "Genesis total difficulty should be non-zero");
    }

    // Test second block of genesis file
    if let Some(block_1) = genesis_file.get_block_by_number(1) {
        let header = block_1.header.decode_header()?;
        assert_eq!(header.number, 1, "Second block number should be 1");
    }
    Ok(())
}

#[test]
fn test_data_preservation_roundtrip() -> Result<(), E2sError> {
    let downloader = Era1TestDownloader::new()?;

    for &filename in &ERA1_FILES_NAMES {
        println!("Testing data preservation roundtrip: {}", filename);

        // open original file
        let original_file = downloader.open_era1_file(filename)?;

        let mut buffer = Vec::new();
        {
            let mut writer = Era1Writer::new(&mut buffer);
            writer.write_era1_file(&original_file)?;
        }

        // read back from buffer
        let mut reader = Era1Reader::new(Cursor::new(&buffer));
        let read_back_file = reader.read(original_file.id.network_name.clone())?;

        // compare file metadata
        assert_eq!(
            original_file.id.network_name, read_back_file.id.network_name,
            "Network name should be preserved"
        );
        assert_eq!(
            original_file.id.start_block, read_back_file.id.start_block,
            "Start block should be preserved"
        );
        assert_eq!(
            original_file.group.blocks.len(),
            read_back_file.group.blocks.len(),
            "Block count should be preserved"
        );

        assert_eq!(
            original_file.group.block_index.starting_number,
            read_back_file.group.block_index.starting_number,
            "Block index starting number should be preserved"
        );

        assert_eq!(
            original_file.group.accumulator.root, read_back_file.group.accumulator.root,
            "Accumulator root should be preserved"
        );

        // Test a sample of blocks for data preservation
        let blocks_to_test = [
            0,                                    // First block
            original_file.group.blocks.len() / 2, // Middle block
            original_file.group.blocks.len() - 1, // Last block
        ];

        for &idx in &blocks_to_test {
            let original_block = &original_file.group.blocks[idx];
            let read_back_block = &read_back_file.group.blocks[idx];
            let block_number = original_file.group.block_index.starting_number + idx as u64;

            // Compare header data after decompression
            let original_header_data = original_block.header.decompress()?;
            let read_back_header_data = read_back_block.header.decompress()?;
            assert_eq!(
                original_header_data, read_back_header_data,
                "Header data should be preserved for block {}",
                block_number
            );

            // Compare body data after decompression
            let original_body_data = original_block.body.decompress()?;
            let read_back_body_data = read_back_block.body.decompress()?;
            assert_eq!(
                original_body_data, read_back_body_data,
                "Body data should be preserved for block {}",
                block_number
            );

            // Compare receipts data after decompression
            let original_receipts_data = original_block.receipts.decompress()?;
            let read_back_receipts_data = read_back_block.receipts.decompress()?;
            assert_eq!(
                original_receipts_data, read_back_receipts_data,
                "Receipts data should be preserved for block {}",
                block_number
            );

            // Compare total difficulty
            assert_eq!(
                original_block.total_difficulty.value, read_back_block.total_difficulty.value,
                "Total difficulty should be preserved for block {}",
                block_number
            );
        }
    }

    Ok(())
}
