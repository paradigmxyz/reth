use crate::BackfillJob;
use std::ops::RangeInclusive;

use reth_node_api::FullNodeComponents;
use reth_primitives::BlockNumber;
use reth_prune_types::PruneModes;
use reth_stages_api::ExecutionStageThresholds;

/// Factory for creating new backfill jobs.
#[derive(Debug, Clone)]
pub struct BackfillJobFactory<E, P> {
    executor: E,
    provider: P,
    prune_modes: PruneModes,
    thresholds: ExecutionStageThresholds,
}

impl<E, P> BackfillJobFactory<E, P> {
    /// Creates a new [`BackfillJobFactory`].
    pub fn new(executor: E, provider: P) -> Self {
        Self {
            executor,
            provider,
            prune_modes: PruneModes::none(),
            thresholds: ExecutionStageThresholds::default(),
        }
    }

    /// Sets the prune modes
    pub fn with_prune_modes(mut self, prune_modes: PruneModes) -> Self {
        self.prune_modes = prune_modes;
        self
    }

    /// Sets the thresholds
    pub const fn with_thresholds(mut self, thresholds: ExecutionStageThresholds) -> Self {
        self.thresholds = thresholds;
        self
    }
}

impl<E: Clone, P: Clone> BackfillJobFactory<E, P> {
    /// Creates a new backfill job for the given range.
    pub fn backfill(&self, range: RangeInclusive<BlockNumber>) -> BackfillJob<E, P> {
        BackfillJob {
            executor: self.executor.clone(),
            provider: self.provider.clone(),
            prune_modes: self.prune_modes.clone(),
            range,
            thresholds: self.thresholds.clone(),
        }
    }
}

impl BackfillJobFactory<(), ()> {
    /// Creates a new [`BackfillJobFactory`] from [`FullNodeComponents`].
    pub fn new_from_components<Node: FullNodeComponents>(
        components: Node,
    ) -> BackfillJobFactory<Node::Executor, Node::Provider> {
        BackfillJobFactory::<_, _>::new(
            components.block_executor().clone(),
            components.provider().clone(),
        )
    }
}
