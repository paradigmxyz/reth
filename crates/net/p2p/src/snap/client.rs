use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
use reth_eth_wire_types::snap::{
    AccountRangeMessage, GetAccountRangeMessage, GetByteCodesMessage, GetStorageRangesMessage,
    GetTrieNodesMessage,
};

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

    /// Sends the storage ranges request to the p2p network and returns the storage ranges
    /// response received from a peer.
    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output;

    /// Sends the storage ranges request to the p2p network with priority set and returns
    /// the storage ranges response received from a peer.
    fn get_storage_ranges_with_priority(
        &self,
        request: GetStorageRangesMessage,
        priority: Priority,
    ) -> Self::Output;

    /// Sends the byte codes request to the p2p network and returns the byte codes
    /// response received from a peer.
    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output;

    /// Sends the byte codes request to the p2p network with priority set and returns
    /// the byte codes response received from a peer.
    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        priority: Priority,
    ) -> Self::Output;

    /// Sends the trie nodes request to the p2p network and returns the trie nodes
    /// response received from a peer.
    fn get_trie_nodes(&self, request: GetTrieNodesMessage) -> Self::Output;

    /// Sends the trie nodes request to the p2p network with priority set and returns
    /// the trie nodes response received from a peer.
    fn get_trie_nodes_with_priority(
        &self,
        request: GetTrieNodesMessage,
        priority: Priority,
    ) -> Self::Output;
}
