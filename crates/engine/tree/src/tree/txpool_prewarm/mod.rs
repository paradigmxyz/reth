//! Txpool-driven state prewarming and immutable snapshot publication.

mod control;
mod worker;

use self::control::Control;
use crate::tree::TxPoolPrewarmCacheSnapshot;
use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, B256};
use parking_lot::Mutex;
use reth_evm::{ConfigureEvm, EvmEnvFor};
use reth_primitives_traits::{NodePrimitives, TxTy};
use reth_provider::{
    BlockNumReader, ChangeSetReader, DatabaseProviderFactory, DatabaseProviderROFactory,
    PruneCheckpointReader, StageCheckpointReader, StateProvider, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_storage_overlay::OverlayStateProviderFactory;
use reth_tasks::{TaskHandle, TaskRuntime};
use std::{fmt::Debug, future::poll_fn, pin::Pin, sync::Arc, task::Poll, time::Duration};

/// Coordinates a long-lived worker and the latest completed immutable snapshot.
pub(crate) struct Handle<N, P, Evm>
where
    N: NodePrimitives,
    Evm: ConfigureEvm<Primitives = N>,
{
    control: Arc<Control<Job<N, P, Evm>>>,
    runtime: TaskRuntime,
    worker: Mutex<Option<WorkerHandle>>,
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
        let worker = runtime.spawn_critical_os_thread(
            "txpool-prewarm",
            "txpool prewarm worker",
            async move { worker::Worker::new(commands, publication, source, evm_config).run() },
        );
        Self {
            control,
            runtime: runtime.clone().into(),
            worker: Mutex::new(Some(WorkerHandle::Native(worker))),
        }
    }

    /// Runs the shared warming algorithm as bounded transactions on the supplied runtime.
    pub(crate) fn spawn_with_runtime(
        runtime: &TaskRuntime,
        source: Arc<dyn Source<N>>,
        evm_config: Evm,
    ) -> Self {
        let (control, commands) = Control::new();
        let publication = control.publication();
        let worker_runtime = runtime.clone();
        let evm_config = evm_config.with_jit_support_enabled(false);
        let worker = runtime.spawn("txpool_prewarm", async move {
            worker::Worker::new(commands, publication, source, evm_config)
                .run_cooperative(worker_runtime)
                .await;
        });
        Self {
            control,
            runtime: runtime.clone(),
            worker: Mutex::new(Some(WorkerHandle::Cooperative(worker))),
        }
    }

    /// Stops speculative work and waits for its transaction, provider and iterator to be dropped.
    /// The wait can be canceled and retried without canceling an in-flight transaction.
    pub(crate) async fn shutdown(&self) {
        self.control.shutdown();
        loop {
            let finished = poll_fn(|cx| {
                let mut worker = self.worker.lock();
                let Some(handle) = worker.as_mut() else { return Poll::Ready(true) };
                match handle {
                    WorkerHandle::Native(handle) if !handle.is_finished() => {
                        return Poll::Ready(false)
                    }
                    WorkerHandle::Native(_) => {
                        let Some(WorkerHandle::Native(handle)) = worker.take() else {
                            unreachable!()
                        };
                        handle.join().expect("txpool prewarming thread panicked");
                    }
                    WorkerHandle::Cooperative(handle) => match Pin::new(handle).poll(cx) {
                        Poll::Pending => return Poll::Ready(false),
                        Poll::Ready(result) => {
                            result.expect("txpool prewarming task failed");
                            *worker = None;
                        }
                    },
                }
                Poll::Ready(true)
            })
            .await;
            if finished {
                return
            }
            self.runtime.sleep(Duration::from_millis(1)).await;
        }
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

    #[cfg(test)]
    pub(crate) fn snapshot_observer(
        &self,
    ) -> impl Fn(B256) -> Option<TxPoolPrewarmCacheSnapshot> + Send + Sync + 'static {
        let publication = self.control.publication();
        move |parent_hash| {
            publication
                .read()
                .as_ref()
                .filter(|snapshot| snapshot.parent_hash() == parent_hash)
                .cloned()
        }
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

/// Kept in the owner while joining, so a canceled shutdown remains retryable.
enum WorkerHandle {
    Native(std::thread::JoinHandle<()>),
    Cooperative(TaskHandle<()>),
}
