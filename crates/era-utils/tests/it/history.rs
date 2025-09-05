use crate::{ClientWithFakeIndex, ITHACA_ERA_INDEX_URL};
use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era::execution_types::MAX_BLOCKS_PER_ERA1;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_era_utils::{export, import, ExportConfig};
use reth_etl::Collector;
use reth_fs_util as fs;
use reth_provider::{test_utils::create_test_provider_factory, BlockNumReader, BlockReader};
use std::str::FromStr;
use tempfile::tempdir;

const EXPORT_FIRST_BLOCK: u64 = 0;
const EXPORT_BLOCKS_PER_FILE: u64 = 250;
const EXPORT_TOTAL_BLOCKS: u64 = 900;
const EXPORT_LAST_BLOCK: u64 = EXPORT_FIRST_BLOCK + EXPORT_TOTAL_BLOCKS - 1;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_history_imports_from_fresh_state_successfully() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str(ITHACA_ERA_INDEX_URL).unwrap();

    // Directory where the ERA1 files will be downloaded to
    let folder = tempdir().unwrap();
    let folder = folder.path();

    let client = EraClient::new(ClientWithFakeIndex(Client::new()), url, folder);

    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let pf = create_test_provider_factory();

    init_genesis(&pf).unwrap();

    let folder = tempdir().unwrap();
    let folder = Some(folder.path().to_owned());
    let mut hash_collector = Collector::new(4096, folder);

    let expected_block_number = 8191;
    let actual_block_number = import(stream, &pf, &mut hash_collector).unwrap();

    assert_eq!(actual_block_number, expected_block_number);
}

/// Test that verifies the complete roundtrip from importing to exporting era1 files.
/// It validates :
/// - Downloads the first era1 file from ithaca's url and import the file data, into the database
/// - Exports blocks from database back to era1 format
/// - Ensure exported files have correct structure and naming
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_roundtrip_export_after_import() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str(ITHACA_ERA_INDEX_URL).unwrap();
    let download_folder = tempdir().unwrap();
    let download_folder = download_folder.path().to_owned().into_boxed_path();

    let client = EraClient::new(ClientWithFakeIndex(Client::new()), url, download_folder);
    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let pf = create_test_provider_factory();
    init_genesis(&pf).unwrap();

    let folder = tempdir().unwrap();
    let folder = Some(folder.path().to_owned());
    let mut hash_collector = Collector::new(4096, folder);

    // Import blocks from one era1 file into database
    let last_imported_block_height = import(stream, &pf, &mut hash_collector).unwrap();

    assert_eq!(last_imported_block_height, 8191);
    let provider_ref = pf.provider_rw().unwrap().0;
    let best_block = provider_ref.best_block_number().unwrap();

    assert!(best_block <= 8191, "Best block {best_block} should not exceed imported count");

    // Verify some blocks exist in the database
    for &block_num in &[0, 1, 2, 10, 50, 100, 5000, 8190, 8191] {
        let block_exists = provider_ref.block_by_number(block_num).unwrap().is_some();
        assert!(block_exists, "Block {block_num} should exist after importing 8191 blocks");
    }

    // The import was verified let's start the export!

    // 900 blocks will be exported from 0 to 899
    // It should be split into 3 files of 250 blocks each, and the last file with 150 blocks
    let export_folder = tempdir().unwrap();
    let export_config = ExportConfig {
        dir: export_folder.path().to_path_buf(),
        first_block_number: EXPORT_FIRST_BLOCK,      // 0
        last_block_number: EXPORT_LAST_BLOCK,        // 899
        max_blocks_per_file: EXPORT_BLOCKS_PER_FILE, // 250 blocks per file
        network: "mainnet".to_string(),
    };

    // Export blocks from database to era1 files
    let exported_files = export(&provider_ref, &export_config).expect("Export should succeed");

    // Calculate how many files we expect based on the configuration
    // We expect 4 files for 900 blocks: first 3 files with 250 blocks each,
    // then 150 for the last file
    let expected_files_number = EXPORT_TOTAL_BLOCKS.div_ceil(EXPORT_BLOCKS_PER_FILE);

    assert_eq!(
        exported_files.len(),
        expected_files_number as usize,
        "Should create {expected_files_number} files for {EXPORT_TOTAL_BLOCKS} blocks with {EXPORT_BLOCKS_PER_FILE} blocks per file"
    );

    for (i, file_path) in exported_files.iter().enumerate() {
        // Verify file exists and has content
        assert!(file_path.exists(), "File {} should exist", i + 1);
        let file_size = fs::metadata(file_path).unwrap().len();
        assert!(file_size > 0, "File {} should not be empty", i + 1);

        // Calculate expected file parameters
        let file_start_block = EXPORT_FIRST_BLOCK + (i as u64 * EXPORT_BLOCKS_PER_FILE);
        let remaining_blocks = EXPORT_TOTAL_BLOCKS - (i as u64 * EXPORT_BLOCKS_PER_FILE);
        let blocks_numbers_per_file = std::cmp::min(EXPORT_BLOCKS_PER_FILE, remaining_blocks);

        // Verify chunking : first 3 files have 250 blocks, last file has 150 blocks - 900 total
        let expected_blocks = if i < 3 { 250 } else { 150 };
        assert_eq!(
            blocks_numbers_per_file,
            expected_blocks,
            "File {} should contain exactly {} blocks, got {}",
            i + 1,
            expected_blocks,
            blocks_numbers_per_file
        );

        // Verify format: mainnet-{era_number:05}-{era_count:05}-{8hexchars}.era1
        let era_number = file_start_block / MAX_BLOCKS_PER_ERA1 as u64;

        // Era count is always 1 for this test, as we are only exporting one era
        let expected_prefix = format!("mainnet-{:05}-{:05}-", era_number, 1);

        let file_name = file_path.file_name().unwrap().to_str().unwrap();
        assert!(
            file_name.starts_with(&expected_prefix),
            "File {} should start with '{expected_prefix}', got '{file_name}'",
            i + 1
        );

        // Verify the hash part is 8 characters
        let hash_start = expected_prefix.len();
        let hash_end = file_name.len() - 5; // remove ".era1"
        let hash_part = &file_name[hash_start..hash_end];
        assert_eq!(
            hash_part.len(),
            8,
            "File {} hash should be 8 characters, got {} in '{}'",
            i + 1,
            hash_part.len(),
            file_name
        );
    }
}
