use crate::processor::EVMProcessor;
use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, PrunableBlockExecutor, StateProvider};
use reth_revm_database::StateProviderDatabase;
use reth_revm_inspectors::stack::{InspectorStack, InspectorStackConfig};
use std::sync::Arc;

/// Factory that spawn Executor.
#[derive(Clone, Debug)]
pub struct EVMProcessorFactory {
    chain_spec: Arc<ChainSpec>,
    stack: Option<InspectorStack>,
}

impl EVMProcessorFactory {
    /// Create new factory
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec, stack: None }
    }

    /// Sets the inspector stack for all generated executors.
    pub fn with_stack(mut self, stack: InspectorStack) -> Self {
        self.stack = Some(stack);
        self
    }

    /// Sets the inspector stack for all generated executors using the provided config.
    pub fn with_stack_config(mut self, config: InspectorStackConfig) -> Self {
        self.stack = Some(InspectorStack::new(config));
        self
    }

    /// Create executor with state provider.
    fn executor_with_state<'a, SP: StateProvider + 'a>(&'a self, sp: SP) -> EVMProcessor<'a> {
        let database_state = StateProviderDatabase::new(sp);
        let mut executor = EVMProcessor::new_with_db(self.chain_spec.clone(), database_state);
        if let Some(ref stack) = self.stack {
            executor.set_stack(stack.clone());
        }
        executor
    }
}

impl ExecutorFactory for EVMProcessorFactory {
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a> {
        Box::new(self.executor_with_state(sp))
    }

    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
