use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_era_utils::{export, import, ExportConfig};
use reth_etl::Collector;
use reth_provider::{
    test_utils::create_test_provider_factory, BlockReader, HeaderProvider,
    StaticFileProviderFactory,
};
use std::{fs, str::FromStr};
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread")]
async fn test_history_import_export() {
    let base_url = Url::from_str("https://mainnet.era1.nimbus.team/").unwrap();

    let folder = tempdir().unwrap();
    let index = folder.path().to_owned().join("index");
    let folder = folder.path().to_owned().into_boxed_path();

    let client = EraClient::new(Client::new(), base_url, folder.clone());

    client.fetch_file_list().await.unwrap();

    fs::write(index, "mainnet-00000-5ec1ffb8.era1").unwrap();

    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let provider_factory = create_test_provider_factory();
    let static_file_provider: reth_provider::providers::StaticFileProvider<
        reth_ethereum_primitives::EthPrimitives,
    > = provider_factory.static_file_provider();
    init_genesis(&provider_factory).unwrap();

    let folder = tempdir().unwrap();
    let folder = Some(folder.path().to_owned());
    let hash_collector = Collector::new(4096, folder);

    let expected_block_number = 8191;
    let actual_block_number =
        import(stream, &provider_factory.provider_rw().unwrap().0, hash_collector).unwrap();

    assert_eq!(actual_block_number, expected_block_number);

    for block_num in [0, 100, 1000, 8191] {
        let block_db = provider_factory.provider_rw().unwrap().block_by_number(block_num).unwrap();
        let header_static = static_file_provider.header_by_number(block_num);
        let static_header_exists = header_static.is_ok() && header_static.unwrap().is_some();

        println!(
            "Block {block_num}: DB={}, StaticHeader={}",
            block_db.is_some(),
            static_header_exists
        );
    }

    let export_dir = tempdir().unwrap();
    let export_config = ExportConfig {
        dir: export_dir.path().to_owned(),
        first_block_number: 0,
        last_block_number: expected_block_number,
        step: 10,
        network: "mainnet-test".to_string(),
    };

    let _exported_files =
        export(&provider_factory.provider_rw().unwrap().0, &export_config).unwrap();
}
