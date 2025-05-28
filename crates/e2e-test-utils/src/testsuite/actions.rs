//! Actions that can be performed in tests.

use crate::testsuite::{Environment, LatestBlockInfo};
use alloy_primitives::{Bytes, B256, U256};
use alloy_rpc_types_engine::{
    payload::ExecutionPayloadEnvelopeV3, ForkchoiceState, PayloadAttributes, PayloadStatusEnum,
};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::{future::Future, marker::PhantomData, time::Duration};
use tokio::time::sleep;
use tracing::debug;

/// An action that can be performed on an instance.
///
/// Actions execute operations and potentially make assertions in a single step.
/// The action name indicates what it does (e.g., `AssertMineBlock` would both
/// mine a block and assert it worked).
pub trait Action<I>: Send + 'static
where
    I: EngineTypes,
{
    /// Executes the action
    fn execute<'a>(&'a mut self, env: &'a mut Environment<I>) -> BoxFuture<'a, Result<()>>;
}

/// Simplified action container for storage in tests
#[expect(missing_debug_implementations)]
pub struct ActionBox<I>(Box<dyn Action<I>>);

impl<I> ActionBox<I>
where
    I: EngineTypes + 'static,
{
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
    I: EngineTypes,
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
pub struct AssertMineBlock<Engine>
where
    Engine: PayloadTypes,
{
    /// The node index to mine
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Bytes>,
    /// Expected block hash (optional)
    pub expected_hash: Option<B256>,
    /// Block's payload attributes
    // TODO: refactor once we have actions to generate payload attributes.
    pub payload_attributes: Engine::PayloadAttributes,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> AssertMineBlock<Engine>
where
    Engine: PayloadTypes,
{
    /// Create a new `AssertMineBlock` action
    pub fn new(
        node_idx: usize,
        transactions: Vec<Bytes>,
        expected_hash: Option<B256>,
        payload_attributes: Engine::PayloadAttributes,
    ) -> Self {
        Self {
            node_idx,
            transactions,
            expected_hash,
            payload_attributes,
            _phantom: Default::default(),
        }
    }
}

impl<Engine> Action<Engine> for AssertMineBlock<Engine>
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index out of bounds: {}", self.node_idx));
            }

            let node_client = &env.node_clients[self.node_idx];
            let rpc_client = &node_client.rpc;
            let engine_client = node_client.engine.http_client();

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

            let fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v2(
                &engine_client,
                fork_choice_state,
                Some(self.payload_attributes.clone()),
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
                            EngineApiClient::<Engine>::get_payload_v2(&engine_client, payload_id)
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
#[derive(Debug, Default)]
pub struct PickNextBlockProducer {}

impl PickNextBlockProducer {
    /// Create a new `PickNextBlockProducer` action
    pub const fn new() -> Self {
        Self {}
    }
}

impl<Engine> Action<Engine> for PickNextBlockProducer
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

            // simple round-robin selection based on next block number
            let next_producer_idx = ((latest_info.number + 1) % num_clients as u64) as usize;

            env.last_producer_idx = Some(next_producer_idx);
            debug!(
                "Selected node {} as the next block producer for block {}",
                next_producer_idx,
                latest_info.number + 1
            );

            Ok(())
        })
    }
}

/// Store payload attributes for the next block.
#[derive(Debug, Default)]
pub struct GeneratePayloadAttributes {}

impl<Engine> Action<Engine> for GeneratePayloadAttributes
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let latest_block = env
                .latest_block_info
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;
            let block_number = latest_block.number;
            let timestamp = env.latest_header_time + env.block_timestamp_increment;
            let payload_attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::random(),
                suggested_fee_recipient: alloy_primitives::Address::random(),
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            env.payload_attributes.insert(latest_block.number + 1, payload_attributes);
            debug!("Stored payload attributes for block {}", block_number + 1);
            Ok(())
        })
    }
}
/// Action that generates the next payload
#[derive(Debug, Default)]
pub struct GenerateNextPayload {}

impl<Engine> Action<Engine> for GenerateNextPayload
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let latest_block = env
                .latest_block_info
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;

            let parent_hash = latest_block.hash;
            debug!("Latest block hash: {parent_hash}");

            let fork_choice_state = ForkchoiceState {
                head_block_hash: parent_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };

            let payload_attributes = env
                .payload_attributes
                .get(&(latest_block.number + 1))
                .cloned()
                .ok_or_else(|| eyre::eyre!("No payload attributes found for next block"))?;

            let producer_idx =
                env.last_producer_idx.ok_or_else(|| eyre::eyre!("No block producer selected"))?;

            let fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v3(
                &env.node_clients[producer_idx].engine.http_client(),
                fork_choice_state,
                Some(payload_attributes.clone().into()),
            )
            .await?;

            debug!("FCU result: {:?}", fcu_result);

            let payload_id = if let Some(payload_id) = fcu_result.payload_id {
                debug!("Received new payload ID: {:?}", payload_id);
                payload_id
            } else {
                debug!("No payload ID returned, generating fresh payload attributes for forking");

                let fresh_payload_attributes = PayloadAttributes {
                    timestamp: env.latest_header_time + env.block_timestamp_increment,
                    prev_randao: B256::random(),
                    suggested_fee_recipient: alloy_primitives::Address::random(),
                    withdrawals: Some(vec![]),
                    parent_beacon_block_root: Some(B256::ZERO),
                };

                let fresh_fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v3(
                    &env.node_clients[producer_idx].engine.http_client(),
                    fork_choice_state,
                    Some(fresh_payload_attributes.clone().into()),
                )
                .await?;

                debug!("Fresh FCU result: {:?}", fresh_fcu_result);

                if let Some(payload_id) = fresh_fcu_result.payload_id {
                    payload_id
                } else {
                    debug!("Engine considers the fork base already canonical, skipping payload generation");
                    return Ok(());
                }
            };

            env.next_payload_id = Some(payload_id);

            sleep(Duration::from_secs(1)).await;

            let built_payload_envelope = EngineApiClient::<Engine>::get_payload_v3(
                &env.node_clients[producer_idx].engine.http_client(),
                payload_id,
            )
            .await?;

            // Store the payload attributes that were used to generate this payload
            let built_payload = payload_attributes.clone();
            env.payload_id_history.insert(latest_block.number + 1, payload_id);
            env.latest_payload_built = Some(built_payload);
            env.latest_payload_envelope = Some(built_payload_envelope);

            Ok(())
        })
    }
}

///Action that broadcasts the latest fork choice state to all clients
#[derive(Debug, Default)]
pub struct BroadcastLatestForkchoice {}

impl<Engine> Action<Engine> for BroadcastLatestForkchoice
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if env.node_clients.is_empty() {
                return Err(eyre::eyre!("No node clients available"));
            }

            // use the hash of the newly executed payload if available
            let head_hash = if let Some(payload_envelope) = &env.latest_payload_envelope {
                let execution_payload_envelope: ExecutionPayloadEnvelopeV3 =
                    payload_envelope.clone().into();
                let new_block_hash = execution_payload_envelope
                    .execution_payload
                    .payload_inner
                    .payload_inner
                    .block_hash;
                debug!("Using newly executed block hash as head: {new_block_hash}");
                new_block_hash
            } else {
                // fallback to RPC query
                let rpc_client = &env.node_clients[0].rpc;
                let current_head_block =
                    EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                        rpc_client,
                        alloy_eips::BlockNumberOrTag::Latest,
                        false,
                    )
                    .await?
                    .ok_or_else(|| eyre::eyre!("No latest block found from RPC"))?;
                debug!("Using RPC latest block hash as head: {}", current_head_block.header.hash);
                current_head_block.header.hash
            };

            let fork_choice_state = ForkchoiceState {
                head_block_hash: head_hash,
                safe_block_hash: head_hash,
                finalized_block_hash: head_hash,
            };
            debug!(
                "Broadcasting forkchoice update to {} clients. Head: {:?}",
                env.node_clients.len(),
                fork_choice_state.head_block_hash
            );

            for (idx, client) in env.node_clients.iter().enumerate() {
                match EngineApiClient::<Engine>::fork_choice_updated_v3(
                    &client.engine.http_client(),
                    fork_choice_state,
                    None,
                )
                .await
                {
                    Ok(resp) => {
                        debug!(
                            "Client {}: Forkchoice update status: {:?}",
                            idx, resp.payload_status.status
                        );
                    }
                    Err(err) => {
                        return Err(eyre::eyre!(
                            "Client {}: Failed to broadcast forkchoice: {:?}",
                            idx,
                            err
                        ));
                    }
                }
            }
            debug!("Forkchoice update broadcasted successfully");
            Ok(())
        })
    }
}

/// Action that updates environment state after block production
#[derive(Debug, Default)]
pub struct UpdateBlockInfo {}

impl<Engine> Action<Engine> for UpdateBlockInfo
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // get the latest block from the first client to update environment state
            let rpc_client = &env.node_clients[0].rpc;
            let latest_block =
                EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                    rpc_client,
                    alloy_eips::BlockNumberOrTag::Latest,
                    false,
                )
                .await?
                .ok_or_else(|| eyre::eyre!("No latest block found from RPC"))?;

            // update environment with the new block information
            env.latest_block_info = Some(LatestBlockInfo {
                hash: latest_block.header.hash,
                number: latest_block.header.number,
            });

            env.latest_header_time = latest_block.header.timestamp;
            env.latest_fork_choice_state.head_block_hash = latest_block.header.hash;

            debug!(
                "Updated environment to block {} (hash: {})",
                latest_block.header.number, latest_block.header.hash
            );

            Ok(())
        })
    }
}

/// Action that checks whether the broadcasted new payload has been accepted
#[derive(Debug, Default)]
pub struct CheckPayloadAccepted {}

impl<Engine> Action<Engine> for CheckPayloadAccepted
where
    Engine: EngineTypes,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut accepted_check: bool = false;

            let latest_block = env
                .latest_block_info
                .as_mut()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;

            let payload_id = *env
                .payload_id_history
                .get(&(latest_block.number + 1))
                .ok_or_else(|| eyre::eyre!("Cannot find payload_id"))?;

            for (idx, client) in env.node_clients.iter().enumerate() {
                let rpc_client = &client.rpc;

                // get the last header by number using latest_head_number
                let rpc_latest_header =
                    EthApiClient::<Transaction, Block, Receipt, Header>::header_by_number(
                        rpc_client,
                        alloy_eips::BlockNumberOrTag::Latest,
                    )
                    .await?
                    .ok_or_else(|| eyre::eyre!("No latest header found from rpc"))?;

                // perform several checks
                let next_new_payload = env
                    .latest_payload_built
                    .as_ref()
                    .ok_or_else(|| eyre::eyre!("No next built payload found"))?;

                let built_payload = EngineApiClient::<Engine>::get_payload_v3(
                    &client.engine.http_client(),
                    payload_id,
                )
                .await?;

                let execution_payload_envelope: ExecutionPayloadEnvelopeV3 = built_payload.into();
                let new_payload_block_hash = execution_payload_envelope
                    .execution_payload
                    .payload_inner
                    .payload_inner
                    .block_hash;

                if rpc_latest_header.hash != new_payload_block_hash {
                    debug!(
                        "Client {}: The hash is not matched: {:?} {:?}",
                        idx, rpc_latest_header.hash, new_payload_block_hash
                    );
                    continue;
                }

                if rpc_latest_header.inner.difficulty != U256::ZERO {
                    debug!(
                        "Client {}: difficulty != 0: {:?}",
                        idx, rpc_latest_header.inner.difficulty
                    );
                    continue;
                }

                if rpc_latest_header.inner.mix_hash != next_new_payload.prev_randao {
                    debug!(
                        "Client {}: The mix_hash and prev_randao is not same: {:?} {:?}",
                        idx, rpc_latest_header.inner.mix_hash, next_new_payload.prev_randao
                    );
                    continue;
                }

                let extra_len = rpc_latest_header.inner.extra_data.len();
                if extra_len <= 32 {
                    debug!("Client {}: extra_len is fewer than 32. extra_len: {}", idx, extra_len);
                    continue;
                }

                // at least one client passes all the check, save the header in Env
                if !accepted_check {
                    accepted_check = true;
                    // save the header in Env
                    env.latest_header_time = next_new_payload.timestamp;

                    // add it to header history
                    env.latest_fork_choice_state.head_block_hash = rpc_latest_header.hash;
                    latest_block.hash = rpc_latest_header.hash;
                    latest_block.number = rpc_latest_header.inner.number;
                }
            }

            if accepted_check {
                Ok(())
            } else {
                Err(eyre::eyre!("No clients passed payload acceptance checks"))
            }
        })
    }
}

/// Action that produces a sequence of blocks using the available clients
#[derive(Debug)]
pub struct ProduceBlocks<Engine> {
    /// Number of blocks to produce
    pub num_blocks: u64,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> ProduceBlocks<Engine> {
    /// Create a new `ProduceBlocks` action
    pub fn new(num_blocks: u64) -> Self {
        Self { num_blocks, _phantom: Default::default() }
    }
}

impl<Engine> Default for ProduceBlocks<Engine> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<Engine> Action<Engine> for ProduceBlocks<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            for _ in 0..self.num_blocks {
                // create a fresh sequence for each block to avoid state pollution
                let mut sequence = Sequence::new(vec![
                    Box::new(PickNextBlockProducer::default()),
                    Box::new(GeneratePayloadAttributes::default()),
                    Box::new(GenerateNextPayload::default()),
                    Box::new(BroadcastNextNewPayload::default()),
                    Box::new(BroadcastLatestForkchoice::default()),
                    Box::new(UpdateBlockInfo::default()),
                ]);
                sequence.execute(env).await?;
            }
            Ok(())
        })
    }
}

/// Run a sequence of actions in series.
#[expect(missing_debug_implementations)]
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

impl<I> Action<I> for Sequence<I>
where
    I: EngineTypes + Sync + Send + 'static,
{
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

/// Action to create a fork from a specified block number and produce blocks on top
#[derive(Debug)]
pub struct CreateFork<Engine> {
    /// Block number to use as the base of the fork
    pub fork_base_block: u64,
    /// Number of blocks to produce on top of the fork base
    pub num_blocks: u64,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> CreateFork<Engine> {
    /// Create a new `CreateFork` action
    pub fn new(fork_base_block: u64, num_blocks: u64) -> Self {
        Self { fork_base_block, num_blocks, _phantom: Default::default() }
    }
}

impl<Engine> Action<Engine> for CreateFork<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut sequence = Sequence::new(vec![
                Box::new(SetForkBase::new(self.fork_base_block)),
                Box::new(ProduceBlocks::new(self.num_blocks)),
                Box::new(ValidateFork::new(self.fork_base_block)),
            ]);

            sequence.execute(env).await
        })
    }
}

/// Sub-action to set the fork base block in the environment
#[derive(Debug)]
pub struct SetForkBase {
    /// Block number to use as the base of the fork
    pub fork_base_block: u64,
}

impl SetForkBase {
    /// Create a new `SetForkBase` action
    pub const fn new(fork_base_block: u64) -> Self {
        Self { fork_base_block }
    }
}

impl<Engine> Action<Engine> for SetForkBase
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if env.node_clients.is_empty() {
                return Err(eyre::eyre!("No node clients available"));
            }

            // get the block at the fork base number to establish the fork point
            let rpc_client = &env.node_clients[0].rpc;
            let fork_base_block =
                EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                    rpc_client,
                    alloy_eips::BlockNumberOrTag::Number(self.fork_base_block),
                    false,
                )
                .await?
                .ok_or_else(|| eyre::eyre!("Fork base block {} not found", self.fork_base_block))?;

            // update environment to point to the fork base block
            env.latest_block_info = Some(LatestBlockInfo {
                hash: fork_base_block.header.hash,
                number: fork_base_block.header.number,
            });

            env.latest_header_time = fork_base_block.header.timestamp;

            // update fork choice state to the fork base
            env.latest_fork_choice_state = ForkchoiceState {
                head_block_hash: fork_base_block.header.hash,
                safe_block_hash: fork_base_block.header.hash,
                finalized_block_hash: fork_base_block.header.hash,
            };

            debug!(
                "Set fork base to block {} (hash: {})",
                self.fork_base_block, fork_base_block.header.hash
            );

            Ok(())
        })
    }
}

/// Sub-action to validate that a fork was created correctly
#[derive(Debug)]
pub struct ValidateFork {
    /// Number of the fork base block (stored here since we need it for validation)
    pub fork_base_number: u64,
}

impl ValidateFork {
    /// Create a new `ValidateFork` action
    pub const fn new(fork_base_number: u64) -> Self {
        Self { fork_base_number }
    }
}

impl<Engine> Action<Engine> for ValidateFork
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let current_block_info = env
                .latest_block_info
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No current block information available"))?;

            // verify that the current tip is at or ahead of the fork base
            if current_block_info.number < self.fork_base_number {
                return Err(eyre::eyre!(
                    "Fork validation failed: current block number {} is behind fork base {}",
                    current_block_info.number,
                    self.fork_base_number
                ));
            }

            // get the fork base hash from the environment's fork choice state
            // we assume the fork choice state was set correctly by SetForkBase
            let fork_base_hash = env.latest_fork_choice_state.finalized_block_hash;

            // trace back from current tip to verify it's a descendant of the fork base
            let rpc_client = &env.node_clients[0].rpc;
            let mut current_hash = current_block_info.hash;
            let mut current_number = current_block_info.number;

            // walk backwards through the chain until we reach the fork base
            while current_number > self.fork_base_number {
                let block = EthApiClient::<Transaction, Block, Receipt, Header>::block_by_hash(
                    rpc_client,
                    current_hash,
                    false,
                )
                .await?
                .ok_or_else(|| {
                    eyre::eyre!("Block with hash {} not found during fork validation", current_hash)
                })?;

                current_hash = block.header.parent_hash;
                current_number = block.header.number.saturating_sub(1);
            }

            // verify we reached the expected fork base
            if current_hash != fork_base_hash {
                return Err(eyre::eyre!(
                    "Fork validation failed: expected fork base hash {}, but found {} at block {}",
                    fork_base_hash,
                    current_hash,
                    current_number
                ));
            }

            debug!(
                "Fork validation successful: tip block {} is descendant of fork base {} ({})",
                current_block_info.number, self.fork_base_number, fork_base_hash
            );

            Ok(())
        })
    }
}

/// Action that broadcasts the next new payload
#[derive(Debug, Default)]
pub struct BroadcastNextNewPayload {}

impl<Engine> Action<Engine> for BroadcastNextNewPayload
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Get the next new payload to broadcast
            let next_new_payload = env
                .latest_payload_built
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No next built payload found"))?;
            let parent_beacon_block_root = next_new_payload
                .parent_beacon_block_root
                .ok_or_else(|| eyre::eyre!("No parent beacon block root for next new payload"))?;

            let payload_envelope = env
                .latest_payload_envelope
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No execution payload envelope available"))?;

            let execution_payload_envelope: ExecutionPayloadEnvelopeV3 =
                payload_envelope.clone().into();
            let execution_payload = execution_payload_envelope.execution_payload;

            // Loop through all clients and broadcast the next new payload
            let mut successful_broadcast: bool = false;

            for client in &env.node_clients {
                let engine = client.engine.http_client();

                // Broadcast the execution payload
                let result = EngineApiClient::<Engine>::new_payload_v3(
                    &engine,
                    execution_payload.clone(),
                    vec![],
                    parent_beacon_block_root,
                )
                .await?;

                // Check if broadcast was successful
                if result.status == PayloadStatusEnum::Valid {
                    successful_broadcast = true;
                    // We don't need to update the latest payload built since it should be the same.
                    // env.latest_payload_built = Some(next_new_payload.clone());
                    env.latest_payload_executed = Some(next_new_payload.clone());
                    break;
                } else if let PayloadStatusEnum::Invalid { validation_error } = result.status {
                    debug!(
                        "Invalid payload status returned from broadcast: {:?}",
                        validation_error
                    );
                }
            }

            if !successful_broadcast {
                return Err(eyre::eyre!("Failed to successfully broadcast payload to any client"));
            }

            Ok(())
        })
    }
}

/// Target for reorg operation
#[derive(Debug, Clone)]
pub enum ReorgTarget {
    /// Direct block hash
    Hash(B256),
    /// Tagged block reference
    Tag(String),
}

/// Action that performs a reorg by setting a new head block as canonical
#[derive(Debug)]
pub struct ReorgTo<Engine> {
    /// Target for the reorg operation
    pub target: ReorgTarget,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> ReorgTo<Engine> {
    /// Create a new `ReorgTo` action with a direct block hash
    pub const fn new(target_hash: B256) -> Self {
        Self { target: ReorgTarget::Hash(target_hash), _phantom: PhantomData }
    }

    /// Create a new `ReorgTo` action with a tagged block reference
    pub fn new_from_tag(tag: impl Into<String>) -> Self {
        Self { target: ReorgTarget::Tag(tag.into()), _phantom: PhantomData }
    }
}

impl<Engine> Action<Engine> for ReorgTo<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // resolve the target hash from either direct hash or tag
            let target_hash = match &self.target {
                ReorgTarget::Hash(hash) => *hash,
                ReorgTarget::Tag(tag) => env
                    .block_registry
                    .get(tag)
                    .copied()
                    .ok_or_else(|| eyre::eyre!("Block tag '{}' not found in registry", tag))?,
            };

            let mut sequence = Sequence::new(vec![
                Box::new(SetReorgTarget::new(target_hash)),
                Box::new(BroadcastLatestForkchoice::default()),
                Box::new(UpdateBlockInfo::default()),
            ]);

            sequence.execute(env).await
        })
    }
}

/// Sub-action to set the reorg target block in the environment
#[derive(Debug)]
pub struct SetReorgTarget {
    /// Hash of the block to reorg to
    pub target_hash: B256,
}

impl SetReorgTarget {
    /// Create a new `SetReorgTarget` action
    pub const fn new(target_hash: B256) -> Self {
        Self { target_hash }
    }
}

impl<Engine> Action<Engine> for SetReorgTarget
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if env.node_clients.is_empty() {
                return Err(eyre::eyre!("No node clients available"));
            }

            // verify the target block exists
            let rpc_client = &env.node_clients[0].rpc;
            let target_block = EthApiClient::<Transaction, Block, Receipt, Header>::block_by_hash(
                rpc_client,
                self.target_hash,
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Target reorg block {} not found", self.target_hash))?;

            debug!(
                "Setting reorg target to block {} (hash: {})",
                target_block.header.number, self.target_hash
            );

            // update environment to point to the target block
            env.latest_block_info = Some(LatestBlockInfo {
                hash: target_block.header.hash,
                number: target_block.header.number,
            });

            env.latest_header_time = target_block.header.timestamp;

            // update fork choice state to make the target block canonical
            env.latest_fork_choice_state = ForkchoiceState {
                head_block_hash: self.target_hash,
                safe_block_hash: self.target_hash, // Simplified - in practice might be different
                finalized_block_hash: self.target_hash, /* Simplified - in practice might be
                                                    * different */
            };

            debug!("Set reorg target to block {}", self.target_hash);
            Ok(())
        })
    }
}

/// Action that captures the current block and tags it with a name for later reference
#[derive(Debug)]
pub struct CaptureBlock {
    /// Tag name to associate with the current block
    pub tag: String,
}

impl CaptureBlock {
    /// Create a new `CaptureBlock` action
    pub fn new(tag: impl Into<String>) -> Self {
        Self { tag: tag.into() }
    }
}

impl<Engine> Action<Engine> for CaptureBlock
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let current_block = env
                .latest_block_info
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No current block information available"))?;

            env.block_registry.insert(self.tag.clone(), current_block.hash);

            debug!(
                "Captured block {} (hash: {}) with tag '{}'",
                current_block.number, current_block.hash, self.tag
            );

            Ok(())
        })
    }
}
