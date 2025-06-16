//! Root module for test modules, so that the tests are built into a single binary.

use alloy_primitives::bytes::Bytes;
use futures_util::{Future, Stream, TryStreamExt};
use reqwest::{Client, IntoUrl};
use reth_era_downloader::HttpClient;

mod genesis;
mod history;

const fn main() {}


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
