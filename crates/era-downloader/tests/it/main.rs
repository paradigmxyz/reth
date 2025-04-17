//! Root module for test modules, so that the tests are built into a single binary.

mod download;
mod list;
mod stream;

const fn main() {}

use bytes::Bytes;
use futures_util::Stream;
use reqwest::IntoUrl;
use reth_era_downloader::HttpClient;
use std::future::Future;

const NIMBUS: &[u8] = include_bytes!("../res/nimbus.html");
const ETH_PORTAL: &[u8] = include_bytes!("../res/ethportal.html");
const MAINNET_0: &[u8] = include_bytes!("../res/mainnet-00000-5ec1ffb8.era1");
const MAINNET_1: &[u8] = include_bytes!("../res/mainnet-00001-a5364e9a.era1");

/// An HTTP client pre-programmed with canned answers to received calls.
/// Panics if it receives an unknown call.
#[derive(Debug, Clone)]
struct StubClient;

impl HttpClient for StubClient {
    fn get<U: IntoUrl>(
        &self,
        url: U,
    ) -> impl Future<Output = eyre::Result<impl Stream<Item = eyre::Result<Bytes>> + Unpin>> {
        let url = url.into_url().unwrap();

        async move {
            match url.to_string().as_str() {
                "https://mainnet.era1.nimbus.team/" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(NIMBUS))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Unpin>)
                }
                "https://era1.ethportal.net/" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(ETH_PORTAL))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Unpin>)
                }
                "https://era1.ethportal.net/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Unpin>)
                }
                "https://era1.ethportal.net/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Unpin>)
                }
                v => unimplemented!("Unexpected URL \"{v}\""),
            }
        }
    }
}
