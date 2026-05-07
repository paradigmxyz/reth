use crate::{download::DownloadClient, error::PeerRequestResult, priority::Priority};
use alloy_primitives::B256;
use auto_impl::auto_impl;
use futures::Future;
use reth_eth_wire_types::BlockAccessLists;

/// Controls whether a BAL request must wait for a capable peer or may complete early when none are
/// available.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BalRequirement {
    /// Keep waiting until an eth/71-capable peer is available.
    #[default]
    Mandatory,
    /// Return early if no connected peer can serve BALs.
    Optional,
}

/// A client capable of downloading block access lists.
#[auto_impl(&, Arc, Box)]
pub trait BlockAccessListsClient: DownloadClient {
    /// The bal type this client fetches.
    type Output: Future<Output = PeerRequestResult<BlockAccessLists>> + Send + Sync + Unpin;

    ///  Fetches the block access lists for given hashes.
    fn get_block_access_lists(&self, hashes: Vec<B256>) -> Self::Output {
        self.get_block_access_lists_with_priority_and_requirement(
            hashes,
            Priority::Normal,
            BalRequirement::Mandatory,
        )
    }

    /// Fetches the block access lists for given hashes with the requested BAL availability policy.
    fn get_block_access_lists_with_requirement(
        &self,
        hashes: Vec<B256>,
        requirement: BalRequirement,
    ) -> Self::Output {
        self.get_block_access_lists_with_priority_and_requirement(
            hashes,
            Priority::Normal,
            requirement,
        )
    }

    ///  Fetches the block access lists for given hashes with priority
    fn get_block_access_lists_with_priority(
        &self,
        hashes: Vec<B256>,
        priority: Priority,
    ) -> Self::Output {
        self.get_block_access_lists_with_priority_and_requirement(
            hashes,
            priority,
            BalRequirement::Mandatory,
        )
    }

    /// Fetches the block access lists for given hashes with priority and BAL availability policy.
    fn get_block_access_lists_with_priority_and_requirement(
        &self,
        hashes: Vec<B256>,
        priority: Priority,
        requirement: BalRequirement,
    ) -> Self::Output;
}
