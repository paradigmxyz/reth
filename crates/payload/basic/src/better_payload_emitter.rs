use crate::{BuildOutcome, PayloadBuilder};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Emits events when a better payload is built. Delegates the actual payload building
/// to an inner [`PayloadBuilder`].
#[derive(Debug, Clone)]
pub struct BetterPayloadEmitter<PB: PayloadBuilder> {
    better_payloads_tx: broadcast::Sender<Arc<PB::BuiltPayload>>,
    inner: PB,
}

impl<PB> BetterPayloadEmitter<PB>
where
    PB: PayloadBuilder,
{
    /// Create a new [`BetterPayloadEmitter`] with the given inner payload builder.
    /// Owns the sender half of a broadcast channel that emits the better payloads.
    pub fn new(better_payloads_tx: broadcast::Sender<Arc<PB::BuiltPayload>>, inner: PB) -> Self {
        Self { better_payloads_tx, inner }
    }
}

impl<PB> PayloadBuilder for BetterPayloadEmitter<PB>
where
    PB: PayloadBuilder,
    <PB as PayloadBuilder>::BuiltPayload: Clone,
{
    type Attributes = PB::Attributes;
    type BuiltPayload = PB::BuiltPayload;

    fn try_build(
        &self,
        args: crate::BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<crate::BuildOutcome<Self::BuiltPayload>, reth_payload_builder::PayloadBuilderError>
    {
        match self.inner.try_build(args) {
            Ok(BuildOutcome::Better { payload, cached_reads }) => {
                let _ = self.better_payloads_tx.send(Arc::new(payload.clone()));
                Ok(BuildOutcome::Better { payload, cached_reads })
            }
            res => res,
        }
    }

    fn build_empty_payload(
        &self,
        config: crate::PayloadConfig<Self::Attributes, crate::HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, reth_payload_builder::PayloadBuilderError> {
        self.inner.build_empty_payload(config)
    }
}
