//! Canonical chain state notification trait and types.
use crate::{chain::BlockReceipts, Chain};
use auto_impl::auto_impl;
use std::sync::Arc;
use tokio::sync::broadcast::{Receiver, Sender};

/// Type alias for a receiver that receives [CanonStateNotification]
pub type CanonStateNotifications = Receiver<CanonStateNotification>;

/// Type alias for a sender that sends [CanonStateNotification]
pub type CanonStateNotificationSender = Sender<CanonStateNotification>;

/// A type that allows to register chain related event subscriptions.
#[auto_impl(&, Arc)]
pub trait CanonStateSubscriptions: Send + Sync {
    /// Get notified when a new block was imported.
    fn subscribe_canon_state(&self) -> CanonStateNotifications;
}

/// Chain action that is triggered when a new block is imported or old block is reverted.
/// and will return all [`crate::PostState`] and [`reth_primitives::SealedBlockWithSenders`] of both
/// reverted and commited blocks.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub enum CanonStateNotification {
    /// Chain reorgs and both old and new chain are returned.
    Reorg { old: Arc<Chain>, new: Arc<Chain> },
    /// Chain got reverted without reorg and only old chain is returned.
    Revert { old: Arc<Chain> },
    /// Chain got extended without reorg and only new chain is returned.
    Commit { new: Arc<Chain> },
}

// For one reason or another, the compiler can't derive PartialEq for CanonStateNotification.
// so we are forced to implement it manually.
impl PartialEq for CanonStateNotification {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Reorg { old: old1, new: new1 }, Self::Reorg { old: old2, new: new2 }) => {
                old1 == old2 && new1 == new2
            }
            (Self::Revert { old: old1 }, Self::Revert { old: old2 }) => old1 == old2,
            (Self::Commit { new: new1 }, Self::Commit { new: new2 }) => new1 == new2,
            _ => false,
        }
    }
}

impl CanonStateNotification {
    /// Get old chain if any.
    pub fn reverted(&self) -> Option<Arc<Chain>> {
        match self {
            Self::Reorg { old, .. } => Some(old.clone()),
            Self::Revert { old } => Some(old.clone()),
            Self::Commit { .. } => None,
        }
    }

    /// Get new chain if any.
    pub fn commited(&self) -> Option<Arc<Chain>> {
        match self {
            Self::Reorg { new, .. } => Some(new.clone()),
            Self::Revert { .. } => None,
            Self::Commit { new } => Some(new.clone()),
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
        if let Some(new) = self.commited() {
            receipts
                .extend(new.receipts_with_attachment().into_iter().map(|receipt| (receipt, false)));
        }
        receipts
    }
}
