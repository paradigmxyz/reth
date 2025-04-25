use futures_util::StreamExt;
use reth_era_downloader::read_dir;
use tokio::fs;

#[tokio::test]
async fn test_streaming_from_local_directory() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_owned();

    fs::write(folder.join("mainnet-00000-5ec1ffb8.era1"), b"").await.unwrap();
    fs::write(folder.join("mainnet-00001-a5364e9a.era1"), b"").await.unwrap();

    let folder = folder.into_boxed_path();
    let mut stream = read_dir(folder.clone()).unwrap();

    let expected_file = folder.join("mainnet-00000-5ec1ffb8.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);

    let expected_file = folder.join("mainnet-00001-a5364e9a.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);
}
