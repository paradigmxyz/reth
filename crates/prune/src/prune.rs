//! Support for pruning.

use futures_util::{Stream, StreamExt};
use reth_primitives::BlockNumber;
use reth_provider::CanonStateNotification;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::debug;

/// Pruning routine. Implements [Future] where the pruning logic happens.
pub struct Pruner<St, Client> {
    /// Stream of canonical state notifications. Pruning is triggered by new incoming
    /// notifications.
    canon_state_stream: St,
    /// Database interaction client.
    #[allow(dead_code)]
    client: Client,
    /// Minimum pruning interval measured in blocks. All prune parts are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: u64,
    /// Maximum prune depth. Used to determine the pruning target for parts that are needed during
    /// the reorg, e.g. changesets.
    #[allow(dead_code)]
    max_prune_depth: u64,
    /// Last pruned block number. Used in conjunction with `min_block_interval` to determine
    /// when the pruning needs to be initiated.
    last_pruned_block_number: Option<BlockNumber>,
}

impl<St, Client> Pruner<St, Client>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    Client: Send + Unpin + 'static,
{
    /// Creates a new [Pruner].
    pub fn new(
        canon_state_stream: St,
        client: Client,
        min_block_interval: u64,
        max_prune_depth: u64,
    ) -> Self {
        Self {
            canon_state_stream,
            client,
            min_block_interval,
            max_prune_depth,
            last_pruned_block_number: None,
        }
    }
}

/// Pruning logic. The behaviour is following:
/// 1. Listen to new blocks via [CanonStateNotification].
/// 2. Check new block height according to `min_block_interval`.
/// 3. Prune.
impl<St, Client> Future for Pruner<St, Client>
where
    St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static,
    Client: Send + Unpin + 'static,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(event) = ready!(this.canon_state_stream.poll_next_unpin(cx)) else {
            // Events stream is closed
            return Poll::Ready(())
        };

        let tip = event.tip();
        let tip_block_number = tip.number;

        // Check minimum pruning interval according to the last pruned block and a new tip.
        // Saturating subtraction is needed for the case when `CanonStateNotification::Revert`
        // is received, meaning current block number might be less than the previously pruned
        // block number. If that's the case, no pruning is needed as outdated data is also
        // reverted.
        if this.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            tip.number.saturating_sub(last_pruned_block_number) >= this.min_block_interval
        }) {
            debug!(
                target: "prune",
                last_pruned_block_number = ?this.last_pruned_block_number,
                %tip_block_number,
                "Minimum pruning interval reached"
            );
            this.last_pruned_block_number = Some(tip.number);
        } else {
            return Poll::Pending
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use crate::Pruner;
    use futures_util::FutureExt;
    use reth_primitives::SealedBlockWithSenders;
    use reth_provider::{
        test_utils::{NoopProvider, TestCanonStateSubscriptions},
        CanonStateSubscriptions, Chain,
    };
    use std::{future::poll_fn, sync::Arc, task::Poll};

    #[tokio::test]
    async fn pruner_respects_last_pruned_block_number() {
        let mut canon_state_stream = TestCanonStateSubscriptions::default();
        let mut pruner =
            Pruner::new(canon_state_stream.canonical_state_stream(), NoopProvider::default(), 5, 0);

        // Last pruned block number is empty on initialization
        assert_eq!(pruner.last_pruned_block_number, None);

        let mut chain = Chain::default();

        let first_block = SealedBlockWithSenders::default();
        let first_block_number = first_block.number;
        chain.blocks.insert(first_block_number, first_block);
        canon_state_stream.add_next_commit(Arc::new(chain.clone()));

        poll_fn(|cx| {
            assert!(pruner.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        // Last pruned block number is set to the first arrived block because we didn't have it set
        // before
        assert_eq!(pruner.last_pruned_block_number, Some(first_block_number));

        let mut second_block = SealedBlockWithSenders::default();
        second_block.block.header.number = first_block_number + pruner.min_block_interval;
        let second_block_number = second_block.number;
        chain.blocks.insert(second_block_number, second_block);
        canon_state_stream.add_next_commit(Arc::new(chain.clone()));

        poll_fn(|cx| {
            assert!(pruner.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        // Last pruned block number is updated because delta is larger than
        // min block interval
        assert_eq!(pruner.last_pruned_block_number, Some(second_block_number));

        let mut third_block = SealedBlockWithSenders::default();
        third_block.block.header.number = second_block_number + 1;
        chain.blocks.insert(third_block.number, third_block);
        canon_state_stream.add_next_commit(Arc::new(chain.clone()));

        poll_fn(|cx| {
            assert!(pruner.poll_unpin(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        // Last pruned block number is not updated because delta is smaller than
        // min block interval
        assert_eq!(pruner.last_pruned_block_number, Some(second_block_number));
    }
}
