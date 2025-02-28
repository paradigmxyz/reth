//! Utilities for running e2e tests against a node or a network of nodes.

use actions::{Action, ActionBox, ActionResult};
use alloy_primitives::{B256, U256};
use alloy_rpc_types_engine::ExecutionPayload;
use assertions::{Assertion, AssertionBox};
use eyre::Result;
use setup::Setup;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

pub mod actions;
pub mod assertions;
pub mod setup;

#[cfg(test)]
mod examples;

/// A boxed future type for async trait methods
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A runner performs operations on an environment.
#[derive(Debug)]
pub struct Runner<I> {
    /// The environment containing the node(s) to test
    env: Environment<I>,
    /// Store action results for later assertions
    results: ResultStore,
}

impl<I: 'static> Runner<I> {
    /// Create a new test runner with the given environment
    pub fn new(instance: I) -> Self {
        Self { env: Environment { instance, ctx: () }, results: ResultStore::default() }
    }

    /// Execute an action and store its result
    pub async fn execute(&mut self, action: ActionBox<I>) -> Result<()> {
        let result = action.execute(&self.env).await?;

        match result {
            ActionResult::Payload(id, payload) => {
                self.results.store_payload(id, payload);
                Ok(())
            }
            ActionResult::BlockHash(id, hash) => {
                self.results.store_block_hash(id, hash);
                Ok(())
            }
            ActionResult::TransactionHash(id, hash) => {
                self.results.store_transaction_hash(id, hash);
                Ok(())
            }
            ActionResult::Value(id, value) => {
                self.results.store_value(id, value);
                Ok(())
            }
            ActionResult::Bool(id, value) => {
                self.results.store_bool(id, value);
                Ok(())
            }
            ActionResult::None => Ok(()),
        }
    }

    /// Run an assertion against the stored results
    pub async fn assert(&self, assertion: AssertionBox) -> Result<()> {
        assertion.assert(&self.results).await
    }

    /// Execute a sequence of actions
    pub async fn run_actions(&mut self, actions: Vec<ActionBox<I>>) -> Result<()> {
        for action in actions {
            self.execute(action).await?;
        }
        Ok(())
    }

    /// Run a complete test scenario with setup, actions, and assertions
    pub async fn run_scenario(
        &mut self,
        setup: Option<Setup>,
        actions: Vec<ActionBox<I>>,
        assertions: Vec<AssertionBox>,
    ) -> Result<()> {
        if let Some(setup) = setup {
            setup.apply(&mut self.env).await?;
        }

        for action in actions {
            self.execute(action).await?;
        }

        for assertion in assertions {
            self.assert(assertion).await?;
        }

        Ok(())
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

/// A store for action results that can be used in assertions
#[derive(Default, Debug)]
pub struct ResultStore {
    payloads: HashMap<String, Arc<ExecutionPayload>>,
    block_hashes: HashMap<String, B256>,
    transaction_hashes: HashMap<String, B256>,
    values: HashMap<String, U256>,
    bools: HashMap<String, bool>,
}

impl ResultStore {
    /// Store a payload with the given identifier
    pub fn store_payload(&mut self, id: String, payload: Arc<ExecutionPayload>) {
        self.payloads.insert(id, payload);
    }

    /// Store a block hash with the given identifier
    pub fn store_block_hash(&mut self, id: String, hash: B256) {
        self.block_hashes.insert(id, hash);
    }

    /// Store a transaction hash with the given identifier
    pub fn store_transaction_hash(&mut self, id: String, hash: B256) {
        self.transaction_hashes.insert(id, hash);
    }

    /// Store a generic value with the given identifier
    pub fn store_value(&mut self, id: String, value: U256) {
        self.values.insert(id, value);
    }

    /// Store a boolean result with the given identifier
    pub fn store_bool(&mut self, id: String, value: bool) {
        self.bools.insert(id, value);
    }

    /// Get a stored block hash by identifier
    pub fn get_block_hash(&self, id: &str) -> Option<&B256> {
        self.block_hashes.get(id)
    }

    /// Get a stored transaction hash by identifier
    pub fn get_transaction_hash(&self, id: &str) -> Option<&B256> {
        self.transaction_hashes.get(id)
    }

    /// Get a stored value by identifier
    pub fn get_value(&self, id: &str) -> Option<&U256> {
        self.values.get(id)
    }

    /// Get a stored boolean by identifier
    pub fn get_bool(&self, id: &str) -> Option<&bool> {
        self.bools.get(id)
    }
}

/// Builder for creating test scenarios
#[allow(missing_debug_implementations)]
pub struct TestBuilder<I> {
    instance: I,
    setup: Option<Setup>,
    actions: Vec<ActionBox<I>>,
    assertions: Vec<AssertionBox>,
}

impl<I: 'static> TestBuilder<I> {
    /// Create a new test builder
    pub fn new(instance: I) -> Self {
        Self { instance, setup: None, actions: Vec::new(), assertions: Vec::new() }
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

    /// Add an assertion to the test
    pub fn with_assertion<A>(mut self, assertion: A) -> Self
    where
        A: Assertion,
    {
        self.assertions.push(AssertionBox::new(assertion));
        self
    }

    /// Run the test scenario
    pub async fn run(self) -> Result<()> {
        let mut runner = Runner::new(self.instance);

        runner.run_scenario(self.setup, self.actions, self.assertions).await?;

        Ok(())
    }
}
