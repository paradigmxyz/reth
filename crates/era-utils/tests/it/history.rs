use alloy_primitives::bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use reqwest::{Client, IntoUrl, Url};
use reth_db_common::init::init_genesis;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig, HttpClient};
use reth_etl::Collector;
use reth_provider::test_utils::create_test_provider_factory;
use std::{future::Future, str::FromStr};
use tempfile::tempdir;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_history_imports_from_fresh_state_successfully() {
    // URL where the ERA1 files are hosted
    let url = Url::from_str("https://era.ithaca.xyz/era1/index.html").unwrap();

    // Directory where the ERA1 files will be downloaded to
    let folder = tempdir().unwrap();
    let folder = folder.path().to_owned().into_boxed_path();

    let client = EraClient::new(ClientWithFakeIndex(Client::new()), url, folder);

    let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);

    let stream = EraStream::new(client, config);
    let pf = create_test_provider_factory();

    init_genesis(&pf).unwrap();

    let folder = tempdir().unwrap();
    let folder = Some(folder.path().to_owned());
    let hash_collector = Collector::new(4096, folder);

    let expected_block_number = 8191;
    let actual_block_number =
        reth_era_utils::import(stream, &pf.provider_rw().unwrap().0, hash_collector).unwrap();

    assert_eq!(actual_block_number, expected_block_number);
}

/// An HTTP client pre-programmed with canned answer to index.
///
/// Passes any other calls to a real HTTP client!
#[derive(Debug, Clone)]
struct ClientWithFakeIndex(Client);

impl HttpClient for ClientWithFakeIndex {
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
                "https://era.ithaca.xyz/era1/index.html" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from_static(b"<a href=\"https://era.ithaca.xyz/era1/mainnet-00000-5ec1ffb8.era1\">mainnet-00000-5ec1ffb8.era1</a>"))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                _ => {
                    let response = Client::get(&self.0, url).send().await?;

                    Ok(Box::new(response.bytes_stream().map_err(|e| eyre::Error::new(e)))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
            }
        }
    }
}
