//! Utilities for running e2e tests against a node or a network of nodes.

use actions::{Action, ActionBox};
use eyre::Result;
use setup::Setup;

pub mod actions;
pub mod setup;

#[cfg(test)]
mod examples;

/// A runner performs operations on an environment.
#[derive(Debug)]
pub struct Runner<I> {
    /// The environment containing the node(s) to test
    env: Environment<I>,
}

impl<I: 'static> Runner<I> {
    /// Create a new test runner with the given environment
    pub fn new(instance: I) -> Self {
        Self { env: Environment { instance, ctx: () } }
    }

    /// Execute an action
    pub async fn execute(&mut self, action: ActionBox<I>) -> Result<()> {
        action.execute(&self.env).await
    }

    /// Execute a sequence of actions
    pub async fn run_actions(&mut self, actions: Vec<ActionBox<I>>) -> Result<()> {
        for action in actions {
            self.execute(action).await?;
        }
        Ok(())
    }

    /// Run a complete test scenario with setup and actions
    pub async fn run_scenario(
        &mut self,
        setup: Option<Setup>,
        actions: Vec<ActionBox<I>>,
    ) -> Result<()> {
        if let Some(setup) = setup {
            setup.apply(&mut self.env).await?;
        }

        self.run_actions(actions).await
    }
}

/// Represents a test environment.
#[derive(Debug)]
pub struct Environment<I> {
    /// The instance against which we can run tests.
    pub instance: I,
    /// Context.
    pub ctx: (),
}

/// Builder for creating test scenarios
#[allow(missing_debug_implementations)]
pub struct TestBuilder<I> {
    instance: I,
    setup: Option<Setup>,
    actions: Vec<ActionBox<I>>,
}

impl<I: 'static> TestBuilder<I> {
    /// Create a new test builder
    pub fn new(instance: I) -> Self {
        Self { instance, setup: None, actions: Vec::new() }
    }

    /// Set the test setup
    pub fn with_setup(mut self, setup: Setup) -> Self {
        self.setup = Some(setup);
        self
    }

    /// Add an action to the test
    pub fn with_action<A>(mut self, action: A) -> Self
    where
        A: Action<I>,
    {
        self.actions.push(ActionBox::new(action));
        self
    }

    /// Run the test scenario
    pub async fn run(self) -> Result<()> {
        let mut runner = Runner::new(self.instance);
        runner.run_scenario(self.setup, self.actions).await
    }
}
