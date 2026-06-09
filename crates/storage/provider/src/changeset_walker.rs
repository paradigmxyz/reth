//! Account/storage changeset iteration support for walking through historical state changes in
//! static files.

use crate::ProviderResult;
use alloy_primitives::BlockNumber;
use reth_db::models::AccountBeforeTx;
use reth_db_api::models::BlockNumberAddress;
use reth_primitives_traits::StorageEntry;
use reth_storage_api::{ChangeSetReader, StorageChangeSetReader};
use std::{
    ops::{Bound, RangeBounds},
    vec::IntoIter,
};

/// Iterator that walks account changesets from static files in a block range.
///
/// This iterator fetches changesets block by block to avoid loading everything into memory.
#[derive(Debug)]
pub struct StaticFileAccountChangesetWalker<P> {
    /// Static file provider
    provider: P,
    /// End block (exclusive). `None` means iterate until exhausted.
    end_block: Option<BlockNumber>,
    /// Current block being processed
    current_block: BlockNumber,
    /// Block number for buffered changesets.
    loaded_block: Option<BlockNumber>,
    /// Changesets for current block
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
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        let end_block = match range.end_bound() {
            Bound::Included(&n) => Some(n + 1),
            Bound::Excluded(&n) => Some(n),
            Bound::Unbounded => None,
        };

        Self {
            provider,
            end_block,
            current_block: start,
            loaded_block: None,
            current_changesets: Vec::new().into_iter(),
        }
    }
}

impl<P> Iterator for StaticFileAccountChangesetWalker<P>
where
    P: ChangeSetReader,
{
    type Item = ProviderResult<(BlockNumber, AccountBeforeTx)>;

    fn next(&mut self) -> Option<Self::Item> {
        // Yield remaining changesets from current block
        if let Some(changeset) = self.current_changesets.next() {
            let block_number = self.loaded_block.expect("buffered changesets have a block");
            return Some(Ok((block_number, changeset)));
        }

        // Load next block with changesets
        while self.end_block.is_none_or(|end| self.current_block < end) {
            let block_number = self.current_block;
            self.current_block += 1;

            match self.provider.account_block_changeset(block_number) {
                Ok(changesets) => {
                    let mut changesets = changesets.into_iter();
                    let Some(first) = changesets.next() else { continue };
                    self.loaded_block = Some(block_number);
                    self.current_changesets = changesets;
                    return Some(Ok((block_number, first)));
                }
                Err(e) => {
                    return Some(Err(e));
                }
            }
        }

        None
    }
}

/// Iterator that walks storage changesets from static files in a block range.
#[derive(Debug)]
pub struct StaticFileStorageChangesetWalker<P> {
    /// Static file provider
    provider: P,
    /// End block (exclusive). `None` means iterate until exhausted.
    end_block: Option<BlockNumber>,
    /// Current block being processed
    current_block: BlockNumber,
    /// Changesets for current block
    current_changesets: IntoIter<(BlockNumberAddress, StorageEntry)>,
}

impl<P> StaticFileStorageChangesetWalker<P> {
    /// Create a new static file storage changeset walker.
    pub fn new(provider: P, range: impl RangeBounds<BlockNumber>) -> Self {
        let start = match range.start_bound() {
            Bound::Included(&n) => n,
            Bound::Excluded(&n) => n + 1,
            Bound::Unbounded => 0,
        };

        let end_block = match range.end_bound() {
            Bound::Included(&n) => Some(n + 1),
            Bound::Excluded(&n) => Some(n),
            Bound::Unbounded => None,
        };

        Self {
            provider,
            end_block,
            current_block: start,
            current_changesets: Vec::new().into_iter(),
        }
    }
}

impl<P> Iterator for StaticFileStorageChangesetWalker<P>
where
    P: StorageChangeSetReader,
{
    type Item = ProviderResult<(BlockNumberAddress, StorageEntry)>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(changeset) = self.current_changesets.next() {
            return Some(Ok(changeset));
        }

        while self.end_block.is_none_or(|end| self.current_block < end) {
            let block_number = self.current_block;
            self.current_block += 1;

            match self.provider.storage_changeset(block_number) {
                Ok(changesets) => {
                    let mut changesets = changesets.into_iter();
                    let Some(first) = changesets.next() else { continue };
                    self.current_changesets = changesets;
                    return Some(Ok(first));
                }
                Err(e) => {
                    return Some(Err(e));
                }
            }
        }

        None
    }
}
