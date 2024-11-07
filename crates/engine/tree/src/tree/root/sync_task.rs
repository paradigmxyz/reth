//! Sync state root task implementation.

use super::common::{
    create_result_channel, send_result, StateRootConfig, StateRootHandle, StateRootResult,
    StateRootTask,
};
use reth_trie::updates::TrieUpdates;
use revm_primitives::{EvmState, B256};
use std::sync::mpsc::{Receiver, RecvError};
use tracing::debug;

/// Wrapper for std channel receiver to maintain compatibility with `UnboundedReceiverStream`
#[allow(dead_code)]
pub(crate) struct StdReceiverStream {
    rx: Receiver<EvmState>,
}

#[allow(dead_code)]
impl StdReceiverStream {
    pub(crate) const fn new(rx: Receiver<EvmState>) -> Self {
        Self { rx }
    }

    pub(crate) fn recv(&self) -> Result<EvmState, RecvError> {
        self.rx.recv()
    }
}

/// Synchronous implementation of the state root task
#[allow(dead_code)]
pub(crate) struct StateRootSyncTask<Factory> {
    state_stream: StdReceiverStream,
    config: StateRootConfig<Factory>,
}

impl<Factory> StateRootTask for StateRootSyncTask<Factory>
where
    Factory: Send + 'static,
{
    type StateStream = StdReceiverStream;
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
        while let Ok(state) = self.state_stream.recv() {
            Self::on_state_update(&self.config.consistent_view, &self.config.input, state);
        }

        // TODO:
        //    * keep track of proof calculation
        //    * keep track of intermediate root computation
        //    * return final state root result
        Ok((B256::default(), TrieUpdates::default()))
    }
}
