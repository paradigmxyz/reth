use super::streaming_database::StateAccess;
use futures::{stream::FuturesUnordered, StreamExt};
use reth_primitives::{keccak256, B256};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DatabaseProviderFactory, StateProviderBox,
};
use reth_tasks::{pool::BlockingTaskPool, TaskSpawner};
use reth_trie::{MultiProof, TrieInput, TrieInputSorted};
use reth_trie_parallel::async_proof::AsyncProof;
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
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

pub(crate) struct GatherProofsParallel<Factory> {
    view: ConsistentDbView<Factory>,
    input: Arc<TrieInputSorted>,
    task_spawner: Box<dyn TaskSpawner>,
    blocking_task_pool: BlockingTaskPool,
    state_stream: mpsc::UnboundedReceiver<StateAccess>,
    closed: bool,
    pending: FuturesUnordered<
        Pin<Box<dyn Future<Output = Result<MultiProof, oneshot::error::RecvError>> + Send>>,
    >,
    multiproof: MultiProof,
}

impl<Factory> GatherProofsParallel<Factory> {
    pub(crate) fn new(
        view: ConsistentDbView<Factory>,
        input: Arc<TrieInputSorted>,
        task_spawner: Box<dyn TaskSpawner>,
        state_stream: mpsc::UnboundedReceiver<StateAccess>,
    ) -> Self {
        Self {
            view,
            input,
            task_spawner,
            state_stream,
            blocking_task_pool: BlockingTaskPool::build().unwrap(),
            closed: false,
            pending: FuturesUnordered::new(),
            multiproof: MultiProof::default(),
        }
    }
}

impl<Factory> Future for GatherProofsParallel<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + Unpin + 'static,
{
    type Output = MultiProof;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        loop {
            if this.closed && this.pending.is_empty() {
                return Poll::Ready(std::mem::take(&mut this.multiproof))
            }

            let mut targets = HashMap::<B256, HashSet<B256>>::default();
            'state: while let Poll::Ready(next) = this.state_stream.poll_recv(cx) {
                match next {
                    Some(key) => match key {
                        StateAccess::Account(address) => {
                            targets.entry(keccak256(address)).or_default();
                        }
                        StateAccess::StorageSlot(address, slot) => {
                            targets.entry(keccak256(address)).or_default().insert(keccak256(slot));
                        }
                    },
                    None => {
                        info!(target: "engine", pending = this.pending.len(), targets = targets.len(), "Channel closed.");
                        this.closed = true;
                        break 'state
                    }
                }
            }
            if !targets.is_empty() {
                info!(target: "engine", account_len = targets.len(), "Spawning proof generation");
                let (tx, rx) = oneshot::channel();
                let view = this.view.clone();
                let blocking_pool = this.blocking_task_pool.clone();
                let input = this.input.clone();
                this.task_spawner.spawn(Box::pin(async move {
                    let result = AsyncProof::new(view, blocking_pool, input)
                        .multiproof(targets)
                        .await
                        .unwrap();
                    let _ = tx.send(result);
                }));
                this.pending.push(Box::pin(rx));
            }

            if let Poll::Ready(Some(result)) = this.pending.poll_next_unpin(cx) {
                info!(target: "engine", "Received result");
                this.multiproof.extend(result.expect("no error"));
                continue
            }

            return Poll::Pending
        }
    }
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
