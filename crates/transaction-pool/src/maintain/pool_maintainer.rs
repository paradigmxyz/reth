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
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_storage_api::{BlockReaderIdExt, StateProviderFactory};
use reth_tasks::TaskSpawner;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::time;
use tracing::{debug, trace};

/// Builder for creating a customizable pool maintainer
#[derive(Debug)]
pub struct PoolMaintainerBuilder<N, Client, P, St, Tasks>
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
    pub const fn new(
        client: Client,
        pool: P,
        events: St,
        task_spawner: Tasks,
        config: MaintainPoolConfig,
    ) -> Self {
        Self { client, pool, events, task_spawner, config, _phantom: std::marker::PhantomData }
    }

    /// Build the pool maintainer with custom component factory
    pub fn build_with_factory<F, M, C>(
        self,
        factory: F,
    ) -> PoolMaintainer<N, Client, P, St, Tasks, M, C>
    where
        F: PoolMaintainerComponentFactory<N, Client, P, M, C>,
        M: DriftMonitoring<Client>,
        C: CanonProcessing<N, Client, P>,
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
    pub fn build(self) -> PoolMaintainer<N, Client, P, St, Tasks> {
        // Use the default component factory to create standard components
        self.build_with_factory(super::interfaces::DefaultComponentFactory)
    }
}

/// Current state of the maintainer Future
enum PoolMaintainerState<N: NodePrimitives> {
    /// Waiting for any of the futures to be ready
    Waiting,
    /// Processing a `DriftMonitor` update
    ProcessingDriftMonitor,
    /// Processing a `CanonState` event
    ProcessingCanonEvent(Option<CanonStateNotification<N>>),
    /// Processing stale transaction eviction
    ProcessingStaleEviction,
    /// Finished (stream ended)
    Finished,
}

/// The main pool maintainer
#[derive(Debug)]
pub struct PoolMaintainer<N, Client, P, St, Tasks, M = DriftMonitor, C = CanonEventProcessor<N>>
where
    N: NodePrimitives,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    M: DriftMonitoring<Client>,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    C: CanonProcessing<N, Client, P>,
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

    _phantom: std::marker::PhantomData<N>,
}

impl<N, Client, P, St, Tasks, M, C> PoolMaintainer<N, Client, P, St, Tasks, M, C>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
    M: DriftMonitoring<Client>,
    C: CanonProcessing<N, Client, P>,
{
    /// Create a pool maintainer with custom components
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
}

impl<N, Client, P, St, Tasks, M, C> Future for PoolMaintainer<N, Client, P, St, Tasks, M, C>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Unpin + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
    M: DriftMonitoring<Client> + Unpin,
    C: CanonProcessing<N, Client, P> + Unpin,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // initialize on first poll
        this.initialize();

        let mut state = PoolMaintainerState::Waiting;

        loop {
            match state {
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
                    if drift_poll.is_ready() {
                        state = PoolMaintainerState::ProcessingDriftMonitor;
                        continue;
                    }

                    // Poll events stream
                    let events_poll = Pin::new(&mut this.events).poll_next(cx);
                    match events_poll {
                        Poll::Ready(ev) => {
                            state = PoolMaintainerState::ProcessingCanonEvent(ev);
                            continue;
                        }
                        Poll::Pending => {}
                    }

                    // Poll interval for stale eviction
                    let interval_poll = Pin::new(&mut this.stale_eviction_interval).poll_tick(cx);
                    if interval_poll.is_ready() {
                        state = PoolMaintainerState::ProcessingStaleEviction;
                        continue;
                    }

                    // Nothing is ready, return pending
                    return Poll::Pending;
                }

                PoolMaintainerState::ProcessingDriftMonitor => {
                    // We know the drift monitor is ready at this point
                    if let Poll::Ready(result) = Pin::new(&mut this.drift_monitor).poll(cx) {
                        match result {
                            DriftMonitorResult::AccountsLoaded(accounts) => {
                                this.pool.update_accounts(accounts.accounts);
                            }
                            DriftMonitorResult::Failed => {
                                debug!(target: "txpool", "Account reload failed");
                            }
                        }
                    }

                    state = PoolMaintainerState::Waiting;
                }

                PoolMaintainerState::ProcessingCanonEvent(ev) => {
                    if ev.is_none() {
                        // The stream ended, transition to Finished state
                        state = PoolMaintainerState::Finished;
                        continue;
                    }

                    this.drift_monitor.on_first_event();

                    if let Some(event) = ev {
                        // Process the event using the canon processor through the trait interface
                        let mut event_processor = this.canon_processor.process_event(
                            event,
                            &this.client,
                            &this.pool,
                            &mut this.drift_monitor,
                        );

                        // Poll the pinned future directly
                        match event_processor.as_mut().poll(cx) {
                            Poll::Ready(_) => {
                                state = PoolMaintainerState::Waiting;
                            }
                            Poll::Pending => {
                                // Store the in-progress future in the state
                                state = PoolMaintainerState::ProcessingCanonEventFuture(event_processor);
                                return Poll::Pending;
                            }
                        }
                    } else {
                        state = PoolMaintainerState::Waiting;
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

                    // Go back to waiting state
                    state = PoolMaintainerState::Waiting;
                }

                PoolMaintainerState::Finished => {
                    return Poll::Ready(());
                }
            }
        }
    }
}
