use crate::{ClientWithFakeIndex, FileMeta, ITHACA_ERA_INDEX_URL};
use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era::era1::types::execution::MAX_BLOCKS_PER_ERA1;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_era_utils::{export, import, Era1, Ere, ExportConfig};
use reth_etl::Collector;
use reth_fs_util as fs;
use reth_primitives_traits::Block as _;
use reth_provider::{
    test_utils::create_test_provider_factory, BlockNumReader, BlockReader, ReceiptProvider,
    StaticFileProviderFactory,
};
use reth_static_file_types::StaticFileSegment;
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
    let actual_block_number =
        import::<Era1, _, _, _, _, _, _>(stream, &pf, &mut hash_collector, None, false).unwrap();

    assert_eq!(actual_block_number, expected_block_number);
}

/// Importing a real `.era1` file with `--with-receipts`-style `store_receipts = true` must
/// decode and persist its (always-present) receipts to the `Receipts` static file segment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_history_import_with_receipts() {
    let url = Url::from_str(ITHACA_ERA_INDEX_URL).unwrap();

    let folder = tempdir().unwrap();
    let client = EraClient::new(ClientWithFakeIndex(Client::new()), url, folder.path());
    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);
    let stream = EraStream::new(client, config);

    let pf = create_test_provider_factory();
    init_genesis(&pf).unwrap();

    let collector_dir = tempdir().unwrap();
    let mut hash_collector = Collector::new(4096, Some(collector_dir.path().to_owned()));

    let imported_height =
        import::<Era1, _, _, _, _, _, _>(stream, &pf, &mut hash_collector, None, true).unwrap();
    assert_eq!(imported_height, 8191);

    assert_eq!(
        pf.static_file_provider().get_highest_static_file_block(StaticFileSegment::Receipts),
        Some(imported_height),
        "receipts static file should cover every imported block"
    );

    let provider = pf.provider().unwrap();
    for &block_num in &[0, 1, 100, 8191] {
        let block = provider.block_by_number(block_num).unwrap().unwrap();
        let receipts = provider.receipts_by_block(block_num.into()).unwrap().unwrap();
        assert_eq!(
            receipts.len(),
            block.body().transactions().count(),
            "block {block_num} should have one receipt per transaction"
        );
    }
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
    let last_imported_block_height =
        import::<Era1, _, _, _, _, _, _>(stream, &pf, &mut hash_collector, None, false).unwrap();

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
    let exported_files =
        export::<Era1, _>(&provider_ref, &export_config).expect("Export should succeed");

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

/// Roundtrip for `.ere` files: import era1 data into a database, export it back out as `.ere`,
/// then reimport those `.ere` files into a fresh database and verify the blocks survive intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_ere_roundtrip_export_after_import() {
    // Populate a database by importing one mainnet era1 file (blocks 0..=8191).
    let url = Url::from_str(ITHACA_ERA_INDEX_URL).unwrap();
    let download_folder = tempdir().unwrap();
    let download_folder = download_folder.path().to_owned().into_boxed_path();
    let client = EraClient::new(ClientWithFakeIndex(Client::new()), url, download_folder);
    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);
    let stream = EraStream::new(client, config);

    let pf = create_test_provider_factory();
    init_genesis(&pf).unwrap();
    let collector_dir = tempdir().unwrap();
    let mut hash_collector = Collector::new(4096, Some(collector_dir.path().to_owned()));

    let imported_height =
        import::<Era1, _, _, _, _, _, _>(stream, &pf, &mut hash_collector, None, false).unwrap();
    assert_eq!(imported_height, 8191);

    let provider_ref = pf.provider_rw().unwrap().0;

    // Export blocks 0..=899 as `.ere` files, 250 blocks per file.
    let ere_folder = tempdir().unwrap();
    let export_config = ExportConfig {
        dir: ere_folder.path().to_path_buf(),
        first_block_number: EXPORT_FIRST_BLOCK,
        last_block_number: EXPORT_LAST_BLOCK,
        max_blocks_per_file: EXPORT_BLOCKS_PER_FILE,
        network: "mainnet".to_string(),
    };
    let ere_files =
        export::<Ere, _>(&provider_ref, &export_config).expect("ERE export should succeed");

    let expected_files_number = EXPORT_TOTAL_BLOCKS.div_ceil(EXPORT_BLOCKS_PER_FILE) as usize;
    assert_eq!(
        ere_files.len(),
        expected_files_number,
        "ERE export should create {expected_files_number} files"
    );
    for (i, file_path) in ere_files.iter().enumerate() {
        assert!(file_path.exists(), "ERE file {} should exist", i + 1);
        assert!(
            fs::metadata(file_path).unwrap().len() > 0,
            "ERE file {} should not be empty",
            i + 1
        );

        // Portal Network header proofs are not produced, so files carry the `noproofs` profile.
        let file_name = file_path.file_name().unwrap().to_str().unwrap();
        assert!(
            file_name.ends_with("-noproofs.ere"),
            "ERE file {} should carry the noproofs profile, got '{file_name}'",
            i + 1
        );

        // The importer streams records by type and never consults the block index, so the
        // exported offsets are otherwise unverified.
        assert_ere_index_offsets_resolve(file_path);
    }

    // Reimport the exported `.ere` files into a fresh database to close the export -> import loop.
    let reimport_pf = create_test_provider_factory();
    init_genesis(&reimport_pf).unwrap();
    let reimport_collector_dir = tempdir().unwrap();
    let mut reimport_collector =
        Collector::new(4096, Some(reimport_collector_dir.path().to_owned()));

    // The exported files are returned in ascending block order, which is the order the importer
    // expects.
    let stream = futures_util::stream::iter(ere_files.into_iter().map(|p| Ok(FileMeta::new(p))));
    let reimported_height =
        import::<Ere, _, _, _, _, _, _>(stream, &reimport_pf, &mut reimport_collector, None, false)
            .unwrap();

    assert_eq!(
        reimported_height, EXPORT_LAST_BLOCK,
        "Reimporting the exported `.ere` files should restore blocks up to {EXPORT_LAST_BLOCK}"
    );

    // The reimported blocks must match the originals exactly (header + body round-trip).
    let reimport_provider = reimport_pf.provider().unwrap();
    for block in [EXPORT_FIRST_BLOCK, EXPORT_LAST_BLOCK / 2, EXPORT_LAST_BLOCK] {
        assert_eq!(
            reimport_provider.block_by_number(block).unwrap(),
            provider_ref.block_by_number(block).unwrap(),
            "Reimported block {block} should match the original",
        );
    }
}

/// Asserts every offset in an exported `.ere` file's [`DynamicBlockIndex`] points at the byte
/// position of the component entry it claims.
///
/// Offsets are `i64`, relative to the index record, laid out per block as
/// `[header, body, receipts, difficulty]` (the `noproofs` profile, so no proof component).
fn assert_ere_index_offsets_resolve(path: &std::path::Path) {
    use reth_era::{
        e2s::types::{Entry, Header},
        ere::types::{
            execution::{
                COMPRESSED_BODY, COMPRESSED_HEADER, COMPRESSED_SLIM_RECEIPTS, TOTAL_DIFFICULTY,
            },
            group::{DynamicBlockIndex, DYNAMIC_BLOCK_INDEX},
        },
    };
    use std::collections::HashMap;

    let bytes = fs::read(path).unwrap();

    // Walk the TLV stream, recording each record's start position and type.
    let mut positions: HashMap<usize, [u8; 2]> = HashMap::new();
    let mut index_pos = None;
    let mut pos = 0usize;
    while pos < bytes.len() {
        let entry_type = [bytes[pos], bytes[pos + 1]];
        let length = u32::from_le_bytes(bytes[pos + 2..pos + 6].try_into().unwrap()) as usize;
        positions.insert(pos, entry_type);
        if entry_type == DYNAMIC_BLOCK_INDEX {
            index_pos = Some(pos);
        }
        pos += Header::SIZE + length;
    }

    let index_pos = index_pos.expect("file must contain a block index");
    let index_entry = Entry::read(&mut &bytes[index_pos..]).unwrap().unwrap();
    let index = DynamicBlockIndex::from_entry(&index_entry).unwrap();

    let expected_types =
        [COMPRESSED_HEADER, COMPRESSED_BODY, COMPRESSED_SLIM_RECEIPTS, TOTAL_DIFFICULTY];
    let start = index.starting_number();
    for block in start..start + index.block_count() as u64 {
        let offsets = index.offsets_for_block(block).expect("offsets for in-range block");
        for (offset, expected_type) in offsets.iter().zip(expected_types) {
            let target = index_pos as i64 + offset;
            assert!(target >= 0, "block {block} offset {offset} points before the file start");
            assert_eq!(
                positions.get(&(target as usize)),
                Some(&expected_type),
                "block {block} offset {offset} should resolve to an entry of the expected type",
            );
        }
    }
}
