use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
use reth_eth_wire_types::snap::{AccountRangeMessage, GetAccountRangeMessage};

/// The snap sync downloader client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SnapClient: DownloadClient {
    /// The output future type for account range requests
    type Output: Future<Output = PeerRequestResult<AccountRangeMessage>> + Send + Sync + Unpin;

    /// Sends the account range request to the p2p network and returns the account range
    /// response received from a peer.
    fn get_account_range(&self, request: GetAccountRangeMessage) -> Self::Output {
        self.get_account_range_with_priority(request, Priority::Normal)
    }

    /// Sends the account range request to the p2p network with priority set and returns
    /// the account range response received from a peer.
    fn get_account_range_with_priority(
        &self,
        request: GetAccountRangeMessage,
        priority: Priority,
    ) -> Self::Output;
}
