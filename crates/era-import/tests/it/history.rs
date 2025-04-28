use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_etl::Collector;
use reth_provider::test_utils::create_test_provider_factory;
use std::{fs, str::FromStr};
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_history_imports_from_fresh_state_successfully() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str("https://mainnet.era1.nimbus.team/").unwrap();

    // Directory where the ERA1 files will be downloaded to
    let folder = tempdir().unwrap();
    let index = folder.path().to_owned().join("index");
    let folder = folder.path().to_owned().into_boxed_path();

    let client = EraClient::new(Client::new(), url, folder);

    client.fetch_file_list().await.unwrap();

    fs::write(index, "mainnet-00000-5ec1ffb8.era1\n").unwrap();

    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let pf = create_test_provider_factory();

    init_genesis(&pf).unwrap();

    let folder = tempdir().unwrap();
    let folder = Some(folder.path().to_owned());
    let hash_collector = Collector::new(4096, folder);

    let expected_block_number = 8191;
    let actual_block_number =
        reth_era_import::import(stream, &pf.provider_rw().unwrap().0, hash_collector).unwrap();

    assert_eq!(actual_block_number, expected_block_number);
}
