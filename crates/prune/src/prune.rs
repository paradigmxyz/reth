//! Support for maintaining the state of the transaction pool

use futures_util::{Stream, StreamExt};
use reth_primitives::BlockNumber;
use reth_provider::CanonStateNotification;
use std::{
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tracing::debug;

// TODO(alexey): use config field from https://github.com/paradigmxyz/reth/pull/3341
/// Minimal pruning interval measured in blocks. All prune parts are checked and, if needed, pruned,
/// when the chain advances by the specified number of blocks.
const MIN_PRUNE_BLOCK_INTERVAL: u64 = 5;

/// Pruning routine. Implements [Future] where the pruning logic happens.
pub struct Pruner<St> {
    /// Stream of canonical state notifications. Pruning is triggered by new incoming
    /// notifications.
    canon_state_stream: St,
    /// Maximum reorg depth. Used to determine the pruning target for parts that are needed during
    /// the reorg, e.g. changesets.
    #[allow(dead_code)]
    max_reorg_depth: u64,
    /// Last pruned block number. Used in conjuction with [MIN_PRUNE_BLOCK_INTERVAL] to determine
    /// when the pruning needs to be initiated.
    last_pruned_block_number: Option<BlockNumber>,
}

impl<St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static> Pruner<St> {
    /// Creates a new [Pruner] with the specified stream of canonical state notifications and
    /// maximum reorg depth.
    pub fn new(canon_state_stream: St, max_reorg_depth: u64) -> Self {
        Self { canon_state_stream, max_reorg_depth, last_pruned_block_number: None }
    }
}

impl<St: Stream<Item = CanonStateNotification> + Send + Unpin + 'static> Future for Pruner<St> {
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
        // block number. If that's the case, no pruning is needed as outdated data are also
        // reverted.
        if this.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            tip.number.saturating_sub(last_pruned_block_number) > MIN_PRUNE_BLOCK_INTERVAL
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
