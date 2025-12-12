//! Root module for test modules, so that the tests are built into a single binary.

use alloy_primitives::bytes::Bytes;
use futures_util::{stream, Stream, TryStreamExt};
use reqwest::{Client, IntoUrl};
use reth_era_downloader::HttpClient;
use tokio_util::either::Either;

// Url where the ERA1 files are hosted
const ITHACA_ERA_INDEX_URL: &str = "https://era.ithaca.xyz/era1/index.html";

// The response containing one file that the fake client will return when the index Url is requested
const GENESIS_ITHACA_INDEX_RESPONSE: &[u8] = b"<a href=\"https://era.ithaca.xyz/era1/mainnet-00000-5ec1ffb8.era1\">mainnet-00000-5ec1ffb8.era1</a>";

mod genesis;
mod history;

const fn main() {}

/// An HTTP client that fakes the file list to always show one known file
///
/// but passes all other calls including actual downloads to a real HTTP client
///
/// In that way, only one file is used but downloads are still performed from the original source.
#[derive(Debug, Clone)]
struct ClientWithFakeIndex(Client);

impl HttpClient for ClientWithFakeIndex {
    async fn get<U: IntoUrl + Send + Sync>(
        &self,
        url: U,
    ) -> eyre::Result<impl Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin> {
        let url = url.into_url()?;

        match url.to_string().as_str() {
            ITHACA_ERA_INDEX_URL => {
                // Create a static stream without boxing
                let stream =
                    stream::iter(vec![Ok(Bytes::from_static(GENESIS_ITHACA_INDEX_RESPONSE))]);
                Ok(Either::Left(stream))
            }
            _ => {
                let response = Client::get(&self.0, url).send().await?;
                let stream = response.bytes_stream().map_err(|e| eyre::Error::new(e));
                Ok(Either::Right(stream))
            }
        }
    }
}
