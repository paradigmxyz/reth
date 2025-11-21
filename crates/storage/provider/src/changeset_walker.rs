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
        // Yield remaining changesets from current block
        if let Some(changeset) = self.current_changesets.get(self.changeset_index).cloned() {
            self.changeset_index += 1;
            return Some(Ok((self.current_block, changeset)));
        }

        // Advance to next block if we exhausted the previous one
        if !self.current_changesets.is_empty() {
            self.current_block += 1;
        }

        // Load next block with changesets
        while self.current_block < self.range.end {
            match self.provider.account_block_changeset(self.current_block) {
                Ok(changesets) if !changesets.is_empty() => {
                    self.current_changesets = changesets;
                    self.changeset_index = 1;
                    return Some(Ok((self.current_block, self.current_changesets[0].clone())));
                }
                Ok(_) => self.current_block += 1,
                Err(e) => {
                    self.current_block += 1;
                    return Some(Err(e));
                }
            }
        }

        None
    }
}
