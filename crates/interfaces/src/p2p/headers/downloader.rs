use crate::{
    consensus::Consensus,
    p2p::error::{DownloadError, DownloadResult},
};
use futures::Stream;
use reth_primitives::{SealedHeader, H256};

/// A downloader capable of fetching and yielding block headers.
///
/// A downloader represents a distinct strategy for submitting requests to download block headers,
/// while a [HeadersClient] represents a client capable of fulfilling these requests.
///
/// A [HeaderDownloader] is a [Stream] that returns batches for headers.
pub trait HeaderDownloader: Send + Sync + Stream<Item = Vec<SealedHeader>> + Unpin {
    /// Updates the gap to sync which ranges from local head to the sync target
    ///
    /// See also [HeaderDownloader::update_sync_target] and [HeaderDownloader::update_local_head]
    fn update_sync_gap(&mut self, head: SealedHeader, target: SyncTarget) {
        self.update_local_head(head);
        self.update_sync_target(target);
    }

    /// Updates the block number of the local database
    fn update_local_head(&mut self, head: SealedHeader);

    /// Updates the target we want to sync to
    fn update_sync_target(&mut self, target: SyncTarget);

    /// Sets the headers batch size that the Stream should return.
    fn set_batch_size(&mut self, limit: usize);
}

/// Specifies the target to sync for [HeaderDownloader::update_sync_target]
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum SyncTarget {
    /// This represents a range missing headers in the form of `(head,..`
    ///
    /// Sync _inclusively_ to the given block hash.
    ///
    /// This target specifies the upper end of the sync gap `(head...tip]`
    Tip(H256),
    /// This represents a gap missing headers bounded by the given header `h` in the form of
    /// `(head,..h),h+1,h+2...`
    ///
    /// Sync _exclusively_ to the given header's parent which is: `(head..h-1]`
    ///
    /// The benefit of this variant is, that this already provides the block number of the highest
    /// missing block.
    Gap(SealedHeader),
}

// === impl SyncTarget ===

impl SyncTarget {
    /// Returns the tip to sync to _inclusively_
    ///
    /// This returns the hash if the target is [SyncTarget::Tip] or the `parent_hash` of the given
    /// header in [SyncTarget::Gap]
    pub fn tip(&self) -> H256 {
        match self {
            SyncTarget::Tip(tip) => *tip,
            SyncTarget::Gap(gap) => gap.parent_hash,
        }
    }
}

/// Validate whether the header is valid in relation to it's parent
///
/// Returns Ok(false) if the
pub fn validate_header_download(
    consensus: &dyn Consensus,
    header: &SealedHeader,
    parent: &SealedHeader,
) -> DownloadResult<()> {
    ensure_parent(header, parent)?;
    consensus
        .validate_header(header, parent)
        .map_err(|error| DownloadError::HeaderValidation { hash: parent.hash(), error })?;
    Ok(())
}

/// Ensures that the given `parent` header is the actual parent of the `header`
pub fn ensure_parent(header: &SealedHeader, parent: &SealedHeader) -> DownloadResult<()> {
    if !(parent.hash() == header.parent_hash && parent.number + 1 == header.number) {
        return Err(DownloadError::MismatchedHeaders {
            header_number: header.number.into(),
            parent_number: parent.number.into(),
            header_hash: header.hash(),
            parent_hash: parent.hash(),
        })
    }
    Ok(())
}
