use bytes::Bytes;
use futures::Stream;
use futures_util::StreamExt;
use reqwest::{IntoUrl, Url};
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig, HttpClient};
use std::{future::Future, str::FromStr};
use tempfile::tempdir;
use test_case::test_case;

#[test_case("https://mainnet.era1.nimbus.team/"; "nimbus")]
#[test_case("https://era1.ethportal.net/"; "ethportal")]
#[test_case("https://era.ithaca.xyz/era1/index.html"; "ithaca")]
#[tokio::test]
async fn test_invalid_checksum_returns_error(url: &str) {
    let base_url = Url::from_str(url).unwrap();
    let folder = tempdir().unwrap();
    let folder = folder.path().to_owned().into_boxed_path();
    let client = EraClient::new(FailingClient, base_url, folder.clone());

    let mut stream = EraStream::new(
        client,
        EraStreamConfig::default().with_max_files(2).with_max_concurrent_downloads(1),
    );

    let actual_err = stream.next().await.unwrap().unwrap_err().to_string();
    let expected_err = "Checksum mismatch, \
got: 87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7, \
expected: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    assert_eq!(actual_err, expected_err);

    let actual_err = stream.next().await.unwrap().unwrap_err().to_string();
    let expected_err = "Checksum mismatch, \
got: 0263829989b6fd954f72baaf2fc64bc2e2f01d692d4de72986ea808f6e99813f, \
expected: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    assert_eq!(actual_err, expected_err);
}

const CHECKSUMS: &[u8] = b"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

/// An HTTP client pre-programmed with canned answers to received calls.
/// Panics if it receives an unknown call.
#[derive(Debug, Clone)]
struct FailingClient;

impl HttpClient for FailingClient {
    fn get<U: IntoUrl + Send + Sync>(
        &self,
        url: U,
    ) -> impl Future<
        Output = eyre::Result<impl Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>,
    > + Send
           + Sync {
        let url = url.into_url().unwrap();

        async move {
            match url.to_string().as_str() {
                "https://mainnet.era1.nimbus.team/" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::NIMBUS))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era1.ethportal.net/" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::ETH_PORTAL))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/index.html" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::ITHACA))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/checksums.txt" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(CHECKSUMS))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era1.ethportal.net/checksums.txt" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(CHECKSUMS))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/checksums.txt" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(CHECKSUMS))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era1.ethportal.net/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era1.ethportal.net/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(crate::MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                v => unimplemented!("Unexpected URL \"{v}\""),
            }
        }
    }
}
