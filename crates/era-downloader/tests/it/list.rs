//! Tests fetching a list of files
use crate::StubClient;
use reqwest::Url;
use reth_era_downloader::EraClient;
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
    str::FromStr,
};
use test_case::test_case;

#[test_case("https://mainnet.era1.nimbus.team/"; "nimbus")]
#[test_case("https://era1.ethportal.net/"; "ethportal")]
#[tokio::test]
async fn test_getting_file_name_after_fetching_file_list(url: &str) {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);

    let url = Url::from_str(url).unwrap();
    let folder = PathBuf::from_str(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
        .join(format!("{:x}", hasher.finish()))
        .into_boxed_path();
    let _ = std::fs::create_dir(&folder);
    let client = EraClient::new(StubClient, url, folder);

    client.fetch_file_list().await.unwrap();

    let actual = client.number_to_file_name(600).await.unwrap();
    let expected = Some("mainnet-00600-a81ae85f.era1".to_owned());

    assert_eq!(actual, expected);
}
