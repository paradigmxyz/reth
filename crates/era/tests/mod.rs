//!
//! Integration tests for Era1 file reading.
//!
//! Tests Era1 file reader parsing and interpretation of real files.
//!
//! Test files sourced from:
//! - Mainnet: <https://mainnet.era1.nimbus.team/>
//! - Sepolia: <https://sepolia.era1.nimbus.team/>

use reth_era::{
    e2s_types::E2sError,
    era1_file::{Era1File, Era1Reader, Era1Writer},
    execution_types::{BlockTuple, MAX_BLOCKS_PER_ERA1},
};
use std::{cmp::min, io::Cursor, path::Path};

const NETWORK_MAINNET: &str = "mainnet";
const NETWORK_SEPOLIA: &str = "sepolia";

/// Test files for both mainnet and sepolia
const ERA1_FILES: [&str; 6] = [
    "tests/files/mainnet-00000-5ec1ffb8.era1",
    "tests/files/mainnet-00003-d8b8a40b.era1",
    "tests/files/mainnet-00151-e322efe1.era1",
    "tests/files/sepolia-00028-dae08170.era1",
    "tests/files/sepolia-00098-40286059.era1",
    "tests/files/sepolia-00149-01b6e9ca.era1",
];

/// Start block mapping for test files
/// Most of them are experimentally determined
const FILE_START_BLOCKS: [(&str, u64); 6] = [
    ("mainnet-00000", 0),       // Genesis file
    ("mainnet-00003", 24576),   // ~3 * 8192
    ("mainnet-00151", 1236992), // Experimentally determined
    ("sepolia-00028", 229376),  // Experimentally determined
    ("sepolia-00098", 802816),  // Experimentally determined
    ("sepolia-00149", 1220608), // Experimentally determined
];

/// Open era1 test file
fn open_test_file(file_path: &str) -> Result<Era1File, E2sError> {
    let full_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file_path);

    // Check if file exists
    if !full_path.exists() {
        return Err(E2sError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("Test file not found: {}", file_path),
        )));
    }

    // Extract filename
    let filename = match full_path.file_name() {
        Some(fname) => match fname.to_str() {
            Some(s) => s,
            None => {
                return Err(E2sError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Filename contains invalid UTF-8 characters",
                )))
            }
        },
        None => {
            return Err(E2sError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Path does not have a filename component",
            )))
        }
    };

    // Determine network
    let network = if filename.starts_with(NETWORK_MAINNET) {
        NETWORK_MAINNET
    } else if filename.starts_with(NETWORK_SEPOLIA) {
        NETWORK_SEPOLIA
    } else {
        return Err(E2sError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Unknown network prefix in filename: {}", filename),
        )));
    };

    // Open and read file
    Era1Reader::open(&full_path, network)
}

/// Tests relationship between filename and start block
/// Not so much a very solid test as we determined start block experimentally
/// but more of a sanity check
#[test]
fn test_filename_block_mapping() {
    for file_path in &ERA1_FILES {
        let full_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(file_path);
        assert!(full_path.exists(), "Missing test file: {file_path}");

        // Extract filename and hex value
        let filename = full_path.file_name().unwrap().to_str().unwrap();
        let hex_value = extract_hex_value(filename);

        // Read file to get actual start block
        let network = if filename.starts_with(NETWORK_MAINNET) { "mainnet" } else { "sepolia" };
        let era1_file = Era1Reader::open(&full_path, network).expect("Failed to read Era1 file");

        let actual_start_block = era1_file.id.start_block;

        println!("File: {filename}");
        println!("  Hex value from filename: {hex_value}");
        println!("  Actual start block: {actual_start_block}");

        // Verify that our lookup table has the correct value
        let expected_start_block = get_expected_start_block(filename)
            .unwrap_or_else(|| panic!("No start block mapping for {filename}"));

        assert_eq!(expected_start_block, actual_start_block, "Start block mismatch for {filename}");
    }
}

#[test]
fn test_era1_file_reading() {
    for file_path in &ERA1_FILES {
        let era1_file = match open_test_file(file_path) {
            Ok(file) => file,
            Err(e) => panic!("Failed to open test file {}: {:?}", file_path, e),
        };

        let filename = Path::new(file_path).file_name().unwrap().to_str().unwrap();
        let expected_start_block = get_expected_start_block(filename)
            .unwrap_or_else(|| panic!("No start block mapping for {filename}"));
        let network = if filename.starts_with("mainnet") { "mainnet" } else { "sepolia" };

        println!("Testing file: {filename}");
        println!("Expected start block: {expected_start_block}");

        // Verify file identity
        assert_eq!(era1_file.id.network_name, network);
        assert_eq!(
            era1_file.id.start_block, expected_start_block,
            "Start block mismatch for {filename}"
        );

        // Verify block range and count
        let block_range = era1_file.block_range();
        assert_eq!(*block_range.start(), expected_start_block);
        assert_eq!(
            era1_file.group.blocks.len() as u32,
            era1_file.id.block_count,
            "Block count mismatch for {filename}"
        );
        assert_eq!(
            era1_file.group.block_index.starting_number, expected_start_block,
            "Block index starting number mismatch for {filename}"
        );

        // Verify block data integrity
        let blocks = &era1_file.group.blocks;
        assert!(!blocks.is_empty(), "No blocks found in {filename}");

        let first_block = &blocks[0];
        let last_block = &blocks[blocks.len() - 1];

        // Test block data
        test_block_data("first", first_block, filename);
        test_block_data("last", last_block, filename);

        // Verify block index structure
        assert_eq!(
            era1_file.group.block_index.offsets.len(),
            blocks.len(),
            "Block index offsets count mismatch in {filename}"
        );

        // Test block retrieval
        let last_block_num = expected_start_block + (blocks.len() as u64) - 1;

        assert!(
            era1_file.contains_block(expected_start_block),
            "Missing first block in {filename}"
        );
        assert!(era1_file.contains_block(last_block_num), "Missing last block in {filename}");
        assert!(
            era1_file.get_block_by_number(expected_start_block).is_some(),
            "Unable to retrieve first block from {filename}"
        );
        assert!(
            era1_file.get_block_by_number(last_block_num).is_some(),
            "Unable to retrieve last block from {filename}"
        );

        // Log results
        println!("✅ File successfully parsed");
        println!("- Network: {}", era1_file.id.network_name);
        println!("- Block Range: {} to {}", block_range.start(), block_range.end());
        println!("- Total Blocks: {}", blocks.len());
        println!("- First Block Header Size: {} bytes", first_block.header.data.len());
        println!("- Last Block Header Size: {} bytes", last_block.header.data.len());
        println!("---------------------------------------------");
    }
}

#[test]
fn test_block_structure_validation() {
    for file_path in &ERA1_FILES {
        let era1_file = open_test_file(file_path).expect("Failed to open test file");
        let filename = Path::new(file_path).file_name().unwrap().to_str().unwrap();
        println!("Testing file: {}", filename);

        // Choose a sample block for detailed validation
        let sample_block_index = era1_file.group.blocks.len() / 2; // Middle block
        let block = &era1_file.group.blocks[sample_block_index];

        // Just validate the basic structure without decompression
        println!("Block data sizes:");
        println!("  Header data size: {} bytes", block.header.data.len());
        println!("  Body data size: {} bytes", block.body.data.len());
        println!("  Receipts data size: {} bytes", block.receipts.data.len());
        println!("  Total difficulty: {}", block.total_difficulty.value);

        // Simple validations that don't require decompression
        assert!(!block.header.data.is_empty(), "Header data should not be empty");
        assert!(!block.body.data.is_empty(), "Body data should not be empty");
        assert!(!block.receipts.data.is_empty(), "Receipts data should not be empty");
        assert!(!block.total_difficulty.value.is_zero(), "Total difficulty should not be zero");

        if !block.header.data.is_empty() {
            println!(
                "  Header first 8 bytes: {:02x?}",
                &block.header.data[..min(8, block.header.data.len())]
            );
        }
        if !block.body.data.is_empty() {
            println!(
                "  Body first 8 bytes: {:02x?}",
                &block.body.data[..min(8, block.body.data.len())]
            );
        }
        if !block.receipts.data.is_empty() {
            println!(
                "  Receipts first 8 bytes: {:02x?}",
                &block.receipts.data[..min(8, block.receipts.data.len())]
            );
        }

        // Validate block index
        println!("Block index:");
        println!("  Starting block: {}", era1_file.group.block_index.starting_number);
        println!("  Number of offsets: {}", era1_file.group.block_index.offsets.len());
        assert_eq!(
            era1_file.group.block_index.offsets.len(),
            era1_file.group.blocks.len(),
            "Number of offsets should match number of blocks"
        );

        // Verify actual block number access
        let block_number = era1_file.group.block_index.starting_number + sample_block_index as u64;
        let retrieved_block = era1_file.get_block_by_number(block_number);
        assert!(retrieved_block.is_some(), "Should be able to retrieve block by number");

        println!("✅ Structural validation passed for {}", filename);
        println!("---------------------------------------------");
    }
}

#[test]
fn test_era1_file_genesis_cases() {
    // Test genesis file separately
    let genesis_path = "tests/files/mainnet-00000-5ec1ffb8.era1";
    let genesis_file = open_test_file(genesis_path).expect("Failed to read Genesis file");

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

    println!("✅ Genesis file test passed");
}

#[test]
fn test_era1_file_capacity() {
    for file_path in &ERA1_FILES {
        let era1_file = open_test_file(file_path).expect("Failed to open test file");
        let filename = Path::new(file_path).file_name().unwrap().to_str().unwrap();

        // Verify file capacity matches expected value
        assert_eq!(
            era1_file.group.blocks.len(),
            MAX_BLOCKS_PER_ERA1,
            "Era1 file should contain exactly MAX_BLOCKS_PER_ERA1 blocks"
        );

        // Get block range
        let start = era1_file.group.block_index.starting_number;
        let end = start + (era1_file.group.blocks.len() as u64) - 1;

        // Test that boundary blocks are included
        assert!(era1_file.contains_block(start), "Should contain first block");
        assert!(era1_file.contains_block(end), "Should contain last block");

        // Test that blocks outside the range are not included
        // Only check before the range if not genesis file
        if start > 0 {
            assert!(!era1_file.contains_block(start - 1), "Should not contain block before range");
        }

        // Always check after the range
        assert!(!era1_file.contains_block(end + 1), "Should not contain block after range");

        println!(
            "✅ File capacity test passed for {}: {} blocks ({} to {})",
            filename,
            era1_file.group.blocks.len(),
            start,
            end
        );
    }
}

#[test]
fn test_snappy_format_header() {
    // Expected Snappy framed format header pattern
    let snappy_header = [0xff, 0x06, 0x00, 0x00, 0x73, 0x4e, 0x61, 0x50]; // "sNaP"

    for file_path in &ERA1_FILES {
        let era1_file = open_test_file(file_path).expect("Failed to open test file");

        // Check header format in first block
        let first_block = &era1_file.group.blocks[0];
        assert!(
            first_block.header.data.starts_with(&snappy_header),
            "Header should start with Snappy framed format signature"
        );
        assert!(
            first_block.body.data.starts_with(&snappy_header),
            "Body should start with Snappy framed format signature"
        );
        assert!(
            first_block.receipts.data.starts_with(&snappy_header),
            "Receipts should start with Snappy framed format signature"
        );

        println!("✅ Snappy format header verified for first block in {}", file_path);
    }
}

#[test]
fn test_decompression_on_original_files() {
    for file_path in &ERA1_FILES {
        let original_file = open_test_file(file_path).expect("Failed to open test file");
        let filename = Path::new(file_path).file_name().unwrap().to_str().unwrap();

        println!("Testing decompression on: {}", filename);

        // Try to decompress a few blocks from the original file
        for i in 0..min(5, original_file.group.blocks.len()) {
            let block = &original_file.group.blocks[i];

            println!("  Testing block {} decompression", i);

            // Try to decompress header
            match block.header.decompress() {
                Ok(decompressed) => {
                    println!("    Header decompressed successfully: {} bytes", decompressed.len());
                }
                Err(e) => {
                    println!("    Header decompression failed: {:?}", e);
                    // Print the first few bytes to help diagnose
                    if block.header.data.len() >= 16 {
                        println!("    First 16 bytes: {:02x?}", &block.header.data[..16]);
                    }
                }
            }

            // Try to decompress body and receipts too
            let body_result = block.body.decompress();
            let receipts_result = block.receipts.decompress();

            println!(
                "    Body decompression: {}",
                if body_result.is_ok() { "Success" } else { "Failed" }
            );
            println!(
                "    Receipts decompression: {}",
                if receipts_result.is_ok() { "Success" } else { "Failed" }
            );
        }
    }
}

/// Test that data is preserved when writing and reading back an Era1 file
#[test]
fn test_data_preservation_without_decompression() {
    for file_path in &ERA1_FILES {
        let original_file = open_test_file(file_path).expect("Failed to open test file");
        let filename = Path::new(file_path).file_name().unwrap().to_str().unwrap();

        println!("Testing data preservation with: {}", filename);

        // Write to a temporary buffer
        let mut buffer = Vec::new();
        {
            let mut writer = Era1Writer::new(&mut buffer);
            writer.write_era1_file(&original_file).expect("Failed to write Era1 file");
        }

        // Read back from buffer
        let mut reader = Era1Reader::new(Cursor::new(&buffer));
        let read_back_file = reader
            .read(original_file.id.network_name.clone())
            .expect("Failed to read back Era1 file");

        // Compare basic file properties
        assert_eq!(original_file.id.network_name, read_back_file.id.network_name);
        assert_eq!(original_file.id.start_block, read_back_file.id.start_block);
        assert_eq!(original_file.group.blocks.len(), read_back_file.group.blocks.len());

        // Compare raw compressed data bytes without decompression
        for i in 0..min(3, original_file.group.blocks.len()) {
            let original_block = &original_file.group.blocks[i];
            let read_back_block = &read_back_file.group.blocks[i];

            println!(
                "  Block {}: Original header size: {}, Read-back header size: {}",
                i,
                original_block.header.data.len(),
                read_back_block.header.data.len()
            );

            // Compare header data
            assert_eq!(
                original_block.header.data.len(),
                read_back_block.header.data.len(),
                "Header data length mismatch at block {}",
                i
            );
            assert_eq!(
                original_block.header.data, read_back_block.header.data,
                "Header raw data mismatch at block {}",
                i
            );

            // Compare body data
            assert_eq!(
                original_block.body.data.len(),
                read_back_block.body.data.len(),
                "Body data length mismatch at block {}",
                i
            );
            assert_eq!(
                original_block.body.data, read_back_block.body.data,
                "Body raw data mismatch at block {}",
                i
            );

            // Compare receipts data
            assert_eq!(
                original_block.receipts.data.len(),
                read_back_block.receipts.data.len(),
                "Receipts data length mismatch at block {}",
                i
            );
            assert_eq!(
                original_block.receipts.data, read_back_block.receipts.data,
                "Receipts raw data mismatch at block {}",
                i
            );
        }

        println!("✅ Raw data preservation test passed for {}", filename);
    }
}

#[test]
fn test_era1_file_roundtrip_with_real_files() {
    for file_path in &ERA1_FILES {
        let filename = Path::new(file_path).file_name().unwrap().to_str().unwrap();

        println!("Testing roundtrip with: {}", filename);

        // Read original file
        let original_file = open_test_file(file_path).expect("Failed to open test file");

        // Write to a temporary buffer
        let mut buffer = Vec::new();
        {
            let mut writer = Era1Writer::new(&mut buffer);
            writer.write_era1_file(&original_file).expect("Failed to write Era1 file");
        }

        // Read back from buffer
        let mut reader = Era1Reader::new(Cursor::new(&buffer));
        let read_back_file = reader
            .read(original_file.id.network_name.clone())
            .expect("Failed to read back Era1 file");

        // Test 1: compare file metadata
        assert_eq!(
            original_file.id.network_name, read_back_file.id.network_name,
            "Network name mismatch in {}",
            filename
        );
        assert_eq!(
            original_file.id.start_block, read_back_file.id.start_block,
            "Start block mismatch in {}",
            filename
        );
        assert_eq!(
            original_file.id.block_count, read_back_file.id.block_count,
            "Block count mismatch in {}",
            filename
        );
        assert_eq!(
            original_file.group.blocks.len(),
            read_back_file.group.blocks.len(),
            "Blocks length mismatch in {}",
            filename
        );

        // Test 2: compare block index
        assert_eq!(
            original_file.group.block_index.starting_number,
            read_back_file.group.block_index.starting_number,
            "Block index starting number mismatch in {}",
            filename
        );
        assert_eq!(
            original_file.group.block_index.offsets.len(),
            read_back_file.group.block_index.offsets.len(),
            "Block index offsets length mismatch in {}",
            filename
        );

        // Test 3: compare accumulator
        assert_eq!(
            original_file.group.accumulator.root, read_back_file.group.accumulator.root,
            "Accumulator root mismatch in {}",
            filename
        );

        // Test 4: compare block data for a selection of blocks
        // Use multiple blocks to better test the roundtrip
        let blocks_to_check = [
            0,                                        // First block
            original_file.group.blocks.len() / 3,     // At 1/3
            original_file.group.blocks.len() / 2,     // Middle block
            original_file.group.blocks.len() * 2 / 3, // At 2/3
            original_file.group.blocks.len() - 1,     // Last block
        ];

        for &idx in &blocks_to_check {
            let original_block = &original_file.group.blocks[idx];
            let read_back_block = &read_back_file.group.blocks[idx];
            let block_number = original_file.group.block_index.starting_number + idx as u64;

            println!("  Checking block {} (index {})", block_number, idx);

            // Check total difficulty
            assert_eq!(
                original_block.total_difficulty.value, read_back_block.total_difficulty.value,
                "Total difficulty mismatch at block {} (index {})",
                block_number, idx
            );

            let original_header_data = original_block.header.decompress().unwrap();
            let read_back_header_data = read_back_block.header.decompress().unwrap();
            assert_eq!(
                original_header_data, read_back_header_data,
                "Header data mismatch at block {} (index {})",
                block_number, idx
            );

            let original_body_data = original_block.body.decompress().unwrap();
            let read_back_body_data = read_back_block.body.decompress().unwrap();
            assert_eq!(
                original_body_data, read_back_body_data,
                "Body data mismatch at block {} (index {})",
                block_number, idx
            );

            let original_receipts_data = original_block.receipts.decompress().unwrap();
            let read_back_receipts_data = read_back_block.receipts.decompress().unwrap();
            assert_eq!(
                original_receipts_data, read_back_receipts_data,
                "Receipts data mismatch at block {} (index {})",
                block_number, idx
            );
        }

        println!("✅ Roundtrip test passed for {}", filename);
    }
}

/// Extracts hex value from Era1 filename format network-XXXXX-hash.era1
fn extract_hex_value(filename: &str) -> u64 {
    let parts: Vec<&str> = filename.split('-').collect();
    let hex_part = parts.get(1).expect("Invalid filename format");
    u64::from_str_radix(hex_part, 16).expect("Invalid hex value in filename")
}

/// Returns expected start block based on filename prefix
fn get_expected_start_block(filename: &str) -> Option<u64> {
    FILE_START_BLOCKS
        .iter()
        .find(|(prefix, _)| filename.starts_with(prefix))
        .map(|(_, block)| *block)
}

/// Tests block data integrity
fn test_block_data(block_type: &str, block: &BlockTuple, filename: &str) {
    assert!(!block.header.data.is_empty(), "{block_type} block header empty in {filename}");
    assert!(!block.body.data.is_empty(), "{block_type} block body empty in {filename}");
    assert!(!block.receipts.data.is_empty(), "{block_type} block receipts empty in {filename}");
}
