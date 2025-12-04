use reth_db_common::init::init_genesis;
use reth_era_utils::{export, ExportConfig};
use reth_fs_util as fs;
use reth_provider::{test_utils::create_test_provider_factory, BlockReader};
use tempfile::tempdir;

#[test]
fn test_export_with_genesis_only() {
    let provider_factory = create_test_provider_factory();
    init_genesis(&provider_factory).unwrap();
    let provider = provider_factory.provider().unwrap();
    assert!(provider.block_by_number(0).unwrap().is_some(), "Genesis block should exist");
    assert!(provider.block_by_number(1).unwrap().is_none(), "Block 1 should not exist");

    let export_dir = tempdir().unwrap();
    let export_config = ExportConfig { dir: export_dir.path().to_owned(), ..Default::default() };

    let exported_files =
        export(&provider_factory.provider_rw().unwrap().0, &export_config).unwrap();

    assert_eq!(exported_files.len(), 1, "Should export exactly one file");

    let file_path = &exported_files[0];
    assert!(file_path.exists(), "Exported file should exist on disk");
    let file_name = file_path.file_name().unwrap().to_str().unwrap();
    assert!(
        file_name.starts_with("mainnet-00000-00001-"),
        "File should have correct prefix with era format"
    );
    assert!(file_name.ends_with(".era1"), "File should have correct extension");
    let metadata = fs::metadata(file_path).unwrap();
    assert!(metadata.len() > 0, "Exported file should not be empty");
}
