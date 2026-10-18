//! Tests downloading files and streaming their filenames
use super::*;
use futures_util::StreamExt;
use reqwest::Url;
use reth_era_downloader::{EraClient, EraMeta, EraRemoteMeta, EraStream, EraStreamConfig};
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

#[test_case(ERE_ETHPANDAOPS_URL; "ethpandaops")]
#[tokio::test]
async fn test_streaming_ere_files_after_fetching_file_list(url: &str) {
    let base_url = Url::from_str(url).unwrap();
    let folder = tempdir().unwrap();
    let folder = folder.path();
    let client = EraClient::new(StubClient, base_url, folder);

    let mut stream = EraStream::new(
        client,
        EraStreamConfig::default().with_max_files(2).with_max_concurrent_downloads(1),
    );

    let expected_file = folder.join("mainnet-00000-a6860fef.erae").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();
    assert_eq!(actual_file.as_ref(), expected_file.as_ref());

    let expected_file = folder.join("mainnet-00001-05c64fc4.erae").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();
    assert_eq!(actual_file.as_ref(), expected_file.as_ref());
}

#[tokio::test]
async fn test_streaming_era1_files_after_fetching_file_list_into_missing_folder_fails() {
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

#[tokio::test]
async fn test_streaming_era_files_after_fetching_file_list_into_missing_folder_fails() {
    let base_url = Url::from_str("https://mainnet.era.nimbus.team").unwrap(); //TODO: change once
                                                                              // ithaca host era
                                                                              // files
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

#[tokio::test]
async fn startup_cleanup_and_acknowledgement_keep_cache_bounded() {
    let folder = tempdir().unwrap();
    let first_path = folder.path().join("mainnet-00000-5ec1ffb8.era1");
    let second_path = folder.path().join("mainnet-00001-a5364e9a.era1");
    reth_fs_util::write(&first_path, ERA1_MAINNET_0).unwrap();
    reth_fs_util::write(&second_path, ERA1_MAINNET_1).unwrap();
    let unrelated = folder.path().join("keep.txt");
    reth_fs_util::write(&unrelated, b"keep").unwrap();
    let other_type = folder.path().join("mainnet-99999-deadbeef.era");
    reth_fs_util::write(&other_type, b"keep").unwrap();
    let client = EraClient::new(
        StubClient,
        Url::parse("https://era1.ethportal.net/").unwrap(),
        folder.path(),
    );
    let mut stream = EraStream::new(client, EraStreamConfig::default().with_max_files(1));

    let first: EraRemoteMeta = stream.next().await.unwrap().unwrap();
    assert_eq!(first.path(), first_path);
    assert!(!second_path.exists(), "startup removes files outside the initial window");
    first.mark_as_processed().unwrap();
    assert!(!first_path.exists());

    let second = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(second.path(), second_path);
    assert!(!first_path.exists());
    second.mark_as_processed().unwrap();
    assert!(!second_path.exists());
    assert!(unrelated.exists());
    assert!(other_type.exists());
}
