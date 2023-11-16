//! EVM execution node arguments

use clap::Args;
use reth_primitives::ChainSpec;
use reth_provider::{BlockReader, PrunableBlockRangeExecutor, RangeExecutorFactory};
use reth_revm::{
    parallel::{factory::ParallelExecutorFactory, queue::TransitionQueueStore},
    EVMProcessorFactory,
};
use reth_tasks::TaskSpawner;
use std::{path::PathBuf, sync::Arc};

/// Parameters for EVM execution
#[derive(Debug, Args, PartialEq, Default)]
#[command(next_help_heading = "Execution")]
pub struct ExecutionArgs {
    /// Run historical execution in parallel.
    #[arg(long = "execution.parallel", default_value_t = false)]
    pub parallel: bool,

    /// Path to the block queues for parallel execution.
    #[arg(long = "execution.parallel-queue-store", required_if_eq("parallel", "true"))]
    pub queue_store: Option<PathBuf>,

    /// Gas threshold for executing in parallel.
    #[arg(long = "execution.gas-threshold", default_value_t = 1_000_000)]
    pub gas_threshold: u64,

    /// Batch size threshold for executing in parallel.
    #[arg(long = "execution.batch-size-threshold", default_value_t = 100)]
    pub batch_size_threshold: u64,
}

impl ExecutionArgs {
    /// Returns executor factory to be used in historical sync.
    pub fn pipeline_executor_factory(
        &self,
        chain_spec: Arc<ChainSpec>,
        task_spawner: Box<dyn TaskSpawner>,
    ) -> eyre::Result<EitherExecutorFactory<EVMProcessorFactory, ParallelExecutorFactory>> {
        let factory = if self.parallel {
            EitherExecutorFactory::Right(ParallelExecutorFactory::new(
                chain_spec,
                task_spawner,
                Arc::new(TransitionQueueStore::new(self.queue_store.clone().expect("is set"))),
                self.gas_threshold,
                self.batch_size_threshold,
            ))
        } else {
            EitherExecutorFactory::Left(EVMProcessorFactory::new(chain_spec))
        };
        Ok(factory)
    }
}

/// A type that represents one of two possible executor factories.
#[derive(Debug, Clone)]
pub enum EitherExecutorFactory<A, B> {
    /// The first factory variant
    Left(A),
    /// The second factory variant
    Right(B),
}

impl<A, B> RangeExecutorFactory for EitherExecutorFactory<A, B>
where
    A: RangeExecutorFactory,
    B: RangeExecutorFactory,
{
    fn chain_spec(&self) -> &ChainSpec {
        match self {
            EitherExecutorFactory::Left(a) => a.chain_spec(),
            EitherExecutorFactory::Right(b) => b.chain_spec(),
        }
    }

    fn with_provider_and_state<'a, Provider, SP>(
        &'a self,
        provider: Provider,
        sp: SP,
    ) -> Box<dyn PrunableBlockRangeExecutor + 'a>
    where
        Provider: BlockReader + 'a,
        SP: reth_provider::StateProvider + 'a,
    {
        match self {
            EitherExecutorFactory::Left(a) => a.with_provider_and_state(provider, sp),
            EitherExecutorFactory::Right(b) => b.with_provider_and_state(provider, sp),
        }
    }
}
