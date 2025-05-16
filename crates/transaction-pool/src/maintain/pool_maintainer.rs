use crate::{
    blobstore::BlobStoreUpdates,
    maintain::{
        drift_monitor::{DriftMonitor, DriftMonitorResult},
        CanonEventProcessor, CanonEventProcessorConfig, LoadedAccounts, MaintainPoolConfig,
        PoolDriftState,
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

/// Hook that takes a drift monitor reference
type DriftMonitorHook = Box<dyn FnMut(&DriftMonitor) + Send>;

/// Hook that takes loaded accounts and pool references
type AccountsLoadedHook<P> = Box<dyn FnMut(&LoadedAccounts, &P) + Send>;

/// Hook that takes canon event, pool, and drift monitor references
type CanonEventHook<N, P> =
    Box<dyn FnMut(&CanonStateNotification<N>, &P, &mut DriftMonitor) + Send>;

/// Hook that takes a size parameter
type SizeHook = Box<dyn FnMut(usize) + Send>;

/// Hook that takes transaction hashes and pool reference
type TxHashesHook<P> = Box<dyn FnMut(&[alloy_primitives::TxHash], &P) + Send>;

/// Hook that takes drift monitor and pool references
type LoopIterationHook<P> = Box<dyn FnMut(&DriftMonitor, &P) + Send>;

/// Event hooks for customizing pool maintenance behavior
pub struct PoolMaintainerHooks<N, P>
where
    N: NodePrimitives,
    P: TransactionPoolExt,
{
    /// Called when drift is detected (pool is out of sync with state)
    pub on_drift_detected: Option<DriftMonitorHook>,

    /// Called when accounts are successfully loaded from state
    pub on_accounts_loaded: Option<AccountsLoadedHook<P>>,

    /// Called when account reload fails
    pub on_accounts_failed: Option<DriftMonitorHook>,

    /// Called when a canonical state event is received
    pub on_canon_event: Option<CanonEventHook<N, P>>,

    /// Called before processing stale transaction eviction
    pub on_stale_eviction_start: Option<SizeHook>,

    /// Called after stale transactions are removed
    pub on_stale_eviction_complete: Option<TxHashesHook<P>>,

    /// Called when blob store cleanup occurs
    pub on_blob_cleanup: Option<SizeHook>,

    /// Called on each loop iteration with current state
    pub on_loop_iteration: Option<LoopIterationHook<P>>,
}

impl<N, P> Default for PoolMaintainerHooks<N, P>
where
    N: NodePrimitives,
    P: TransactionPoolExt,
{
    fn default() -> Self {
        Self {
            on_drift_detected: None,
            on_accounts_loaded: None,
            on_accounts_failed: None,
            on_canon_event: None,
            on_stale_eviction_start: None,
            on_stale_eviction_complete: None,
            on_blob_cleanup: None,
            on_loop_iteration: None,
        }
    }
}

impl<N, P> std::fmt::Debug for PoolMaintainerHooks<N, P>
where
    N: NodePrimitives,
    P: TransactionPoolExt,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PoolMaintainerHooks")
            .field("on_drift_detected", &self.on_drift_detected.as_ref().map(|_| "Fn"))
            .field("on_accounts_loaded", &self.on_accounts_loaded.as_ref().map(|_| "Fn"))
            .field("on_accounts_failed", &self.on_accounts_failed.as_ref().map(|_| "Fn"))
            .field("on_canon_event", &self.on_canon_event.as_ref().map(|_| "Fn"))
            .field("on_stale_eviction_start", &self.on_stale_eviction_start.as_ref().map(|_| "Fn"))
            .field(
                "on_stale_eviction_complete",
                &self.on_stale_eviction_complete.as_ref().map(|_| "Fn"),
            )
            .field("on_blob_cleanup", &self.on_blob_cleanup.as_ref().map(|_| "Fn"))
            .field("on_loop_iteration", &self.on_loop_iteration.as_ref().map(|_| "Fn"))
            .finish()
    }
}

/// Builder for creating a customizable pool maintainer
#[derive(Debug)]
pub struct PoolMaintainerBuilder<N, Client, P, St, Tasks>
where
    N: NodePrimitives,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
{
    client: Client,
    pool: P,
    events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,
    hooks: PoolMaintainerHooks<N, P>,
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
    pub fn new(
        client: Client,
        pool: P,
        events: St,
        task_spawner: Tasks,
        config: MaintainPoolConfig,
    ) -> Self {
        Self {
            client,
            pool,
            events,
            task_spawner,
            config,
            hooks: PoolMaintainerHooks::default(),
            _phantom: std::marker::PhantomData,
        }
    }

    /// Set a hook that's called when drift is detected
    pub fn on_drift_detected<F>(mut self, hook: F) -> Self
    where
        F: FnMut(&DriftMonitor) + Send + 'static,
    {
        self.hooks.on_drift_detected = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called when accounts are successfully loaded
    pub fn on_accounts_loaded<F>(mut self, hook: F) -> Self
    where
        F: FnMut(&LoadedAccounts, &P) + Send + 'static,
    {
        self.hooks.on_accounts_loaded = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called when account reload fails
    pub fn on_accounts_failed<F>(mut self, hook: F) -> Self
    where
        F: FnMut(&DriftMonitor) + Send + 'static,
    {
        self.hooks.on_accounts_failed = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called for each canonical state event
    pub fn on_canon_event<F>(mut self, hook: F) -> Self
    where
        F: FnMut(&CanonStateNotification<N>, &P, &mut DriftMonitor) + Send + 'static,
    {
        self.hooks.on_canon_event = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called before stale transaction eviction
    pub fn on_stale_eviction_start<F>(mut self, hook: F) -> Self
    where
        F: FnMut(usize) + Send + 'static,
    {
        self.hooks.on_stale_eviction_start = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called after stale transactions are removed
    pub fn on_stale_eviction_complete<F>(mut self, hook: F) -> Self
    where
        F: FnMut(&[alloy_primitives::TxHash], &P) + Send + 'static,
    {
        self.hooks.on_stale_eviction_complete = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called on blob store cleanup
    pub fn on_blob_cleanup<F>(mut self, hook: F) -> Self
    where
        F: FnMut(usize) + Send + 'static,
    {
        self.hooks.on_blob_cleanup = Some(Box::new(hook));
        self
    }

    /// Set a hook that's called on each loop iteration
    pub fn on_loop_iteration<F>(mut self, hook: F) -> Self
    where
        F: FnMut(&DriftMonitor, &P) + Send + 'static,
    {
        self.hooks.on_loop_iteration = Some(Box::new(hook));
        self
    }

    /// Build the pool maintainer with the configured hooks
    pub fn build(self) -> PoolMaintainer<N, Client, P, St, Tasks> {
        PoolMaintainer::new_with_hooks(
            self.client,
            self.pool,
            self.events,
            self.task_spawner,
            self.config,
            self.hooks,
        )
    }
}

/// The main pool maintainer
#[derive(Debug)]
pub struct PoolMaintainer<N: NodePrimitives, Client, P: TransactionPoolExt, St, Tasks> {
    client: Client,
    pool: P,
    events: St,
    task_spawner: Tasks,
    config: MaintainPoolConfig,

    // Internal state
    drift_monitor: DriftMonitor,
    canon_processor: CanonEventProcessor<N>,
    stale_eviction_interval: time::Interval,
    hooks: PoolMaintainerHooks<N, P>,

    // Initialization flag
    initialized: bool,

    _phantom: std::marker::PhantomData<N>,
}

impl<N, Client, P, St, Tasks> PoolMaintainer<N, Client, P, St, Tasks>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
{
    fn new_with_hooks(
        client: Client,
        pool: P,
        events: St,
        task_spawner: Tasks,
        config: MaintainPoolConfig,
        hooks: PoolMaintainerHooks<N, P>,
    ) -> Self {
        let metrics = MaintainPoolMetrics::default();
        let MaintainPoolConfig { max_reload_accounts, max_update_depth, .. } = config;

        let drift_monitor = DriftMonitor::new(max_reload_accounts, metrics.clone());
        let canon_processor_config = CanonEventProcessorConfig { max_update_depth };
        let canon_processor = CanonEventProcessor::new(
            client.finalized_block_number().ok().flatten(),
            canon_processor_config,
            metrics,
        );
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
            hooks,
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

impl<N, Client, P, St, Tasks> Future for PoolMaintainer<N, Client, P, St, Tasks>
where
    N: NodePrimitives,
    Client: StateProviderFactory + BlockReaderIdExt + ChainSpecProvider + Clone + Unpin + 'static,
    P: TransactionPoolExt<Transaction: PoolTransaction<Consensus = N::SignedTx>> + Unpin + 'static,
    St: Stream<Item = CanonStateNotification<N>> + Send + Unpin + 'static,
    Tasks: TaskSpawner + 'static,
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

                    if let Some(ref mut hook) = this.hooks.on_loop_iteration {
                        hook(&this.drift_monitor, &this.pool);
                    }

                    this.drift_monitor.update_metrics();
                    let pool_info = this.pool.block_info();

                    if this.drift_monitor.state().is_drifted() {
                        if let Some(ref mut hook) = this.hooks.on_drift_detected {
                            hook(&this.drift_monitor);
                        }

                        this.drift_monitor.set_dirty_addresses(this.pool.unique_senders());
                        this.drift_monitor.set_state(PoolDriftState::InSync);
                    }

                    if this.drift_monitor.has_dirty_addresses() &&
                        !this.drift_monitor.is_reloading()
                    {
                        this.drift_monitor.start_reload_accounts(
                            this.client.clone(),
                            pool_info.last_seen_block_hash,
                            &this.task_spawner,
                        );
                    }

                    if let Some(BlobStoreUpdates::Finalized(blobs)) =
                        this.canon_processor.update_finalized(&this.client)
                    {
                        if let Some(ref mut hook) = this.hooks.on_blob_cleanup {
                            hook(blobs.len());
                        }

                        this.pool.delete_blobs(blobs);
                        let pool = this.pool.clone();
                        this.task_spawner.spawn_blocking(Box::pin(async move {
                            debug!(target: "txpool", "cleaning up blob store");
                            pool.cleanup_blobs();
                        }));
                    }

                    // poll drift monitor
                    let drift_poll = Pin::new(&mut this.drift_monitor).poll(cx);
                    if drift_poll.is_ready() {
                        state = PoolMaintainerState::ProcessingDriftMonitor;
                        continue;
                    }

                    // poll events stream
                    let events_poll = Pin::new(&mut this.events).poll_next(cx);
                    match events_poll {
                        Poll::Ready(ev) => {
                            state = PoolMaintainerState::ProcessingCanonEvent(ev);
                            continue;
                        }
                        Poll::Pending => {}
                    }

                    // poll interval for stale eviction
                    let interval_poll = Pin::new(&mut this.stale_eviction_interval).poll_tick(cx);
                    if interval_poll.is_ready() {
                        state = PoolMaintainerState::ProcessingStaleEviction;
                        continue;
                    }

                    // nothing is ready, return pending
                    return Poll::Pending;
                }

                PoolMaintainerState::ProcessingDriftMonitor => {
                    // we know the drift monitor is ready at this point
                    if let Poll::Ready(result) = Pin::new(&mut this.drift_monitor).poll(cx) {
                        match result {
                            DriftMonitorResult::AccountsLoaded(accounts) => {
                                if let Some(ref mut hook) = this.hooks.on_accounts_loaded {
                                    hook(&accounts, &this.pool);
                                }
                                this.pool.update_accounts(accounts.accounts);
                            }
                            DriftMonitorResult::Failed => {
                                if let Some(ref mut hook) = this.hooks.on_accounts_failed {
                                    hook(&this.drift_monitor);
                                }
                                debug!(target: "txpool", dirty_addresses=%this.drift_monitor.dirty_address_count(), "Account reload failed");
                            }
                        }
                    }

                    state = PoolMaintainerState::Waiting;
                }

                PoolMaintainerState::ProcessingCanonEvent(ev) => {
                    if ev.is_none() {
                        // the stream ended, transition to Finished state
                        state = PoolMaintainerState::Finished;
                        continue;
                    }

                    this.drift_monitor.on_first_event();

                    if let Some(event) = ev {
                        if let Some(ref mut hook) = this.hooks.on_canon_event {
                            hook(&event, &this.pool, &mut this.drift_monitor);
                        }

                        // manual implementation of processing the event with CanonProcessor
                        // box the future to make it Unpin
                        let mut event_processor = Box::pin(this.canon_processor.process_event(
                            event,
                            &this.client,
                            &this.pool,
                            &mut this.drift_monitor,
                        ));

                        // poll the pinned future directly
                        match event_processor.as_mut().poll(cx) {
                            Poll::Ready(_) => {
                                state = PoolMaintainerState::Waiting;
                            }
                            Poll::Pending => {
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

                    if let Some(ref mut hook) = this.hooks.on_stale_eviction_start {
                        hook(stale_txs.len());
                    }

                    debug!(target: "txpool", count=%stale_txs.len(), "removing stale transactions");
                    this.pool.remove_transactions(stale_txs.clone());

                    if let Some(ref mut hook) = this.hooks.on_stale_eviction_complete {
                        hook(&stale_txs, &this.pool);
                    }

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
