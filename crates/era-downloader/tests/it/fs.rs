use alloy_primitives::hex::ToHexExt;
use futures_util::StreamExt;
use reth_era_downloader::read_dir;
use sha2::Digest;
use tokio::fs;

const CONTENTS_0: &[u8; 1] = b"a";
const CONTENTS_1: &[u8; 1] = b"b";

#[tokio::test]
async fn test_streaming_from_local_directory_reads_all_files_successfully() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_owned();

    let checksums = format!(
        "{}\n{}",
        sha2::Sha256::digest(CONTENTS_0).encode_hex(),
        sha2::Sha256::digest(CONTENTS_1).encode_hex()
    );
    fs::write(folder.join("checksums.txt"), checksums).await.unwrap();
    fs::write(folder.join("mainnet-00000-5ec1ffb8.era1"), CONTENTS_0).await.unwrap();
    fs::write(folder.join("mainnet-00001-a5364e9a.era1"), CONTENTS_1).await.unwrap();

    let folder = folder.into_boxed_path();
    let mut stream = read_dir(folder.clone()).unwrap();

    let expected_file = folder.join("mainnet-00000-5ec1ffb8.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);

    let expected_file = folder.join("mainnet-00001-a5364e9a.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);
}

#[tokio::test]
async fn test_streaming_from_local_directory_with_invalid_checksums_fails() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_owned();

    let checksums = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fs::write(folder.join("checksums.txt"), checksums).await.unwrap();
    fs::write(folder.join("mainnet-00000-5ec1ffb8.era1"), CONTENTS_0).await.unwrap();
    fs::write(folder.join("mainnet-00001-a5364e9a.era1"), CONTENTS_1).await.unwrap();

    let folder = folder.into_boxed_path();
    let mut stream = read_dir(folder.clone()).unwrap();

    let actual_err = stream.next().await.unwrap().unwrap_err().to_string();
    let expected_err = "Checksum mismatch, \
got: ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb, \
expected: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    assert_eq!(actual_err, expected_err);

    let actual_err = stream.next().await.unwrap().unwrap_err().to_string();
    let expected_err = "Checksum mismatch, \
got: 3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d, \
expected: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    assert_eq!(actual_err, expected_err);
}

#[tokio::test]
async fn test_streaming_from_local_directory_with_one_invalid_checksum_partially_fails() {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_owned();

    let checksums = format!(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n{}",
        sha2::Sha256::digest(CONTENTS_1).encode_hex()
    );
    fs::write(folder.join("checksums.txt"), checksums).await.unwrap();
    fs::write(folder.join("mainnet-00000-5ec1ffb8.era1"), CONTENTS_0).await.unwrap();
    fs::write(folder.join("mainnet-00001-a5364e9a.era1"), CONTENTS_1).await.unwrap();

    let folder = folder.into_boxed_path();
    let mut stream = read_dir(folder.clone()).unwrap();

    let actual_err = stream.next().await.unwrap().unwrap_err().to_string();
    let expected_err = "Checksum mismatch, \
got: ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb, \
expected: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    assert_eq!(actual_err, expected_err);

    let expected_file = folder.join("mainnet-00001-a5364e9a.era1").into_boxed_path();
    let actual_file = stream.next().await.unwrap().unwrap();

    assert_eq!(actual_file, expected_file);
}
