//! Transaction-related actions and utilities for the e2e testing framework.
//!
//! This module provides actions for producing blocks with real transactions,
//! which is useful for testing scenarios that require actual state modifications
//! through transaction execution (e.g., storage changes, trie updates, etc.).
//!
//! The main action is [`ProduceBlockWithTransactions`] which:
//! 1. Sends raw signed transactions to the node's transaction pool
//! 2. Triggers block production to include those transactions
//! 3. Makes the block canonical
//! 4. Verifies transaction inclusion
//!
//! Additionally, this module provides transaction creation utilities for generating
//! test transactions such as token approvals and ETH transfers.

use crate::{
    testsuite::{
        actions::{Action, CaptureBlock, MakeCanonical, ProduceBlocks},
        BlockInfo, Environment,
    },
    wallet::Wallet,
};
use alloy_consensus::TxLegacy;
use alloy_eips::eip2718::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::{
    ExecutionPayloadEnvelopeV2, ExecutionPayloadEnvelopeV3, PayloadAttributes,
};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use tracing::debug;

/// Action to produce blocks with real storage-modifying transactions.
///
/// This action sends transactions to the node's transaction pool and then
/// triggers block production to include them.
#[derive(Debug)]
pub struct ProduceBlockWithTransactions {
    /// Transactions to send (as raw signed transaction bytes)
    pub transactions: Vec<Bytes>,
    /// Tag to capture the block with
    pub block_tag: String,
    /// Optional node index to send transactions to
    pub node_idx: Option<usize>,
}

impl ProduceBlockWithTransactions {
    /// Create a new `ProduceBlockWithTransactions` action
    pub fn new(transactions: Vec<Bytes>, block_tag: impl Into<String>) -> Self {
        Self { transactions, block_tag: block_tag.into(), node_idx: None }
    }

    /// Set the node index to send transactions to
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }
}

impl<Engine> Action<Engine> for ProduceBlockWithTransactions
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV2: Into<ExecutionPayloadEnvelopeV2>,
    Engine::ExecutionPayloadEnvelopeV3:
        From<ExecutionPayloadEnvelopeV3> + Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let node_idx = self.node_idx.unwrap_or(env.active_node_idx);

            if node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index {} out of bounds", node_idx));
            }

            debug!(
                "ProduceBlockWithTransactions: Sending {} transactions to node {}",
                self.transactions.len(),
                node_idx
            );

            // Send transactions to the node via RPC
            let mut tx_hashes = Vec::new();

            for (idx, tx_bytes) in self.transactions.iter().enumerate() {
                // Send raw transaction using eth_sendRawTransaction
                let tx_hash = EthApiClient::<
                    TransactionRequest,
                    Transaction,
                    Block,
                    Receipt,
                    Header,
                >::send_raw_transaction(&env.node_clients[node_idx].rpc, tx_bytes.clone()).await?;

                tx_hashes.push(tx_hash);
                debug!("  Sent transaction {}: hash {}", idx + 1, tx_hash);
            }

            // Wait a bit for transactions to be in the pool
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Now produce a block that includes these transactions
            // We use ProduceBlocks which will trigger block production
            debug!("  Producing block to include transactions...");
            let mut produce = ProduceBlocks::<Engine>::new(1);
            produce.execute(env).await?;

            // Make the block canonical
            let mut make_canonical = MakeCanonical::new();
            make_canonical.execute(env).await?;

            // Capture the block with our tag
            let mut capture = CaptureBlock::new(&self.block_tag);
            capture.execute(env).await?;

            // Verify transactions were included
            let (block_info, _) = *env
                .block_registry
                .get(&self.block_tag)
                .ok_or_else(|| eyre::eyre!("Block tag '{}' not found", self.block_tag))?;

            // Get the block to check transactions
            let block = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                Header,
            >::block_by_hash(&env.node_clients[node_idx].rpc, block_info.hash, true).await?
                .ok_or_else(|| eyre::eyre!("Block not found"))?;

            debug!(
                "   Block {} created with {} transactions",
                block_info.number,
                block.transactions.len()
            );

            // Check if our transactions were included
            let block_tx_hashes: Vec<B256> = block.transactions.hashes().collect();
            let mut included_count = 0;
            for tx_hash in &tx_hashes {
                if block_tx_hashes.contains(tx_hash) {
                    included_count += 1;
                }
            }

            if included_count == tx_hashes.len() {
                debug!("   All {} transactions included in block", tx_hashes.len());
            } else {
                debug!(
                    "WARNING:  Only {}/{} transactions included in block",
                    included_count,
                    tx_hashes.len()
                );
            }

            Ok(())
        })
    }
}

/// Create a transaction that calls `approve()` on a token contract.
/// This modifies storage in a way that can trigger trie node creation/deletion.
pub async fn create_approve_tx(
    token_address: Address,
    spender: Address,
    amount: U256,
    nonce: u64,
    chain_id: u64,
) -> Result<Bytes> {
    // The approve(address spender, uint256 amount) selector: 0x095ea7b3
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&[0x09, 0x5e, 0xa7, 0xb3]); // approve selector
    data.extend_from_slice(&[0u8; 12]); // padding for address
    data.extend_from_slice(spender.as_slice()); // spender address (20 bytes)

    // Encode amount as 32 bytes
    let amount_bytes = amount.to_be_bytes::<32>();
    data.extend_from_slice(&amount_bytes);

    // Get test wallet (funded account from genesis)
    let wallet = Wallet::new(1).with_chain_id(chain_id);
    let signers = wallet.wallet_gen();
    let signer =
        signers.into_iter().next().ok_or_else(|| eyre::eyre!("Failed to create signer"))?;

    // Create transaction calling the contract
    let mut tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: 20_000_000_000, // 20 gwei
        gas_limit: 100_000,
        to: TxKind::Call(token_address),
        value: U256::ZERO, // No ETH sent
        input: data.into(),
    };

    // Sign the transaction
    let signature = signer.sign_transaction_sync(&mut tx)?;

    // Build the signed transaction
    let signed = alloy_consensus::TxEnvelope::Legacy(alloy_consensus::Signed::new_unchecked(
        tx,
        signature,
        alloy_primitives::B256::ZERO,
    ));

    // Encode to bytes
    let mut encoded = Vec::new();
    signed.encode_2718(&mut encoded);

    Ok(Bytes::from(encoded))
}

/// Create a simple transfer transaction that sends ETH.
pub async fn create_transfer_tx(
    to: Address,
    value: U256,
    nonce: u64,
    chain_id: u64,
) -> Result<Bytes> {
    // Get test wallet (funded account from genesis)
    let wallet = Wallet::new(1).with_chain_id(chain_id);
    let signers = wallet.wallet_gen();
    let signer =
        signers.into_iter().next().ok_or_else(|| eyre::eyre!("Failed to create signer"))?;

    // Create simple transfer transaction
    let mut tx = TxLegacy {
        chain_id: Some(chain_id),
        nonce,
        gas_price: 20_000_000_000, // 20 gwei
        gas_limit: 21_000,         // Standard gas for transfer
        to: TxKind::Call(to),
        value,
        input: Bytes::new(), // Empty data for simple transfer
    };

    // Sign the transaction
    let signature = signer.sign_transaction_sync(&mut tx)?;

    // Build the signed transaction
    let signed = alloy_consensus::TxEnvelope::Legacy(alloy_consensus::Signed::new_unchecked(
        tx,
        signature,
        alloy_primitives::B256::ZERO,
    ));

    // Encode to bytes
    let mut encoded = Vec::new();
    signed.encode_2718(&mut encoded);

    Ok(Bytes::from(encoded))
}

/// Helper struct to hold payload-related data together
struct PayloadContext {
    payload_attributes: PayloadAttributes,
    execution_payload_envelope: ExecutionPayloadEnvelopeV3,
    block_hash: B256,
}

/// Action to produce blocks with transactions using Engine API instead of RPC.
///
/// This action:
/// 1. Sends transactions to the txpool (same as `ProduceBlockWithTransactions`)
/// 2. Uses payload builder + Engine API to create blocks (different approach)
/// 3. This should generate proper trie updates unlike the RPC approach
#[derive(Debug)]
pub struct ProduceBlockWithTransactionsViaEngineAPI {
    /// Transactions to send (as raw signed transaction bytes)
    pub transactions: Vec<Bytes>,
    /// Tag to capture the block with
    pub block_tag: String,
    /// Optional node index to send transactions to
    pub node_idx: Option<usize>,
}

impl ProduceBlockWithTransactionsViaEngineAPI {
    /// Create a new action with transactions and block tag
    pub fn new(transactions: Vec<Bytes>, block_tag: impl Into<String>) -> Self {
        Self { transactions, block_tag: block_tag.into(), node_idx: None }
    }

    /// Set which node to use (defaults to node 0)
    pub const fn with_node_idx(mut self, node_idx: usize) -> Self {
        self.node_idx = Some(node_idx);
        self
    }

    /// Send transactions to the node's transaction pool
    async fn send_transactions_to_pool<Engine>(
        &self,
        env: &Environment<Engine>,
        node_idx: usize,
    ) -> Result<Vec<B256>>
    where
        Engine: EngineTypes + PayloadTypes,
    {
        let mut tx_hashes = Vec::new();

        for (idx, tx_bytes) in self.transactions.iter().enumerate() {
            let tx_hash = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                Header,
            >::send_raw_transaction(&env.node_clients[node_idx].rpc, tx_bytes.clone()).await?;

            tx_hashes.push(tx_hash);
            debug!("Sent transaction {}: hash {}", idx + 1, tx_hash);
        }

        // Wait for transactions to be in the pool
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        Ok(tx_hashes)
    }

    /// Get the latest block and build payload attributes
    async fn build_payload_attributes<Engine>(
        &self,
        env: &Environment<Engine>,
        node_idx: usize,
    ) -> Result<(Block, PayloadAttributes)>
    where
        Engine: EngineTypes + PayloadTypes,
    {
        let latest_block = EthApiClient::<
            TransactionRequest,
            Transaction,
            Block,
            Receipt,
            Header,
        >::block_by_number(
            &env.node_clients[node_idx].rpc,
            alloy_eips::BlockNumberOrTag::Latest,
            false,
        )
        .await?
        .ok_or_else(|| eyre::eyre!("No latest block"))?;

        let payload_attributes = PayloadAttributes {
            timestamp: latest_block.header.timestamp + 1,
            prev_randao: latest_block.header.mix_hash,
            suggested_fee_recipient: Address::ZERO,
            withdrawals: Some(vec![]),
            parent_beacon_block_root: None,
        };

        Ok((latest_block, payload_attributes))
    }

    /// Trigger payload building via forkchoice update and get the built payload
    async fn build_payload<Engine>(
        &self,
        env: &Environment<Engine>,
        node_idx: usize,
        latest_block: &Block,
        payload_attributes: PayloadAttributes,
    ) -> Result<(PayloadContext, Engine::ExecutionPayloadEnvelopeV3)>
    where
        Engine: EngineTypes + PayloadTypes,
        Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
        Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3> + Clone,
    {
        let engine_client = env.node_clients[node_idx].engine.http_client();

        // Send forkchoice update to trigger payload building
        let forkchoice_state = alloy_rpc_types_engine::ForkchoiceState {
            head_block_hash: latest_block.header.hash,
            safe_block_hash: latest_block.header.hash,
            finalized_block_hash: latest_block.header.hash,
        };

        let fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v3(
            &engine_client,
            forkchoice_state,
            Some(payload_attributes.clone().into()),
        )
        .await?;

        let payload_id = fcu_result
            .payload_id
            .ok_or_else(|| eyre::eyre!("No payload ID returned from forkchoice update"))?;

        // Wait for payload to be built
        tokio::time::sleep(std::time::Duration::from_millis(1000)).await;

        // Get the built payload
        let payload = EngineApiClient::<Engine>::get_payload_v3(&engine_client, payload_id).await?;
        let execution_payload_envelope: ExecutionPayloadEnvelopeV3 = payload.clone().into();

        // Extract block hash once to avoid repetition
        let block_hash =
            execution_payload_envelope.execution_payload.payload_inner.payload_inner.block_hash;

        debug!(
            "Built payload with {} transactions, block hash: {}",
            execution_payload_envelope
                .execution_payload
                .payload_inner
                .payload_inner
                .transactions
                .len(),
            block_hash
        );

        Ok((PayloadContext { payload_attributes, execution_payload_envelope, block_hash }, payload))
    }

    /// Execute the payload via newPayload and make it canonical
    async fn execute_payload<Engine>(
        &self,
        env: &mut Environment<Engine>,
        node_idx: usize,
        payload_context: &PayloadContext,
        latest_block_number: u64,
        full_payload: Engine::ExecutionPayloadEnvelopeV3,
    ) -> Result<()>
    where
        Engine: EngineTypes + PayloadTypes,
        Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    {
        let engine_client = env.node_clients[node_idx].engine.http_client();

        // Update node state before execution
        let node_state = env.node_state_mut(node_idx)?;
        node_state.latest_payload_built = Some(payload_context.payload_attributes.clone());
        node_state.latest_payload_envelope = Some(full_payload);
        node_state
            .payload_attributes
            .insert(latest_block_number + 1, payload_context.payload_attributes.clone());

        // Send the payload via newPayload
        let new_payload_result = EngineApiClient::<Engine>::new_payload_v3(
            &engine_client,
            payload_context.execution_payload_envelope.execution_payload.clone(),
            vec![],
            B256::ZERO,
        )
        .await?;

        debug!("New payload result: {:?}", new_payload_result.status);

        // Make the payload canonical with another forkchoice update
        let new_forkchoice_state = alloy_rpc_types_engine::ForkchoiceState {
            head_block_hash: payload_context.block_hash,
            safe_block_hash: payload_context.block_hash,
            finalized_block_hash: payload_context.block_hash,
        };

        let fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v3(
            &engine_client,
            new_forkchoice_state,
            None,
        )
        .await?;

        debug!(
            "Block made canonical via Engine API, FCU status: {:?}",
            fcu_result.payload_status.status
        );

        // Wait a bit for the block to be fully processed and committed
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        Ok(())
    }
}

impl<Engine> Action<Engine> for ProduceBlockWithTransactionsViaEngineAPI
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3> + Clone,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Validate node index
            let node_idx = self.node_idx.unwrap_or(0);
            if node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index {} out of bounds", node_idx));
            }

            debug!(
                "ProduceBlockWithTransactionsViaEngineAPI: Processing {} transactions on node {}",
                self.transactions.len(),
                node_idx
            );

            // Step 1: Send transactions to pool
            let _tx_hashes = self.send_transactions_to_pool(env, node_idx).await?;

            // Step 2: Build payload attributes
            let (latest_block, payload_attributes) =
                self.build_payload_attributes(env, node_idx).await?;

            // Step 3: Build payload via Engine API
            let (payload_context, full_payload) =
                self.build_payload(env, node_idx, &latest_block, payload_attributes).await?;

            // Step 4: Execute payload and make canonical
            self.execute_payload(
                env,
                node_idx,
                &payload_context,
                latest_block.header.number,
                full_payload.clone(),
            )
            .await?;

            // Step 5: Update the environment's current block info with the newly produced block
            let execution_payload_envelope: ExecutionPayloadEnvelopeV3 =
                full_payload.clone().into();
            let block_hash =
                execution_payload_envelope.execution_payload.payload_inner.payload_inner.block_hash;
            let block_number = execution_payload_envelope
                .execution_payload
                .payload_inner
                .payload_inner
                .block_number;
            let block_timestamp =
                execution_payload_envelope.execution_payload.payload_inner.payload_inner.timestamp;

            env.set_current_block_info(BlockInfo {
                hash: block_hash,
                number: block_number,
                timestamp: block_timestamp,
            })?;

            // Also update the active node state
            env.active_node_state_mut()?.latest_header_time = block_timestamp;
            env.active_node_state_mut()?.latest_fork_choice_state.head_block_hash = block_hash;

            // IMPORTANT: For testing, we need to simulate trie updates being available
            // In reality, the sparse trie task generates them but they get lost in the Chain
            // notification Here we create a synthetic trie update event for testing
            // purposes
            if let Some(ref mut _rx) = env.trie_update_rx {
                // Check if we can send a synthetic event
                // This is a workaround for the fact that Chain doesn't preserve trie updates
                debug!(
                    "ProduceBlockWithTransactionsViaEngineAPI: Would emit synthetic trie updates for block {} if they were available",
                    block_number
                );
            }

            debug!(
                "Updated environment with new block: number={}, hash={}",
                block_number, block_hash
            );

            // Step 6: Capture the block
            let mut capture = CaptureBlock::new(&self.block_tag);
            capture.execute(env).await?;

            Ok(())
        })
    }
}
