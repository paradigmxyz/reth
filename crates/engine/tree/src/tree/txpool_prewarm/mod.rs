//! Txpool-driven state prewarming and immutable snapshot publication.
//!
//! If block execution reuses prewarm results, the worker executes pool transactions in a predicted
//! environment of the next block and records their results, which prewarming of that block may
//! reuse, see [`PoolResults`].

mod control;
mod worker;

use self::control::Control;
use crate::tree::{
    payload_processor::reuse::{BlockEnvFields, PoolResults, PoolSeeds, POOL_BENEFICIARY},
    TxPoolPrewarmCacheSnapshot,
};
use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, B256};
use parking_lot::RwLock;
use reth_evm::{ConfigureEvm, EvmEnv, EvmEnvFor, HaltReasonFor};
use reth_primitives_traits::{NodePrimitives, SealedHeader, TxTy};
use reth_provider::{
    BlockNumReader, ChangeSetReader, DatabaseProviderFactory, DatabaseProviderROFactory,
    PruneCheckpointReader, StageCheckpointReader, StateProvider, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_storage_overlay::OverlayStateProviderFactory;
use revm::context::Block;
use std::{fmt::Debug, sync::Arc};

/// Coordinates a long-lived worker and the latest completed immutable snapshot.
pub(crate) struct Handle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    control: Arc<Control<Job<N, P, Evm>>>,
    /// Predicts the environment of the next block.
    evm_config: Evm,
    /// Whether to record results of pool transactions in the predicted environment of the next
    /// block.
    record_results: bool,
    /// The parent, predicted environment and results of the recorded speculation, if any.
    speculation: RwLock<Option<Speculation<Evm>>>,
}

impl<N, P, Evm> Debug for Handle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Handle").field("control", &self.control).finish()
    }
}

impl<N, P, Evm> Handle<N, P, Evm>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory + 'static,
    P::Provider: BlockNumReader
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    OverlayStateProviderFactory<P, N>: DatabaseProviderROFactory<Provider: StateProvider> + Send,
    Evm: ConfigureEvm<Primitives = N> + 'static,
{
    /// Spawns the long-lived worker, which owns its mutable read cache and starts a fresh one for
    /// each new head.
    ///
    /// If `record_results` is set, the worker records results of pool transactions that
    /// prewarming may reuse.
    pub(crate) fn spawn(
        runtime: &reth_tasks::Runtime,
        source: Arc<dyn Source<N>>,
        evm_config: Evm,
        record_results: bool,
    ) -> Self {
        let (control, commands) = Control::new();
        let publication = control.publication();
        let worker_evm_config = evm_config.clone();
        runtime.spawn_critical_os_thread("txpool-prewarm", "txpool prewarm worker", async move {
            worker::Worker::new(commands, publication, source, worker_evm_config).run()
        });
        Self { control, evm_config, record_results, speculation: RwLock::new(None) }
    }

    /// Pauses speculative work.
    ///
    /// Returns a guard that will resume the worker when dropped. There could be multiple
    /// outstanding guards, in which case the worker will not resume until all guards are dropped.
    ///
    /// Pausing is asynchronous and never blocks the caller: the worker observes it between
    /// transactions, so speculative work may overlap the guard's scope by at most one
    /// transaction.
    pub(crate) fn pause(&self) -> impl Drop + Send + 'static {
        self.control.pause()
    }

    /// Returns the latest fully published snapshot for `parent_hash`, or `None` if no snapshot is
    /// available for that hash.
    pub(crate) fn snapshot(&self, parent_hash: B256) -> Option<TxPoolPrewarmCacheSnapshot> {
        self.control.snapshot(parent_hash)
    }

    /// Starts continuous warming for the latest canonical head `parent`.
    ///
    /// Results are recorded in the predicted environment of the next block if it is available,
    /// otherwise warming uses the head block's own environment `evm_env`.
    pub(crate) fn start(
        &self,
        parent: &SealedHeader<N::BlockHeader>,
        evm_env: EvmEnvFor<Evm>,
        state_provider_factory: OverlayStateProviderFactory<P, N>,
    ) {
        let speculation = self
            .record_results
            .then(|| self.evm_config.speculative_next_evm_env(parent, POOL_BENEFICIARY))
            .flatten()
            .map(|evm_env| Speculation {
                parent_hash: parent.hash(),
                evm_env,
                results: Arc::default(),
            });
        let job = Job {
            evm_env: speculation.as_ref().map_or(evm_env, |s| s.evm_env.clone()),
            state_provider_factory,
            results: speculation.as_ref().map(|s| Arc::clone(&s.results)),
        };
        *self.speculation.write() = speculation;
        self.control.start(parent.hash(), job);
    }

    /// Returns the recorded results of pool transactions on top of `parent_hash` that prewarming
    /// of a block with `evm_env` may reuse.
    pub(crate) fn seeds(
        &self,
        parent_hash: B256,
        evm_env: &EvmEnvFor<Evm>,
    ) -> Option<PoolSeeds<HaltReasonFor<Evm>>> {
        let speculation = self.speculation.read();
        let speculation = speculation.as_ref().filter(|s| s.parent_hash == parent_hash)?;
        let differs = env_differences(&speculation.evm_env, evm_env)?;
        Some(PoolSeeds::new(
            Arc::clone(&speculation.results),
            evm_env.block_env.beneficiary(),
            differs,
        ))
    }
}

/// Results of pool transactions recorded on top of a parent in a predicted block environment.
struct Speculation<Evm: ConfigureEvm> {
    parent_hash: B256,
    evm_env: EvmEnvFor<Evm>,
    results: Arc<PoolResults<HaltReasonFor<Evm>>>,
}

/// Returns the block environment fields that differ between the `predicted` environment and the
/// `actual` one, or `None` if they differ in a way that every execution may depend on.
fn env_differences<Spec: Clone + PartialEq, B: Block>(
    predicted: &EvmEnv<Spec, B>,
    actual: &EvmEnv<Spec, B>,
) -> Option<BlockEnvFields> {
    let mut cfg_env = predicted.cfg_env.clone();
    // Results are recorded with these checks disabled and show that the checks would pass.
    cfg_env.disable_nonce_check = actual.cfg_env.disable_nonce_check;
    cfg_env.disable_balance_check = actual.cfg_env.disable_balance_check;
    if cfg_env != actual.cfg_env {
        return None
    }
    let (predicted, actual) = (&predicted.block_env, &actual.block_env);
    (predicted.number() == actual.number() &&
        predicted.basefee() == actual.basefee() &&
        predicted.difficulty() == actual.difficulty() &&
        predicted.blob_excess_gas_and_price() == actual.blob_excess_gas_and_price() &&
        predicted.slot_num() == actual.slot_num())
    .then(|| BlockEnvFields {
        // Recorded results never access the beneficiary.
        beneficiary: false,
        timestamp: predicted.timestamp() != actual.timestamp(),
        prevrandao: predicted.prevrandao() != actual.prevrandao(),
        gas_limit: predicted.gas_limit() != actual.gas_limit(),
    })
}

/// A live, forward-only view of the pool's best transactions for one canonical parent.
///
/// Returning [`None`](Iterator::next) only means no transaction is currently ready. The same
/// iterator can yield transactions that become pending later.
pub type Transactions<N> = Box<dyn Iterator<Item = Transaction<N>> + Send>;

/// A transaction selected from the txpool for cache-only prewarming.
#[derive(Debug, Clone)]
pub struct Transaction<N: NodePrimitives> {
    /// Transaction hash.
    pub hash: B256,
    /// Recovered sender.
    pub sender: Address,
    /// Recovered consensus transaction.
    pub transaction: Recovered<TxTy<N>>,
}

/// Source of txpool transactions for best-effort cache prewarming.
pub trait Source<N: NodePrimitives>: Send + Sync + Debug {
    /// Opens a live best-transactions iterator for `parent_hash`.
    ///
    /// The worker opens this once per canonical parent and retains it across empty polls, snapshot
    /// publications, and validation pauses. Sources should return [`None`] if they are not yet
    /// tracking `parent_hash`.
    fn best_transactions(&self, parent_hash: B256) -> Option<Transactions<N>>;
}

/// A request to warm txpool transactions against one fully validated parent state.
struct Job<N: NodePrimitives, P, Evm: ConfigureEvm<Primitives = N>> {
    evm_env: EvmEnvFor<Evm>,
    state_provider_factory: OverlayStateProviderFactory<P, N>,
    /// Where to record results of pool transactions, if `evm_env` is the predicted environment
    /// of the next block.
    results: Option<Arc<PoolResults<HaltReasonFor<Evm>>>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::U256;
    use reth_evm_ethereum::EthEvmConfig;

    #[test]
    fn predicted_env_differs_in_unobserved_fields() {
        let config = EthEvmConfig::mainnet();
        // A half full parent without blobs keeps the base fee and excess blob gas.
        let parent = Header {
            number: 25_000_000,
            timestamp: 1_800_000_000,
            gas_limit: 60_000_000,
            gas_used: 30_000_000,
            base_fee_per_gas: Some(1_000_000_000),
            excess_blob_gas: Some(0),
            blob_gas_used: Some(0),
            ..Default::default()
        };
        let block = Header {
            number: parent.number + 1,
            timestamp: parent.timestamp + 12,
            beneficiary: Address::repeat_byte(1),
            mix_hash: B256::repeat_byte(2),
            gas_limit: parent.gas_limit + 1,
            ..parent.clone()
        };
        let mut predicted = config
            .speculative_next_evm_env(&SealedHeader::seal_slow(parent), POOL_BENEFICIARY)
            .unwrap();
        // Recorded results disable the nonce check.
        predicted.cfg_env.disable_nonce_check = true;
        let mut actual = config.evm_env(&block).unwrap();
        assert_eq!(
            env_differences(&predicted, &actual),
            Some(BlockEnvFields { prevrandao: true, gas_limit: true, ..Default::default() })
        );

        actual.block_env.timestamp += U256::from(12);
        assert_eq!(
            env_differences(&predicted, &actual),
            Some(BlockEnvFields {
                timestamp: true,
                prevrandao: true,
                gas_limit: true,
                ..Default::default()
            })
        );

        actual.block_env.basefee += 1;
        assert_eq!(env_differences(&predicted, &actual), None);
    }
}
