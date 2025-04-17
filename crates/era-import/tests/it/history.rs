use reqwest::{Client, Url};
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_provider::test_utils::create_test_provider_factory;
use std::str::FromStr;
use tempfile::tempdir;

#[tokio::test]
async fn test_history_imports_from_fresh_state_successfully() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str("https://mainnet.era1.nimbus.team/").unwrap();

    // Directory where the ERA1 files will be downloaded to
    let folder = tempdir().unwrap();
    let folder = folder.path().to_owned().into_boxed_path();

    let client = EraClient::new(Client::new(), url, folder);

    client.fetch_file_list().await.unwrap();

    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let pf = create_test_provider_factory();

    let _result = reth_era_import::import(stream, &pf.provider_rw().unwrap().0).unwrap();
}
