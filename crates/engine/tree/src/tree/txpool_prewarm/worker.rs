use super::{
    cache::Cache,
    coordination::{Coordinator, Shutdown},
    Job, Source, Transactions,
};
use crate::tree::StateProviderDatabase;
use alloy_evm::Evm;
use reth_evm::ConfigureEvm;
use reth_primitives_traits::NodePrimitives;
use reth_provider::{BlockReader, StateProviderFactory, StateReader};
use reth_revm::db::State;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, trace};

/// Maximum interval between snapshot publications and delay when no transaction is ready.
const REFRESH_INTERVAL: Duration = Duration::from_millis(100);

/// Delay while waiting for pool maintenance to advance to the state being warmed.
const HEAD_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(super) fn run<N, P, Evm>(
    coordinator: Arc<Coordinator<Job<N, P, Evm>>>,
    source: Arc<dyn Source<N>>,
    evm_config: Evm,
) where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone + Send + Sync + 'static,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    let mut cache = Cache::default();
    let mut current_job = None;
    let mut best_transactions: Option<Transactions<N>> = None;
    let mut best_transactions_parent_hash = None;

    loop {
        let (replacement, activity_guard) = match coordinator.begin_activity(current_job.is_some())
        {
            Ok(activity) => activity,
            Err(Shutdown) => break,
        };
        if let Some(job) = replacement {
            if best_transactions_parent_hash != Some(job.parent_hash) {
                best_transactions = None;
                best_transactions_parent_hash = None;
            }
            current_job = Some(job);
        }

        let job = current_job.as_ref().expect("runnable job exists");

        if best_transactions_parent_hash != Some(job.parent_hash) {
            let Some(opened) = source.best_transactions(job.parent_hash) else {
                drop(activity_guard);
                coordinator.wait_for_change(HEAD_POLL_INTERVAL);
                continue
            };
            best_transactions = Some(opened);
            best_transactions_parent_hash = Some(job.parent_hash);
        }

        if cache.parent_hash() != Some(job.parent_hash) {
            cache.reset(job.parent_hash);
            debug!(
                target: "engine::tree::txpool_prewarm",
                parent_hash = ?job.parent_hash,
                "started txpool prewarming"
            );
        }

        let transaction_count = prewarm_batch(
            &evm_config,
            job,
            &cache,
            best_transactions.as_mut().expect("best transactions opened for active parent"),
            || activity_guard.is_interrupted(),
        );

        if activity_guard.is_interrupted() {
            drop(activity_guard);
            continue
        }
        if transaction_count == 0 {
            drop(activity_guard);
            coordinator.wait_for_change(REFRESH_INTERVAL);
            continue
        }

        let snapshot = cache.snapshot();
        let (accounts, storage, bytecodes) = snapshot.entry_counts();
        if activity_guard.publish(snapshot) {
            debug!(
                target: "engine::tree::txpool_prewarm",
                parent_hash = ?job.parent_hash,
                transactions = transaction_count,
                accounts,
                storage,
                bytecodes,
                "published txpool prewarming snapshot"
            );
        }
    }
}

fn prewarm_batch<N, P, Evm>(
    evm_config: &Evm,
    job: &Job<N, P, Evm>,
    cache: &Cache,
    transactions: &mut Transactions<N>,
    is_interrupted: impl Fn() -> bool,
) -> usize
where
    N: NodePrimitives,
    P: BlockReader + StateProviderFactory + StateReader + Clone,
    Evm: ConfigureEvm<Primitives = N>,
{
    if is_interrupted() {
        return 0
    }

    let state_provider = match job.provider_builder.build() {
        Ok(provider) => provider,
        Err(err) => {
            trace!(
                target: "engine::tree::txpool_prewarm",
                %err,
                parent_hash = ?job.parent_hash,
                "failed to build txpool prewarming state provider"
            );
            return 0
        }
    };
    let state_provider = cache.state_provider(state_provider);
    let state_provider = StateProviderDatabase::new(state_provider);
    let mut state = State::builder().with_database(state_provider).build();
    let mut evm_env = job.evm_env.clone();
    evm_env.cfg_env.disable_nonce_check = true;
    evm_env.cfg_env.disable_balance_check = true;
    let mut evm = evm_config.evm_with_env(&mut state, evm_env);

    let deadline = Instant::now() + REFRESH_INTERVAL;
    let mut transaction_count = 0;
    loop {
        if is_interrupted() || Instant::now() >= deadline {
            break
        }
        let Some(transaction) = transactions.next() else { break };
        if let Err(err) = evm.transact(transaction.transaction) {
            trace!(
                target: "engine::tree::txpool_prewarm",
                %err,
                tx_hash = ?transaction.hash,
                sender = %transaction.sender,
                "speculative txpool transaction execution failed"
            );
        }
        transaction_count += 1;
    }

    transaction_count
}
