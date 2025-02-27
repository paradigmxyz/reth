use crate::tree::{
    cached_state::{CachedStateMetrics, CachedStateProvider, ProviderCaches, SavedCache},
    payload_processor::{
        executor::WorkloadExecutor, multiproof::MultiProofMessage, ExecutionCache,
    },
    StateProviderBuilder,
};
use alloy_consensus::transaction::Recovered;
use alloy_primitives::{keccak256, map::B256Set, B256};
use reth_evm::{ConfigureEvm, ConfigureEvmEnvFor, Evm};
use reth_primitives_traits::{header::SealedHeaderFor, NodePrimitives, SignedTransaction};
use reth_provider::{BlockReader, StateCommitmentProvider, StateProviderFactory, StateReader};
use reth_revm::{database::StateProviderDatabase, db::BundleState, state::EvmState};
use reth_trie::MultiProofTargets;
use std::{
    collections::VecDeque,
    sync::mpsc::{channel, Receiver, Sender},
};
use tracing::{debug, trace};

/// A task that is responsible for caching and prewarming the cache by executing transactions
/// individually in parallel.
///
/// Note: This task runs until cancelled externally.
pub(super) struct PrewarmCacheTask<N: NodePrimitives, P, Evm> {
    /// The executor used to spawn execution tasks.
    executor: WorkloadExecutor,
    /// Shared execution cache.
    execution_cache: ExecutionCache,
    /// Transactions pending execution.
    pending: VecDeque<Recovered<N::SignedTx>>,
    /// Context provided to execution tasks
    ctx: PrewarmContext<N, P, Evm>,
    /// How many txs are currently in progress
    in_progress: usize,
    /// How many transactions should be executed in parallel
    max_concurrency: usize,
    /// Sender to emit evm state outcome messages, if any.
    to_multi_proof: Option<Sender<MultiProofMessage>>,
    /// Receiver for events produced by tx execution
    actions_rx: Receiver<PrewarmTaskEvent>,
    /// Sender the transactions use to send their result back
    actions_tx: Sender<PrewarmTaskEvent>,
}

impl<N, P, Evm> PrewarmCacheTask<N, P, Evm>
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
        execution_cache: ExecutionCache,
        ctx: PrewarmContext<N, P, Evm>,
        to_multi_proof: Option<Sender<MultiProofMessage>>,
        pending: VecDeque<Recovered<N::SignedTx>>,
    ) -> Self {
        let (actions_tx, actions_rx) = channel();
        Self {
            executor,
            execution_cache,
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

    /// Returns the sender that can communicate with this task.
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
    fn spawn_transaction(&self, tx: Recovered<N::SignedTx>) {
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

    /// Returns true if the tx prewarming tasks should prepare multiproof targets.
    fn should_prepare_multi_proof_targets(&self) -> bool {
        self.to_multi_proof.is_some()
    }

    /// If configured and the tx returned proof targets, emit the targets the transaction produced
    fn send_multi_proof_targets(&self, targets: Option<MultiProofTargets>) {
        if let Some((proof_targets, to_multi_proof)) = targets.zip(self.to_multi_proof.as_ref()) {
            let _ = to_multi_proof.send(MultiProofMessage::PrefetchProofs(proof_targets));
        }
    }

    /// Save the state to the shared cache for the given block.
    fn save_cache(&self, state: BundleState) {
        let cache = SavedCache::new(
            self.ctx.header.hash(),
            self.ctx.cache.clone(),
            self.ctx.cache_metrics.clone(),
        );
        if cache.cache().insert_state(&state).is_err() {
            return
        }

        cache.update_metrics();

        debug!(target: "engine::caching", "Updated state caches");

        // update the cache for the executed block
        self.execution_cache.save_cache(cache);
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
                PrewarmTaskEvent::TerminateTransactionExecution => {
                    // stop tx processing
                    self.pending.clear();
                }
                PrewarmTaskEvent::Outcome { proof_targets } => {
                    // completed a transaction, frees up one slot
                    self.in_progress -= 1;
                    self.send_multi_proof_targets(proof_targets);
                }
                PrewarmTaskEvent::Terminate { block_output } => {
                    // terminate the task
                    if let Some(state) = block_output {
                        self.save_cache(state);
                    }

                    break
                }
            }

            // schedule followup transactions
            self.spawn_next();
        }
    }
}

/// Context required by tx execution tasks.
#[derive(Debug, Clone)]
pub(super) struct PrewarmContext<N: NodePrimitives, P, Evm> {
    pub(super) header: SealedHeaderFor<N>,
    pub(super) evm_config: Evm,
    pub(super) cache: ProviderCaches,
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
    fn prepare_multiproof_targets(self, tx: Recovered<N::SignedTx>) -> Option<MultiProofTargets> {
        let state = self.transact(tx)?;

        let mut targets = MultiProofTargets::with_capacity(state.len());

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
    fn transact(self, tx: Recovered<N::SignedTx>) -> Option<EvmState> {
        let Self { header, evm_config, cache: caches, cache_metrics, provider } = self;
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
    /// Forcefully terminate all remaining transaction execution.
    TerminateTransactionExecution,
    /// Forcefully terminate the task on demand and update the shared cache with the given output
    /// before exiting.
    Terminate {
        /// The final block state output.
        block_output: Option<BundleState>,
    },
    /// The outcome of a pre-warm task
    Outcome {
        /// The prepared proof targets based on the evm state outcome
        proof_targets: Option<MultiProofTargets>,
    },
}
