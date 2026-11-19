//! Account/storage changeset iteration support for walking through historical state changes in
//! static files.

use crate::ProviderResult;
use alloy_primitives::BlockNumber;
use reth_db::models::AccountBeforeTx;
use reth_db_api::models::BlockNumberAddress;
use reth_primitives_traits::StorageEntry;
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader};
use std::{
    ops::{Bound, RangeBounds, RangeInclusive},
    vec::IntoIter,
};

/// Iterator that walks account changesets from static files in a block range.
///
/// This iterator fetches changesets block by block to avoid loading everything into memory.
#[derive(Debug)]
pub struct StaticFileAccountChangesetWalker<P> {
    /// Static file provider.
    provider: P,
    /// Blocks still to be fetched.
    blocks: RangeInclusive<BlockNumber>,
    /// Block number attached to the buffered account changesets.
    current_block: BlockNumber,
    /// Remaining changesets for the current block.
    current_changesets: IntoIter<AccountBeforeTx>,
}

impl<P> StaticFileAccountChangesetWalker<P> {
    /// Create a new static file changeset walker.
    ///
    /// Accepts any range type that implements `RangeBounds<BlockNumber>`, including:
    /// - `Range<BlockNumber>` (e.g., `0..100`)
    /// - `RangeInclusive<BlockNumber>` (e.g., `0..=99`)
    /// - `RangeFrom<BlockNumber>` (e.g., `0..`) - iterates until exhausted
    ///
    /// If there is no start bound, 0 is used as the start block.
    pub fn new(provider: P, range: impl RangeBounds<BlockNumber>) -> Self {
        Self {
            provider,
            blocks: changeset_block_range(range, BlockNumber::MAX),
            current_block: 0,
            current_changesets: Default::default(),
        }
    }
}

impl<P> Iterator for StaticFileAccountChangesetWalker<P>
where
    P: ChangeSetReader,
{
    type Item = ProviderResult<(BlockNumber, AccountBeforeTx)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(changeset) = self.current_changesets.next() {
                return Some(Ok((self.current_block, changeset)))
            }
            self.current_block = self.blocks.next()?;
            match self.provider.account_block_changeset(self.current_block) {
                Ok(changesets) => self.current_changesets = changesets.into_iter(),
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

/// Iterator that walks storage changesets from static files in a block range.
#[derive(Debug)]
pub struct StaticFileStorageChangesetWalker<P> {
    /// Static file provider.
    provider: P,
    /// Blocks still to be fetched.
    blocks: RangeInclusive<BlockNumber>,
    /// Remaining changesets for the current block.
    current_changesets: IntoIter<(BlockNumberAddress, StorageEntry)>,
}

impl<P> StaticFileStorageChangesetWalker<P> {
    /// Create a new static file storage changeset walker.
    pub fn new(provider: P, range: impl RangeBounds<BlockNumber>) -> Self {
        Self {
            provider,
            blocks: changeset_block_range(range, BlockNumber::MAX),
            current_changesets: Default::default(),
        }
    }
}

impl<P> Iterator for StaticFileStorageChangesetWalker<P>
where
    P: StorageChangeSetReader,
{
    type Item = ProviderResult<(BlockNumberAddress, StorageEntry)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(changeset) = self.current_changesets.next() {
                return Some(Ok(changeset))
            }
            match self.provider.storage_changeset(self.blocks.next()?) {
                Ok(changesets) => self.current_changesets = changesets.into_iter(),
                Err(err) => return Some(Err(err)),
            }
        }
    }
}

/// Converts changeset bounds to an inclusive block range capped at the available tip.
pub(crate) fn changeset_block_range(
    range: impl RangeBounds<BlockNumber>,
    tip: BlockNumber,
) -> RangeInclusive<BlockNumber> {
    let start = match range.start_bound() {
        Bound::Included(&n) => n,
        Bound::Excluded(&BlockNumber::MAX) => return RangeInclusive::new(1, 0),
        Bound::Excluded(&n) => n + 1,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Included(&n) => n.min(tip),
        Bound::Excluded(&0) => return RangeInclusive::new(1, 0),
        Bound::Excluded(&n) => (n - 1).min(tip),
        Bound::Unbounded => tip,
    };
    start..=end
}
