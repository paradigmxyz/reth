//! Entrypoint for payload processing.

use reth_workload_executor::WorkloadExecutor;

/// Entrypoint for starting the background processing
pub struct PayloadProcessor {
    executor: WorkloadExecutor,
}

impl PayloadProcessor {

    /// Spawns all background tasks and returns a handle connected to the tasks.
    ///
    /// - Transaction prewarming task
    /// - State root task
    /// - Sparse trie task
    fn spawn(&self) {

        // TODO batch updates: `on_state_update`

    }
}

pub struct PayloadTaskHandle {

    // TODO should internals be an enum to represent no parallel workload

    // needs receiver to await the stateroot from the background task

    // need channel to emit `StateUpdates` to the state root task
}