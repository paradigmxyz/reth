use crate::{
    blobstore::BlobStoreUpdates,
    maintain::{
        drift_monitor::{DriftMonitor, DriftMonitorResult},
        interfaces::{CanonProcessing, DriftMonitoring, PoolMaintainerComponentFactory},
        CanonEventProcessor, CanonEventProcessorConfig, MaintainPoolConfig, PoolDriftState,
    },
    metrics::MaintainPoolMetrics,
    traits::TransactionPoolExt,
    BlockInfo, PoolTransaction,
};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumberOrTag;
use futures_util::Stream;
use reth_chain_state::CanonStateNotification;
use reth_chainspec::{ChainSpecProvider, EthChainSpec};
use reth_primitives_traits::{BlockBody, NodePrimitives, SealedHeader, SignerRecoverable};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::time;
use tracing::{debug, trace};

/// Builder for creating a customizable pool maintainer
#[derive(Debug)]
pub(crate) struct PoolMaintainerBuilder<N, Client, P, St, Tasks>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
{
    client: Client,
    pool: P,
    events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,
    _phantom: std::marker::PhantomData<N>,
}

impl<N, Client, P, St, Tasks> PoolMaintainerBuilder<N, Client, P, St, Tasks>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    /// Creates a new `PoolMaintainerBuilder`.
    pub(crate) const fn new(
        client: Client,
        pool: P,
        events: St,
        task_spawner: Tasks,
        config: MaintainPoolConfig,
    ) -> Self {
        Self { client, pool, events, task_spawner, config, _phantom: std::marker::PhantomData }
    }

    /// Build the pool maintainer with custom component factory
    pub(crate) fn build_with_factory<F, M, C>(
        self,
        factory: F,
    ) -> PoolMaintainer<N, Client, P, St, Tasks, M, C>
    where
        F: PoolMaintainerComponentFactory<N, Client, P, M, C> + Clone + Send + Unpin + 'static,
        M: DriftMonitoring<Client> + Send + Clone + 'static,
        C: CanonProcessing<N, Client, P> + Send + Clone + 'static,
    {
        let metrics = MaintainPoolMetrics::default();
        let MaintainPoolConfig { max_reload_accounts, max_update_depth, .. } = self.config;

        let drift_monitor = factory.create_drift_monitor(max_reload_accounts, metrics.clone());
        let canon_processor_config = CanonEventProcessorConfig { max_update_depth };
        let canon_processor = factory.create_canon_processor(
            self.client.finalized_block_number().ok().flatten(),
            canon_processor_config,
            metrics,
        );

        PoolMaintainer::new_with_custom_components(
            self.client,
            self.pool,
            self.events,
            self.task_spawner,
            self.config,
            drift_monitor,
            canon_processor,
        )
    }

    /// Build the pool maintainer with the default components
    pub(crate) fn build(self) -> PoolMaintainer<N, Client, P, St, Tasks> {
        // Use the default component factory to create standard components
        self.build_with_factory(super::interfaces::DefaultComponentFactory)
    }
}

/// Current state of the maintainer Future
enum PoolMaintainerState {
    /// Waiting for any of the futures to be ready
    Waiting,
    /// Processing a reorg event
    ProcessingReorg { future: Pin<Box<dyn Future<Output = ()> + Send + 'static>> },
    /// Processing stale transaction eviction
    ProcessingStaleEviction,
    /// Finished (stream ended)
    Finished,
}

impl std::fmt::Debug for PoolMaintainerState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Waiting => write!(f, "PoolMaintainerState::Waiting"),
            Self::ProcessingReorg { .. } => {
                write!(f, "PoolMaintainerState::ProcessingReorg {{ future: <future> }}")
            }
            Self::ProcessingStaleEviction => {
                write!(f, "PoolMaintainerState::ProcessingStaleEviction")
            }
            Self::Finished => write!(f, "PoolMaintainerState::Finished"),
        }
    }
}

/// The main pool maintainer
#[derive(Debug)]
pub(crate) struct PoolMaintainer<
    N,
    Client,
    P,
    St,
    Tasks,
    M = DriftMonitor,
    C = CanonEventProcessor<N>,
> where
    N: NodePrimitives,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    M: DriftMonitoring<Client> + Send + Clone + 'static,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    Tasks: TaskSpawner + 'static,
    C: CanonProcessing<N, Client, P> + Send + Clone + 'static,
{
    client: Client,
    pool: P,
    events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,

    // Internal state
    drift_monitor: M,
    canon_processor: C,
    stale_eviction_interval: time::Interval,

    // Initialization flag
    initialized: bool,

    // Current state of the state machine
    state: PoolMaintainerState,

    _phantom: std::marker::PhantomData<N>,
}

impl<N, Client, P, St, Tasks, M, C> PoolMaintainer<N, Client, P, St, Tasks, M, C>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
    M: DriftMonitoring<Client> + Send + Clone + 'static,
    C: CanonProcessing<N, Client, P> + Send + Clone + 'static,
{
    /// Create a pool maintainer with custom components
    #[allow(clippy::too_many_arguments)]
    fn new_with_custom_components(
        client: Client,
        pool: P,
        events: St,
        task_spawner: Tasks,
        config: MaintainPoolConfig,
        drift_monitor: M,
        canon_processor: C,
    ) -> Self {
        let stale_eviction_interval = time::interval(config.max_tx_lifetime);

        Self {
            client,
            pool,
            events,
            task_spawner,
            config,
            drift_monitor,
            canon_processor,
            stale_eviction_interval,
            initialized: false,
            state: PoolMaintainerState::Waiting,
            _phantom: std::marker::PhantomData,
        }
    }

    /// Initialize the pool with the latest state (called once on first poll)
    fn initialize(&mut self) {
        if self.initialized {
            return;
        }

        // Ensure the pool points to latest state
        if let Ok(Some(latest)) = self.client.header_by_number_or_tag(BlockNumberOrTag::Latest) {
            let latest = SealedHeader::seal_slow(latest);
            let chain_spec = self.client.chain_spec();
            let info = BlockInfo {
                block_gas_limit: latest.gas_limit(),
                last_seen_block_hash: latest.hash(),
                last_seen_block_number: latest.number(),
                pending_basefee: latest
                    .next_block_base_fee(
                        chain_spec.base_fee_params_at_timestamp(latest.timestamp()),
                    )
                    .unwrap_or_default(),
                pending_blob_fee: latest.maybe_next_block_blob_fee(
                    chain_spec.blob_params_at_timestamp(latest.timestamp()),
                ),
            };
            self.pool.set_block_info(info);
        }

        self.initialized = true;
    }

    /// Handle a canon event
    fn handle_canon_event(&mut self, event: CanonStateNotification<N>) {
        match &event {
            CanonStateNotification::Reorg { old: _, new: _ } => {
                let mut canon_processor = self.canon_processor.clone();
                let client = self.client.clone();
                let pool = self.pool.clone();
                let mut drift_monitor = self.drift_monitor.clone();

                let fut = async move {
                    canon_processor
                        .process_reorg(event.clone(), &client, &pool, &mut drift_monitor)
                        .await;
                };

                // transition to the ProcessingReorg state
                self.state = PoolMaintainerState::ProcessingReorg { future: Box::pin(fut) };
            }
            CanonStateNotification::Commit { new } => {
                self.canon_processor.process_commit(
                    event.clone(),
                    &self.client,
                    &self.pool,
                    &mut self.drift_monitor,
                );

                let (blocks, _) = new.inner();
                let block = blocks.tip();
                // mark affected accounts as dirty for reload
                let affected_accounts: HashSet<_> = block
                    .body()
                    .transactions()
                    .iter()
                    .filter_map(|tx| tx.recover_signer().ok())
                    .collect();

                if !affected_accounts.is_empty() {
                    self.drift_monitor.add_dirty_addresses(affected_accounts);
                }
            }
        }
    }
}

impl<N, Client, P, St, Tasks, M, C> Future for PoolMaintainer<N, Client, P, St, Tasks, M, C>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Unpin + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
    M: DriftMonitoring<Client> + Unpin + Send + Clone,
    C: CanonProcessing<N, Client, P> + Unpin + Send + Clone,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // initialize on first poll
        this.initialize();

        loop {
            match &mut this.state {
                PoolMaintainerState::Waiting => {
                    trace!(target: "txpool", state=?this.drift_monitor.state(), "awaiting new block or reorg");

                    this.drift_monitor.update_metrics();
                    let pool_info = this.pool.block_info();

                    // If drifted, mark all senders as dirty
                    if this.drift_monitor.state().is_drifted() {
                        this.drift_monitor.set_dirty_addresses(this.pool.unique_senders());
                        this.drift_monitor.set_state(PoolDriftState::InSync);
                    }

                    // Start reloading accounts if needed
                    if this.drift_monitor.has_dirty_addresses() &&
                        !this.drift_monitor.is_reloading()
                    {
                        this.drift_monitor.start_reload_accounts(
                            this.client.clone(),
                            pool_info.last_seen_block_hash,
                            &this.task_spawner,
                        );
                    }

                    // Check for finalized blocks and clean up blobs if needed
                    if let Some(BlobStoreUpdates::Finalized(blobs)) =
                        this.canon_processor.update_finalized(&this.client)
                    {
                        this.pool.delete_blobs(blobs.clone());
                        let pool = this.pool.clone();
                        this.task_spawner.spawn_blocking(Box::pin(async move {
                            debug!(target: "txpool", finalized_blobs=%blobs.len(), "cleaning up blob store");
                            pool.cleanup_blobs();
                        }));
                    }

                    // Poll drift monitor
                    let drift_poll = Pin::new(&mut this.drift_monitor).poll(cx);
                    if let Poll::Ready(result) = drift_poll {
                        match result {
                            DriftMonitorResult::AccountsLoaded(accounts) => {
                                this.pool.update_accounts(accounts.accounts);
                            }
                            DriftMonitorResult::Failed => {
                                debug!(target: "txpool", "Account reload failed");
                            }
                        }
                        // Continue to process more states if possible
                        continue;
                    }

                    // Poll events stream
                    let events_poll = Pin::new(&mut this.events).poll_next(cx);
                    if let Poll::Ready(ev) = events_poll {
                        if let Some(event) = ev {
                            this.drift_monitor.on_first_event();
                            this.handle_canon_event(event);
                            // If handle_canon_event transitioned to a new state, process it right
                            // away
                            continue;
                        }
                        // Stream ended
                        this.state = PoolMaintainerState::Finished;
                        continue;
                    }

                    // poll interval for stale eviction
                    let interval_poll = Pin::new(&mut this.stale_eviction_interval).poll_tick(cx);
                    if interval_poll.is_ready() {
                        // transition to removing stale transactions
                        this.state = PoolMaintainerState::ProcessingStaleEviction;
                        continue;
                    }

                    return Poll::Pending;
                }
                PoolMaintainerState::ProcessingReorg { future } => {
                    match future.as_mut().poll(cx) {
                        Poll::Ready(_) => {
                            // reorg processing is complete, return to idle
                            this.state = PoolMaintainerState::Waiting;
                        }
                        Poll::Pending => {
                            // reorg is still processing, keep waiting
                            return Poll::Pending;
                        }
                    }
                }
                PoolMaintainerState::ProcessingStaleEviction => {
                    let stale_txs: Vec<_> = this
                        .pool
                        .queued_transactions()
                        .into_iter()
                        .filter(|tx| {
                            (tx.origin.is_external() || this.config.no_local_exemptions) &&
                                tx.timestamp.elapsed() > this.config.max_tx_lifetime
                        })
                        .map(|tx| *tx.hash())
                        .collect();

                    debug!(target: "txpool", count=%stale_txs.len(), "removing stale transactions");
                    this.pool.remove_transactions(stale_txs);

                    // return to idle state
                    this.state = PoolMaintainerState::Waiting;
                }
                PoolMaintainerState::Finished => return Poll::Ready(()),
            }
        }
    }
}
