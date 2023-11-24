use auto_impl::auto_impl;
use reth_interfaces::{p2p::headers::downloader::SyncTarget, RethResult};
use reth_primitives::{BlockHashOrNumber, BlockNumber, SealedHeader, B256};
use tokio::sync::watch;

/// The header sync mode.
#[derive(Clone, Debug)]
pub enum HeaderSyncMode {
    /// A sync mode in which the stage continuously requests the downloader for
    /// next blocks.
    Continuous,
    /// A sync mode in which the stage polls the receiver for the next tip
    /// to download from.
    Tip(watch::Receiver<B256>),
}

/// Represents a gap to sync: from `local_head` to `target`
#[derive(Clone, Debug)]
pub struct HeaderSyncGap {
    /// The local head block. Represents lower bound of sync range.
    pub local_head: SealedHeader,

    /// The sync target. Represents upper bound of sync range.
    pub target: SyncTarget,
}

impl HeaderSyncGap {
    /// Returns `true` if the gap from the head to the target was closed
    #[inline]
    pub fn is_closed(&self) -> bool {
        match self.target.tip() {
            BlockHashOrNumber::Hash(hash) => self.local_head.hash() == hash,
            BlockHashOrNumber::Number(num) => self.local_head.number == num,
        }
    }
}

/// Client trait for determining the current headers sync gap.
#[auto_impl(&, Arc)]
pub trait HeaderSyncGapProvider: Send + Sync {
    /// Find a current sync gap for the headers depending on the [HeaderSyncMode] and the last
    /// uninterrupted block number. Last uninterrupted block represents the block number before
    /// which there are no gaps. It's up to the caller to ensure that last uninterrupted block is
    /// determined correctly.
    fn sync_gap(
        &self,
        mode: HeaderSyncMode,
        highest_uninterrupted_block: BlockNumber,
    ) -> RethResult<HeaderSyncGap>;
}
