//! Canonical chain state notification trait and types.

use crate::{BlockReceipts, Chain};
use auto_impl::auto_impl;
use derive_more::{Deref, DerefMut};
use reth_primitives::{SealedBlockWithSenders, SealedHeader};
use std::{
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, Stream};
use tracing::debug;

/// Type alias for a receiver that receives [`CanonStateNotification`]
pub type CanonStateNotifications = broadcast::Receiver<CanonStateNotification>;

/// Type alias for a sender that sends [`CanonStateNotification`]
pub type CanonStateNotificationSender = broadcast::Sender<CanonStateNotification>;

/// A type that allows to register chain related event subscriptions.
#[auto_impl(&, Arc)]
pub trait CanonStateSubscriptions: Send + Sync {
    /// Get notified when a new canonical chain was imported.
    ///
    /// A canonical chain be one or more blocks, a reorg or a revert.
    fn subscribe_to_canonical_state(&self) -> CanonStateNotifications;

    /// Convenience method to get a stream of [`CanonStateNotification`].
    fn canonical_state_stream(&self) -> CanonStateNotificationStream {
        CanonStateNotificationStream {
            st: BroadcastStream::new(self.subscribe_to_canonical_state()),
        }
    }
}

/// A Stream of [`CanonStateNotification`].
#[derive(Debug)]
#[pin_project::pin_project]
pub struct CanonStateNotificationStream {
    #[pin]
    st: BroadcastStream<CanonStateNotification>,
}

impl Stream for CanonStateNotificationStream {
    type Item = CanonStateNotification;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Ok(notification)) => Poll::Ready(Some(notification)),
                Some(Err(err)) => {
                    debug!(%err, "canonical state notification stream lagging behind");
                    continue
                }
                None => Poll::Ready(None),
            }
        }
    }
}

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

/// Wrapper around a broadcast receiver that receives fork choice notifications.
#[derive(Debug, Deref, DerefMut)]
pub struct ForkChoiceNotifications(broadcast::Receiver<SealedHeader>);

/// A trait that allows to register to fork choice related events
/// and get notified when a new fork choice is available.
pub trait ForkChoiceSubscriptions: Send + Sync {
    /// Get notified when a new head of the chain is selected.
    fn subscribe_to_fork_choice(&self) -> ForkChoiceNotifications;

    /// Convenience method to get a stream of the new head of the chain.
    fn fork_choice_stream(&self) -> ForkChoiceStream {
        ForkChoiceStream { st: BroadcastStream::new(self.subscribe_to_fork_choice().0) }
    }
}

/// A stream of the fork choices in the form of [`SealedHeader`].
#[derive(Debug)]
#[pin_project::pin_project]
pub struct ForkChoiceStream {
    #[pin]
    st: BroadcastStream<SealedHeader>,
}

impl Stream for ForkChoiceStream {
    type Item = SealedHeader;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Ok(notification)) => Poll::Ready(Some(notification)),
                Some(Err(err)) => {
                    debug!(%err, "finalized header notification stream lagging behind");
                    continue
                }
                None => Poll::Ready(None),
            };
        }
    }
}
