use crate::{
    database::StateProviderDatabase,
    processor::EVMProcessor,
    stack::{InspectorStack, InspectorStackConfig},
};
use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, PrunableBlockExecutor, StateProvider};
use std::sync::Arc;

/// Factory that spawn Executor.
#[derive(Clone, Debug)]
pub struct Factory {
    chain_spec: Arc<ChainSpec>,
    stack: Option<InspectorStack>,
}

impl Factory {
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
}

impl ExecutorFactory for Factory {
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a> {
        let database_state = StateProviderDatabase::new(sp);
        let mut evm = Box::new(EVMProcessor::new_with_db(self.chain_spec.clone(), database_state));
        if let Some(ref stack) = self.stack {
            evm.set_stack(stack.clone());
        }
        evm
    }

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
