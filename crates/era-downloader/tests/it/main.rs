//! Root module for test modules, so that the tests are built into a single binary.

mod checksums;
mod download;
mod fs;
mod list;
mod stream;

use std::pin::Pin;

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use reqwest::IntoUrl;

use reth_era_downloader::client::HttpClient;

const MAINNET_1: &[u8] = include_bytes!("../data/mainnet-00001-a5364e9a.era1");

pub(crate) struct StubClient;

#[async_trait]
impl HttpClient for StubClient {
    async fn get(
        &self,
        url: impl IntoUrl,
    ) -> eyre::Result<Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>> {
        match url.into_url()?.as_str() {
            "https://era.ithaca.xyz/era1/" => {
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
            "https://ethportal.net/era1/" => Ok(Box::new(futures::stream::once(Box::pin(async move {
                Ok(bytes::Bytes::from(MAINNET_1))
            })))
                as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>),
            "https://ethportal.net/era1/mainnet-00001-a5364e9a.era1" => {
                Ok(Box::new(futures::stream::once(Box::pin(async move {
                    Ok(bytes::Bytes::from(MAINNET_1))
                })))
                    as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
            }
            "https://era.nimbus.team/era1/" => Ok(Box::new(futures::stream::once(Box::pin(async move {
                Ok(bytes::Bytes::from(MAINNET_1))
            })))
                as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>),
            "https://era.nimbus.team/era1/mainnet-00001-a5364e9a.era1" => {
                Ok(Box::new(futures::stream::once(Box::pin(async move {
                    Ok(bytes::Bytes::from(MAINNET_1))
                })))
                    as Box<dyn Stream<Item = eyre::Result<Bytes>> + Send + Sync + Unpin>)
            }
            v => unimplemented!("Unexpected URL \"{v}\""),
        }
    }
}
