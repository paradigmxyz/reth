//! Sync state root task implementation.

use super::common::{
    create_result_channel, send_result, StateRootConfig, StateRootHandle, StateRootResult,
    StateRootTask,
};
use reth_trie::updates::TrieUpdates;
use revm_primitives::{EvmState, B256};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::debug;

/// Synchronous implementation of the state root task
#[allow(dead_code)]
pub(crate) struct StateRootSyncTask<Factory> {
    state_stream: UnboundedReceiverStream<EvmState>,
    config: StateRootConfig<Factory>,
}

impl<Factory> StateRootTask for StateRootSyncTask<Factory>
where
    Factory: Send + 'static,
{
    type StateStream = UnboundedReceiverStream<EvmState>;
    type Factory = Factory;

    fn new(config: StateRootConfig<Factory>, state_stream: Self::StateStream) -> Self {
        Self { config, state_stream }
    }

    fn spawn(self) -> StateRootHandle {
        let (tx, rx) = create_result_channel();
        rayon::spawn(move || {
            debug!(target: "engine::tree", "Starting sync state root task");
            let result = self.run();
            send_result(tx, result);
        });

        StateRootHandle::new(rx)
    }

    fn on_state_update(
        _view: &reth_provider::providers::ConsistentDbView<impl Send + 'static>,
        _input: &std::sync::Arc<reth_trie::TrieInput>,
        _state: EvmState,
    ) {
        // Default implementation of state update handling
        // TODO: calculate hashed state update and dispatch proof gathering for it.
    }
}

#[allow(dead_code)]
impl<Factory> StateRootSyncTask<Factory>
where
    Factory: Send + 'static,
{
    fn run(self) -> StateRootResult {
        let mut receiver = self.state_stream.into_inner();

        while let Ok(state) = receiver.try_recv() {
            Self::on_state_update(&self.config.consistent_view, &self.config.input, state);
        }

        // TODO:
        //    * keep track of proof calculation
        //    * keep track of intermediate root computation
        //    * return final state root result
        Ok((B256::default(), TrieUpdates::default()))
    }
}
