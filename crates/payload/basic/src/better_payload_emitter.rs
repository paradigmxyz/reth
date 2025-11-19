use crate::{BuildArguments, BuildOutcome, HeaderForPayload, PayloadBuilder, PayloadConfig};
use reth_payload_builder::PayloadBuilderError;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Emits events when a payload is built (both `Better` and `Freeze` outcomes).
/// Delegates the actual payload building to an inner [`PayloadBuilder`].
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
    /// Owns the sender half of a broadcast channel that emits payloads when they are built
    /// (for both `Better` and `Freeze` outcomes).
    pub const fn new(
        better_payloads_tx: broadcast::Sender<Arc<PB::BuiltPayload>>,
        inner: PB,
    ) -> Self {
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
        args: BuildArguments<Self::Attributes, Self::BuiltPayload>,
    ) -> Result<BuildOutcome<Self::BuiltPayload>, PayloadBuilderError> {
        match self.inner.try_build(args) {
            Ok(res) => {
                // Emit payload for both Better and Freeze outcomes, as both represent valid
                // payloads that should be available to subscribers (e.g., for
                // insertion into engine service).
                if let Some(payload) = res.payload().cloned() {
                    let _ = self.better_payloads_tx.send(Arc::new(payload));
                }
                Ok(res)
            }
            res => res,
        }
    }

    fn build_empty_payload(
        &self,
        config: PayloadConfig<Self::Attributes, HeaderForPayload<Self::BuiltPayload>>,
    ) -> Result<Self::BuiltPayload, PayloadBuilderError> {
        self.inner.build_empty_payload(config)
    }
}
