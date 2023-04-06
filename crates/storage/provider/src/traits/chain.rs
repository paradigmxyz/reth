///! Canonical chain state notification trait and types.
use crate::PostState;
use auto_impl::auto_impl;
use core::fmt;
use reth_primitives::{BlockNumber, SealedBlockWithSenders};
use std::{collections::BTreeMap, fmt::Formatter, sync::Arc};
use tokio::sync::broadcast::{Receiver, Sender};

/// Trait that holds blocks and post state of execution those blocks.
#[auto_impl(&, Arc)]
pub trait SubChain: Send + Sync {
    /// Get chain post state.
    fn state(&self) -> &PostState;

    /// Get chain blocks.
    fn blocks(&self) -> &BTreeMap<BlockNumber, SealedBlockWithSenders>;
}

impl fmt::Debug for dyn SubChain {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubChain")
            .field("state", &self.state())
            .field("blocks", &self.blocks())
            .finish()
    }
}

impl PartialEq for dyn SubChain {
    fn eq(&self, other: &Self) -> bool {
        self.state() == other.state() && self.blocks() == other.blocks()
    }
}

/// Chain action that is triggered when a new block is imported or old block is reverted.
/// and will return all [`PostState`] and [`SealedBlockWithSenders`] of both reverted and commited
/// blocks.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub enum CanonStateNotification {
    /// Chain reorgs and both old and new chain are returned.
    Reorg { old: Arc<dyn SubChain>, new: Arc<dyn SubChain> },
    /// Chain got reverted without reorg and only old chain is returned.
    Revert { old: Arc<dyn SubChain> },
    /// Chain got extended without reorg and only new chain is returned.
    Commit { new: Arc<dyn SubChain> },
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
    pub fn old(&self) -> Option<Arc<dyn SubChain>> {
        match self {
            Self::Reorg { old, .. } => Some(old.clone()),
            Self::Revert { old } => Some(old.clone()),
            Self::Commit { .. } => None,
        }
    }

    /// Get new chain if any.
    pub fn new(&self) -> Option<Arc<dyn SubChain>> {
        match self {
            Self::Reorg { new, .. } => Some(new.clone()),
            Self::Revert { .. } => None,
            Self::Commit { new } => Some(new.clone()),
        }
    }
}

/// Type alias for a receiver that receives [NewBlockNotification]
pub type CanonStateNotifications = Receiver<CanonStateNotification>;

/// Type alias for a sender that sends [CanonChainStateNotification]
pub type CanonStateNotificationSender = Sender<CanonStateNotification>;

/// A type that allows to register chain related event subscriptions.
#[auto_impl(&, Arc)]
pub trait CanonStateSubscriptions: Send + Sync {
    /// Get notified when a new block was imported.
    fn subscribe_canon_state(&self) -> CanonStateNotifications;
}
