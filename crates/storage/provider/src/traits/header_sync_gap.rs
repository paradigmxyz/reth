use alloy_consensus::{BlockHeader, Header};
use alloy_eips::BlockHashOrNumber;
use alloy_primitives::{BlockNumber, Sealable};
use reth_network_p2p::headers::downloader::SyncTarget;
use reth_primitives_traits::SealedHeader;
use reth_storage_errors::provider::ProviderResult;

/// Represents a gap to sync: from `local_head` to `target`
#[derive(Clone, Debug)]
pub struct HeaderSyncGap<H = Header> {
    /// The local head block. Represents lower bound of sync range.
    pub local_head: SealedHeader<H>,

    /// The sync target. Represents upper bound of sync range.
    pub target: SyncTarget,
}

impl<H: BlockHeader + Sealable> HeaderSyncGap<H> {
    /// Returns `true` if the gap from the head to the target was closed
    #[inline]
    pub fn is_closed(&self) -> bool {
        match self.target.tip() {
            BlockHashOrNumber::Hash(hash) => self.local_head.hash() == hash,
            BlockHashOrNumber::Number(num) => self.local_head.number() == num,
        }
    }
}

/// Client trait for determining the current headers sync gap.
#[auto_impl::auto_impl(&, Arc)]
pub trait HeaderSyncGapProvider: Send + Sync {
    /// The header type.
    type Header: Send + Sync;

    /// Find a current sync gap for the headers depending on the last
    /// uninterrupted block number. Last uninterrupted block represents the block number before
    /// which there are no gaps. It's up to the caller to ensure that last uninterrupted block is
    /// determined correctly.
    fn local_tip_header(
        &self,
        highest_uninterrupted_block: BlockNumber,
    ) -> ProviderResult<SealedHeader<Self::Header>>;
}
