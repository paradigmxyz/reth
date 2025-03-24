//! Stream wrapper that skips specified number of new payload messages.

use alloy_rpc_types_engine::{PayloadStatus, PayloadStatusEnum};
use futures::{Stream, StreamExt};
use reth_engine_primitives::{BeaconEngineMessage, ExecutionPayload};
use reth_payload_primitives::PayloadTypes;
use std::{
    pin::Pin,
    task::{ready, Context, Poll},
};

/// Engine API stream wrapper that skips the specified number of new payload messages.
#[derive(Debug)]
#[pin_project::pin_project]
pub struct EngineSkipNewPayload<S> {
    #[pin]
    stream: S,
    /// The number of messages to skip.
    threshold: usize,
    /// Current count of skipped messages.
    skipped: usize,
}

impl<S> EngineSkipNewPayload<S> {
    /// Creates new [`EngineSkipNewPayload`] stream wrapper.
    pub const fn new(stream: S, threshold: usize) -> Self {
        Self { stream, threshold, skipped: 0 }
    }
}

impl<S, T> Stream for EngineSkipNewPayload<S>
where
    S: Stream<Item = BeaconEngineMessage<T>>,
    T: PayloadTypes,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            let next = ready!(this.stream.poll_next_unpin(cx));
            let item = match next {
                Some(BeaconEngineMessage::NewPayload { payload, tx }) => {
                    if this.skipped < this.threshold {
                        *this.skipped += 1;
                        tracing::warn!(
                            target: "engine::stream::skip_new_payload",
                            block_number = payload.block_number(),
                            block_hash = %payload.block_hash(),
                            ?payload,
                            threshold=this.threshold,
                            skipped=this.skipped, "Skipping new payload"
                        );
                        let _ = tx.send(Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing)));
                        continue
                    }
                    *this.skipped = 0;
                    Some(BeaconEngineMessage::NewPayload { payload, tx })
                }
                next => next,
            };
            return Poll::Ready(item)
        }
    }
}
