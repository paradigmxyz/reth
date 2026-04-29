use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
use reth_eth_wire_types::snap::{
    AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage, GetAccountRangeMessage,
    GetBlockAccessListsMessage, GetByteCodesMessage, GetStorageRangesMessage, StorageRangesMessage,
};

/// Response types for snap sync requests
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapResponse {
    /// Response containing account range data
    AccountRange(AccountRangeMessage),
    /// Response containing storage ranges data
    StorageRanges(StorageRangesMessage),
    /// Response containing bytecode data
    ByteCodes(ByteCodesMessage),
    /// Response containing snap/2 block access list data
    BlockAccessLists(BlockAccessListsMessage),
}

/// The snap sync downloader client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SnapClient: DownloadClient {
    /// The output future type for snap requests
    type Output: Future<Output = PeerRequestResult<SnapResponse>> + Send + Sync + Unpin;

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

    /// Sends the snap/2 block access lists request to the p2p network and returns the response
    /// received from a peer.
    fn get_snap_block_access_lists(&self, request: GetBlockAccessListsMessage) -> Self::Output {
        self.get_snap_block_access_lists_with_priority(request, Priority::Normal)
    }

    /// Sends the snap/2 block access lists request to the p2p network with priority set.
    fn get_snap_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        priority: Priority,
    ) -> Self::Output;
}
