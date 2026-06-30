//! Background BAL rebuilding from committed state updates.

use alloy_eip7928::{compute_block_access_list_hash, BlockAccessIndex};
use alloy_primitives::B256;
use crossbeam_channel::Sender;
use reth_evm::OnStateHook;
use reth_tasks::{LazyHandle, Runtime};
use revm::{database::bal::BalState, state::EvmState};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

/// Handle to a block access list hash computed in the background.
pub(crate) type BalHashHandle = LazyHandle<B256>;

/// Rebuilds the canonical BAL on a background worker from state-hook updates.
#[derive(Debug)]
pub(crate) struct BalRebuilder {
    index: Arc<AtomicU64>,
    updates_tx: Sender<IndexedStateUpdate>,
    result: BalHashHandle,
}

impl BalRebuilder {
    /// Spawns a background BAL rebuilder.
    pub(crate) fn spawn(runtime: &Runtime) -> Self {
        let (updates_tx, updates_rx) = crossbeam_channel::unbounded();
        let result = runtime.spawn_blocking_named("bal-rebuild", move || rebuild_bal(updates_rx));

        Self { index: Arc::new(AtomicU64::new(0)), updates_tx, result }
    }

    /// Returns a state hook that streams committed state into the rebuilder.
    pub(crate) fn state_hook(&self) -> BalRebuilderStateHook {
        BalRebuilderStateHook {
            index: Arc::clone(&self.index),
            updates_tx: self.updates_tx.clone(),
        }
    }

    /// Advances the BAL index used by subsequent state-hook updates.
    pub(crate) fn bump_index(&self) {
        self.index.fetch_add(1, Ordering::Relaxed);
    }

    /// Finishes rebuilding and returns a handle for the rebuilt BAL hash.
    pub(crate) fn finish(self) -> BalHashHandle {
        drop(self.updates_tx);
        self.result
    }
}

/// State hook used by [`BalRebuilder`].
pub(crate) struct BalRebuilderStateHook {
    index: Arc<AtomicU64>,
    updates_tx: Sender<IndexedStateUpdate>,
}

impl OnStateHook for BalRebuilderStateHook {
    fn on_state(&mut self, state: EvmState) {
        let index = BlockAccessIndex::new(self.index.load(Ordering::Relaxed));
        let _ = self.updates_tx.send(IndexedStateUpdate { index, state });
    }
}

/// Fanout hook for paths that already stream state updates elsewhere.
pub(crate) struct FanoutStateHook<A, B> {
    /// First hook to receive a cloned update.
    pub(crate) first: A,
    /// Second hook to receive the original update.
    pub(crate) second: B,
}

impl<A, B> OnStateHook for FanoutStateHook<A, B>
where
    A: OnStateHook,
    B: OnStateHook,
{
    fn on_state(&mut self, state: EvmState) {
        self.first.on_state(state.clone());
        self.second.on_state(state);
    }
}

struct IndexedStateUpdate {
    index: BlockAccessIndex,
    state: EvmState,
}

fn rebuild_bal(updates_rx: crossbeam_channel::Receiver<IndexedStateUpdate>) -> B256 {
    let mut bal_state = BalState::new().with_bal_builder();
    for IndexedStateUpdate { index, state } in updates_rx {
        bal_state.bal_index = index;
        bal_state.commit(&state);
    }

    let bal = bal_state.take_built_alloy_bal().expect("BAL builder enabled");
    compute_block_access_list_hash(&bal)
}
