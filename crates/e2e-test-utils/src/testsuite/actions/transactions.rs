//! Transaction-related actions for the e2e testing framework.
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
//! Also provides transaction creation utilities for common test scenarios.

use crate::{
    testsuite::{
        actions::{Action, CaptureBlock, MakeCanonical, ProduceBlocks},
        Environment,
    },
    wallet::Wallet,
};
use alloy_consensus::{Signed, TxEnvelope, TxLegacy};
use alloy_eips::eip2718::Encodable2718;
use alloy_network::TxSignerSync;
use alloy_primitives::{Address, Bytes, TxKind, B256, U256};
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV3, PayloadAttributes};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::EthApiClient;
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
    pub fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }
}

impl<Engine> Action<Engine> for ProduceBlockWithTransactions
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3: Into<ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let node_idx = self.node_idx.unwrap_or(env.active_node_idx);

            if node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index out of bounds: {}", node_idx));
            }

            debug!("Sending {} transactions to node {}", self.transactions.len(), node_idx);

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
                >::send_raw_transaction(&env.node_clients[node_idx].rpc, tx_bytes.clone())
                .await?;

                tx_hashes.push(tx_hash);
                debug!("Sent transaction {}: hash {}", idx + 1, tx_hash);
            }

            // Wait a bit for transactions to be in the pool
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            // Now produce a block that includes these transactions
            // We use ProduceBlocks which will trigger block production
            debug!("Producing block to include transactions");
            let mut produce = ProduceBlocks::<Engine>::new(1);
            produce.execute(env).await?;

            // Make the block canonical
            let mut make_canonical = MakeCanonical::new();
            make_canonical.execute(env).await?;

            // Capture the block with our tag
            let mut capture = CaptureBlock::new(&self.block_tag);
            capture.execute(env).await?;

            // Verify transactions were included
            let (block_info, _) = env
                .block_registry
                .get(&self.block_tag)
                .ok_or_else(|| eyre::eyre!("Block tag '{}' not found in registry", self.block_tag))?
                .clone();

            // Get the block to check transactions
            let block = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                Header,
            >::block_by_hash(&env.node_clients[node_idx].rpc, block_info.hash, true)
            .await?
            .ok_or_else(|| eyre::eyre!("Block not found for hash"))?;

            debug!(
                "Block {} created with {} transactions",
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
                debug!("All {} transactions included in block", tx_hashes.len());
            } else {
                debug!(
                    "Only {}/{} transactions included in block",
                    included_count,
                    tx_hashes.len()
                );
            }

            Ok(())
        })
    }
}

// Transaction creation utilities

/// Create a transaction that calls approve() on a token contract.
/// This modifies storage in a way that can trigger trie node creation/deletion.
///
/// # Arguments
/// * `token_address` - Address of the token contract
/// * `spender` - Address to approve for spending
/// * `amount` - Amount to approve
/// * `nonce` - Transaction nonce
/// * `chain_id` - Chain ID for the transaction
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
        signers.into_iter().next().ok_or_else(|| eyre::eyre!("Failed to create test signer"))?;

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
    let signed = TxEnvelope::Legacy(Signed::new_unchecked(tx, signature, B256::ZERO));

    // Encode to bytes
    let mut encoded = Vec::new();
    signed.encode_2718(&mut encoded);

    Ok(Bytes::from(encoded))
}

/// Create a simple transfer transaction that sends ETH.
///
/// # Arguments
/// * `to` - Recipient address
/// * `value` - Amount of ETH to send (in wei)
/// * `nonce` - Transaction nonce
/// * `chain_id` - Chain ID for the transaction
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
        signers.into_iter().next().ok_or_else(|| eyre::eyre!("Failed to create test signer"))?;

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
    let signed = TxEnvelope::Legacy(Signed::new_unchecked(tx, signature, B256::ZERO));

    // Encode to bytes
    let mut encoded = Vec::new();
    signed.encode_2718(&mut encoded);

    Ok(Bytes::from(encoded))
}

/// Helper to create approve transaction for the default test token contract.
/// The default contract address is 0x77d34361f991fa724ff1db9b1d760063a16770db
/// which is used in the trie corruption tests.
pub async fn create_test_approve_tx(
    spender: Address,
    amount: U256,
    nonce: u64,
    chain_id: u64,
) -> Result<Bytes> {
    let token_address = Address::parse_checksummed("0x77d34361f991fa724ff1db9b1d760063a16770db", None)
        .map_err(|e| eyre::eyre!("Invalid test token address: {}", e))?;
    create_approve_tx(token_address, spender, amount, nonce, chain_id).await
}
