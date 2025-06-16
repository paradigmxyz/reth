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
#[test_case("https://era.ithaca.xyz/era1/index.html"; "ithaca")]
#[tokio::test]
async fn test_streaming_files_after_fetching_file_list(url: &str) {
    let base_url = Url::from_str(url).unwrap();
    let folder = tempdir().unwrap();
    let folder = folder.path();
    let client = EraClient::new(StubClient, base_url, folder);

    let mut stream = EraStream::new(
        client,
        EraStreamConfig::default().with_max_files(2).with_max_concurrent_downloads(1),
    );

    let expected_file = folder.join("mainnet-00000-5ec1ffb8.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file.as_ref(), expected_file.as_ref());

    let expected_file = folder.join("mainnet-00001-a5364e9a.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file.as_ref(), expected_file.as_ref());
}

#[tokio::test]
async fn test_streaming_files_after_fetching_file_list_into_missing_folder_fails() {
    let base_url = Url::from_str("https://era.ithaca.xyz/era1/index.html").unwrap();
    let folder = tempdir().unwrap().path().to_owned();
    let client = EraClient::new(StubClient, base_url, folder);

    let mut stream = EraStream::new(
        client,
        EraStreamConfig::default().with_max_files(2).with_max_concurrent_downloads(1),
    );

    let actual_error = stream.next().await.unwrap().unwrap_err().to_string();
    let expected_error = "No such file or directory (os error 2)".to_owned();

    assert_eq!(actual_error, expected_error);
}
