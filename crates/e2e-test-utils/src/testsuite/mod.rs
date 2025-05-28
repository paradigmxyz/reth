//! Utilities for running e2e tests against a node or a network of nodes.

use crate::{
    testsuite::actions::{Action, ActionBox},
    NodeBuilderHelper, PayloadAttributesBuilder,
};
use alloy_primitives::B256;
use eyre::Result;
use jsonrpsee::http_client::HttpClient;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_node_api::{EngineTypes, NodeTypes, PayloadTypes};
use reth_payload_builder::PayloadId;
use std::{collections::HashMap, marker::PhantomData};
pub mod actions;
pub mod setup;
use crate::testsuite::setup::Setup;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use reth_rpc_builder::auth::AuthServerHandle;

#[cfg(test)]
mod examples;

/// Client handles for both regular RPC and Engine API endpoints
#[derive(Debug)]
pub struct NodeClient {
    /// Regular JSON-RPC client
    pub rpc: HttpClient,
    /// Engine API client
    pub engine: AuthServerHandle,
}

impl NodeClient {
    /// Instantiates a new [`NodeClient`] with the given handles
    pub const fn new(rpc: HttpClient, engine: AuthServerHandle) -> Self {
        Self { rpc, engine }
    }
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
pub struct Environment<I>
where
    I: EngineTypes,
{
    /// Combined clients with both RPC and Engine API endpoints
    pub node_clients: Vec<NodeClient>,
    /// Tracks instance generic.
    _phantom: PhantomData<I>,
    /// Latest block information
    pub latest_block_info: Option<LatestBlockInfo>,
    /// Last producer index
    pub last_producer_idx: Option<usize>,
    /// Stores payload attributes indexed by block number
    pub payload_attributes: HashMap<u64, PayloadAttributes>,
    /// Tracks the latest block header timestamp
    pub latest_header_time: u64,
    /// Defines the increment for block timestamps (default: 2 seconds)
    pub block_timestamp_increment: u64,
    /// Stores payload IDs returned by block producers, indexed by block number
    pub payload_id_history: HashMap<u64, PayloadId>,
    /// Stores the next expected payload ID
    pub next_payload_id: Option<PayloadId>,
    /// Stores the latest fork choice state
    pub latest_fork_choice_state: ForkchoiceState,
    /// Stores the most recent built execution payload
    pub latest_payload_built: Option<PayloadAttributes>,
    /// Stores the most recent executed payload
    pub latest_payload_executed: Option<PayloadAttributes>,
    /// Stores the most recent built execution payload envelope
    pub latest_payload_envelope: Option<I::ExecutionPayloadEnvelopeV3>,
    /// Number of slots until a block is considered safe
    pub slots_to_safe: u64,
    /// Number of slots until a block is considered finalized
    pub slots_to_finalized: u64,
    /// Registry for tagged blocks, mapping tag names to block hashes
    pub block_registry: HashMap<String, B256>,
}

impl<I> Default for Environment<I>
where
    I: EngineTypes,
{
    fn default() -> Self {
        Self {
            node_clients: vec![],
            _phantom: Default::default(),
            latest_block_info: None,
            last_producer_idx: None,
            payload_attributes: Default::default(),
            latest_header_time: 0,
            block_timestamp_increment: 2,
            payload_id_history: HashMap::new(),
            next_payload_id: None,
            latest_fork_choice_state: ForkchoiceState::default(),
            latest_payload_built: None,
            latest_payload_executed: None,
            latest_payload_envelope: None,
            slots_to_safe: 0,
            slots_to_finalized: 0,
            block_registry: HashMap::new(),
        }
    }
}

/// Builder for creating test scenarios
#[expect(missing_debug_implementations)]
pub struct TestBuilder<I>
where
    I: EngineTypes,
{
    setup: Option<Setup<I>>,
    actions: Vec<ActionBox<I>>,
    env: Environment<I>,
}

impl<I> Default for TestBuilder<I>
where
    I: EngineTypes,
{
    fn default() -> Self {
        Self { setup: None, actions: Vec::new(), env: Default::default() }
    }
}

impl<I> TestBuilder<I>
where
    I: EngineTypes + 'static,
{
    /// Create a new test builder
    pub fn new() -> Self {
        Self::default()
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
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
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
