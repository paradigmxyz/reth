//! Tests fetching a file
use crate::StubClient;
use reqwest::Url;
use reth_era_downloader::EraClient;
use std::str::FromStr;
use tempfile::tempdir;
use test_case::test_case;

#[test_case("https://mainnet.era1.nimbus.team/"; "nimbus")]
#[test_case("https://era1.ethportal.net/"; "ethportal")]
#[tokio::test]
async fn test_getting_file_url_after_fetching_file_list(url: &str) {
    let base_url = Url::from_str(url).unwrap();
    let folder = tempdir().unwrap();
    let folder = folder.path().to_owned().into_boxed_path();
    let client = EraClient::new(StubClient, base_url, folder);

    client.fetch_file_list().await.unwrap();

    let expected_url = Some(Url::from_str(&format!("{url}mainnet-00000-5ec1ffb8.era1")).unwrap());
    let actual_url = client.url(0).await.unwrap();

    assert_eq!(actual_url, expected_url);
}

#[test_case("https://mainnet.era1.nimbus.team/"; "nimbus")]
#[test_case("https://era1.ethportal.net/"; "ethportal")]
#[tokio::test]
async fn test_getting_file_after_fetching_file_list(url: &str) {
    let base_url = Url::from_str(url).unwrap();
    let folder = tempdir().unwrap();
    let folder = folder.path().to_owned().into_boxed_path();
    let mut client = EraClient::new(StubClient, base_url, folder);

    client.fetch_file_list().await.unwrap();

    let url = client.url(0).await.unwrap().unwrap();

    let expected_count = 0;
    let actual_count = client.files_count().await;
    assert_eq!(actual_count, expected_count);

    client.download_to_file(url).await.unwrap();

    let expected_count = 1;
    let actual_count = client.files_count().await;
    assert_eq!(actual_count, expected_count);
}
