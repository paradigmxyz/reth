//! API related to syncing blocks.

use std::fmt::Debug;

use alloy_primitives::B256;
use futures::Future;
use reth_network_p2p::{
    error::{PeerRequestResult, RequestError},
    BalRequirement, BlockAccessLists, BlockClient,
};
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

    /// Fetches block access lists for the given block hashes, if supported by this network.
    ///
    /// This keeps callers that only have a network handle from depending on the concrete
    /// [`BlockAccessListsClient`] returned by [`Self::fetch_client`]. Implementations with BAL
    /// support can delegate to the fetch client internally, while noop or custom networks can use
    /// the default unsupported response without forcing their block client to implement BAL
    /// requests.
    fn fetch_block_access_lists(
        &self,
        _hashes: Vec<B256>,
        _requirement: BalRequirement,
    ) -> impl Future<Output = PeerRequestResult<BlockAccessLists>> + Send {
        futures::future::ready(Err(RequestError::UnsupportedCapability))
    }
}
