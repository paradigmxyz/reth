use crate::{segments::SegmentSet, Pruner};
use reth_chainspec::MAINNET;
use reth_config::PruneConfig;
use reth_db::transaction::DbTxMut;
use reth_exex_types::FinishedExExHeight;
use reth_provider::{
    providers::StaticFileProvider, BlockReader, DBProvider, DatabaseProviderFactory,
    PruneCheckpointWriter, StaticFileProviderFactory, TransactionsProvider,
};
use reth_prune_types::PruneModes;
use std::time::Duration;
use tokio::sync::watch;

/// Contains the information required to build a pruner
#[derive(Debug, Clone)]
pub struct PrunerBuilder {
    /// Minimum pruning interval measured in blocks.
    block_interval: usize,
    /// Pruning configuration for every part of the data that can be pruned.
    segments: PruneModes,
    /// The delete limit for pruner, per run.
    delete_limit: usize,
    /// Time a pruner job can run before timing out.
    timeout: Option<Duration>,
    /// The finished height of all `ExEx`'s.
    finished_exex_height: watch::Receiver<FinishedExExHeight>,
}

impl PrunerBuilder {
    /// Default timeout for a prune run.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);

    /// Creates a new [`PrunerBuilder`] from the given [`PruneConfig`].
    pub fn new(pruner_config: PruneConfig) -> Self {
        Self::default()
            .block_interval(pruner_config.block_interval)
            .segments(pruner_config.segments)
    }

    /// Sets the minimum pruning interval measured in blocks.
    pub const fn block_interval(mut self, block_interval: usize) -> Self {
        self.block_interval = block_interval;
        self
    }

    /// Sets the configuration for every part of the data that can be pruned.
    pub fn segments(mut self, segments: PruneModes) -> Self {
        self.segments = segments;
        self
    }

    /// Sets the delete limit for pruner, per run.
    pub const fn delete_limit(mut self, prune_delete_limit: usize) -> Self {
        self.delete_limit = prune_delete_limit;
        self
    }

    /// Sets the timeout for pruner, per run.
    ///
    /// CAUTION: Account and Storage History prune segments treat this timeout as a soft limit,
    /// meaning they can go beyond it.
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets the receiver for the finished height of all `ExEx`'s.
    pub fn finished_exex_height(
        mut self,
        finished_exex_height: watch::Receiver<FinishedExExHeight>,
    ) -> Self {
        self.finished_exex_height = finished_exex_height;
        self
    }

    /// Builds a [Pruner] from the current configuration with the given provider factory.
    pub fn build_with_provider_factory<PF>(self, provider_factory: PF) -> Pruner<PF::ProviderRW, PF>
    where
        PF: DatabaseProviderFactory<ProviderRW: PruneCheckpointWriter + BlockReader>
            + StaticFileProviderFactory,
    {
        let segments =
            SegmentSet::from_components(provider_factory.static_file_provider(), self.segments);

        Pruner::new_with_factory(
            provider_factory,
            segments.into_vec(),
            self.block_interval,
            self.delete_limit,
            self.timeout,
            self.finished_exex_height,
        )
    }

    /// Builds a [Pruner] from the current configuration with the given static file provider.
    pub fn build<Provider>(self, static_file_provider: StaticFileProvider) -> Pruner<Provider, ()>
    where
        Provider:
            DBProvider<Tx: DbTxMut> + BlockReader + PruneCheckpointWriter + TransactionsProvider,
    {
        let segments = SegmentSet::<Provider>::from_components(static_file_provider, self.segments);

        Pruner::new(
            segments.into_vec(),
            self.block_interval,
            self.delete_limit,
            self.timeout,
            self.finished_exex_height,
        )
    }
}

impl Default for PrunerBuilder {
    fn default() -> Self {
        Self {
            block_interval: 5,
            segments: PruneModes::none(),
            delete_limit: MAINNET.prune_delete_limit,
            timeout: None,
            finished_exex_height: watch::channel(FinishedExExHeight::NoExExs).1,
        }
    }
}
