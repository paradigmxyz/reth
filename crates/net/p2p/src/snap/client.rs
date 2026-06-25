use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use futures::Future;
use reth_eth_wire_types::snap::{
    AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage, GetAccountRangeMessage,
    GetBlockAccessListsMessage, GetByteCodesMessage, GetStorageRangesMessage, SnapProtocolMessage,
    StorageRangesMessage,
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
    /// Response containing block access lists.
    ///
    /// Only valid for `snap/2` (EIP-8189).
    BlockAccessLists(BlockAccessListsMessage),
}

impl TryFrom<SnapProtocolMessage> for SnapResponse {
    /// The original message, returned unchanged when it is a request rather than a response.
    type Error = SnapProtocolMessage;

    fn try_from(msg: SnapProtocolMessage) -> Result<Self, Self::Error> {
        match msg {
            SnapProtocolMessage::AccountRange(m) => Ok(Self::AccountRange(m)),
            SnapProtocolMessage::StorageRanges(m) => Ok(Self::StorageRanges(m)),
            SnapProtocolMessage::ByteCodes(m) => Ok(Self::ByteCodes(m)),
            SnapProtocolMessage::BlockAccessLists(m) => Ok(Self::BlockAccessLists(m)),
            request => Err(request),
        }
    }
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

    /// Sends the block access lists request to the p2p network and returns the block
    /// access lists response received from a peer.
    ///
    /// Only valid for `snap/2` (EIP-8189).
    fn get_block_access_lists(&self, request: GetBlockAccessListsMessage) -> Self::Output {
        self.get_block_access_lists_with_priority(request, Priority::Normal)
    }

    /// Sends the block access lists request to the p2p network with priority set and returns
    /// the block access lists response received from a peer.
    ///
    /// Only valid for `snap/2` (EIP-8189).
    fn get_block_access_lists_with_priority(
        &self,
        request: GetBlockAccessListsMessage,
        priority: Priority,
    ) -> Self::Output;
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_eth_wire_types::BlockAccessLists;
    use test_case::test_case;

    #[test_case(
        SnapProtocolMessage::GetAccountRange(GetAccountRangeMessage {
            request_id: 1, root_hash: Default::default(), starting_hash: Default::default(),
            limit_hash: Default::default(), response_bytes: 0,
        }), false ; "account range request is not a response"
    )]
    #[test_case(
        SnapProtocolMessage::GetBlockAccessLists(GetBlockAccessListsMessage {
            request_id: 1, block_hashes: vec![], response_bytes: 0,
        }), false ; "block access lists request is not a response"
    )]
    #[test_case(
        SnapProtocolMessage::AccountRange(AccountRangeMessage {
            request_id: 1, accounts: vec![], proof: vec![],
        }), true ; "account range response converts"
    )]
    #[test_case(
        SnapProtocolMessage::ByteCodes(ByteCodesMessage { request_id: 1, codes: vec![] }),
        true ; "byte codes response converts"
    )]
    #[test_case(
        SnapProtocolMessage::BlockAccessLists(BlockAccessListsMessage {
            request_id: 1, block_access_lists: BlockAccessLists(vec![]),
        }), true ; "block access lists response converts"
    )]
    fn try_from_snap_message(msg: SnapProtocolMessage, is_response: bool) {
        let original = msg.clone();
        match SnapResponse::try_from(msg) {
            Ok(_) => assert!(is_response),
            // requests are returned unchanged
            Err(returned) => {
                assert!(!is_response);
                assert_eq!(returned, original);
            }
        }
    }
}
