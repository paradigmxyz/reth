use crate::BackfillJob;
use std::{ops::RangeInclusive, time::Duration};

use alloy_primitives::BlockNumber;
use reth_node_api::FullNodeComponents;
use reth_prune_types::PruneModes;
use reth_stages_api::ExecutionStageThresholds;

use super::stream::DEFAULT_PARALLELISM;

/// Factory for creating new backfill jobs.
#[derive(Debug, Clone)]
pub struct BackfillJobFactory<E, P> {
    executor: E,
    provider: P,
    prune_modes: PruneModes,
    thresholds: ExecutionStageThresholds,
    stream_parallelism: usize,
}

impl<E, P> BackfillJobFactory<E, P> {
    /// Creates a new [`BackfillJobFactory`].
    pub fn new(executor: E, provider: P) -> Self {
        Self {
            executor,
            provider,
            prune_modes: PruneModes::none(),
            thresholds: ExecutionStageThresholds {
                // Default duration for a database transaction to be considered long-lived is
                // 60 seconds, so we limit the backfill job to the half of it to be sure we finish
                // before the warning is logged.
                //
                // See `reth_db::implementation::mdbx::tx::LONG_TRANSACTION_DURATION`.
                max_duration: Some(Duration::from_secs(30)),
                ..Default::default()
            },
            stream_parallelism: DEFAULT_PARALLELISM,
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

    /// Sets the stream parallelism.
    ///
    /// Configures the [`StreamBackfillJob`](super::stream::StreamBackfillJob) created via
    /// [`BackfillJob::into_stream`].
    pub const fn with_stream_parallelism(mut self, stream_parallelism: usize) -> Self {
        self.stream_parallelism = stream_parallelism;
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
            stream_parallelism: self.stream_parallelism,
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
