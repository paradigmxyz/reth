use super::streaming_database::StateAccess;
use reth_primitives::keccak256;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, StateProviderBox,
};
use reth_tasks::pool::BlockingTaskPool;
use reth_trie::{MultiProof, TrieInput, TrieInputSorted};
use reth_trie_parallel::async_proof::AsyncProof;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use tracing::info;

pub(crate) async fn gather_proofs(
    provider: StateProviderBox,
    mut state_rx: mpsc::UnboundedReceiver<StateAccess>,
    tx: oneshot::Sender<(StateProviderBox, MultiProof, Duration)>,
) {
    let started_at = Instant::now();
    let mut multiproof = MultiProof::default();
    while let Some(next) = state_rx.recv().await {
        let mut targets = HashMap::from([match next {
            StateAccess::Account(address) => (keccak256(address), HashSet::default()),
            StateAccess::StorageSlot(address, slot) => {
                (keccak256(address), HashSet::from([keccak256(slot)]))
            }
        }]);

        while let Ok(next) = state_rx.try_recv() {
            match next {
                StateAccess::Account(address) => {
                    targets.entry(keccak256(address)).or_default();
                }
                StateAccess::StorageSlot(address, slot) => {
                    targets.entry(keccak256(address)).or_default().insert(keccak256(slot));
                }
            }
        }

        info!(target: "engine", accounts_len = targets.len(), "Computing multiproof");
        multiproof.extend(provider.multiproof(Default::default(), targets).unwrap());
    }

    let _ = tx.send((provider, multiproof, started_at.elapsed()));
}

pub(crate) async fn gather_proofs_parallel<Factory>(
    view: ConsistentDbView<Factory>,
    provider: StateProviderBox,
    input: Arc<TrieInputSorted>,
    mut state_rx: mpsc::UnboundedReceiver<StateAccess>,
    tx: oneshot::Sender<(StateProviderBox, MultiProof, Duration)>,
) where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + 'static,
{
    let started_at = Instant::now();
    let blocking_pool = BlockingTaskPool::build().unwrap();
    let async_proof_calculator = AsyncProof::new(view, blocking_pool, input);
    let mut multiproof = MultiProof::default();
    while let Some(next) = state_rx.recv().await {
        let mut targets = HashMap::from([match next {
            StateAccess::Account(address) => (keccak256(address), HashSet::default()),
            StateAccess::StorageSlot(address, slot) => {
                (keccak256(address), HashSet::from([keccak256(slot)]))
            }
        }]);

        while let Ok(next) = state_rx.try_recv() {
            match next {
                StateAccess::Account(address) => {
                    targets.entry(keccak256(address)).or_default();
                }
                StateAccess::StorageSlot(address, slot) => {
                    targets.entry(keccak256(address)).or_default().insert(keccak256(slot));
                }
            }
        }

        info!(target: "engine", accounts_len = targets.len(), "Computing multiproof");
        let result = async_proof_calculator.multiproof(targets).await.unwrap();
        multiproof.extend(result);
    }

    let _ = tx.send((provider, multiproof, started_at.elapsed()));
}
