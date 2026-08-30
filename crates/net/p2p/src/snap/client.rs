use crate::{
    download::DownloadClient,
    error::{PeerRequestResult, RequestError},
    full_block::NoopFullBlockClient,
    priority::Priority,
};
use futures::Future;
use reth_eth_wire_types::{
    snap::{
        AccountRangeMessage, BlockAccessListsMessage, ByteCodesMessage, GetAccountRangeMessage,
        GetBlockAccessListsMessage, GetByteCodesMessage, GetStorageRangesMessage,
        SnapProtocolMessage, StorageRangesMessage,
    },
    NetworkPrimitives,
};
use reth_network_peers::PeerId;

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

impl From<SnapResponse> for SnapProtocolMessage {
    fn from(response: SnapResponse) -> Self {
        match response {
            SnapResponse::AccountRange(m) => Self::AccountRange(m),
            SnapResponse::StorageRanges(m) => Self::StorageRanges(m),
            SnapResponse::ByteCodes(m) => Self::ByteCodes(m),
            SnapResponse::BlockAccessLists(m) => Self::BlockAccessLists(m),
        }
    }
}

/// The snap sync downloader client
#[auto_impl::auto_impl(&, Arc, Box)]
pub trait SnapClient: DownloadClient {
    /// The output future type for snap requests
    type Output: Future<Output = PeerRequestResult<SnapResponse>> + Send + Sync + Unpin;

    /// Sends `request` to the p2p network under `options` and returns the response received from
    /// a peer.
    ///
    /// Fails with [`RequestError::UnsupportedCapability`] when no peer can serve the request,
    /// which includes the case where every capable peer is excluded.
    fn request_snap(
        &self,
        request: SnapProtocolMessage,
        options: SnapRequestOptions,
    ) -> Self::Output;

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
    ) -> Self::Output {
        self.request_snap(
            SnapProtocolMessage::GetAccountRange(request),
            SnapRequestOptions::with_priority(priority),
        )
    }

    /// Sends the storage ranges request to the p2p network and returns the storage ranges
    /// response received from a peer.
    fn get_storage_ranges(&self, request: GetStorageRangesMessage) -> Self::Output {
        self.get_storage_ranges_with_priority(request, Priority::Normal)
    }

    /// Sends the storage ranges request to the p2p network with priority set and returns
    /// the storage ranges response received from a peer.
    fn get_storage_ranges_with_priority(
        &self,
        request: GetStorageRangesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.request_snap(
            SnapProtocolMessage::GetStorageRanges(request),
            SnapRequestOptions::with_priority(priority),
        )
    }

    /// Sends the byte codes request to the p2p network and returns the byte codes
    /// response received from a peer.
    fn get_byte_codes(&self, request: GetByteCodesMessage) -> Self::Output {
        self.get_byte_codes_with_priority(request, Priority::Normal)
    }

    /// Sends the byte codes request to the p2p network with priority set and returns
    /// the byte codes response received from a peer.
    fn get_byte_codes_with_priority(
        &self,
        request: GetByteCodesMessage,
        priority: Priority,
    ) -> Self::Output {
        self.request_snap(
            SnapProtocolMessage::GetByteCodes(request),
            SnapRequestOptions::with_priority(priority),
        )
    }

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
    ) -> Self::Output {
        self.request_snap(
            SnapProtocolMessage::GetBlockAccessLists(request),
            SnapRequestOptions::with_priority(priority),
        )
    }
}

/// How a snap request is dispatched to a peer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SnapRequestOptions {
    /// Queue position the request is dispatched at.
    pub priority: Priority,
    /// Peers that must not receive this logical request.
    pub excluded_peers: Vec<PeerId>,
}

impl SnapRequestOptions {
    /// Dispatches at `priority` without excluding any peer.
    pub const fn with_priority(priority: Priority) -> Self {
        Self { priority, excluded_peers: Vec::new() }
    }

    /// Excludes `peer_id`, keeping the exclusions free of duplicates so a peer caught twice does
    /// not grow the list a retry has to carry.
    pub fn exclude_peer(&mut self, peer_id: PeerId) {
        if !self.excluded_peers.contains(&peer_id) {
            self.excluded_peers.push(peer_id);
        }
    }
}

/// Fails every snap request with [`RequestError::UnsupportedCapability`], so the noop client can
/// stand in wherever a [`SnapClient`] bound is required but snap is not served.
impl<Net> SnapClient for NoopFullBlockClient<Net>
where
    Net: NetworkPrimitives,
{
    type Output = futures::future::Ready<PeerRequestResult<SnapResponse>>;

    /// Fails every snap request as unsupported, whatever the options ask for.
    fn request_snap(
        &self,
        _request: SnapProtocolMessage,
        _options: SnapRequestOptions,
    ) -> Self::Output {
        futures::future::ready(Err(RequestError::UnsupportedCapability))
    }
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
