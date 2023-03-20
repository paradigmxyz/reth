use crate::{
    database::{State, SubState},
    stack::{InspectorStack, InspectorStackConfig},
};
use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, StateProvider};

use crate::executor::Executor;
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
    type Executor<SP: StateProvider> = Executor<SP>;

    /// Executor with [`StateProvider`]
    fn with_sp<SP: StateProvider>(&self, sp: SP) -> Self::Executor<SP> {
        let substate = SubState::new(State::new(sp));

        let mut executor = Executor::new(self.chain_spec.clone(), substate);
        if let Some(ref stack) = self.stack {
            executor = executor.with_stack(stack.clone());
        }
        executor
    }

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
