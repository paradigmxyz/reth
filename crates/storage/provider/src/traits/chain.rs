//! Canonical chain state notification trait and types.

use auto_impl::auto_impl;
use derive_more::{Deref, DerefMut};
use reth_primitives::SealedHeader;
use reth_provider_types::CanonStateNotification;
use std::{
    pin::Pin,
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
