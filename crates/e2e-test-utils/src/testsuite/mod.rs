//! Utilities for running e2e tests against a node or a network of nodes.

use crate::{
    testsuite::actions::{Action, ActionBox},
    NodeBuilderHelper, PayloadAttributesBuilder,
};
use alloy_primitives::B256;
use eyre::Result;
use jsonrpsee::http_client::{transport::HttpBackend, HttpClient};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_node_api::{NodeTypesWithEngine, PayloadTypes};
use reth_rpc_layer::AuthClientService;
use setup::Setup;
use std::marker::PhantomData;
pub mod actions;
pub mod setup;

#[cfg(test)]
mod examples;

/// Client handles for both regular RPC and Engine API endpoints
#[derive(Debug)]
pub struct NodeClient {
    /// Regular JSON-RPC client
    pub rpc: HttpClient,
    /// Engine API client
    pub engine: HttpClient<AuthClientService<HttpBackend>>,
}

/// Represents the latest block information.
#[derive(Debug, Clone)]
pub struct LatestBlockInfo {
    /// Hash of the latest block
    pub hash: B256,
    /// Number of the latest block
    pub number: u64,
}
/// Represents a test environment.
#[derive(Debug)]
pub struct Environment<I> {
    /// Combined clients with both RPC and Engine API endpoints
    pub node_clients: Vec<NodeClient>,
    /// Tracks instance generic.
    _phantom: PhantomData<I>,
    /// Latest block information
    pub latest_block_info: Option<LatestBlockInfo>,
    /// Last producer index
    pub last_producer_idx: Option<usize>,
}

impl<I> Default for Environment<I> {
    fn default() -> Self {
        Self {
            node_clients: vec![],
            _phantom: Default::default(),
            latest_block_info: None,
            last_producer_idx: None,
        }
    }
}

/// Builder for creating test scenarios
#[allow(missing_debug_implementations)]
#[derive(Default)]
pub struct TestBuilder<I> {
    setup: Option<Setup<I>>,
    actions: Vec<ActionBox<I>>,
    env: Environment<I>,
}

impl<I: 'static> TestBuilder<I> {
    /// Create a new test builder
    pub fn new() -> Self {
        Self { setup: None, actions: Vec::new(), env: Default::default() }
    }

    /// Set the test setup
    pub fn with_setup(mut self, setup: Setup<I>) -> Self {
        self.setup = Some(setup);
        self
    }

    /// Add an action to the test
    pub fn with_action<A>(mut self, action: A) -> Self
    where
        A: Action<I>,
    {
        self.actions.push(ActionBox::<I>::new(action));
        self
    }

    /// Add multiple actions to the test
    pub fn with_actions<II, A>(mut self, actions: II) -> Self
    where
        II: IntoIterator<Item = A>,
        A: Action<I>,
    {
        self.actions.extend(actions.into_iter().map(ActionBox::new));
        self
    }

    /// Run the test scenario
    pub async fn run<N>(mut self) -> Result<()>
    where
        N: NodeBuilderHelper,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadAttributes,
        >,
    {
        let mut setup = self.setup.take();

        if let Some(ref mut s) = setup {
            s.apply::<N>(&mut self.env).await?;
        }

        let actions = std::mem::take(&mut self.actions);

        for action in actions {
            action.execute(&mut self.env).await?;
        }

        // explicitly drop the setup to shutdown the nodes
        // after all actions have completed
        drop(setup);

        Ok(())
    }
}
