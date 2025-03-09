//! Utilities for running e2e tests against a node or a network of nodes.

use std::marker::PhantomData;

use crate::{Adapter, NodeBuilderHelper, TmpDB, TmpNodeAdapter};
use actions::{Action, ActionBox};
use eyre::Result;
use jsonrpsee::http_client::{transport::HttpBackend, HttpClient};
use reth_chainspec::ChainSpec;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::NodePrimitives;
use reth_node_builder::{
    components::NodeComponentsBuilder,
    rpc::{EngineValidatorAddOn, RethRpcAddOns},
    Node, NodeComponents, NodeTypesWithDBAdapter, NodeTypesWithEngine, PayloadAttributesBuilder,
    PayloadTypes,
};
use reth_provider::providers::BlockchainProvider;
use reth_rpc_layer::AuthClientService;
use setup::Setup;

pub mod actions;
pub mod setup;

#[cfg(test)]
mod examples;

/// A runner performs operations on an environment.
#[derive(Debug, Default)]
pub struct Runner<I> {
    /// The environment containing the node(s) to test
    env: Environment<I>,
}

impl<I: 'static> Runner<I> {
    /// Create a new test runner with an empty environment
    pub fn new() -> Self {
        Self { env: Environment::default() }
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
    pub async fn run_scenario<N>(
        &mut self,
        setup: Option<Setup<I>>,
        actions: Vec<ActionBox<I>>,
    ) -> Result<()>
    where
        N: NodeBuilderHelper
            + Node<TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        N::Primitives: NodePrimitives<
            BlockHeader = alloy_consensus::Header,
            BlockBody = alloy_consensus::BlockBody<<N::Primitives as NodePrimitives>::SignedTx>,
        >,
        N::ComponentsBuilder: NodeComponentsBuilder<
            TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>,
            Components: NodeComponents<
                TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>,
                Network: PeersHandleProvider,
            >,
        >,
        N::AddOns: RethRpcAddOns<Adapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>
            + EngineValidatorAddOn<Adapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadAttributes,
        >,
        <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadBuilderAttributes:
            From<reth_payload_builder::EthPayloadBuilderAttributes>,
        N::ChainSpec: From<ChainSpec> + Clone,
    {
        // keep the setup object in scope for the entire function
        let mut setup_instance = None;

        if let Some(mut setup) = setup {
            setup.apply::<N>(&mut self.env).await?;
            setup_instance = Some(setup);
        }

        let result = self.run_actions(actions).await;

        // explicitly drop the setup_instance to shutdown the nodes
        // after all actions have completed
        drop(setup_instance);

        result
    }
}

/// Client handles for both regular RPC and Engine API endpoints
#[derive(Debug)]
pub struct NodeClient {
    /// Regular JSON-RPC client
    pub rpc: HttpClient,
    /// Engine API client
    pub engine: HttpClient<AuthClientService<HttpBackend>>,
}

/// Represents a test environment.
#[derive(Debug)]
pub struct Environment<I> {
    /// Combined clients with both RPC and Engine API endpoints
    pub node_clients: Vec<NodeClient>,
    /// Tracks instance generic.
    _phantom: PhantomData<I>,
}

impl<I> Default for Environment<I> {
    fn default() -> Self {
        Self { node_clients: vec![], _phantom: Default::default() }
    }
}

/// Builder for creating test scenarios
#[allow(missing_debug_implementations)]
#[derive(Default)]
pub struct TestBuilder<I> {
    setup: Option<Setup<I>>,
    actions: Vec<ActionBox<I>>,
}

impl<I: 'static> TestBuilder<I> {
    /// Create a new test builder
    pub fn new() -> Self {
        Self { setup: None, actions: Vec::new() }
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
    pub async fn run<N>(self) -> Result<()>
    where
        N: NodeBuilderHelper
            + Node<TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        N::Primitives: NodePrimitives<
            BlockHeader = alloy_consensus::Header,
            BlockBody = alloy_consensus::BlockBody<<N::Primitives as NodePrimitives>::SignedTx>,
        >,
        N::ComponentsBuilder: NodeComponentsBuilder<
            TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>,
            Components: NodeComponents<
                TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>,
                Network: PeersHandleProvider,
            >,
        >,
        N::AddOns: RethRpcAddOns<Adapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>
            + EngineValidatorAddOn<Adapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadAttributes,
        >,
        <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadBuilderAttributes:
            From<reth_payload_builder::EthPayloadBuilderAttributes>,
        N::ChainSpec: From<ChainSpec> + Clone,
    {
        let mut runner = Runner::new();
        runner.run_scenario::<N>(self.setup, self.actions).await
    }
}
