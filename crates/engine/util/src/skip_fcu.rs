//! Stream wrapper that skips specified number of FCUs.

use futures::{Stream, StreamExt};
use reth_beacon_consensus::{BeaconEngineMessage, OnForkChoiceUpdated};
use reth_engine_primitives::EngineTypes;
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

/// Engine API stream wrapper that skips the specified number of forkchoice updated messages.
#[derive(Debug)]
#[pin_project::pin_project]
pub struct EngineSkipFcu<S> {
    #[pin]
    stream: S,
    /// The number of FCUs to skip.
    threshold: usize,
    /// Current count of skipped FCUs.
    skipped: usize,
}

impl<S> EngineSkipFcu<S> {
    /// Creates new [`EngineSkipFcu`] stream wrapper.
    pub const fn new(stream: S, threshold: usize) -> Self {
        Self {
            stream,
            threshold,
            // Start with `threshold` so that the first FCU goes through.
            skipped: threshold,
        }
    }
}

impl<S, Engine> Stream for EngineSkipFcu<S>
where
    S: Stream<Item = BeaconEngineMessage<Engine>>,
    Engine: EngineTypes,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            let next = ready!(this.stream.poll_next_unpin(cx));
            let item = match next {
                Some(BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx }) => {
                    if this.skipped < this.threshold {
                        *this.skipped += 1;
                        tracing::warn!(target: "engine::stream::skip_fcu", ?state, ?payload_attrs, threshold=this.threshold, skipped=this.skipped, "Skipping FCU");
                        let _ = tx.send(Ok(OnForkChoiceUpdated::syncing()));
                        continue
                    }
                    *this.skipped = 0;
                    Some(BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx })
                }
                next => next,
            };
            return Poll::Ready(item)
        }
    }
}
