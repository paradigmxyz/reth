//! State root computation related code.

mod async_task;
mod sync_task;

#[cfg(test)]
mod tests;

use reth_provider::providers::ConsistentDbView;
use reth_trie::{updates::TrieUpdates, TrieInput};
use reth_trie_parallel::parallel_root::ParallelStateRootError;
use revm_primitives::{EvmState, B256};
use std::sync::{mpsc, Arc};

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

/// Trait defining common behavior for state root tasks
#[allow(dead_code)]
pub(crate) trait StateRootTask: Sized {
    /// The type of the state stream used by this task
    type StateStream;
    /// The factory type used for database access
    type Factory: Send + 'static;

    /// Creates a new state root task instance
    fn new(config: StateRootConfig<Self::Factory>, state_stream: Self::StateStream) -> Self;

    /// Spawns the task and returns a handle to await its result
    fn spawn(self) -> StateRootHandle;

    /// Handles individual state updates
    fn on_state_update(
        _view: &ConsistentDbView<impl Send + 'static>,
        _input: &Arc<TrieInput>,
        _state: EvmState,
    );
}
