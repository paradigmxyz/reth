//! Tests downloading files and streaming their filenames
use crate::StubClient;
use futures_util::StreamExt;
use reqwest::Url;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use std::str::FromStr;
use tempfile::tempdir;
use test_case::test_case;

#[test_case("https://mainnet.era1.nimbus.team/"; "nimbus")]
#[test_case("https://era1.ethportal.net/"; "ethportal")]
#[tokio::test]
async fn test_streaming_files_after_fetching_file_list(url: &str) {
    let base_url = Url::from_str(url).unwrap();
    let folder = tempdir().unwrap();
    let folder = folder.path().to_owned().into_boxed_path();
    let client = EraClient::new(StubClient, base_url, folder.clone());

    client.fetch_file_list().await.unwrap();

    let mut stream = EraStream::new(
        client,
        EraStreamConfig::default().with_max_files(2).with_max_concurrent_downloads(1),
    );

    let expected_file = folder.join("mainnet-00000-5ec1ffb8.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);

    let expected_file = folder.join("mainnet-00001-a5364e9a.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);
}
