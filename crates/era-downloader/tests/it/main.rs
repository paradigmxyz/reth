//! Root module for test modules, so that the tests are built into a single binary.

mod checksums;
mod download;
mod fs;
mod list;
mod stream;

const fn main() {}

use bytes::Bytes;
use futures_util::Stream;
use reqwest::IntoUrl;
use reth_era_downloader::HttpClient;
use std::future::Future;

pub(crate) const NIMBUS: &[u8] = include_bytes!("../res/nimbus.html");
pub(crate) const ETH_PORTAL: &[u8] = include_bytes!("../res/ethportal.html");
pub(crate) const ITHACA: &[u8] = include_bytes!("../res/ithaca.html");
pub(crate) const CHECKSUMS: &[u8] = include_bytes!("../res/checksums.txt");
pub(crate) const MAINNET_0: &[u8] = include_bytes!("../res/mainnet-00000-5ec1ffb8.era1");
pub(crate) const MAINNET_1: &[u8] = include_bytes!("../res/mainnet-00001-a5364e9a.era1");

/// An HTTP client pre-programmed with canned answers to received calls.
/// Panics if it receives an unknown call.
#[derive(Debug, Clone)]
struct StubClient;

impl HttpClient for StubClient {
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
                        Ok(bytes::Bytes::from(NIMBUS))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era1.ethportal.net/" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(ETH_PORTAL))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/index.html" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(ITHACA))
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
                        Ok(bytes::Bytes::from(MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/mainnet-00000-5ec1ffb8.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_0))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era1.ethportal.net/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://mainnet.era1.nimbus.team/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                "https://era.ithaca.xyz/era1/mainnet-00001-a5364e9a.era1" => {
                    Ok(Box::new(futures::stream::once(Box::pin(async move {
                        Ok(bytes::Bytes::from(MAINNET_1))
                    })))
                        as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
                }
                v => unimplemented!("Unexpected URL \"{v}\""),
            }
        }
    }
}
