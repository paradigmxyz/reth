use crate::{
    database::StateProviderDatabase,
    processor::EVMProcessor,
    stack::{InspectorStack, InspectorStackConfig},
};
use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, PrunableBlockExecutor, StateProvider};
use std::sync::Arc;

/// Factory for creating [EVMProcessor].
#[derive(Clone, Debug)]
pub struct EvmProcessorFactory<Env> {
    chain_spec: Arc<ChainSpec>,
    stack: Option<InspectorStack>,
    /// Type that defines how the produced EVM should be configured.
    evm_config: Env,
}

impl<Env> EvmProcessorFactory<Env> {
    /// Create new factory
    pub fn new(chain_spec: Arc<ChainSpec>, evm_config: Env) -> Self {
        Self { chain_spec, stack: None, evm_config }
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

impl<Env> ExecutorFactory for EvmProcessorFactory<Env>
where
    Env: Send + Sync + Clone + 'static,
{
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a> {
        let database_state = StateProviderDatabase::new(sp);
        let mut evm = Box::new(EVMProcessor::new_with_db(
            self.chain_spec.clone(),
            database_state,
            self.evm_config.clone(),
        ));
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
