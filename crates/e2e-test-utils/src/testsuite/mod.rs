//! Utilities for running e2e tests against a node or a network of nodes.

use crate::{
    testsuite::actions::{Action, ActionBox},
    NodeBuilderHelper, NodeHelperType, PayloadAttributesBuilder,
};
use alloy_primitives::B256;
use eyre::Result;
use jsonrpsee::http_client::HttpClient;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_node_api::{EngineTypes, NodeTypes, PayloadTypes};
use reth_node_ethereum::EthereumNode;
use reth_payload_builder::PayloadId;
use reth_trie_common::updates::TrieUpdates;
use std::{collections::HashMap, marker::PhantomData, sync::Arc};
use tokio::sync::{mpsc, Mutex};

/// Type alias for shared trie updates storage
type SharedTrieUpdatesStorage = Vec<Arc<Mutex<HashMap<B256, TrieUpdates>>>>;
pub mod actions;
pub mod setup;
use crate::testsuite::setup::Setup;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use reth_engine_primitives::ConsensusEngineHandle;
use reth_rpc_builder::auth::AuthServerHandle;
use url::Url;

/// Event carrying trie updates from canonical state notifications
#[derive(Debug, Clone)]
pub struct TrieUpdateEvent {
    /// Index of the node that emitted this event
    pub node_idx: usize,
    /// Block hash associated with these trie updates
    pub block_hash: B256,
    /// The trie updates from the canonical chain
    pub trie_updates: TrieUpdates,
}

/// Client handles for both regular RPC and Engine API endpoints
#[derive(Clone)]
pub struct NodeClient<Payload>
where
    Payload: PayloadTypes,
{
    /// Regular JSON-RPC client
    pub rpc: HttpClient,
    /// Engine API client
    pub engine: AuthServerHandle,
    /// Beacon consensus engine handle for direct interaction with the consensus engine
    pub beacon_engine_handle: Option<ConsensusEngineHandle<Payload>>,
    /// Alloy provider for interacting with the node
    provider: Arc<dyn Provider + Send + Sync>,
}

impl<Payload> NodeClient<Payload>
where
    Payload: PayloadTypes,
{
    /// Instantiates a new [`NodeClient`] with the given handles and RPC URL
    pub fn new(rpc: HttpClient, engine: AuthServerHandle, url: Url) -> Self {
        let provider =
            Arc::new(ProviderBuilder::new().connect_http(url)) as Arc<dyn Provider + Send + Sync>;
        Self { rpc, engine, beacon_engine_handle: None, provider }
    }

    /// Instantiates a new [`NodeClient`] with the given handles, RPC URL, and beacon engine handle
    pub fn new_with_beacon_engine(
        rpc: HttpClient,
        engine: AuthServerHandle,
        url: Url,
        beacon_engine_handle: ConsensusEngineHandle<Payload>,
    ) -> Self {
        let provider =
            Arc::new(ProviderBuilder::new().connect_http(url)) as Arc<dyn Provider + Send + Sync>;
        Self { rpc, engine, beacon_engine_handle: Some(beacon_engine_handle), provider }
    }

    /// Get a block by number using the alloy provider
    pub async fn get_block_by_number(
        &self,
        number: alloy_eips::BlockNumberOrTag,
    ) -> Result<Option<alloy_rpc_types_eth::Block>> {
        self.provider
            .get_block_by_number(number)
            .await
            .map_err(|e| eyre::eyre!("Failed to get block by number: {}", e))
    }

    /// Check if the node is ready by attempting to get the latest block
    pub async fn is_ready(&self) -> bool {
        self.get_block_by_number(alloy_eips::BlockNumberOrTag::Latest).await.is_ok()
    }

    /// Get the alloy provider for direct access to node data
    pub fn provider(&self) -> Arc<dyn Provider + Send + Sync> {
        self.provider.clone()
    }
}

impl<Payload> std::fmt::Debug for NodeClient<Payload>
where
    Payload: PayloadTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeClient")
            .field("rpc", &self.rpc)
            .field("engine", &self.engine)
            .field("beacon_engine_handle", &self.beacon_engine_handle.is_some())
            .field("provider", &"<Provider>")
            .finish()
    }
}

/// Represents complete block information.
#[derive(Debug, Clone, Copy)]
pub struct BlockInfo {
    /// Hash of the block
    pub hash: B256,
    /// Number of the block
    pub number: u64,
    /// Timestamp of the block
    pub timestamp: u64,
}

/// Per-node state tracking for multi-node environments
#[derive(Clone)]
pub struct NodeState<I>
where
    I: EngineTypes,
{
    /// Current block information for this node
    pub current_block_info: Option<BlockInfo>,
    /// Stores payload attributes indexed by block number for this node
    pub payload_attributes: HashMap<u64, PayloadAttributes>,
    /// Tracks the latest block header timestamp for this node
    pub latest_header_time: u64,
    /// Stores payload IDs returned by this node, indexed by block number
    pub payload_id_history: HashMap<u64, PayloadId>,
    /// Stores the next expected payload ID for this node
    pub next_payload_id: Option<PayloadId>,
    /// Stores the latest fork choice state for this node
    pub latest_fork_choice_state: ForkchoiceState,
    /// Stores the most recent built execution payload for this node
    pub latest_payload_built: Option<PayloadAttributes>,
    /// Stores the most recent executed payload for this node
    pub latest_payload_executed: Option<PayloadAttributes>,
    /// Stores the most recent built execution payload envelope for this node
    pub latest_payload_envelope: Option<I::ExecutionPayloadEnvelopeV3>,
    /// Fork base block number for validation (if this node is currently on a fork)
    pub current_fork_base: Option<u64>,
    /// Stores trie updates for blocks by hash (for internal test verification)
    pub block_trie_updates: HashMap<B256, TrieUpdates>,
}

impl<I> Default for NodeState<I>
where
    I: EngineTypes,
{
    fn default() -> Self {
        Self {
            current_block_info: None,
            payload_attributes: HashMap::new(),
            latest_header_time: 0,
            payload_id_history: HashMap::new(),
            next_payload_id: None,
            latest_fork_choice_state: ForkchoiceState::default(),
            latest_payload_built: None,
            latest_payload_executed: None,
            latest_payload_envelope: None,
            current_fork_base: None,
            block_trie_updates: HashMap::new(),
        }
    }
}

impl<I> std::fmt::Debug for NodeState<I>
where
    I: EngineTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeState")
            .field("current_block_info", &self.current_block_info)
            .field("payload_attributes", &self.payload_attributes)
            .field("latest_header_time", &self.latest_header_time)
            .field("payload_id_history", &self.payload_id_history)
            .field("next_payload_id", &self.next_payload_id)
            .field("latest_fork_choice_state", &self.latest_fork_choice_state)
            .field("latest_payload_built", &self.latest_payload_built)
            .field("latest_payload_executed", &self.latest_payload_executed)
            .field("latest_payload_envelope", &"<ExecutionPayloadEnvelopeV3>")
            .field("current_fork_base", &self.current_fork_base)
            .field("block_trie_updates", &format!("<{} entries>", self.block_trie_updates.len()))
            .finish()
    }
}

/// Represents a test environment.
pub struct Environment<I>
where
    I: EngineTypes,
{
    /// Combined clients with both RPC and Engine API endpoints
    pub node_clients: Vec<NodeClient<I>>,
    /// Per-node state tracking
    pub node_states: Vec<NodeState<I>>,
    /// Tracks instance generic.
    _phantom: PhantomData<I>,
    /// Last producer index
    pub last_producer_idx: Option<usize>,
    /// Defines the increment for block timestamps (default: 2 seconds)
    pub block_timestamp_increment: u64,
    /// Number of slots until a block is considered safe
    pub slots_to_safe: u64,
    /// Number of slots until a block is considered finalized
    pub slots_to_finalized: u64,
    /// Registry for tagged blocks, mapping tag names to block info and node index
    pub block_registry: HashMap<String, (BlockInfo, usize)>,
    /// Currently active node index for backward compatibility with single-node actions
    pub active_node_idx: usize,
    /// Node contexts for accessing canonical streams (wrapped in `Arc<Mutex>` for sharing)
    pub node_contexts: Option<Arc<Mutex<Vec<NodeHelperType<EthereumNode>>>>>,
    /// Shared storage for trie updates captured from canonical state notifications
    pub shared_trie_updates: Option<SharedTrieUpdatesStorage>,
    /// Receiver for trie update events from canonical state notifications
    pub trie_update_rx: Option<mpsc::UnboundedReceiver<TrieUpdateEvent>>,
}

impl<I> std::fmt::Debug for Environment<I>
where
    I: EngineTypes,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("node_clients", &self.node_clients.len())
            .field("node_states", &self.node_states.len())
            .field("last_producer_idx", &self.last_producer_idx)
            .field("block_timestamp_increment", &self.block_timestamp_increment)
            .field("slots_to_safe", &self.slots_to_safe)
            .field("slots_to_finalized", &self.slots_to_finalized)
            .field("block_registry", &format!("<{} entries>", self.block_registry.len()))
            .field("active_node_idx", &self.active_node_idx)
            .field("node_contexts", &self.node_contexts.as_ref().map(|_| "<node contexts>"))
            .field(
                "shared_trie_updates",
                &self.shared_trie_updates.as_ref().map(|v| format!("<{} nodes>", v.len())),
            )
            .field("trie_update_rx", &self.trie_update_rx.as_ref().map(|_| "<receiver>"))
            .finish()
    }
}

impl<I> Default for Environment<I>
where
    I: EngineTypes,
{
    fn default() -> Self {
        Self {
            node_clients: vec![],
            node_states: vec![],
            _phantom: Default::default(),
            last_producer_idx: None,
            block_timestamp_increment: 2,
            slots_to_safe: 0,
            slots_to_finalized: 0,
            block_registry: HashMap::new(),
            active_node_idx: 0,
            node_contexts: None,
            shared_trie_updates: None,
            trie_update_rx: None,
        }
    }
}

impl<I> Environment<I>
where
    I: EngineTypes,
{
    /// Get the number of nodes in the environment
    pub const fn node_count(&self) -> usize {
        self.node_clients.len()
    }

    /// Get mutable reference to a specific node's state
    pub fn node_state_mut(&mut self, node_idx: usize) -> Result<&mut NodeState<I>, eyre::Error> {
        let node_count = self.node_count();
        self.node_states.get_mut(node_idx).ok_or_else(|| {
            eyre::eyre!("Node index {} out of bounds (have {} nodes)", node_idx, node_count)
        })
    }

    /// Get immutable reference to a specific node's state
    pub fn node_state(&self, node_idx: usize) -> Result<&NodeState<I>, eyre::Error> {
        self.node_states.get(node_idx).ok_or_else(|| {
            eyre::eyre!("Node index {} out of bounds (have {} nodes)", node_idx, self.node_count())
        })
    }

    /// Get the currently active node's state
    pub fn active_node_state(&self) -> Result<&NodeState<I>, eyre::Error> {
        self.node_state(self.active_node_idx)
    }

    /// Get mutable reference to the currently active node's state
    pub fn active_node_state_mut(&mut self) -> Result<&mut NodeState<I>, eyre::Error> {
        let idx = self.active_node_idx;
        self.node_state_mut(idx)
    }

    /// Set the active node index
    pub fn set_active_node(&mut self, node_idx: usize) -> Result<(), eyre::Error> {
        if node_idx >= self.node_count() {
            return Err(eyre::eyre!(
                "Node index {} out of bounds (have {} nodes)",
                node_idx,
                self.node_count()
            ));
        }
        self.active_node_idx = node_idx;
        Ok(())
    }

    /// Initialize node states when nodes are created
    pub fn initialize_node_states(&mut self, node_count: usize) {
        self.node_states = (0..node_count).map(|_| NodeState::default()).collect();
    }

    /// Set the node contexts (storing them for future access)
    pub fn set_node_contexts(&mut self, nodes: Vec<NodeHelperType<EthereumNode>>) {
        let node_contexts = Arc::new(Mutex::new(nodes));
        self.node_contexts = Some(node_contexts);
    }

    /// Drain all pending trie update events from the receiver and persist them to node states
    /// This should be called before asserting on trie updates to ensure all events are processed
    pub fn drain_trie_updates(&mut self) {
        let mut events = Vec::new();

        // First, collect all events without holding the borrow on self
        if let Some(rx) = &mut self.trie_update_rx {
            while let Ok(event) = rx.try_recv() {
                events.push(event);
            }
        }

        // Then, process each event
        for event in events {
            if event.node_idx < self.node_states.len() {
                self.node_states[event.node_idx]
                    .block_trie_updates
                    .insert(event.block_hash, event.trie_updates);
                tracing::debug!(
                    "Persisted trie updates for block {} from node {}",
                    event.block_hash,
                    event.node_idx
                );
            } else {
                tracing::warn!(
                    "Received trie update event for invalid node index: {}",
                    event.node_idx
                );
            }
        }
    }

    /// Get trie updates for a specific block on a specific node
    pub async fn get_trie_updates(
        &self,
        node_idx: usize,
        block_hash: alloy_primitives::B256,
    ) -> Option<TrieUpdates> {
        if let Some(shared_updates) = &self.shared_trie_updates {
            if let Some(node_updates) = shared_updates.get(node_idx) {
                let map = node_updates.lock().await;
                return map.get(&block_hash).cloned();
            }
        }
        None
    }

    /// Get current block info from active node
    pub fn current_block_info(&self) -> Option<BlockInfo> {
        self.active_node_state().ok()?.current_block_info
    }

    /// Set current block info on active node
    pub fn set_current_block_info(&mut self, block_info: BlockInfo) -> Result<(), eyre::Error> {
        self.active_node_state_mut()?.current_block_info = Some(block_info);
        Ok(())
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

    /// Set the test setup with chain import from RLP file
    pub fn with_setup_and_import(
        mut self,
        mut setup: Setup<I>,
        rlp_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        setup.import_rlp_path = Some(rlp_path.into());
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
        N: NodeBuilderHelper<Payload = I>,
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
