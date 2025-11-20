//! Account changeset iteration support for walking through historical account state changes in
//! static files.

use crate::ProviderResult;
use alloy_primitives::BlockNumber;
use reth_db::models::AccountBeforeTx;
use reth_storage_api::ChangeSetReader;
use std::ops::Range;

/// Iterator that walks account changesets from static files in a block range.
///
/// This iterator fetches changesets block by block to avoid loading everything into memory.
#[derive(Debug)]
pub struct StaticFileAccountChangesetWalker<P> {
    /// Static file provider
    provider: P,
    /// Block range to iterate over
    range: Range<BlockNumber>,
    /// Current block being processed
    current_block: BlockNumber,
    /// Changesets for current block
    current_changesets: Vec<AccountBeforeTx>,
    /// Index within current block's changesets
    changeset_index: usize,
}

impl<P> StaticFileAccountChangesetWalker<P> {
    /// Create a new static file changeset walker.
    pub fn new(provider: P, range: Range<BlockNumber>) -> Self {
        Self {
            provider,
            range: range.clone(),
            current_block: range.start,
            current_changesets: Vec::new(),
            changeset_index: 0,
        }
    }
}

impl<P> Iterator for StaticFileAccountChangesetWalker<P>
where
    P: ChangeSetReader,
{
    type Item = ProviderResult<(BlockNumber, AccountBeforeTx)>;

    fn next(&mut self) -> Option<Self::Item> {
        // Return next changeset from current block if available
        if self.changeset_index < self.current_changesets.len() {
            let changeset = self.current_changesets[self.changeset_index].clone();
            self.changeset_index += 1;
            return Some(Ok((self.current_block, changeset)));
        }

        // Move to next block and fetch changesets until we find some or reach the end
        while self.current_block < self.range.end {
            // Fetch changesets for current block
            match self.provider.account_block_changeset(self.current_block) {
                Ok(changesets) if !changesets.is_empty() => {
                    // Found changesets, prepare to yield them
                    self.current_changesets = changesets;
                    self.changeset_index = 1;
                    let first = self.current_changesets[0].clone();
                    let block = self.current_block;
                    self.current_block += 1;
                    return Some(Ok((block, first)));
                }
                Ok(_) => {
                    // No changesets for this block, continue to next
                    self.current_block += 1;
                }
                Err(e) => {
                    self.current_block += 1;
                    return Some(Err(e));
                }
            }
        }

        None
    }
}
