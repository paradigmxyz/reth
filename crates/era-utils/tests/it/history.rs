use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_etl::Collector;
use reth_provider::{
    test_utils::create_test_provider_factory, BlockReader, HeaderProvider,
    StaticFileProviderFactory,
};
use std::{fs, str::FromStr};
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_history_imports_from_fresh_state_successfully() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str("https://era.ithaca.xyz/era1/").unwrap();

    // Directory where the ERA1 files will be downloaded to
    let folder = tempdir().unwrap();
    let index = folder.path().to_owned().join("index");
    let folder = folder.path().to_owned().into_boxed_path();

    let client = EraClient::new(Client::new(), url, folder);

    client.fetch_file_list().await.unwrap();

    fs::write(index, "mainnet-00000-5ec1ffb8.era1\n").unwrap();

    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let provider_factory = create_test_provider_factory();

    init_genesis(&provider_factory).unwrap();

    let folder = tempdir().unwrap();
    let folder = Some(folder.path().to_owned());
    let hash_collector = Collector::new(4096, folder);

    let expected_block_number = 8191;
    let actual_block_number =
        reth_era_utils::import(stream, &provider_factory, hash_collector).unwrap();

    let static_file_provider = provider_factory.static_file_provider();

    let checkpoint_blocks = [0, 100, 1000, 8191];

    for block_num in checkpoint_blocks {
        let block_db = provider_factory.provider_rw().unwrap().block_by_number(block_num).unwrap();
        let header_static = static_file_provider.header_by_number(block_num);
        let static_header_exists =  header_static.is_ok_and(|h| h.is_some());

        assert!(static_header_exists, "Block {block_num} should exist in static files");
        assert!(block_db.is_some(), "Block should exist in the db");
    }
    assert_eq!(actual_block_number, expected_block_number);
}
