//! Actions that can be performed in tests.

use crate::testsuite::Environment;
use alloy_primitives::{Address, Bytes, B256};
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes, PayloadStatusEnum};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::EngineTypes;
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::{future::Future, marker::PhantomData};
use tracing::debug;

/// An action that can be performed on an instance.
///
/// Actions execute operations and potentially make assertions in a single step.
/// The action name indicates what it does (e.g., `AssertMineBlock` would both
/// mine a block and assert it worked).
pub trait Action<I>: Send + 'static {
    /// Executes the action
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>>;
}

/// Simplified action container for storage in tests
#[allow(missing_debug_implementations)]
pub struct ActionBox<I>(Box<dyn Action<I>>);

impl<I: 'static> ActionBox<I> {
    /// Constructor for [`ActionBox`].
    pub fn new<A: Action<I>>(action: A) -> Self {
        Self(Box::new(action))
    }

    /// Executes an [`ActionBox`] with the given [`Environment`] reference.
    pub async fn execute(mut self, env: &mut Environment<I>) -> Result<()> {
        self.0.execute(env).await
    }
}

/// Implementation of `Action` for any function/closure that takes an Environment
/// reference and returns a Future resolving to Result<()>.
///
/// This allows using closures directly as actions with `.with_action(async move |env| {...})`.
impl<I, F, Fut> Action<I> for F
where
    F: FnMut(&Environment<I>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(self(env))
    }
}

/// Mine a single block with the given transactions and verify the block was created
/// successfully.
#[derive(Debug)]
pub struct AssertMineBlock<Engine> {
    /// The node index to mine
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Bytes>,
    /// Expected block hash (optional)
    pub expected_hash: Option<B256>,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> AssertMineBlock<Engine> {
    /// Create a new `AssertMineBlock` action
    pub fn new(node_idx: usize, transactions: Vec<Bytes>, expected_hash: Option<B256>) -> Self {
        Self { node_idx, transactions, expected_hash, _phantom: Default::default() }
    }
}

impl<Engine> Action<Engine> for AssertMineBlock<Engine>
where
    Engine: EngineTypes,
    Engine::PayloadAttributes: From<PayloadAttributes>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index out of bounds: {}", self.node_idx));
            }

            let node_client = &env.node_clients[self.node_idx];
            let rpc_client = &node_client.rpc;
            let engine_client = &node_client.engine;

            // get the latest block to use as parent
            let latest_block =
                EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                    rpc_client,
                    alloy_eips::BlockNumberOrTag::Latest,
                    false,
                )
                .await?;

            let latest_block = latest_block.ok_or_else(|| eyre::eyre!("Latest block not found"))?;
            let parent_hash = latest_block.header.hash;

            debug!("Latest block hash: {parent_hash}");

            // create a simple forkchoice state with the latest block as head
            let fork_choice_state = ForkchoiceState {
                head_block_hash: parent_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            // create payload attributes for the new block
            let payload_attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::random(),
                suggested_fee_recipient: Address::random(),
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            let engine_payload_attributes: Engine::PayloadAttributes = payload_attributes.into();

            let fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v3(
                engine_client,
                fork_choice_state,
                Some(engine_payload_attributes),
            )
            .await?;

            debug!("FCU result: {:?}", fcu_result);

            // check if we got a valid payload ID
            match fcu_result.payload_status.status {
                PayloadStatusEnum::Valid => {
                    if let Some(payload_id) = fcu_result.payload_id {
                        debug!("Got payload ID: {payload_id}");

                        // get the payload that was built
                        let _engine_payload =
                            EngineApiClient::<Engine>::get_payload_v3(engine_client, payload_id)
                                .await?;
                        Ok(())
                    } else {
                        Err(eyre::eyre!("No payload ID returned from forkchoiceUpdated"))
                    }
                }
                _ => Err(eyre::eyre!("Payload status not valid: {:?}", fcu_result.payload_status)),
            }
        })
    }
}
/// Pick the next block producer based on the latest block information.
#[derive(Debug)]
pub struct PickNextBlockProducer<Engine> {
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> Default for PickNextBlockProducer<Engine> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Engine> PickNextBlockProducer<Engine> {
    /// Create a new `PickNextBlockProducer` action
    pub fn new() -> Self {
        Self { _phantom: Default::default() }
    }
}

impl<Engine> Action<Engine> for PickNextBlockProducer<Engine>
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let num_clients = env.node_clients.len();
            if num_clients == 0 {
                return Err(eyre::eyre!("No node clients available"));
            }

            let latest_info = env
                .latest_block_info
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;

            // Calculate the starting index based on the latest block number
            let start_idx = ((latest_info.number + 1) % num_clients as u64) as usize;

            for i in 0..num_clients {
                let idx = (start_idx + i) % num_clients;
                let node_client = &env.node_clients[idx];
                let rpc_client = &node_client.rpc;

                let latest_block =
                    EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                        rpc_client,
                        alloy_eips::BlockNumberOrTag::Latest,
                        false,
                    )
                    .await?;

                if let Some(block) = latest_block {
                    let block_number = block.header.number;
                    let block_hash = block.header.hash;

                    // Check if the block hash and number match the latest block info
                    if block_hash == latest_info.hash && block_number == latest_info.number {
                        env.last_producer_idx = Some(idx);
                        debug!("Selected node {} as the next block producer", idx);
                        return Ok(());
                    }
                }
            }

            Err(eyre::eyre!("No suitable block producer found"))
        })
    }
}
/// Run a sequence of actions in series.
#[allow(missing_debug_implementations)]
pub struct Sequence<I> {
    /// Actions to execute in sequence
    pub actions: Vec<Box<dyn Action<I>>>,
}

impl<I> Sequence<I> {
    /// Create a new sequence of actions
    pub fn new(actions: Vec<Box<dyn Action<I>>>) -> Self {
        Self { actions }
    }
}

impl<I: Sync + Send + 'static> Action<I> for Sequence<I> {
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Execute each action in sequence
            for action in &mut self.actions {
                action.execute(env).await?;
            }

            Ok(())
        })
    }
}
