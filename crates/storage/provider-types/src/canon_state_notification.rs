use std::sync::Arc;

use reth_execution_types::{BlockReceipts, Chain};
use reth_primitives::SealedBlockWithSenders;

/// Chain action that is triggered when a new block is imported or old block is reverted.
/// and will return all [`crate::ExecutionOutcome`] and
/// [`reth_primitives::SealedBlockWithSenders`] of both reverted and committed blocks.
#[derive(Clone, Debug)]
pub enum CanonStateNotification {
    /// Chain got extended without reorg and only new chain is returned.
    Commit {
        /// The newly extended chain.
        new: Arc<Chain>,
    },
    /// Chain reorgs and both old and new chain are returned.
    /// Revert is just a subset of reorg where the new chain is empty.
    Reorg {
        /// The old chain before reorganization.
        old: Arc<Chain>,
        /// The new chain after reorganization.
        new: Arc<Chain>,
    },
}

// For one reason or another, the compiler can't derive PartialEq for CanonStateNotification.
// so we are forced to implement it manually.
impl PartialEq for CanonStateNotification {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Reorg { old: old1, new: new1 }, Self::Reorg { old: old2, new: new2 }) => {
                old1 == old2 && new1 == new2
            }
            (Self::Commit { new: new1 }, Self::Commit { new: new2 }) => new1 == new2,
            _ => false,
        }
    }
}

impl CanonStateNotification {
    /// Get old chain if any.
    pub fn reverted(&self) -> Option<Arc<Chain>> {
        match self {
            Self::Commit { .. } => None,
            Self::Reorg { old, .. } => Some(old.clone()),
        }
    }

    /// Get the new chain if any.
    ///
    /// Returns the new committed [Chain] for [`Self::Reorg`] and [`Self::Commit`] variants.
    pub fn committed(&self) -> Arc<Chain> {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.clone(),
        }
    }

    /// Returns the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    pub fn tip(&self) -> &SealedBlockWithSenders {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.tip(),
        }
    }

    /// Return receipt with its block number and transaction hash.
    ///
    /// Last boolean is true if receipt is from reverted block.
    pub fn block_receipts(&self) -> Vec<(BlockReceipts, bool)> {
        let mut receipts = Vec::new();

        // get old receipts
        if let Some(old) = self.reverted() {
            receipts
                .extend(old.receipts_with_attachment().into_iter().map(|receipt| (receipt, true)));
        }
        // get new receipts
        receipts.extend(
            self.committed().receipts_with_attachment().into_iter().map(|receipt| (receipt, false)),
        );
        receipts
    }
}
