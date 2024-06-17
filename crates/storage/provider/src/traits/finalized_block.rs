use auto_impl::auto_impl;
use reth_errors::ProviderResult;
use reth_primitives::{BlockNumber, SealedHeader};
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::watch;
use tokio_stream::{wrappers::WatchStream, Stream};

/// Functionality to read the last known finalized block from the database.
pub trait FinalizedBlockReader: Send + Sync {
    /// Returns the last finalized block number.
    fn last_finalized_block_number(&self) -> ProviderResult<BlockNumber>;
}

/// Functionality to write the last known finalized block to the database.
pub trait FinalizedBlockWriter: Send + Sync {
    /// Saves the given finalized block number in the DB.
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;
}

/// Type alias for a receiver that receives [`CanonStateNotification`]
pub type FinalizedBlockNotifications = watch::Receiver<FinalizedBlockNotification>;

/// Type alias for a sender that sends [`CanonStateNotification`]
pub type FinalizedBlockNotificationSender = watch::Sender<FinalizedBlockNotification>;

/// A type that allows to register finalized block event subscriptions.
#[auto_impl(&, Arc)]
pub trait FinalizedBlocksSubscriptions: Send + Sync {
    /// Get notified when a new block was finalized.
    ///
    /// A finalized block
    fn subscribe_to_finalized_blocks(&self) -> FinalizedBlockNotifications;

    /// Convenience method to get a stream of [`CanonStateNotification`].
    fn finalized_block_stream(&self) -> FinalizedBlockNotificationStream {
        FinalizedBlockNotificationStream {
            st: WatchStream::new(self.subscribe_to_finalized_blocks()),
        }
    }
}

/// A Stream of [CanonStateNotification].
#[derive(Debug)]
#[pin_project::pin_project]
pub struct FinalizedBlockNotificationStream {
    #[pin]
    st: WatchStream<FinalizedBlockNotification>,
}

impl Stream for FinalizedBlockNotificationStream {
    type Item = SealedHeader;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            return match ready!(self.as_mut().project().st.poll_next(cx)) {
                Some(Some(notification)) => Poll::Ready(Some(notification)),
                Some(None) => continue,
                None => Poll::Ready(None),
            }
        }
    }
}

/// Finalized block header
pub type FinalizedBlockNotification = Option<SealedHeader>;
