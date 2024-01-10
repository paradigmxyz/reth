use crate::{segments::SegmentSet, Pruner};
use reth_config::PruneConfig;
use reth_db::database::Database;
use reth_primitives::{PruneModes, MAINNET};
use reth_provider::ProviderFactory;

/// Contains the information required to build a pruner
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrunerBuilder {
    /// Minimum pruning interval measured in blocks.
    pub block_interval: usize,
    /// Pruning configuration for every part of the data that can be pruned.
    pub segments: PruneModes,
    /// The number of blocks that can be re-orged.
    pub max_reorg_depth: usize,
    /// The delete limit for pruner, per block. In the actual pruner run it will be multiplied by
    /// the amount of blocks between pruner runs to account for the difference in amount of new
    /// data coming in.
    pub prune_delete_limit: usize,
}

impl PrunerBuilder {
    /// Creates a new [PrunerBuilder] from the given [PruneConfig].
    pub fn new(pruner_config: PruneConfig) -> Self {
        PrunerBuilder::default()
            .block_interval(pruner_config.block_interval)
            .segments(pruner_config.segments)
    }

    /// Sets the minimum pruning interval measured in blocks.
    pub fn block_interval(mut self, block_interval: usize) -> Self {
        self.block_interval = block_interval;
        self
    }

    /// Sets the configuration for every part of the data that can be pruned.
    pub fn segments(mut self, segments: PruneModes) -> Self {
        self.segments = segments;
        self
    }

    /// Sets the number of blocks that can be re-orged.
    pub fn max_reorg_depth(mut self, max_reorg_depth: usize) -> Self {
        self.max_reorg_depth = max_reorg_depth;
        self
    }

    /// Sets the delete limit for pruner, per block.
    pub fn prune_delete_limit(mut self, prune_delete_limit: usize) -> Self {
        self.prune_delete_limit = prune_delete_limit;
        self
    }

    /// Builds a [Pruner] from the current configuration.
    pub fn build<DB: Database>(self, provider_factory: ProviderFactory<DB>) -> Pruner<DB> {
        let segments = SegmentSet::<DB>::from_prune_modes(self.segments);

        Pruner::new(
            provider_factory,
            segments.into_vec(),
            self.block_interval,
            self.prune_delete_limit,
            self.max_reorg_depth,
        )
    }
}

impl Default for PrunerBuilder {
    fn default() -> Self {
        Self {
            block_interval: 5,
            segments: PruneModes::none(),
            max_reorg_depth: 64,
            prune_delete_limit: MAINNET.prune_delete_limit,
        }
    }
}
