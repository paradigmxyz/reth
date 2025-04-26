//! API related to syncing blocks.

use core::marker::PhantomData;
use std::fmt::Debug;

use futures::Future;
use reth_eth_wire_types::EthNetworkPrimitives;
use reth_network_p2p::{test_utils::NoopFullBlockClient, BlockClient};
use tokio::sync::oneshot;

/// Provides client for downloading blocks.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockDownloaderProvider {
    /// The client this type can provide.
    type Client: BlockClient<Header: Debug, Body: Debug> + Send + Sync + Clone + 'static;

    /// Returns a new [`BlockClient`], used for fetching blocks from peers.
    ///
    /// The client is the entrypoint for sending block requests to the network.
    fn fetch_client(
        &self,
    ) -> impl Future<Output = Result<Self::Client, oneshot::error::RecvError>> + Send;
}

pub struct NoopBlockchainDownloaderProvider<NetPrimitives = EthNetworkPrimitives>(
    PhantomData<NetPrimitives>,
);

impl<NetPrimitives> BlockDownloaderProvider for NoopBlockchainDownloaderProvider<NetPrimitives> {
    type Client = NoopFullBlockClient<NetPrimitives>;

    fn fetch_client(
        &self,
    ) -> impl Future<Output = Result<Self::Client, oneshot::error::RecvError>> + Send {
        async { Ok(NoopFullBlockClient::<_>(PhantomData)) }
    }
}
