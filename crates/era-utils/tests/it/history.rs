use crate::ClientWithFakeIndex;
use reqwest::{Client, Url};
use reth_db_common::init::init_genesis;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_etl::Collector;
use reth_provider::test_utils::create_test_provider_factory;
use std::str::FromStr;
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_history_imports_from_fresh_state_successfully() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str("https://era.ithaca.xyz/era1/index.html").unwrap();

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
    let actual_block_number = reth_era_utils::import(stream, &pf, &mut hash_collector).unwrap();

    assert_eq!(actual_block_number, expected_block_number);
}
