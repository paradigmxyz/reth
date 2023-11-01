//! Factory for parallel EVM executor.
use crate::{executor::ParallelExecutor, queue::BlockQueueStore};
use reth_primitives::ChainSpec;
use reth_provider::{AsyncExecutorFactory, PrunableAsyncBlockExecutor, StateProvider};
use reth_revm_database::StateProviderDatabase;
use std::sync::Arc;

/// Factory to create parallel executor.
#[derive(Clone, Debug)]
pub struct ParallelExecutorFactory {
    chain_spec: Arc<ChainSpec>,
    queue_store: Arc<BlockQueueStore>,
}

impl ParallelExecutorFactory {
    /// Create new factory
    pub fn new(chain_spec: Arc<ChainSpec>, queue_store: Arc<BlockQueueStore>) -> Self {
        Self { chain_spec, queue_store }
    }
}

impl AsyncExecutorFactory for ParallelExecutorFactory {
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableAsyncBlockExecutor + 'a> {
        Box::new(
            ParallelExecutor::new(
                Arc::clone(&self.chain_spec),
                Arc::clone(&self.queue_store),
                Box::new(StateProviderDatabase::new(sp)),
                None,
            )
            .expect("success"), // TODO:
        )
    }

    fn chain_spec(&self) -> &ChainSpec {
        &self.chain_spec
    }
}
