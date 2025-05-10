use alloy_primitives::hex::ToHexExt;
use futures_util::StreamExt;
use reth_era_downloader::read_dir;
use sha2::Digest;
use tokio::fs;

const CONTENTS_0: &[u8; 1] = b"a";
const CONTENTS_1: &[u8; 1] = b"b";

#[test_case::test_case(
    Ok(format!(
        "{}\n{}",
        sha2::Sha256::digest(CONTENTS_0).encode_hex(),
        sha2::Sha256::digest(CONTENTS_1).encode_hex()
    )),
    [
        Ok("mainnet-00000-5ec1ffb8.era1"),
        Ok("mainnet-00001-a5364e9a.era1"),
    ];
    "Reads all files successfully"
)]
#[test_case::test_case(
    Ok("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
    bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    [
        Err("Checksum mismatch, \
            got: ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb, \
            expected: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        Err("Checksum mismatch, \
            got: 3e23e8160039594a33894f6564e1b1348bbd7a0088d42c4acb73eeaed59c009d, \
            expected: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    ];
    "With invalid checksums fails"
)]
#[test_case::test_case(
    Ok(format!(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n{}",
        sha2::Sha256::digest(CONTENTS_1).encode_hex()
    )),
    [
        Err("Checksum mismatch, \
            got: ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb, \
            expected: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        Ok("mainnet-00001-a5364e9a.era1"),
    ];
    "With one invalid checksum partially fails"
)]
#[test_case::test_case(
    Err::<&str, _>("Missing file `checksums.txt` in the `dir`"),
    [
        Err("Checksum mismatch, \
            got: ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb, \
            expected: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        Ok("mainnet-00001-a5364e9a.era1"),
    ];
    "With missing checksums file fails"
)]
#[tokio::test]
async fn test_streaming_from_local_directory(
    checksums: Result<impl AsRef<[u8]>, &str>,
    expected: [Result<&str, &str>; 2],
) {
    let folder = tempfile::tempdir().unwrap();
    let folder = folder.path().to_owned();

    if let Ok(checksums) = &checksums {
        fs::write(folder.join("checksums.txt"), checksums).await.unwrap();
    }
    fs::write(folder.join("mainnet-00000-5ec1ffb8.era1"), CONTENTS_0).await.unwrap();
    fs::write(folder.join("mainnet-00001-a5364e9a.era1"), CONTENTS_1).await.unwrap();

    let folder = folder.into_boxed_path();
    let actual = read_dir(folder.clone());

    match checksums {
        Ok(_) => match actual {
            Ok(mut stream) => {
                for expected in expected {
                    let actual = stream.next().await.unwrap();

                    match expected {
                        Ok(expected_file) => {
                            let actual_file = actual.expect("should be ok");
                            let expected_file = folder.join(expected_file).into_boxed_path();

                            assert_eq!(actual_file, expected_file)
                        }
                        Err(expected_err) => {
                            let actual_err = actual.expect_err("should be err").to_string();

                            assert_eq!(actual_err, expected_err)
                        }
                    }
                }
            }

            Err(err) => panic!("expected ok, got: {err:?}"),
        },
        Err(expected_err) => match actual {
            Ok(_) => panic!("should be err"),
            Err(actual_err) => assert_eq!(actual_err.to_string(), expected_err),
        },
    }
}
