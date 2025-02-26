use crate::tree::{
    cached_state::{CachedStateMetrics, CachedStateProvider, ProviderCaches, SavedCache},
    payload_processor::multiproof::StateRootMessage,
    StateProviderBuilder,
};
use alloy_consensus::{transaction::Recovered, BlockHeader};
use alloy_primitives::{keccak256, map::B256Set, B256};
use futures::SinkExt;
use reth_evm::{
    execute::BlockExecutorProvider,
    system_calls::{NoopHook, OnStateHook},
    ConfigureEvm, ConfigureEvmEnvFor, Evm,
};
use reth_primitives_traits::{
    header::SealedHeaderFor, NodePrimitives, RecoveredBlock, SignedTransaction,
};
use reth_provider::{
    providers::ConsistentDbView, BlockReader, DBProvider, DatabaseProviderFactory,
    StateCommitmentProvider, StateProviderFactory, StateReader,
};
use reth_revm::{database::StateProviderDatabase, state::EvmState};
use reth_trie::{
    hashed_cursor::HashedPostStateCursorFactory, proof::ProofBlindedProviderFactory,
    trie_cursor::InMemoryTrieCursorFactory, MultiProofTargets, TrieInput,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use reth_trie_parallel::root::ParallelStateRootError;
use reth_trie_sparse::SparseStateTrie;
use reth_workload_executor::WorkloadExecutor;
use std::{
    collections::VecDeque,
    sync::{
        mpsc,
        mpsc::{channel, Receiver, Sender},
        Arc, RwLock,
    },
    time::Instant,
};
use tracing::{debug, trace};

/// A task that executes transactions individually in parallel.
pub struct PrewarmTask<N: NodePrimitives, P, Evm> {
    /// The executor used to spawn execution tasks.
    executor: WorkloadExecutor,
    /// Transactions pending execution.
    pending: VecDeque<Recovered<N::SignedTx>>,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, Evm>,
    /// How many txs are currently in progress
    in_progress: usize,
    /// How many transactions should be executed in parallel
    max_concurrency: usize,
    /// Sender to emit evm state outcome messages, if any.
    to_multi_proof: Option<mpsc::Sender<StateRootMessage>>,
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent>,
    /// Sender the transactions use to send their result back
    actions_tx: Sender<PrewarmTaskEvent>,
}

impl<N, P, Evm> PrewarmTask<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + StateCommitmentProvider + Clone + 'static,
    Evm: ConfigureEvmEnvFor<N>
        + 'static
        + ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + 'static,
{
    /// Intializes the task with the given transactions pending execution
    pub(super) fn new(
        executor: WorkloadExecutor,
        ctx: PrewarmContext<N, P, Evm>,
        to_multi_proof: Option<Sender<StateRootMessage>>,
        pending: VecDeque<Recovered<N::SignedTx>>,
    ) -> Self {
        let (actions_tx, actions_rx) = channel();
        Self {
            executor,
            pending,
            ctx,
            in_progress: 0,
            // TODO settings
            max_concurrency: 4,
            to_multi_proof,
            actions_rx,
            actions_tx,
        }
    }

    pub(super) fn actions_tx(&self) -> Sender<PrewarmTaskEvent> {
        self.actions_tx.clone()
    }

    /// Spawns the next transactions
    fn spawn_next(&mut self) {
        while self.in_progress < self.max_concurrency {
            if let Some(tx) = self.pending.pop_front() {
                self.spawn_transaction(tx);
            } else {
                break
            }
        }
    }

    /// Spawns the given transaction as a blocking task.
    fn spawn_transaction(&mut self, tx: Recovered<N::SignedTx>) {
        let ctx = self.ctx.clone();
        let actions_tx = self.actions_tx.clone();
        let prepare_proof_targets = self.should_prepare_multi_proof_targets();
        self.executor.spawn_blocking(move || {
            // depending on whether this task needs he proof targets we either just transact or
            // transact and prepare the targets
            let proof_targets = if prepare_proof_targets {
                ctx.prepare_multiproof_targets(tx)
            } else {
                ctx.transact(tx);
                None
            };
            let _ = actions_tx.send(PrewarmTaskEvent::Outcome { proof_targets });
        });
    }

    fn is_done(&self) -> bool {
        self.in_progress == 0 && self.pending.is_empty()
    }

    /// Returns true if the tx prewarming tasks should prepare multiproof targets.
    fn should_prepare_multi_proof_targets(&self) -> bool {
        self.to_multi_proof.is_some()
    }

    /// If configured and the tx returned proof targets, emit the targets the transaction produced
    fn send_multi_proof_targets(&self, targets: Option<MultiProofTargets>) {
        if let Some((proof_targets, to_multi_proof)) = targets.zip(self.to_multi_proof.as_ref()) {
            let _ = to_multi_proof.send(StateRootMessage::PrefetchProofs(proof_targets));
        }
    }

    /// Executes the task.
    ///
    /// This will execute the transactions until all transactions have been processed or the task
    /// was cancelled.
    pub(super) fn run(mut self) {
        // spawn execution tasks.
        self.spawn_next();

        while let Ok(event) = self.actions_rx.recv() {
            match event {
                PrewarmTaskEvent::Terminate => {
                    // received terminate signal
                    break
                }
                PrewarmTaskEvent::Outcome { proof_targets } => {
                    // completed a transaction, frees up one slot
                    self.in_progress -= 1;
                    self.send_multi_proof_targets(proof_targets);
                }
            }

            // schedule followup transactions
            self.spawn_next();

            if self.is_done() {
                /// we're done and terminate this task
                // TODO should we wait for terminate signale and flush the cache here?
                break
            }
        }
    }
}

/// Context required by tx execution tasks.
#[derive(Debug, Clone)]
pub(super) struct PrewarmContext<N: NodePrimitives, P, Evm> {
    pub(super) header: SealedHeaderFor<N>,
    pub(super) evm_config: Evm,
    pub(super) caches: ProviderCaches,
    pub(super) cache_metrics: CachedStateMetrics,
    /// Provider to obtain the state
    pub(super) provider: StateProviderBuilder<N, P>,
}

impl<N, P, Evm> PrewarmContext<N, P, Evm>
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + StateCommitmentProvider + Clone + 'static,
    Evm: ConfigureEvmEnvFor<N>
        + 'static
        + ConfigureEvm<Header = N::BlockHeader, Transaction = N::SignedTx>
        + 'static,
{
    /// Transacts the the transactions and transform the state into [`MultiProofTargets`].
    fn prepare_multiproof_targets(
        mut self,
        tx: Recovered<N::SignedTx>,
    ) -> Option<MultiProofTargets> {
        let state = self.transact(tx)?;

        let mut targets =
            MultiProofTargets::with_capacity_and_hasher(state.len(), Default::default());

        for (addr, account) in state {
            // if the account was not touched, or if the account was selfdestructed, do not
            // fetch proofs for it
            //
            // Since selfdestruct can only happen in the same transaction, we can skip
            // prefetching proofs for selfdestructed accounts
            //
            // See: https://eips.ethereum.org/EIPS/eip-6780
            if !account.is_touched() || account.is_selfdestructed() {
                continue
            }

            let mut storage_set =
                B256Set::with_capacity_and_hasher(account.storage.len(), Default::default());
            for (key, slot) in account.storage {
                // do nothing if unchanged
                if !slot.is_changed() {
                    continue
                }

                storage_set.insert(keccak256(B256::new(key.to_be_bytes())));
            }

            targets.insert(keccak256(addr), storage_set);
        }

        Some(targets)
    }

    /// Transacts the transaction and returns the state outcome.
    ///
    /// Returns `None` if executing the transaction failed to a non Revert error.
    /// Returns the touched+modified state of the transaction.
    ///
    /// Note: Since here are no ordering guarantees this won't the state the tx produces when
    /// executed sequentially.
    fn transact(mut self, tx: Recovered<N::SignedTx>) -> Option<EvmState> {
        let Self { header, evm_config, caches, cache_metrics, provider } = self;
        // Create the state provider inside the thread
        let state_provider = match provider.build() {
            Ok(provider) => provider,
            Err(err) => {
                trace!(
                    target: "engine::tree",
                    %err,
                    "Failed to build state provider in prewarm thread"
                );
                return None
            }
        };

        // Use the caches to create a new provider with caching
        let state_provider =
            CachedStateProvider::new_with_caches(state_provider, caches, cache_metrics);

        let state_provider = StateProviderDatabase::new(&state_provider);

        let mut evm_env = evm_config.evm_env(&header);

        // we must disable the nonce check so that we can execute the transaction even if the nonce
        // doesn't match what's on chain.
        evm_env.cfg_env.disable_nonce_check = true;

        // create a new executor and disable nonce checks in the env
        let mut evm = evm_config.evm_with_env(state_provider, evm_env);

        // create the tx env and reset nonce
        let tx_env = evm_config.tx_env(&tx);
        let res = match evm.transact(tx_env) {
            Ok(res) => res,
            Err(err) => {
                trace!(
                    target: "engine::tree",
                    %err,
                    tx_hash=%tx.tx_hash(),
                    sender=%tx.signer(),
                    "Error when executing prewarm transaction",
                );
                return None
            }
        };

        Some(res.state)
    }
}

/// The events the pre-warm task can handle.
pub(super) enum PrewarmTaskEvent {
    /// Forcefully terminate the task on demand, e.g. when no longer required.
    Terminate,
    /// The outcome of a pre-warm task
    Outcome {
        /// The prepared proof targets based on the evm state outcome
        proof_targets: Option<MultiProofTargets>,
    },
}
