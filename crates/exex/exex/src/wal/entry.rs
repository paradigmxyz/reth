use reth_exex_types::ExExNotification;
use serde::{Deserialize, Serialize};

/// A single entry in the WAL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalEntry {
    pub(crate) target: NotificationCommitTarget,
    pub(crate) notification: ExExNotification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum NotificationCommitTarget {
    Commit,
    Canonicalize,
}

impl NotificationCommitTarget {
    pub(crate) const fn is_commit(&self) -> bool {
        matches!(self, Self::Commit)
    }

    pub(crate) const fn is_canonicalize(&self) -> bool {
        matches!(self, Self::Canonicalize)
    }
}
