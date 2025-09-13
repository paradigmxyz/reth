//! Block production actions for the e2e testing framework.

use crate::testsuite::{
    actions::{expect_fcu_not_syncing_or_accepted, validate_fcu_response, Action, Sequence},
    BlockInfo, Environment,
};
use alloy_primitives::{Bytes, B256};
use alloy_rpc_types_engine::{
    payload::ExecutionPayloadEnvelopeV3, ForkchoiceState, PayloadAttributes, PayloadStatusEnum,
};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::{collections::HashSet, marker::PhantomData, time::Duration};
use tokio::time::sleep;
use tracing::debug;

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
            let latest_block = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                Header,
            >::block_by_number(
                rpc_client, alloy_eips::BlockNumberOrTag::Latest, false
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
                .current_block_info()
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
                .current_block_info()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;
            let block_number = latest_block.number;
            let timestamp =
                env.active_node_state()?.latest_header_time + env.block_timestamp_increment;
            let payload_attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::random(),
                suggested_fee_recipient: alloy_primitives::Address::random(),
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            env.active_node_state_mut()?
                .payload_attributes
                .insert(latest_block.number + 1, payload_attributes);
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
                .current_block_info()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;

            let parent_hash = latest_block.hash;
            debug!("Latest block hash: {parent_hash}");

            let fork_choice_state = ForkchoiceState {
                head_block_hash: parent_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };

            let payload_attributes = env
                .active_node_state()?
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

            // validate the FCU status before proceeding
            // Note: In the context of GenerateNextPayload, Syncing usually means the engine
            // doesn't have the requested head block, which should be an error
            expect_fcu_not_syncing_or_accepted(&fcu_result, "GenerateNextPayload")?;

            let payload_id = if let Some(payload_id) = fcu_result.payload_id {
                debug!("Received new payload ID: {:?}", payload_id);
                payload_id
            } else {
                debug!("No payload ID returned, generating fresh payload attributes for forking");

                let fresh_payload_attributes = PayloadAttributes {
                    timestamp: env.active_node_state()?.latest_header_time +
                        env.block_timestamp_increment,
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

                // validate the fresh FCU status
                expect_fcu_not_syncing_or_accepted(
                    &fresh_fcu_result,
                    "GenerateNextPayload (fresh)",
                )?;

                if let Some(payload_id) = fresh_fcu_result.payload_id {
                    payload_id
                } else {
                    debug!("Engine considers the fork base already canonical, skipping payload generation");
                    return Ok(());
                }
            };

            env.active_node_state_mut()?.next_payload_id = Some(payload_id);

            sleep(Duration::from_secs(1)).await;

            let built_payload_envelope = EngineApiClient::<Engine>::get_payload_v3(
                &env.node_clients[producer_idx].engine.http_client(),
                payload_id,
            )
            .await?;

            // Store the payload attributes that were used to generate this payload
            let built_payload = payload_attributes.clone();
            env.active_node_state_mut()?
                .payload_id_history
                .insert(latest_block.number + 1, payload_id);
            env.active_node_state_mut()?.latest_payload_built = Some(built_payload);
            env.active_node_state_mut()?.latest_payload_envelope = Some(built_payload_envelope);

            Ok(())
        })
    }
}

/// Action that broadcasts the latest fork choice state to all clients
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
            let head_hash = if let Some(payload_envelope) =
                &env.active_node_state()?.latest_payload_envelope
            {
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
                let current_head_block = EthApiClient::<
                    TransactionRequest,
                    Transaction,
                    Block,
                    Receipt,
                    Header,
                >::block_by_number(
                    rpc_client, alloy_eips::BlockNumberOrTag::Latest, false
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
                        // validate that the forkchoice update was accepted
                        validate_fcu_response(&resp, &format!("Client {idx}"))?;
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

/// Action that syncs environment state with the node's canonical chain via RPC.
///
/// This queries the latest canonical block from the node and updates the environment
/// to match. Typically used after forkchoice operations to ensure the environment
/// is in sync with the node's view of the canonical chain.
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
            let latest_block = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                Header,
            >::block_by_number(
                rpc_client, alloy_eips::BlockNumberOrTag::Latest, false
            )
            .await?
            .ok_or_else(|| eyre::eyre!("No latest block found from RPC"))?;

            // update environment with the new block information
            env.set_current_block_info(BlockInfo {
                hash: latest_block.header.hash,
                number: latest_block.header.number,
                timestamp: latest_block.header.timestamp,
            })?;

            env.active_node_state_mut()?.latest_header_time = latest_block.header.timestamp;
            env.active_node_state_mut()?.latest_fork_choice_state.head_block_hash =
                latest_block.header.hash;

            debug!(
                "Updated environment to block {} (hash: {})",
                latest_block.header.number, latest_block.header.hash
            );

            Ok(())
        })
    }
}

/// Action that updates environment state using the locally produced payload.
///
/// This uses the execution payload stored in the environment rather than querying RPC,
/// making it more efficient and reliable during block production. Preferred over
/// `UpdateBlockInfo` when we have just produced a block and have the payload available.
#[derive(Debug, Default)]
pub struct UpdateBlockInfoToLatestPayload {}

impl<Engine> Action<Engine> for UpdateBlockInfoToLatestPayload
where
    Engine: EngineTypes + PayloadTypes,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let payload_envelope = env
                .active_node_state()?
                .latest_payload_envelope
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No execution payload envelope available"))?;

            let execution_payload_envelope: ExecutionPayloadEnvelopeV3 =
                payload_envelope.clone().into();
            let execution_payload = execution_payload_envelope.execution_payload;

            let block_hash = execution_payload.payload_inner.payload_inner.block_hash;
            let block_number = execution_payload.payload_inner.payload_inner.block_number;
            let block_timestamp = execution_payload.payload_inner.payload_inner.timestamp;

            // update environment with the new block information from the payload
            env.set_current_block_info(BlockInfo {
                hash: block_hash,
                number: block_number,
                timestamp: block_timestamp,
            })?;

            env.active_node_state_mut()?.latest_header_time = block_timestamp;
            env.active_node_state_mut()?.latest_fork_choice_state.head_block_hash = block_hash;

            debug!(
                "Updated environment to newly produced block {} (hash: {})",
                block_number, block_hash
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

            let mut latest_block = env
                .current_block_info()
                .ok_or_else(|| eyre::eyre!("No latest block information available"))?;

            let payload_id = *env
                .active_node_state()?
                .payload_id_history
                .get(&(latest_block.number + 1))
                .ok_or_else(|| eyre::eyre!("Cannot find payload_id"))?;

            let node_clients = env.node_clients.clone();
            for (idx, client) in node_clients.iter().enumerate() {
                let rpc_client = &client.rpc;

                // get the last header by number using latest_head_number
                let rpc_latest_header = EthApiClient::<
                    TransactionRequest,
                    Transaction,
                    Block,
                    Receipt,
                    Header,
                >::header_by_number(
                    rpc_client, alloy_eips::BlockNumberOrTag::Latest
                )
                .await?
                .ok_or_else(|| eyre::eyre!("No latest header found from rpc"))?;

                // perform several checks
                let next_new_payload = env
                    .active_node_state()?
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

                if rpc_latest_header.inner.difficulty != alloy_primitives::U256::ZERO {
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
                    env.active_node_state_mut()?.latest_header_time = next_new_payload.timestamp;

                    // add it to header history
                    env.active_node_state_mut()?.latest_fork_choice_state.head_block_hash =
                        rpc_latest_header.hash;
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

/// Action that broadcasts the next new payload
#[derive(Debug, Default)]
pub struct BroadcastNextNewPayload {
    /// If true, only send to the active node. If false, broadcast to all nodes.
    active_node_only: bool,
}

impl BroadcastNextNewPayload {
    /// Create a new `BroadcastNextNewPayload` action that only sends to the active node
    pub const fn with_active_node() -> Self {
        Self { active_node_only: true }
    }
}

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
                .active_node_state()?
                .latest_payload_built
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No next built payload found"))?
                .clone();
            let parent_beacon_block_root = next_new_payload
                .parent_beacon_block_root
                .ok_or_else(|| eyre::eyre!("No parent beacon block root for next new payload"))?;

            let payload_envelope = env
                .active_node_state()?
                .latest_payload_envelope
                .as_ref()
                .ok_or_else(|| eyre::eyre!("No execution payload envelope available"))?
                .clone();

            let execution_payload_envelope: ExecutionPayloadEnvelopeV3 = payload_envelope.into();
            let execution_payload = execution_payload_envelope.execution_payload;

            if self.active_node_only {
                // Send only to the active node
                let active_idx = env.active_node_idx;
                let engine = env.node_clients[active_idx].engine.http_client();

                let result = EngineApiClient::<Engine>::new_payload_v3(
                    &engine,
                    execution_payload.clone(),
                    vec![],
                    parent_beacon_block_root,
                )
                .await?;

                debug!("Active node {}: new_payload status: {:?}", active_idx, result.status);

                // Validate the response
                match result.status {
                    PayloadStatusEnum::Valid => {
                        env.active_node_state_mut()?.latest_payload_executed =
                            Some(next_new_payload);
                        Ok(())
                    }
                    other => Err(eyre::eyre!(
                        "Active node {}: Unexpected payload status: {:?}",
                        active_idx,
                        other
                    )),
                }
            } else {
                // Loop through all clients and broadcast the next new payload
                let mut broadcast_results = Vec::new();
                let mut first_valid_seen = false;

                for (idx, client) in env.node_clients.iter().enumerate() {
                    let engine = client.engine.http_client();

                    // Broadcast the execution payload
                    let result = EngineApiClient::<Engine>::new_payload_v3(
                        &engine,
                        execution_payload.clone(),
                        vec![],
                        parent_beacon_block_root,
                    )
                    .await?;

                    broadcast_results.push((idx, result.status.clone()));
                    debug!("Node {}: new_payload broadcast status: {:?}", idx, result.status);

                    // Check if this node accepted the payload
                    if result.status == PayloadStatusEnum::Valid && !first_valid_seen {
                        first_valid_seen = true;
                    } else if let PayloadStatusEnum::Invalid { validation_error } = result.status {
                        debug!(
                            "Node {}: Invalid payload status returned from broadcast: {:?}",
                            idx, validation_error
                        );
                    }
                }

                // Update the executed payload state after broadcasting to all nodes
                if first_valid_seen {
                    env.active_node_state_mut()?.latest_payload_executed = Some(next_new_payload);
                }

                // Check if at least one node accepted the payload
                let any_valid =
                    broadcast_results.iter().any(|(_, status)| *status == PayloadStatusEnum::Valid);
                if !any_valid {
                    return Err(eyre::eyre!(
                        "Failed to successfully broadcast payload to any client"
                    ));
                }

                debug!("Broadcast complete. Results: {:?}", broadcast_results);

                Ok(())
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
                // Note: This produces blocks but does NOT make them canonical
                // Use MakeCanonical action explicitly if canonicalization is needed
                let mut sequence = Sequence::new(vec![
                    Box::new(PickNextBlockProducer::default()),
                    Box::new(GeneratePayloadAttributes::default()),
                    Box::new(GenerateNextPayload::default()),
                    Box::new(BroadcastNextNewPayload::default()),
                    Box::new(UpdateBlockInfoToLatestPayload::default()),
                ]);
                sequence.execute(env).await?;
            }
            Ok(())
        })
    }
}

/// Action to test forkchoice update to a tagged block with expected status
#[derive(Debug)]
pub struct TestFcuToTag {
    /// Tag name of the target block
    pub tag: String,
    /// Expected payload status
    pub expected_status: PayloadStatusEnum,
}

impl TestFcuToTag {
    /// Create a new `TestFcuToTag` action
    pub fn new(tag: impl Into<String>, expected_status: PayloadStatusEnum) -> Self {
        Self { tag: tag.into(), expected_status }
    }
}

impl<Engine> Action<Engine> for TestFcuToTag
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // get the target block from the registry
            let (target_block, _node_idx) = env
                .block_registry
                .get(&self.tag)
                .copied()
                .ok_or_else(|| eyre::eyre!("Block tag '{}' not found in registry", self.tag))?;

            let engine_client = env.node_clients[0].engine.http_client();
            let fcu_state = ForkchoiceState {
                head_block_hash: target_block.hash,
                safe_block_hash: target_block.hash,
                finalized_block_hash: target_block.hash,
            };

            let fcu_response =
                EngineApiClient::<Engine>::fork_choice_updated_v2(&engine_client, fcu_state, None)
                    .await?;

            // validate the response matches expected status
            match (&fcu_response.payload_status.status, &self.expected_status) {
                (PayloadStatusEnum::Valid, PayloadStatusEnum::Valid) => {
                    debug!("FCU to '{}' returned VALID as expected", self.tag);
                }
                (PayloadStatusEnum::Invalid { .. }, PayloadStatusEnum::Invalid { .. }) => {
                    debug!("FCU to '{}' returned INVALID as expected", self.tag);
                }
                (PayloadStatusEnum::Syncing, PayloadStatusEnum::Syncing) => {
                    debug!("FCU to '{}' returned SYNCING as expected", self.tag);
                }
                (PayloadStatusEnum::Accepted, PayloadStatusEnum::Accepted) => {
                    debug!("FCU to '{}' returned ACCEPTED as expected", self.tag);
                }
                (actual, expected) => {
                    return Err(eyre::eyre!(
                        "FCU to '{}': expected status {:?}, but got {:?}",
                        self.tag,
                        expected,
                        actual
                    ));
                }
            }

            Ok(())
        })
    }
}

/// Action to expect a specific FCU status when targeting a tagged block
#[derive(Debug)]
pub struct ExpectFcuStatus {
    /// Tag name of the target block
    pub target_tag: String,
    /// Expected payload status
    pub expected_status: PayloadStatusEnum,
}

impl ExpectFcuStatus {
    /// Create a new `ExpectFcuStatus` action expecting VALID status
    pub fn valid(target_tag: impl Into<String>) -> Self {
        Self { target_tag: target_tag.into(), expected_status: PayloadStatusEnum::Valid }
    }

    /// Create a new `ExpectFcuStatus` action expecting INVALID status
    pub fn invalid(target_tag: impl Into<String>) -> Self {
        Self {
            target_tag: target_tag.into(),
            expected_status: PayloadStatusEnum::Invalid {
                validation_error: "corrupted block".to_string(),
            },
        }
    }

    /// Create a new `ExpectFcuStatus` action expecting SYNCING status
    pub fn syncing(target_tag: impl Into<String>) -> Self {
        Self { target_tag: target_tag.into(), expected_status: PayloadStatusEnum::Syncing }
    }

    /// Create a new `ExpectFcuStatus` action expecting ACCEPTED status
    pub fn accepted(target_tag: impl Into<String>) -> Self {
        Self { target_tag: target_tag.into(), expected_status: PayloadStatusEnum::Accepted }
    }
}

impl<Engine> Action<Engine> for ExpectFcuStatus
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut test_fcu = TestFcuToTag::new(&self.target_tag, self.expected_status.clone());
            test_fcu.execute(env).await
        })
    }
}

/// Action to validate that a tagged block remains canonical by performing FCU to it
#[derive(Debug)]
pub struct ValidateCanonicalTag {
    /// Tag name of the block to validate as canonical
    pub tag: String,
}

impl ValidateCanonicalTag {
    /// Create a new `ValidateCanonicalTag` action
    pub fn new(tag: impl Into<String>) -> Self {
        Self { tag: tag.into() }
    }
}

impl<Engine> Action<Engine> for ValidateCanonicalTag
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let mut expect_valid = ExpectFcuStatus::valid(&self.tag);
            expect_valid.execute(env).await?;

            debug!("Successfully validated that '{}' remains canonical", self.tag);
            Ok(())
        })
    }
}

/// Action that produces blocks locally without broadcasting to other nodes
/// This sends the payload only to the active node to ensure it's available locally
#[derive(Debug)]
pub struct ProduceBlocksLocally<Engine> {
    /// Number of blocks to produce
    pub num_blocks: u64,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> ProduceBlocksLocally<Engine> {
    /// Create a new `ProduceBlocksLocally` action
    pub fn new(num_blocks: u64) -> Self {
        Self { num_blocks, _phantom: Default::default() }
    }
}

impl<Engine> Default for ProduceBlocksLocally<Engine> {
    fn default() -> Self {
        Self::new(0)
    }
}

impl<Engine> Action<Engine> for ProduceBlocksLocally<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Remember the active node to ensure all blocks are produced on the same node
            let producer_idx = env.active_node_idx;

            for _ in 0..self.num_blocks {
                // Ensure we always use the same producer
                env.last_producer_idx = Some(producer_idx);

                // create a sequence that produces blocks and sends only to active node
                let mut sequence = Sequence::new(vec![
                    // Skip PickNextBlockProducer to maintain the same producer
                    Box::new(GeneratePayloadAttributes::default()),
                    Box::new(GenerateNextPayload::default()),
                    // Send payload only to the active node to make it available
                    Box::new(BroadcastNextNewPayload::with_active_node()),
                    Box::new(UpdateBlockInfoToLatestPayload::default()),
                ]);
                sequence.execute(env).await?;
            }
            Ok(())
        })
    }
}

/// Action that produces a sequence of blocks where some blocks are intentionally invalid
#[derive(Debug)]
pub struct ProduceInvalidBlocks<Engine> {
    /// Number of blocks to produce
    pub num_blocks: u64,
    /// Set of indices (0-based) where blocks should be made invalid
    pub invalid_indices: HashSet<u64>,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> ProduceInvalidBlocks<Engine> {
    /// Create a new `ProduceInvalidBlocks` action
    pub fn new(num_blocks: u64, invalid_indices: HashSet<u64>) -> Self {
        Self { num_blocks, invalid_indices, _phantom: Default::default() }
    }

    /// Create a new `ProduceInvalidBlocks` action with a single invalid block at the specified
    /// index
    pub fn with_invalid_at(num_blocks: u64, invalid_index: u64) -> Self {
        let mut invalid_indices = HashSet::new();
        invalid_indices.insert(invalid_index);
        Self::new(num_blocks, invalid_indices)
    }
}

impl<Engine> Action<Engine> for ProduceInvalidBlocks<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            for block_index in 0..self.num_blocks {
                let is_invalid = self.invalid_indices.contains(&block_index);

                if is_invalid {
                    debug!("Producing invalid block at index {}", block_index);

                    // produce a valid block first, then corrupt it
                    let mut sequence = Sequence::new(vec![
                        Box::new(PickNextBlockProducer::default()),
                        Box::new(GeneratePayloadAttributes::default()),
                        Box::new(GenerateNextPayload::default()),
                    ]);
                    sequence.execute(env).await?;

                    // get the latest payload and corrupt it
                    let latest_envelope =
                        env.active_node_state()?.latest_payload_envelope.as_ref().ok_or_else(
                            || eyre::eyre!("No payload envelope available to corrupt"),
                        )?;

                    let envelope_v3: ExecutionPayloadEnvelopeV3 = latest_envelope.clone().into();
                    let mut corrupted_payload = envelope_v3.execution_payload;

                    // corrupt the state root to make the block invalid
                    corrupted_payload.payload_inner.payload_inner.state_root = B256::random();

                    debug!(
                        "Corrupted state root for block {} to: {}",
                        block_index, corrupted_payload.payload_inner.payload_inner.state_root
                    );

                    // send the corrupted payload via newPayload
                    let engine_client = env.node_clients[0].engine.http_client();
                    // for simplicity, we'll use empty versioned hashes for invalid block testing
                    let versioned_hashes = Vec::new();
                    // use a random parent beacon block root since this is for invalid block testing
                    let parent_beacon_block_root = B256::random();

                    let new_payload_response = EngineApiClient::<Engine>::new_payload_v3(
                        &engine_client,
                        corrupted_payload.clone(),
                        versioned_hashes,
                        parent_beacon_block_root,
                    )
                    .await?;

                    // expect the payload to be rejected as invalid
                    match new_payload_response.status {
                        PayloadStatusEnum::Invalid { validation_error } => {
                            debug!(
                                "Block {} correctly rejected as invalid: {:?}",
                                block_index, validation_error
                            );
                        }
                        other_status => {
                            return Err(eyre::eyre!(
                                "Expected block {} to be rejected as INVALID, but got: {:?}",
                                block_index,
                                other_status
                            ));
                        }
                    }

                    // update block info with the corrupted block (for potential future reference)
                    env.set_current_block_info(BlockInfo {
                        hash: corrupted_payload.payload_inner.payload_inner.block_hash,
                        number: corrupted_payload.payload_inner.payload_inner.block_number,
                        timestamp: corrupted_payload.timestamp(),
                    })?;
                } else {
                    debug!("Producing valid block at index {}", block_index);

                    // produce a valid block normally
                    let mut sequence = Sequence::new(vec![
                        Box::new(PickNextBlockProducer::default()),
                        Box::new(GeneratePayloadAttributes::default()),
                        Box::new(GenerateNextPayload::default()),
                        Box::new(BroadcastNextNewPayload::default()),
                        Box::new(UpdateBlockInfoToLatestPayload::default()),
                    ]);
                    sequence.execute(env).await?;
                }
            }
            Ok(())
        })
    }
}
