use alloc::boxed::Box;

use crate::Database;
use reth_execution_types::{EvmState, State};

/// A hook that is called when state changes are committed.
pub trait OnStateHook: Send + 'static {
    /// Invoked with the state being committed.
    fn on_state(&mut self, state: &EvmState);
}

impl<F> OnStateHook for F
where
    F: FnMut(&EvmState) + Send + 'static,
{
    fn on_state(&mut self, state: &EvmState) {
        self(state)
    }
}

/// Extension trait for installing reth state hooks on the database state.
pub trait StateHookExt {
    /// Sets the state hook.
    fn set_reth_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>);
}

impl<DB: Database> StateHookExt for State<DB> {
    fn set_reth_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.set_state_hook(hook.map(|hook| Box::new(RethStateHook(hook)) as _));
    }
}

struct RethStateHook(Box<dyn OnStateHook>);

impl reth_execution_types::OnStateHook for RethStateHook {
    fn on_state(&mut self, state: &EvmState) {
        self.0.on_state(state);
    }
}
