//! Stream wrapper that skips specified number of new payload messages.

use futures::{Stream, StreamExt};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_engine_primitives::EngineTypes;
use reth_rpc_types::engine::{PayloadStatus, PayloadStatusEnum};
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
    /// Creates new [EngineSkipNewPayload] stream wrapper.
    pub fn new(stream: S, threshold: usize) -> Self {
        Self { stream, threshold, skipped: 0 }
    }
}

impl<Engine, S> Stream for EngineSkipNewPayload<S>
where
    Engine: EngineTypes,
    S: Stream<Item = BeaconEngineMessage<Engine>>,
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            let next = ready!(this.stream.poll_next_unpin(cx));
            let item = match next {
                Some(BeaconEngineMessage::NewPayload { payload, cancun_fields, tx }) => {
                    if this.skipped < this.threshold {
                        *this.skipped += 1;
                        tracing::warn!(
                            target: "engine::intercept",
                            block_number = payload.block_number(),
                            block_hash = %payload.block_hash(),
                            ?cancun_fields,
                            threshold=this.threshold,
                            skipped=this.skipped, "Skipping new payload"
                        );
                        let _ = tx.send(Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing)));
                        continue
                    } else {
                        *this.skipped = 0;
                        Some(BeaconEngineMessage::NewPayload { payload, cancun_fields, tx })
                    }
                }
                next => next,
            };
            return Poll::Ready(item)
        }
    }
}
