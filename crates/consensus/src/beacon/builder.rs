use super::BeaconConsensus;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::ChainSpec;
use std::sync::Arc;
use tokio::sync::watch;

/// TODO:
#[derive(Debug, Default)]
pub struct BeaconConsensusBuilder;

impl BeaconConsensusBuilder {
    /// Create new instance of [BeaconConsensus] and forkchoice notifier. Internally, creates a
    /// [watch::channel] for updating the forkchoice state.
    pub fn build(
        self,
        chain_spec: Arc<ChainSpec>,
    ) -> (Arc<BeaconConsensus>, watch::Sender<ForkchoiceState>) {
        let (forkchoice_state_tx, forkchoice_state_rx) = watch::channel(ForkchoiceState::default());
        let inner = Arc::new(BeaconConsensus::new(chain_spec, forkchoice_state_rx));
        (inner, forkchoice_state_tx)
    }
}
