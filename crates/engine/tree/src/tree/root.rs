use alloy_rlp::{BufMut, Encodable};
use futures::{stream::FuturesOrdered, FutureExt, StreamExt};
use rayon::prelude::*;
use reth_errors::ProviderResult;
use reth_execution_errors::TrieWitnessError;
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
};
use reth_tasks::pool::BlockingTaskPool;
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory,
    proof::Proof,
    trie_cursor::InMemoryTrieCursorFactory,
    updates::TrieUpdates,
    witness::{next_root_from_proofs, target_nodes},
    HashedPostState, HashedStorage, MultiProof, Nibbles, TrieAccount, TrieInputSorted,
    EMPTY_ROOT_HASH,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::{async_proof::AsyncProof, async_root::AsyncStateRootError};
use revm_primitives::{keccak256, B256};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::debug;

type AsyncStateRootFut =
    Pin<Box<dyn Future<Output = ProviderResult<(B256, MultiProof, TrieUpdates, Duration)>> + Send>>;

type AsyncStateProofFut = Pin<
    Box<
        dyn Future<
                Output = Result<Result<MultiProof, AsyncStateRootError>, oneshot::error::RecvError>,
            > + Send,
    >,
>;

pub(crate) struct StateRootTask<Factory> {
    consistent_view: ConsistentDbView<Factory>,
    blocking_task_pool: BlockingTaskPool,
    state_stream: UnboundedReceiverStream<revm_primitives::EvmState>,
    state_stream_closed: bool,
    input: Arc<TrieInputSorted>,
    state: HashedPostState,
    trie_updates: TrieUpdates,
    pending_proofs: FuturesOrdered<AsyncStateProofFut>,
    task_state: StateRootTaskState,
}

enum StateRootTaskState {
    Idle(MultiProof, B256),
    Pending(MultiProof, AsyncStateRootFut),
}

impl StateRootTaskState {
    fn add_proofs(&mut self, proofs: MultiProof) {
        match self {
            Self::Idle(multiproof, _) => {
                multiproof.extend(proofs);
            }
            Self::Pending(multiproof, _) => {
                multiproof.extend(proofs);
            }
        }
    }
}

impl<Factory> StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + Unpin + 'static,
{
    pub(crate) fn new(
        consistent_view: ConsistentDbView<Factory>,
        input: Arc<TrieInputSorted>,
        state_stream: UnboundedReceiverStream<revm_primitives::EvmState>,
        parent_state_root: B256,
    ) -> Self {
        Self {
            consistent_view,
            blocking_task_pool: BlockingTaskPool::build().unwrap(),
            state_stream,
            state_stream_closed: false,
            input,
            state: HashedPostState::default(),
            trie_updates: TrieUpdates::default(),
            pending_proofs: FuturesOrdered::new(),
            task_state: StateRootTaskState::Idle(MultiProof::default(), parent_state_root),
        }
    }

    fn on_state_update(&mut self, update: revm_primitives::EvmState) {
        let mut hashed_state_update = HashedPostState::default();
        for (address, account) in update {
            if account.is_touched() {
                let hashed_address = keccak256(address);

                let destroyed = account.is_selfdestructed();
                hashed_state_update.accounts.insert(
                    hashed_address,
                    if destroyed || account.is_empty() { None } else { Some(account.info.into()) },
                );

                if destroyed || !account.storage.is_empty() {
                    let storage = account.storage.into_iter().filter_map(|(slot, value)| {
                        (!destroyed && value.is_changed())
                            .then(|| (keccak256(B256::from(slot)), value.present_value))
                    });
                    hashed_state_update
                        .storages
                        .insert(hashed_address, HashedStorage::from_iter(destroyed, storage));
                }
            }
        }

        // Dispatch proof gathering for this state update
        // TODO: batch these
        let view = self.consistent_view.clone();
        let task_pool = self.blocking_task_pool.clone();
        let input = self.input.clone();

        let targets = hashed_state_update
            .accounts
            .keys()
            .filter(|hashed_address| {
                !self.state.accounts.contains_key(*hashed_address) &&
                    !self.state.storages.contains_key(*hashed_address)
            })
            .map(|hashed_address| (*hashed_address, HashSet::default()))
            // TODO: filter storages
            .chain(hashed_state_update.storages.iter().map(|(hashed_address, storage)| {
                (*hashed_address, storage.storage.keys().copied().collect())
            }))
            .collect::<HashMap<_, _>>();

        let (tx, rx) = oneshot::channel();
        rayon::spawn(|| {
            let result = futures::executor::block_on(async move {
                AsyncProof::new(view, task_pool, input).multiproof(targets).await
            });
            let _ = tx.send(result);
        });
        self.pending_proofs.push_back(Box::pin(async move { rx.await }));

        self.state.extend(hashed_state_update);
    }
}

impl<Factory> Future for StateRootTask<Factory>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone + Send + Sync + Unpin + 'static,
{
    type Output = Result<(B256, TrieUpdates), AsyncStateRootError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            // TODO: fix
            if this.state_stream_closed && this.pending_proofs.is_empty() {
                if let StateRootTaskState::Idle(_multiproof, state_root) = &mut this.task_state {
                    return Poll::Ready(Ok((*state_root, std::mem::take(&mut this.trie_updates))))
                }
            }

            if let Poll::Ready(next) = this.state_stream.poll_next_unpin(cx) {
                if let Some(update) = next {
                    debug!(target: "engine::root", len = update.len(), "Received new state update");
                    this.on_state_update(update);
                    continue
                } else {
                    this.state_stream_closed = true;
                }
            }

            if let Poll::Ready(Some(result)) = this.pending_proofs.poll_next_unpin(cx) {
                let multiproof = result.unwrap().unwrap();
                this.task_state.add_proofs(multiproof);
                continue
            }

            if let StateRootTaskState::Pending(multiproof, pending) = &mut this.task_state {
                if let Poll::Ready((state_root, mut multiproof2, trie_updates, elapsed)) =
                    pending.poll_unpin(cx)?
                {
                    debug!(target: "engine::root", %state_root, ?elapsed, "Computed intermediate root");
                    this.trie_updates.extend(trie_updates);
                    multiproof2.extend(std::mem::take(multiproof));
                    this.task_state = StateRootTaskState::Idle(multiproof2, state_root);
                    continue
                }
            }

            if let StateRootTaskState::Idle(multiproof, _) = &mut this.task_state {
                debug!(target: "engine::root", accounts_len = this.state.accounts.len(), "Spawning state root task");
                let view = this.consistent_view.clone();
                let input = this.input.clone();
                let multiproof = std::mem::take(multiproof);
                let state = this.state.clone();
                let (tx, rx) = oneshot::channel();
                rayon::spawn(|| {
                    let result = calculate_state_root_from_proofs(view, input, multiproof, state);
                    let _ = tx.send(result);
                });
                this.task_state = StateRootTaskState::Pending(
                    Default::default(),
                    Box::pin(async move { rx.await.unwrap() }),
                );
                continue
            }

            return Poll::Pending
        }
    }
}

fn calculate_state_root_from_proofs<Factory>(
    view: ConsistentDbView<Factory>,
    input: Arc<TrieInputSorted>,
    multiproof: MultiProof,
    state: HashedPostState,
) -> ProviderResult<(B256, MultiProof, TrieUpdates, Duration)>
where
    Factory: DatabaseProviderFactory<Provider: BlockReader> + Clone,
{
    let started_at = Instant::now();
    let provider_ro = view.provider_ro()?;

    let proof_targets: HashMap<B256, HashSet<B256>> = HashMap::from_iter(
        state.accounts.keys().map(|hashed_address| (*hashed_address, HashSet::default())).chain(
            state.storages.iter().map(|(hashed_address, storage)| {
                (*hashed_address, storage.storage.keys().copied().collect())
            }),
        ),
    );

    let account_trie_nodes = proof_targets
        .into_par_iter()
        .map(|(hashed_address, hashed_slots)| {
            // Gather and record storage trie nodes for this account.
            let mut storage_trie_nodes = Vec::with_capacity(hashed_slots.len());
            let storage = state.storages.get(&hashed_address);
            for hashed_slot in hashed_slots {
                let slot_key = Nibbles::unpack(hashed_slot);
                let slot_value = storage
                    .and_then(|s| s.storage.get(&hashed_slot))
                    .filter(|v| !v.is_zero())
                    .map(|v| alloy_rlp::encode_fixed_size(v).to_vec());
                let proof = multiproof
                    .storages
                    .get(&hashed_address)
                    .map(|proof| {
                        proof
                            .subtree
                            .iter()
                            .filter(|e| slot_key.starts_with(e.0))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                storage_trie_nodes.extend(target_nodes(slot_key.clone(), slot_value, proof, None)?);
            }

            let storage_root = next_root_from_proofs(storage_trie_nodes, true, |key: Nibbles| {
                // Right pad the target with 0s.
                let mut padded_key = key.pack();
                padded_key.resize(32, 0);
                let mut proof = Proof::new(
                    InMemoryTrieCursorFactory::new(
                        DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                        &input.nodes,
                    ),
                    HashedPostStateCursorFactory::new(
                        DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                        &input.state,
                    ),
                )
                .with_target((hashed_address, HashSet::from([B256::from_slice(&padded_key)])))
                .multiproof()
                .unwrap();

                // The subtree only contains the proof for a single target.
                let node = proof
                    .storages
                    .get_mut(&hashed_address)
                    .and_then(|storage_multiproof| storage_multiproof.subtree.remove(&key))
                    .ok_or(TrieWitnessError::MissingTargetNode(key))?;
                Ok(node)
            })?;

            // Gather and record account trie nodes.
            let account = state
                .accounts
                .get(&hashed_address)
                .ok_or(TrieWitnessError::MissingAccount(hashed_address))?;
            let value = if account.is_some() || storage_root != EMPTY_ROOT_HASH {
                let mut encoded = Vec::with_capacity(128);
                TrieAccount::from((account.unwrap_or_default(), storage_root))
                    .encode(&mut encoded as &mut dyn BufMut);
                Some(encoded)
            } else {
                None
            };
            let key = Nibbles::unpack(hashed_address);
            let proof = multiproof.account_subtree.iter().filter(|e| key.starts_with(e.0));
            Ok(target_nodes(key.clone(), value, proof, None)?)
        })
        .collect::<ProviderResult<Vec<_>>>()?;

    let state_root =
        next_root_from_proofs(account_trie_nodes.into_iter().flatten(), true, |key: Nibbles| {
            // Right pad the target with 0s.
            let mut padded_key = key.pack();
            padded_key.resize(32, 0);
            let mut proof = Proof::new(
                InMemoryTrieCursorFactory::new(
                    DatabaseTrieCursorFactory::new(provider_ro.tx_ref()),
                    &input.nodes,
                ),
                HashedPostStateCursorFactory::new(
                    DatabaseHashedCursorFactory::new(provider_ro.tx_ref()),
                    &input.state,
                ),
            )
            .with_target((B256::from_slice(&padded_key), Default::default()))
            .multiproof()
            .unwrap();

            // The subtree only contains the proof for a single target.
            let node = proof
                .account_subtree
                .remove(&key)
                .ok_or(TrieWitnessError::MissingTargetNode(key))?;
            Ok(node)
        })?;

    Ok((state_root, multiproof, Default::default(), started_at.elapsed()))
}
