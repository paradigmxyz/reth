use std::sync::Arc;

use reth_provider::{CanonStateNotification, Chain};

/// Notifications sent to an ExEx.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExExNotification {
    /// Chain got committed without a reorg, and only the new chain is returned.
    ChainCommitted {
        /// The new chain after commit.
        new: Arc<Chain>,
    },
    /// Chain got reorged, and both the old and the new chains are returned.
    ChainReorged {
        /// The old chain before reorg.
        old: Arc<Chain>,
        /// The new chain after reorg.
        new: Arc<Chain>,
    },
    /// Chain got reverted, and only the old chain is returned.
    ChainReverted {
        /// The old chain before reversion.
        old: Arc<Chain>,
    },
}

impl ExExNotification {
    /// Returns the committed chain from the [Self::ChainCommitted] and [Self::ChainReorged]
    /// variants, if any.
    pub fn committed_chain(&self) -> Option<Arc<Chain>> {
        match self {
            Self::ChainCommitted { new } | Self::ChainReorged { old: _, new } => Some(new.clone()),
            Self::ChainReverted { .. } => None,
        }
    }

    /// Returns the reverted chain from the [Self::ChainReorged] and [Self::ChainReverted] variants,
    /// if any.
    pub fn reverted_chain(&self) -> Option<Arc<Chain>> {
        match self {
            Self::ChainReorged { old, new: _ } | Self::ChainReverted { old } => Some(old.clone()),
            Self::ChainCommitted { .. } => None,
        }
    }
}

impl From<CanonStateNotification> for ExExNotification {
    fn from(notification: CanonStateNotification) -> Self {
        match notification {
            CanonStateNotification::Commit { new } => Self::ChainCommitted { new },
            CanonStateNotification::Reorg { old, new } => Self::ChainReorged { old, new },
        }
    }
}
