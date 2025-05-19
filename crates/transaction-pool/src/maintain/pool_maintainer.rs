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
    pub fn build(self) -> PoolMaintainer<N, Client, P, St, Tasks> {
        // Use the default component factory to create standard components
        self.build_with_factory(super::interfaces::DefaultComponentFactory)
    }
}

/// The main pool maintainer
#[derive(Debug)]
pub struct PoolMaintainer<N, Client, P, St, Tasks, M = DriftMonitor, C = CanonEventProcessor<N>>
where
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

    // To flag if we're processing stale transactions
    processing_stale: bool,

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
            processing_stale: false,
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

    /// Handle a canon event
    fn handle_canon_event(&mut self, event: CanonStateNotification<N>) {
        let event_clone = event.clone();

        let client = self.client.clone();
        let pool = self.pool.clone();
        let mut drift_monitor = self.drift_monitor.clone();
        let mut canon_processor = self.canon_processor.clone();

        // spawn a task to handle the async processing
        self.task_spawner.spawn(Box::pin(async move {
            canon_processor.process_event(event_clone, &client, &pool, &mut drift_monitor).await;
        }));

        // for commit events, marking the accounts as dirty is synchronous
        if let CanonStateNotification::Commit { new } = &event {
            let (blocks, _) = new.inner();
            let block = blocks.tip();
            // Mark affected accounts as dirty for reload
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

        // If we're marked as processing stale, run the stale processing
        if this.processing_stale {
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

            // Clear the stale processing flag
            this.processing_stale = false;
        }

        trace!(target: "txpool", state=?this.drift_monitor.state(), "awaiting new block or reorg");

        this.drift_monitor.update_metrics();
        let pool_info = this.pool.block_info();

        // If drifted, mark all senders as dirty
        if this.drift_monitor.state().is_drifted() {
            this.drift_monitor.set_dirty_addresses(this.pool.unique_senders());
            this.drift_monitor.set_state(PoolDriftState::InSync);
        }

        // Start reloading accounts if needed
        if this.drift_monitor.has_dirty_addresses() && !this.drift_monitor.is_reloading() {
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
            // We can process the result immediately
            if let Poll::Ready(result) = drift_poll {
                match result {
                    DriftMonitorResult::AccountsLoaded(accounts) => {
                        this.pool.update_accounts(accounts.accounts);
                    }
                    DriftMonitorResult::Failed => {
                        debug!(target: "txpool", "Account reload failed");
                    }
                }
            }
            // Don't need to return here, can continue with the rest of processing
        }

        // Poll events stream
        let events_poll = Pin::new(&mut this.events).poll_next(cx);
        if let Poll::Ready(ev) = events_poll {
            if ev.is_none() {
                // The stream ended
                return Poll::Ready(());
            }

            this.drift_monitor.on_first_event();

            if let Some(event) = ev {
                // Process the event immediately
                this.handle_canon_event(event);
            }
        }

        // Poll interval for stale eviction
        let interval_poll = Pin::new(&mut this.stale_eviction_interval).poll_tick(cx);
        if interval_poll.is_ready() {
            // Mark that we're going to process stale transactions
            this.processing_stale = true;
        }

        // Nothing is ready, return pending
        Poll::Pending
    }
}
