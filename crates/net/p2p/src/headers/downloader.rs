use super::error::HeadersDownloaderResult;
use crate::error::{DownloadError, DownloadResult};
use alloy_eips::{eip1898::BlockWithParent, BlockHashOrNumber};
use alloy_primitives::{Sealable, B256};
use futures::Stream;
use reth_consensus::HeaderValidator;
use reth_primitives_traits::{BlockHeader, Header, SealedHeader};
use std::fmt::Debug;

/// A downloader capable of fetching and yielding block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [`HeadersClient`][crate::headers::client::HeadersClient] represents a client capable
/// of fulfilling these requests.
///
/// A [`HeaderDownloader`] is a [Stream] that returns batches of headers.
pub trait HeaderDownloader:
    Send
    + Sync
    + Stream<Item = HeadersDownloaderResult<Vec<SealedHeader<Self::Header>>, Self::Header>>
    + Unpin
{
    /// The header type being downloaded.
    type Header: Sealable + Debug + Send + Sync + Unpin + 'static;

    /// Updates the gap to sync which ranges from local head to the sync target.
    ///
    /// See also [`HeaderDownloader::update_sync_target`] and
    /// [`HeaderDownloader::update_local_head`]
    fn update_sync_gap(&mut self, head: SealedHeader<Self::Header>, target: SyncTarget) {
        self.update_local_head(head);
        self.update_sync_target(target);
    }

    /// Updates the block number of the local database
    fn update_local_head(&mut self, head: SealedHeader<Self::Header>);

    /// Updates the target we want to sync to.
    fn update_sync_target(&mut self, target: SyncTarget);

    /// Sets the headers batch size that the Stream should return.
    fn set_batch_size(&mut self, limit: usize);
}

/// Specifies the target to sync for [`HeaderDownloader::update_sync_target`]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SyncTarget {
    /// This represents a range missing headers in the form of `(head,..`
    ///
    /// Sync _inclusively_ to the given block hash.
    ///
    /// This target specifies the upper end of the sync gap `(head...tip]`
    Tip(B256),
    /// This represents a gap missing headers bounded by the given header `h` in the form of
    /// `(head,..h),h+1,h+2...`
    ///
    /// Sync _exclusively_ to the given header's parent which is: `(head..h-1]`
    ///
    /// The benefit of this variant is, that this already provides the block number of the highest
    /// missing block.
    Gap(BlockWithParent),
    /// This represents a tip by block number
    TipNum(u64),
}

// === impl SyncTarget ===

impl SyncTarget {
    /// Returns the tip to sync to _inclusively_
    ///
    /// This returns the hash if the target is [`SyncTarget::Tip`] or the `parent_hash` of the given
    /// header in [`SyncTarget::Gap`]
    pub fn tip(&self) -> BlockHashOrNumber {
        match self {
            Self::Tip(tip) => (*tip).into(),
            Self::Gap(gap) => gap.parent.into(),
            Self::TipNum(num) => (*num).into(),
        }
    }
}

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

/// Validate whether the header is valid in relation to it's parent
///
/// Returns Ok(false) if the
pub fn validate_header_download<H: BlockHeader>(
    consensus: &dyn HeaderValidator<H>,
    header: &SealedHeader<H>,
    parent: &SealedHeader<H>,
) -> DownloadResult<()> {
    // validate header against parent
    consensus.validate_header_against_parent(header, parent).map_err(|error| {
        DownloadError::HeaderValidation {
            hash: header.hash(),
            number: header.number(),
            error: Box::new(error),
        }
    })?;
    // validate header standalone
    consensus.validate_header(header).map_err(|error| DownloadError::HeaderValidation {
        hash: header.hash(),
        number: header.number(),
        error: Box::new(error),
    })?;
    Ok(())
}
