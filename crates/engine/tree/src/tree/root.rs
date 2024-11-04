//! State root task related functionality.

use reth_provider::providers::ConsistentDbView;
use reth_trie::{updates::TrieUpdates, TrieInput};
use reth_trie_parallel::parallel_root::ParallelStateRootError;
use revm_primitives::{EvmState, B256};
use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Standalone task that receives a transaction state stream and updates relevant
/// data structures to calculate state root.
///
/// It is responsile of  initializing a blinded sparse trie and subscribe to
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
impl<Factory> StateRootTask<Factory> {
    /// Creates a new `StateRootTask`.
    pub(crate) const fn new(
        consistent_view: ConsistentDbView<Factory>,
        input: Arc<TrieInput>,
        state_stream: UnboundedReceiverStream<EvmState>,
    ) -> Self {
        Self { consistent_view, state_stream, input }
    }

    /// Handles state updates.
    pub(crate) fn on_state_update(&self, _update: EvmState) {
        // TODO: calculate hashed state update and dispatch proof gathering for it.
    }
}

impl<Factory> Future for StateRootTask<Factory> {
    type Output = Result<(B256, TrieUpdates), ParallelStateRootError>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        // TODO:
        //    * poll incoming state updates stream
        //    * keep track of proof calculation
        //    * keep track of intermediate root computation
        Poll::Pending
    }
}
