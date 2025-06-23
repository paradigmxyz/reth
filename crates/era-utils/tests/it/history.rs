use crate::{ClientWithFakeIndex, ITHACA_ERA_INDEX_URL};
use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_era_utils::{export, import, ExportConfig};
use reth_etl::Collector;
use reth_fs_util as fs;
use reth_provider::{test_utils::create_test_provider_factory, BlockNumReader, BlockReader};
use std::str::FromStr;
use tempfile::tempdir;

const EXPORT_FIRST_BLOCK: u64 = 0;
const EXPORT_BLOCK_PER_FILE: u64 = 250;
const EXPORT_TOTAL_BLOCKS: u64 = 1000;
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

    let imported_blocks = import(stream, &pf, &mut hash_collector).unwrap();

    assert_eq!(imported_blocks, 8191);
    let provider_ref = pf.provider_rw().unwrap().0;
    let best_block = provider_ref.best_block_number().unwrap();

    assert!(best_block <= 8191, "Best block {best_block} should not exceed imported count");

    for &block_num in &[0, 1, 2, 10, 50] {
        let block_exists = provider_ref.block_by_number(block_num).unwrap().is_some();
        assert!(block_exists, "Block {block_num} should exist after importing 8191 blocks");
    }

    let expected_files = EXPORT_TOTAL_BLOCKS.div_ceil(EXPORT_BLOCK_PER_FILE);

    let export_folder = tempdir().unwrap();
    let export_config = ExportConfig {
        dir: export_folder.path().to_path_buf(),
        first_block_number: EXPORT_FIRST_BLOCK,
        last_block_number: EXPORT_LAST_BLOCK,
        max_blocks_per_file: EXPORT_BLOCK_PER_FILE,
        network: "mainnet".to_string(),
    };

    let exported_files = export(&provider_ref, &export_config).expect("Export should succeed");

    assert_eq!(
        exported_files.len(),
        expected_files as usize,
        "Should create {expected_files} files for {EXPORT_TOTAL_BLOCKS} blocks with {EXPORT_BLOCK_PER_FILE} blocks per file"
    );

    for (i, file_path) in exported_files.iter().enumerate() {
        assert!(file_path.exists(), "File {} should exist", i + 1);

        let file_size = fs::metadata(file_path).unwrap().len();
        assert!(file_size > 0, "File {} should not be empty", i + 1);
        assert!(
            file_size > 500,
            "File {} should be substantial size, got {} bytes",
            i + 1,
            file_size
        );

        let file_name = file_path.file_name().unwrap().to_str().unwrap();
        assert!(file_name.starts_with("mainnet-"), "File {} should start with 'mainnet-'", i + 1);
        assert!(file_name.ends_with(".era1"), "File {} should end with '.era1'", i + 1);

        // Calculate expected filename: each file contains EXPORT_BLOCK_PER_FILE blocks, except
        // possibly the last one
        let file_start_block = EXPORT_FIRST_BLOCK + (i as u64 * EXPORT_BLOCK_PER_FILE);
        let remaining_blocks = EXPORT_TOTAL_BLOCKS - (i as u64 * EXPORT_BLOCK_PER_FILE);
        let blocks_in_this_file = std::cmp::min(EXPORT_BLOCK_PER_FILE, remaining_blocks);

        let expected_filename = format!("mainnet-{file_start_block}-{blocks_in_this_file}.era1");
        assert_eq!(file_name, expected_filename, "File {} should have correct name", i + 1);
    }

    // Verify total coverage matches expected range
    let total_blocks_exported = (EXPORT_LAST_BLOCK - EXPORT_FIRST_BLOCK) + 1;
    assert_eq!(
        total_blocks_exported, EXPORT_TOTAL_BLOCKS,
        "Should export exactly {EXPORT_TOTAL_BLOCKS} blocks (from {EXPORT_FIRST_BLOCK} to {EXPORT_LAST_BLOCK})"
    );
}
