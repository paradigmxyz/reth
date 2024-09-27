//! Canonical chain state notification trait and types.

use auto_impl::auto_impl;
use derive_more::{Deref, DerefMut};
use reth_execution_types::{BlockReceipts, Chain};
use reth_primitives::{SealedBlockWithSenders, SealedHeader};
use std::{
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{broadcast, watch};
use tokio_stream::{
    wrappers::{BroadcastStream, WatchStream},
    Stream,
};
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

/// A notification that is sent when a new block is imported, or an old block is reverted.
///
/// The notification contains at least one [`Chain`] with the imported segment. If some blocks were
/// reverted (e.g. during a reorg), the old chain is also returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CanonStateNotification {
    /// The canonical chain was extended.
    Commit {
        /// The newly added chain segment.
        new: Arc<Chain>,
    },
    /// A chain segment was reverted or reorged.
    ///
    /// - In the case of a reorg, the reverted blocks are present in `old`, and the new blocks are
    ///   present in `new`.
    /// - In the case of a revert, the reverted blocks are present in `old`, and `new` is an empty
    ///   chain segment.
    Reorg {
        /// The chain segment that was reverted.
        old: Arc<Chain>,
        /// The chain segment that was added on top of the canonical chain, minus the reverted
        /// blocks.
        ///
        /// In the case of a revert, not a reorg, this chain segment is empty.
        new: Arc<Chain>,
    },
}

impl CanonStateNotification {
    /// Get the chain segment that was reverted, if any.
    pub fn reverted(&self) -> Option<Arc<Chain>> {
        match self {
            Self::Commit { .. } => None,
            Self::Reorg { old, .. } => Some(old.clone()),
        }
    }

    /// Get the newly imported chain segment, if any.
    pub fn committed(&self) -> Arc<Chain> {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.clone(),
        }
    }

    /// Get the new tip of the chain.
    ///
    /// Returns the new tip for [`Self::Reorg`] and [`Self::Commit`] variants which commit at least
    /// 1 new block.
    pub fn tip(&self) -> &SealedBlockWithSenders {
        match self {
            Self::Commit { new } | Self::Reorg { new, .. } => new.tip(),
        }
    }

    /// Get receipts in the reverted and newly imported chain segments with their corresponding
    /// block numbers and transaction hashes.
    ///
    /// The boolean in the tuple (2nd element) denotes whether the receipt was from the reverted
    /// chain segment.
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
pub struct ForkChoiceNotifications(pub watch::Receiver<Option<SealedHeader>>);

/// A trait that allows to register to fork choice related events
/// and get notified when a new fork choice is available.
pub trait ForkChoiceSubscriptions: Send + Sync {
    /// Get notified when a new safe block of the chain is selected.
    fn subscribe_safe_block(&self) -> ForkChoiceNotifications;

    /// Get notified when a new finalized block of the chain is selected.
    fn subscribe_finalized_block(&self) -> ForkChoiceNotifications;

    /// Convenience method to get a stream of the new safe blocks of the chain.
    fn safe_block_stream(&self) -> ForkChoiceStream<SealedHeader> {
        ForkChoiceStream::new(self.subscribe_safe_block().0)
    }

    /// Convenience method to get a stream of the new finalized blocks of the chain.
    fn finalized_block_stream(&self) -> ForkChoiceStream<SealedHeader> {
        ForkChoiceStream::new(self.subscribe_finalized_block().0)
    }
}

/// A stream for fork choice watch channels (pending, safe or finalized watchers)
#[derive(Debug)]
#[pin_project::pin_project]
pub struct ForkChoiceStream<T> {
    #[pin]
    st: WatchStream<Option<T>>,
}

impl<T: Clone + Sync + Send + 'static> ForkChoiceStream<T> {
    /// Creates a new `ForkChoiceStream`
    pub fn new(rx: watch::Receiver<Option<T>>) -> Self {
        Self { st: WatchStream::from_changes(rx) }
    }
}

impl<T: Clone + Sync + Send + 'static> Stream for ForkChoiceStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Some(notification)) => return Poll::Ready(Some(notification)),
                Some(None) => continue,
                None => return Poll::Ready(None),
            }
        }
    }
}
