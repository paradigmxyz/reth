//! Tests fetching a list of files
use reqwest::Url;
use reth_era_downloader::EraClient;
use std::{path::PathBuf, str::FromStr};

#[tokio::test]
async fn test_getting_file_name_after_fetching_file_list() {
    let url = Url::from_str("https://mainnet.era1.nimbus.team/").unwrap();
    let folder = PathBuf::from_str(env!("CARGO_TARGET_TMPDIR")).unwrap().into_boxed_path();
    let client = EraClient::new(reqwest::Client::new(), url, folder);

    client.fetch_file_list().await.unwrap();

    let actual = client.number_to_file_name(600).await.unwrap();
    let expected = Some("mainnet-00600-a81ae85f.era1".to_owned());

    assert_eq!(actual, expected);
}
