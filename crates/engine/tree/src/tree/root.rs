//! State root task related functionality.

use futures::StreamExt;
use reth_provider::providers::ConsistentDbView;
use reth_trie::{updates::TrieUpdates, TrieInput};
use reth_trie_parallel::parallel_root::ParallelStateRootError;
use revm_primitives::{EvmState, B256};
use std::sync::{mpsc, Arc};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::debug;

/// Result of the state root calculation
pub(crate) type StateRootResult = Result<(B256, TrieUpdates), ParallelStateRootError>;

/// Handle to a spawned state root task.
#[derive(Debug)]
pub(crate) struct StateRootHandle {
    /// Channel for receiving the final result.
    rx: mpsc::Receiver<StateRootResult>,
}

impl StateRootHandle {
    /// Waits for the state root calculation to complete.
    #[allow(dead_code)]
    pub(crate) fn wait_result(self) -> StateRootResult {
        self.rx.recv().expect("state root task was dropped without sending result")
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
    /// View over the state in the database.
    consistent_view: ConsistentDbView<Factory>,
    /// Incoming state updates.
    state_stream: UnboundedReceiverStream<EvmState>,
    /// Latest trie input.
    input: Arc<TrieInput>,
}

#[allow(dead_code)]
impl<Factory> StateRootTask<Factory>
where
    Factory: Send + 'static,
{
    /// Creates a new `StateRootTask`.
    pub(crate) const fn new(
        consistent_view: ConsistentDbView<Factory>,
        input: Arc<TrieInput>,
        state_stream: UnboundedReceiverStream<EvmState>,
    ) -> Self {
        Self { consistent_view, state_stream, input }
    }

    /// Spawns the state root task and returns a handle to await its result.
    pub(crate) fn spawn(mut self) -> StateRootHandle {
        let (tx, rx) = mpsc::channel();

        // Spawn the task that will process state updates and calculate the root
        tokio::spawn(async move {
            debug!(target: "engine::tree", "Starting state root task");
            let result = self.run().await;
            let _ = tx.send(result);
        });

        StateRootHandle { rx }
    }

    /// Handles state updates.
    fn on_state_update(&self, _update: EvmState) {
        // TODO: calculate hashed state update and dispatch proof gathering for it.
    }

    async fn run(&mut self) -> StateRootResult {
        while let Some(state) = self.state_stream.next().await {
            self.on_state_update(state);
        }

        // TODO:
        //    * keep track of proof calculation
        //    * keep track of intermediate root computation
        //    * return final state root result
        Ok((B256::default(), TrieUpdates::default()))
    }
}
