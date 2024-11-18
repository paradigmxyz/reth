//! State root task related functionality.

use reth_provider::providers::ConsistentDbView;
use reth_trie::{updates::TrieUpdates, TrieInput};
use reth_trie_parallel::root::ParallelStateRootError;
use revm_primitives::{EvmState, B256};
use std::sync::{
    mpsc::{self, Receiver, RecvError},
    Arc,
};
use tracing::debug;

/// Result of the state root calculation
pub(crate) type StateRootResult = Result<(B256, TrieUpdates), ParallelStateRootError>;

/// Handle to a spawned state root task.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct StateRootHandle {
    /// Channel for receiving the final result.
    rx: mpsc::Receiver<StateRootResult>,
}

#[allow(dead_code)]
impl StateRootHandle {
    /// Creates a new handle from a receiver.
    pub(crate) const fn new(rx: mpsc::Receiver<StateRootResult>) -> Self {
        Self { rx }
    }

    /// Waits for the state root calculation to complete.
    pub(crate) fn wait_for_result(self) -> StateRootResult {
        self.rx.recv().expect("state root task was dropped without sending result")
    }
}

/// Common configuration for state root tasks
#[derive(Debug)]
pub(crate) struct StateRootConfig<Factory> {
    /// View over the state in the database.
    pub consistent_view: ConsistentDbView<Factory>,
    /// Latest trie input.
    pub input: Arc<TrieInput>,
}

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

/// Standalone task that receives a transaction state stream and updates relevant
/// data structures to calculate state root.
///
/// It is responsible of  initializing a blinded sparse trie and subscribe to
/// transaction state stream. As it receives transaction execution results, it
/// fetches the proofs for relevant accounts from the database and reveal them
/// to the tree.
/// Then it updates relevant leaves according to the result of the transaction.
#[allow(dead_code)]
pub(crate) struct StateRootTask<Factory> {
    /// Incoming state updates.
    state_stream: StdReceiverStream,
    /// Task configuration.
    config: StateRootConfig<Factory>,
}

#[allow(dead_code)]
impl<Factory> StateRootTask<Factory>
where
    Factory: Send + 'static,
{
    /// Creates a new `StateRootTask`.
    pub(crate) const fn new(
        config: StateRootConfig<Factory>,
        state_stream: StdReceiverStream,
    ) -> Self {
        Self { config, state_stream }
    }

    /// Spawns the state root task and returns a handle to await its result.
    pub(crate) fn spawn(self) -> StateRootHandle {
        let (tx, rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("State Root Task".to_string())
            .spawn(move || {
                debug!(target: "engine::tree", "Starting state root task");
                let result = self.run();
                let _ = tx.send(result);
            })
            .expect("failed to spawn state root thread");

        StateRootHandle::new(rx)
    }

    /// Handles state updates.
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
impl<Factory> StateRootTask<Factory>
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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::{providers::ConsistentDbView, test_utils::MockEthProvider};
    use reth_trie::TrieInput;
    use revm_primitives::{
        Account, AccountInfo, AccountStatus, Address, EvmState, EvmStorage, EvmStorageSlot,
        HashMap, B256, U256,
    };
    use std::sync::Arc;

    fn create_mock_config() -> StateRootConfig<MockEthProvider> {
        let factory = MockEthProvider::default();
        let view = ConsistentDbView::new(factory, None);
        let input = Arc::new(TrieInput::default());
        StateRootConfig { consistent_view: view, input }
    }

    fn create_mock_state() -> revm_primitives::EvmState {
        let mut state_changes: EvmState = HashMap::default();
        let storage = EvmStorage::from_iter([(U256::from(1), EvmStorageSlot::new(U256::from(2)))]);
        let account = Account {
            info: AccountInfo {
                balance: U256::from(100),
                nonce: 10,
                code_hash: B256::random(),
                code: Default::default(),
            },
            storage,
            status: AccountStatus::Loaded,
        };

        let address = Address::random();
        state_changes.insert(address, account);

        state_changes
    }

    #[test]
    fn test_state_root_task() {
        let config = create_mock_config();
        let (tx, rx) = std::sync::mpsc::channel();
        let stream = StdReceiverStream::new(rx);

        let task = StateRootTask::new(config, stream);
        let handle = task.spawn();

        for _ in 0..10 {
            tx.send(create_mock_state()).expect("failed to send state");
        }
        drop(tx);

        let result = handle.wait_for_result();
        assert!(result.is_ok(), "sync block execution failed");
    }
}
