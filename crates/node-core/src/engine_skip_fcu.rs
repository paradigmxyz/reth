//! Stores engine API messages to disk for later inspection and replay.

use reth_beacon_consensus::{BeaconEngineMessage, OnForkChoiceUpdated};
use reth_engine_primitives::EngineTypes;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// Intercept Engine API message and skip FCUs.
#[derive(Debug)]
pub struct EngineApiSkipFcu {
    /// The number of FCUs to skip.
    threshold: usize,
    /// Current count of skipped FCUs.
    skipped: usize,
}

impl EngineApiSkipFcu {
    /// Creates new [EngineApiSkipFcu] interceptor.
    pub fn new(threshold: usize) -> Self {
        Self { threshold, skipped: 0 }
    }

    /// Intercepts an incoming engine API message, skips FCU or forwards it
    /// to the engine depending on current number of skipped FCUs.
    pub async fn intercept<Engine>(
        mut self,
        mut rx: UnboundedReceiver<BeaconEngineMessage<Engine>>,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    ) where
        Engine: EngineTypes,
        BeaconEngineMessage<Engine>: std::fmt::Debug,
    {
        while let Some(msg) = rx.recv().await {
            if let BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } = msg {
                if self.skipped < self.threshold {
                    self.skipped += 1;
                    tracing::warn!(target: "engine::intercept", ?state, ?payload_attrs, threshold=self.threshold, skipped=self.skipped, "Skipping FCU");
                    let _ = tx.send(Ok(OnForkChoiceUpdated::syncing()));
                } else {
                    self.skipped = 0;
                    let _ = to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
                        state,
                        payload_attrs,
                        tx,
                    });
                }
            } else {
                let _ = to_engine.send(msg);
            }
        }
    }
}
