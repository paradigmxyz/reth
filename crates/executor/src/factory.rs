use reth_primitives::ChainSpec;
use reth_provider::{ExecutorFactory, StateProvider};
use reth_revm::database::{State, SubState};

use crate::executor::Executor;
use std::sync::Arc;

/// Factory that spawn Executor.
#[derive(Clone, Debug)]
pub struct Factory {
    chain_spec: Arc<ChainSpec>,
}

impl Factory {
    /// Create new factory
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl ExecutorFactory for Factory {
    type Executor<SP: StateProvider> = Executor<SP>;

    /// Executor with [`StateProvider`]
    fn with_sp<SP: StateProvider>(&self, sp: SP) -> Self::Executor<SP> {
        let substate = SubState::new(State::new(sp));
        Executor::new(self.chain_spec.clone(), substate)
    }

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec {
        self.chain_spec.as_ref()
    }
}
