use crate::{pipeline::BoxedStage, MetricEventsSender, Pipeline, Stage, StageId, StageSet};
use alloy_primitives::{BlockNumber, B256};
use reth_provider::{providers::ProviderNodeTypes, DatabaseProviderFactory, ProviderFactory};
use reth_static_file::StaticFileProducer;
use tokio::sync::watch;

/// Builds a [`Pipeline`].
#[must_use = "call `build` to construct the pipeline"]
pub struct PipelineBuilder<Provider> {
    /// All configured stages in the order they will be executed.
    stages: Vec<BoxedStage<Provider>>,
    /// The maximum block number to sync to.
    max_block: Option<BlockNumber>,
    /// A receiver for the current chain tip to sync to.
    tip_tx: Option<watch::Sender<B256>>,
    metrics_tx: Option<MetricEventsSender>,
}

impl<Provider> PipelineBuilder<Provider> {
    /// Add a stage to the pipeline.
    pub fn add_stage<S>(mut self, stage: S) -> Self
    where
        S: Stage<Provider> + 'static,
    {
        self.stages.push(Box::new(stage));
        self
    }

    /// Add a set of stages to the pipeline.
    ///
    /// Stages can be grouped into a set by using a [`StageSet`].
    ///
    /// To customize the stages in the set (reorder, disable, insert a stage) call
    /// [`builder`][StageSet::builder] on the set which will convert it to a
    /// [`StageSetBuilder`][crate::StageSetBuilder].
    pub fn add_stages<Set: StageSet<Provider>>(mut self, set: Set) -> Self {
        let states = set.builder().build();
        self.stages.reserve_exact(states.len());
        for stage in states {
            self.stages.push(stage);
        }
        self
    }

    /// Set the target block.
    ///
    /// Once this block is reached, the pipeline will stop.
    pub const fn with_max_block(mut self, block: BlockNumber) -> Self {
        self.max_block = Some(block);
        self
    }

    /// Set the tip sender.
    pub fn with_tip_sender(mut self, tip_tx: watch::Sender<B256>) -> Self {
        self.tip_tx = Some(tip_tx);
        self
    }

    /// Set the metric events sender.
    pub fn with_metrics_tx(mut self, metrics_tx: MetricEventsSender) -> Self {
        self.metrics_tx = Some(metrics_tx);
        self
    }

    /// Builds the final [`Pipeline`] using the given database.
    pub fn build<N>(
        self,
        provider_factory: ProviderFactory<N>,
        static_file_producer: StaticFileProducer<ProviderFactory<N>>,
    ) -> Pipeline<N>
    where
        N: ProviderNodeTypes,
        ProviderFactory<N>: DatabaseProviderFactory<ProviderRW = Provider>,
    {
        let Self { stages, max_block, tip_tx, metrics_tx } = self;
        Pipeline {
            provider_factory,
            stages,
            max_block,
            static_file_producer,
            tip_tx,
            event_sender: Default::default(),
            progress: Default::default(),
            metrics_tx,
        }
    }
}

impl<Provider> Default for PipelineBuilder<Provider> {
    fn default() -> Self {
        Self { stages: Vec::new(), max_block: None, tip_tx: None, metrics_tx: None }
    }
}

impl<Provider> std::fmt::Debug for PipelineBuilder<Provider> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineBuilder")
            .field("stages", &self.stages.iter().map(|stage| stage.id()).collect::<Vec<StageId>>())
            .field("max_block", &self.max_block)
            .finish()
    }
}
