//! API related to syncing blocks.

use futures::Future;
use reth_network_p2p::{BodiesClient, HeadersClient};
use tokio::sync::oneshot;

/// Provides client for downloading blocks.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockDownloaderProvider {
    /// Returns a new combined [`HeadersClient`] and [`BodiesClient`], used for syncing blocks from
    /// peers.
    ///
    /// The client is the entrypoint for sending block requests to the network.
    fn fetch_client(
        &self,
    ) -> impl Future<
        Output = Result<
            impl HeadersClient + BodiesClient + Unpin + Clone + 'static,
            oneshot::error::RecvError,
        >,
    > + Send;
}
