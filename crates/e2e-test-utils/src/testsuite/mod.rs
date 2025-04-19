//! Utilities for running e2e tests against a node or a network of nodes.

use crate::{
    testsuite::actions::{Action, ActionBox},
    NodeBuilderHelper, PayloadAttributesBuilder,
};
use alloy_primitives::B256;
use eyre::Result;
use jsonrpsee::http_client::{transport::HttpBackend, HttpClient};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_node_api::{TestNode, NodeTypes, PayloadTypes, FullNodeTypesAdapter, NodeTypesWithDBAdapter};
use reth_payload_builder::PayloadId;
use reth_rpc_layer::AuthClientService;
use setup::Setup;
use std::{collections::HashMap, marker::PhantomData};
pub mod actions;
pub mod setup;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use reth_rpc_api::EngineApiClient;
use alloy_rpc_types_engine::{ExecutionPayloadV3,PayloadStatus};
use eyre::eyre;
use tracing::error;
use futures_util::{future::BoxFuture,FutureExt};
use reth_provider::providers::BlockchainProvider;
use crate::test_utils::TmpDB;

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
    /// recently executed payload after a successful newPayloadV3 broadcast
    pub latest_payload_executed: Option<ExecutionPayloadV3>,
}

impl<I> Default for Environment<I> {
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
        }
    }
}

/// Builder for creating test scenarios
#[expect(missing_debug_implementations)]
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

type Engine = FullNodeTypesAdapter<TestNode, TmpDB, BlockchainProvider<NodeTypesWithDBAdapter<TestNode, TmpDB>>>;

impl NodeClient {
    pub async fn new_payload_v3_wait(
        &self,
        payload: ExecutionPayloadV3,
        versioned_hashes: Vec<B256>,
        parent_beacon_block_root: B256,
    ) -> eyre::Result<PayloadStatus> {

        let mut status = <HttpClient<AuthClientService<HttpBackend>> as EngineApiClient<Engine>>::new_payload_v3(
            &self.engine,
            payload.clone(),
            versioned_hashes.clone(),
            parent_beacon_block_root,
        ).await?;

        while !status.is_valid() {
            if status.is_invalid() {
                error!(
                    ?status,
                    ?payload,
                    ?versioned_hashes,
                    ?parent_beacon_block_root,
                    "Invalid newPayloadV3",
                );
                panic!("Invalid newPayloadV3: {status:?}");
            }

            if status.is_syncing() {
                return Err(eyre::eyre!("Payload syncing: no canonical state for parent"));
            }

            status = EngineApiClient::new_payload_v3(
                &self.engine,
                payload.clone(),
                versioned_hashes.clone(),
                parent_beacon_block_root,
            )
            .await?
        }

        Ok(status)
    }
}

pub struct BroadcastNextPayload {
    pub versioned_hashes: Vec<B256>,
    pub parent_beacon_block_root: B256,
}

impl<I: Send + 'static> Action<I> for BroadcastNextPayload {
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>> {
        async move {
            let payload = env
                .latest_payload_executed
                .clone()
                .ok_or_else(|| eyre!("No latest_payload_executed in env"))?;

            let mut valid_found = false;

            for client in &env.node_clients {
                let result = client
                    .new_payload_v3_wait(
                        payload.clone(),
                        self.versioned_hashes.clone(),
                        self.parent_beacon_block_root,
                    )
                    .await;

                match result {
                    Ok(status) if status.is_valid() => {
                        env.latest_payload_executed = Some(payload.clone());
                        valid_found = true;
                        break;
                    }
                    Ok(status) => {
                        tracing::warn!(?status, "Client did not return valid payload");
                    }
                    Err(err) => {
                        tracing::error!(?err, "Error during payload broadcast");
                    }
                }
            }

            if !valid_found {
                return Err(eyre!("No client responded with a valid payload"));
            }

            Ok(())
        }
        .boxed()
    }
}
