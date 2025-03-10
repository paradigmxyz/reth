//! Utilities for running e2e tests against a node or a network of nodes.

use crate::{
    testsuite::actions::{Action, ActionBox},
    Adapter, NodeBuilderHelper, PayloadAttributesBuilder, TmpDB, TmpNodeAdapter,
};
use eyre::Result;
use jsonrpsee::http_client::{transport::HttpBackend, HttpClient};
use reth_chainspec::ChainSpec;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::{NodePrimitives, NodeTypesWithEngine, PayloadTypes};
use reth_node_builder::{
    rpc::{EngineValidatorAddOn, RethRpcAddOns},
    NodeComponents, NodeComponentsBuilder, NodeTypesWithDBAdapter,
};
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_provider::providers::BlockchainProvider;
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
            From<EthPayloadBuilderAttributes>,
        N::ChainSpec: From<ChainSpec> + Clone,
    {
        let mut setup = self.setup.take();

        if let Some(ref mut s) = setup {
            s.apply::<N>(&mut self.env).await?;
        }

        let actions = std::mem::take(&mut self.actions);

        for action in actions {
            action.execute(&self.env).await?;
        }

        // explicitly drop the setup to shutdown the nodes
        // after all actions have completed
        drop(setup);

        Ok(())
    }
}
