use alloc::boxed::Box;

use revm::{database::State, state::EvmState};

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

impl<DB: revm::Database> StateHookExt for State<DB> {
    fn set_reth_state_hook(&mut self, hook: Option<Box<dyn OnStateHook>>) {
        self.set_state_hook(
            hook.map(|hook| Box::new(RevmStateHook(hook)) as Box<dyn revm::OnStateHook>),
        );
    }
}

struct RevmStateHook(Box<dyn OnStateHook>);

impl revm::OnStateHook for RevmStateHook {
    fn on_state(&mut self, state: &EvmState) {
        self.0.on_state(state);
    }
}
