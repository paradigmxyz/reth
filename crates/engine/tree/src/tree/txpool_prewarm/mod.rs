//! Txpool-driven state prewarming and immutable snapshot publication.
//!
//! If block execution reuses prewarm results, the worker executes pool transactions in a predicted
//! environment of the next block and records their results, which prewarming of that block may
//! reuse, see [`PoolResults`].

mod control;
mod worker;

use self::control::Control;
use crate::tree::{
    payload_processor::reuse::{
        cfg_reuses_prewarm_results, BlockEnvFields, PoolResults, PoolSeeds, POOL_BENEFICIARY,
    },
    TxPoolPrewarmCacheSnapshot,
};
use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, B256};
use parking_lot::RwLock;
use reth_evm::{ConfigureEvm, EvmEnv, EvmEnvFor, EvmTypesFor};
use reth_primitives_traits::{NodePrimitives, SealedHeader, TxTy};
use reth_provider::{
    BlockNumReader, ChangeSetReader, DatabaseProviderFactory, DatabaseProviderROFactory,
    PruneCheckpointReader, StageCheckpointReader, StateProvider, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_storage_overlay::OverlayStateProviderFactory;
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
    /// Results are recorded in the predicted environment of the next block if it is available and
    /// the block may reuse them, otherwise warming uses the head block's own environment
    /// `evm_env`.
    pub(crate) fn start(
        &self,
        parent: &SealedHeader<N::BlockHeader>,
        evm_env: EvmEnvFor<Evm>,
        state_provider_factory: OverlayStateProviderFactory<P, N>,
    ) {
        let mut speculation = self.speculation.write();
        // The worker keeps executed transactions of the same head, so it would not record them
        // again.
        if speculation.as_ref().is_none_or(|s| s.parent_hash != parent.hash()) {
            *speculation = self
                .record_results
                .then(|| self.evm_config.speculative_next_evm_env(parent, POOL_BENEFICIARY))
                .flatten()
                .filter(|evm_env| cfg_reuses_prewarm_results(evm_env.version()))
                .map(|evm_env| Speculation {
                    parent_hash: parent.hash(),
                    evm_env,
                    results: Arc::default(),
                });
        }
        let job = Job {
            evm_env: speculation.as_ref().map_or(evm_env, |s| s.evm_env.clone()),
            state_provider_factory,
            results: speculation.as_ref().map(|s| Arc::clone(&s.results)),
        };
        drop(speculation);
        self.control.start(parent.hash(), job);
    }

    /// Returns the recorded results of pool transactions on top of `parent_hash` that prewarming
    /// of a block with `evm_env` may reuse, `None` if there are none.
    pub(crate) fn seeds(
        &self,
        parent_hash: B256,
        evm_env: &EvmEnvFor<Evm>,
    ) -> Option<PoolSeeds<EvmTypesFor<Evm>>> {
        let speculation = self.speculation.read();
        let speculation = speculation
            .as_ref()
            .filter(|s| s.parent_hash == parent_hash && !s.results.is_empty())?;
        let differs = env_differences(&speculation.evm_env, evm_env)?;
        Some(PoolSeeds::new(
            Arc::clone(&speculation.results),
            evm_env.block_env().beneficiary,
            differs,
        ))
    }
}

/// Results of pool transactions recorded on top of a parent in a predicted block environment.
struct Speculation<Evm: ConfigureEvm> {
    parent_hash: B256,
    evm_env: EvmEnvFor<Evm>,
    results: Arc<PoolResults<EvmTypesFor<Evm>>>,
}

/// Returns the block environment fields that differ between the `predicted` environment and the
/// `actual` one, or `None` if they differ in a way that every execution may depend on.
fn env_differences<E: EvmEnv>(predicted: &E, actual: &E) -> Option<BlockEnvFields> {
    let predicted_version = predicted.version();
    let actual_version = actual.version();
    let checks = evm2::EvmFeatures::NONCE_CHECK | evm2::EvmFeatures::BALANCE_CHECK;
    if Into::<evm2::SpecId>::into(predicted.spec_id()) !=
        Into::<evm2::SpecId>::into(actual.spec_id()) ||
        predicted_version.gas_params != actual_version.gas_params ||
        (predicted_version.features | checks) != (actual_version.features | checks) ||
        predicted_version.chain_id != actual_version.chain_id ||
        predicted_version.tx_gas_limit_cap != actual_version.tx_gas_limit_cap ||
        predicted_version.memory_limit != actual_version.memory_limit ||
        predicted_version.max_code_size != actual_version.max_code_size ||
        predicted_version.max_initcode_size != actual_version.max_initcode_size ||
        predicted_version.max_blobs_per_tx != actual_version.max_blobs_per_tx
    {
        return None
    }
    let (predicted, actual) = (predicted.block_env(), actual.block_env());
    (predicted.number == actual.number &&
        predicted.basefee == actual.basefee &&
        predicted.difficulty == actual.difficulty &&
        predicted.blob_basefee == actual.blob_basefee &&
        predicted.slot_num == actual.slot_num)
        .then(|| BlockEnvFields {
            beneficiary: false,
            timestamp: predicted.timestamp != actual.timestamp,
            prevrandao: predicted.prevrandao != actual.prevrandao,
            gas_limit: predicted.gas_limit != actual.gas_limit,
        })
}

/// A live, forward-only view of the pool's best transactions for one canonical parent.
///
/// Returning [`None`](Iterator::next) only means no transaction is currently ready. The same
/// iterator can yield transactions that become pending later.
pub type Transactions<N> = Box<dyn Iterator<Item = Transaction<N>> + Send>;

/// A transaction selected from the txpool for prewarming.
#[derive(Debug, Clone)]
pub struct Transaction<N: NodePrimitives> {
    /// Transaction hash.
    pub hash: B256,
    /// Recovered sender.
    pub sender: Address,
    /// Recovered consensus transaction.
    pub transaction: Recovered<TxTy<N>>,
}

/// Source of txpool transactions for best-effort prewarming.
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
    results: Option<Arc<PoolResults<EvmTypesFor<Evm>>>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use alloy_primitives::U256;
    use evm2::BaseEvmTypes;
    use reth_chainspec::ChainSpecBuilder;
    use reth_ethereum_primitives::EthPrimitives;
    use reth_evm_ethereum::EthEvmConfig;
    use reth_provider::test_utils::MockEthProvider;
    use reth_storage_overlay::OverlayManager;
    use reth_tasks::Runtime;

    type TestHandle = Handle<EthPrimitives, MockEthProvider, EthEvmConfig>;

    /// A pool that never tracks the head.
    #[derive(Debug)]
    struct NoPool;

    impl Source<EthPrimitives> for NoPool {
        fn best_transactions(&self, _parent_hash: B256) -> Option<Transactions<EthPrimitives>> {
            None
        }
    }

    fn start(handle: &TestHandle, parent: &SealedHeader) {
        let state_provider_factory = OverlayStateProviderFactory::new(
            MockEthProvider::default(),
            OverlayManager::default().overlay_builder(parent.hash()),
        );
        handle.start(parent, Default::default(), state_provider_factory);
    }

    fn recorded(handle: &TestHandle) -> Option<Arc<PoolResults<BaseEvmTypes>>> {
        handle.speculation.read().as_ref().map(|s| Arc::clone(&s.results))
    }

    #[test]
    fn keeps_results_of_the_same_head() {
        let handle =
            TestHandle::spawn(&Runtime::test(), Arc::new(NoPool), EthEvmConfig::mainnet(), true);
        let parent = SealedHeader::seal_slow(Header::default());
        start(&handle, &parent);
        let results = recorded(&handle).unwrap();
        start(&handle, &parent);
        assert!(Arc::ptr_eq(&results, &recorded(&handle).unwrap()));

        start(&handle, &SealedHeader::seal_slow(Header { number: 1, ..Default::default() }));
        assert!(!Arc::ptr_eq(&results, &recorded(&handle).unwrap()));
    }

    #[test]
    fn offers_no_seeds_without_results() {
        let handle =
            TestHandle::spawn(&Runtime::test(), Arc::new(NoPool), EthEvmConfig::mainnet(), true);
        let parent = SealedHeader::seal_slow(Header::default());
        start(&handle, &parent);
        let evm_env = handle.speculation.read().as_ref().unwrap().evm_env.clone();
        assert!(handle.seeds(parent.hash(), &evm_env).is_none());
    }

    #[test]
    fn records_no_results_with_eip8037() {
        let chain_spec = ChainSpecBuilder::mainnet().amsterdam_activated().build();
        let evm_config = EthEvmConfig::new(Arc::new(chain_spec));
        let handle = TestHandle::spawn(&Runtime::test(), Arc::new(NoPool), evm_config, true);
        start(&handle, &SealedHeader::seal_slow(Header::default()));
        assert!(recorded(&handle).is_none());
    }

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
        predicted.version.features.remove(evm2::EvmFeatures::NONCE_CHECK);
        let mut actual = config.evm_env(&block).unwrap();
        assert_eq!(
            env_differences(&predicted, &actual),
            Some(BlockEnvFields { prevrandao: true, gas_limit: true, ..Default::default() })
        );

        actual.block.timestamp += U256::from(12);
        assert_eq!(
            env_differences(&predicted, &actual),
            Some(BlockEnvFields {
                timestamp: true,
                prevrandao: true,
                gas_limit: true,
                ..Default::default()
            })
        );

        let mut other_chain = actual.clone();
        other_chain.version.chain_id += 1;
        assert_eq!(env_differences(&predicted, &other_chain), None);

        actual.block.basefee += U256::ONE;
        assert_eq!(env_differences(&predicted, &actual), None);
    }
}
