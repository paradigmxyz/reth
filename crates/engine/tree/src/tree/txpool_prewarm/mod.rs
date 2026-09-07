//! Txpool-driven state prewarming and immutable snapshot publication.

mod control;
mod worker;

use self::control::Control;
use crate::tree::TxPoolPrewarmCacheSnapshot;
use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, B256};
use reth_evm::{ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{NodePrimitives, TxTy};
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
    pub(crate) fn spawn(
        runtime: &reth_tasks::Runtime,
        source: Arc<dyn Source<N>>,
        evm_config: Evm,
    ) -> Self {
        let (control, commands) = Control::new();
        let publication = control.publication();
        runtime.spawn_critical_os_thread("txpool-prewarm", "txpool prewarm worker", async move {
            worker::Worker::new(commands, publication, source, evm_config).run()
        });
        Self { control }
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

    /// Starts continuous warming for the latest canonical head.
    pub(crate) fn start(
        &self,
        parent_hash: B256,
        evm_env: EvmEnvFor<Evm>,
        state_provider_factory: OverlayStateProviderFactory<P, N>,
    ) {
        self.control.start(parent_hash, Job { evm_env, state_provider_factory });
    }
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
}
