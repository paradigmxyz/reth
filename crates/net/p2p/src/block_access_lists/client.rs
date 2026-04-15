use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_primitives::B256;
use auto_impl::auto_impl;
use futures::Future;
use reth_eth_wire_types::BlockAccessLists;

/// A client capable of downloading block access lists.
#[auto_impl(&, Arc, Box)]
pub trait BlockAccessListsClient: DownloadClient {
    /// The bal type this client fetches.
    type Output: Future<Output = PeerRequestResult<BlockAccessLists>> + Send + Sync + Unpin;

    ///  Fetches the block access lists for given hashes.
    fn get_block_access_lists(&self, hashes: Vec<B256>) -> Self::Output {
        self.get_block_access_lists_with_priority(hashes, Priority::Normal)
    }

    ///  Fetches the block access lists for given hashes with priority
    fn get_block_access_lists_with_priority(
        &self,
        hashes: Vec<B256>,
        priority: Priority,
    ) -> Self::Output;
}
