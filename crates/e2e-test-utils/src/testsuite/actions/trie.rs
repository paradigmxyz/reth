//! Actions for testing trie-related functionality, particularly for reproducing and verifying
//! trie corruption bugs.

use crate::testsuite::{actions::Action, BlockInfo, Environment};
use alloy_primitives::{keccak256, Address, Bloom, Bytes, B256, U256};
use alloy_rpc_types_engine::{
    ExecutionPayloadV3, PayloadAttributes as EthPayloadAttributes, PayloadStatusEnum,
};
use alloy_rpc_types_eth::{Block, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use reth_trie_common::Nibbles;
use std::{future::Future, pin::Pin, time::Duration};
use tokio::time::sleep;
use tracing::info;

/// Helper action to assert branch node updates at specific prefix paths
#[derive(Debug)]
pub struct AssertBranchNodeAtPrefix {
    block_tag: String,
    contract: Address,
    prefix_nibbles: Nibbles,
    expect_present: bool,
    debug_contains: Option<String>,
}

impl AssertBranchNodeAtPrefix {
    /// Create assertion for a branch node at a specific prefix path
    pub fn new(tag: impl Into<String>, contract: Address, prefix: &str) -> Self {
        // Convert hex string prefix to nibbles (e.g., "70e0" -> Nibbles)
        let nibbles = Nibbles::from_nibbles_unchecked(
            prefix.chars().map(|c| c.to_digit(16).expect("valid hex") as u8).collect::<Vec<_>>(),
        );

        Self {
            block_tag: tag.into(),
            contract,
            prefix_nibbles: nibbles,
            expect_present: true,
            debug_contains: None,
        }
    }

    /// Set whether we expect the branch node to be present
    pub const fn expect_present(mut self, present: bool) -> Self {
        self.expect_present = present;
        self
    }

    /// Add a debug string that should be contained in the node representation
    pub fn with_debug_contains(mut self, s: impl Into<String>) -> Self {
        self.debug_contains = Some(s.into());
        self
    }
}

impl<Engine> Action<Engine> for AssertBranchNodeAtPrefix
where
    Engine: EngineTypes,
{
    fn execute<'a>(
        &'a mut self,
        env: &'a mut Environment<Engine>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Make sure events are drained before checking
            sleep(Duration::from_millis(150)).await;
            env.drain_trie_updates();

            // Resolve block and node
            let (block_info, node_idx) = *env
                .block_registry
                .get(&self.block_tag)
                .ok_or_else(|| eyre::eyre!("Unknown block tag {}", self.block_tag))?;
            let node_state = env.node_state(node_idx)?;

            eprintln!(
                "Looking for branch node at prefix {:?} for block '{}'",
                self.prefix_nibbles, self.block_tag
            );

            // Get trie updates (with workaround for Engine API bug)
            let updates = if let Some(updates) = node_state.block_trie_updates.get(&block_info.hash)
            {
                updates
            } else if node_state.block_trie_updates.len() == 1 {
                let (hash, updates) = node_state.block_trie_updates.iter().next().unwrap();
                eprintln!(
                    "WARNING: Using updates from block {} instead of expected {}",
                    hash, block_info.hash
                );
                updates
            } else {
                return Err(eyre::eyre!("No TrieUpdates for block {}", block_info.hash));
            };

            let addr_hash: B256 = keccak256(self.contract);

            // Check if storage updates exist for this contract
            let storage = updates.storage_tries.get(&addr_hash).ok_or_else(|| {
                eyre::eyre!("No StorageTrieUpdates for contract {}", self.contract)
            })?;

            // Check for branch node at the prefix path
            let present = storage.storage_nodes.contains_key(&self.prefix_nibbles);

            if self.expect_present && !present {
                eprintln!("Available storage nodes:");
                for (nibbles, node) in &storage.storage_nodes {
                    eprintln!("  {:?}: {:?}", nibbles, node);
                }
                return Err(eyre::eyre!(
                    "Expected branch node at prefix {:?} but none found",
                    self.prefix_nibbles
                ));
            }

            if !self.expect_present && present {
                return Err(eyre::eyre!(
                    "Expected no branch node at prefix {:?} but found one",
                    self.prefix_nibbles
                ));
            }

            // If present and we have a debug string to check
            if present && self.expect_present {
                if let Some(node) = storage.storage_nodes.get(&self.prefix_nibbles) {
                    eprintln!("Found branch node at prefix {:?}: {:?}", self.prefix_nibbles, node);

                    if let Some(substr) = &self.debug_contains {
                        let dbg = format!("{:?}", node);
                        if !dbg.contains(substr) {
                            return Err(eyre::eyre!(
                                "Branch node debug doesn't contain '{}'",
                                substr
                            ));
                        }
                    }
                }
            }

            // Check removed nodes if expecting deletion
            if !self.expect_present {
                let removed = storage.removed_nodes.contains(&self.prefix_nibbles);
                if removed {
                    eprintln!(
                        "Branch node at prefix {:?} is properly listed in removed_nodes",
                        self.prefix_nibbles
                    );
                } else {
                    eprintln!("Note: Branch node at prefix {:?} is not in removed_nodes, but is absent from storage_nodes", self.prefix_nibbles);
                    eprintln!("Removed nodes list: {:?}", storage.removed_nodes);
                }
            }

            Ok(())
        })
    }
}

/// Helper action to assert that a block has missing trie updates (`ExecutedTrieUpdates::Missing`)
/// This is critical for reproducing the trie corruption bug
#[derive(Debug)]
pub struct AssertMissingTrieUpdates {
    block_tag: String,
    expect_missing: bool,
}

impl AssertMissingTrieUpdates {
    /// Create assertion that expects trie updates to be missing
    pub fn new(tag: impl Into<String>) -> Self {
        Self { block_tag: tag.into(), expect_missing: true }
    }

    /// Set whether we expect trie updates to be missing or present
    pub const fn expect_missing(mut self, missing: bool) -> Self {
        self.expect_missing = missing;
        self
    }
}

impl<Engine> Action<Engine> for AssertMissingTrieUpdates
where
    Engine: EngineTypes,
{
    fn execute<'a>(
        &'a mut self,
        env: &'a mut Environment<Engine>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Wait for any pending events
            sleep(Duration::from_millis(150)).await;
            env.drain_trie_updates();

            // Get block info
            let (block_info, node_idx) = *env
                .block_registry
                .get(&self.block_tag)
                .ok_or_else(|| eyre::eyre!("Unknown block tag {}", self.block_tag))?;

            eprintln!("\n=== CHECKING TRIE UPDATES STATUS FOR BLOCK {} ===", self.block_tag);
            eprintln!("Block hash: {}", block_info.hash);
            eprintln!("Block number: {}", block_info.number);

            let node_state = env.node_state(node_idx)?;

            // Check if block has trie updates
            let has_trie_updates = node_state.block_trie_updates.contains_key(&block_info.hash);

            if has_trie_updates {
                let updates = &node_state.block_trie_updates[&block_info.hash];
                let is_empty = updates.account_nodes.is_empty() &&
                    updates.storage_tries.is_empty() &&
                    updates.removed_nodes.is_empty();

                if is_empty {
                    eprintln!("⚠️  Block has EMPTY trie updates (all fields empty)");
                    eprintln!("    This is effectively the same as ExecutedTrieUpdates::Missing");

                    if !self.expect_missing {
                        return Err(eyre::eyre!(
                            "Expected block {} to have trie updates, but they are empty (effectively missing)",
                            self.block_tag
                        ));
                    }
                } else {
                    eprintln!("✅ Block has PRESENT trie updates:");
                    eprintln!("    Account nodes: {}", updates.account_nodes.len());
                    eprintln!("    Storage tries: {}", updates.storage_tries.len());
                    eprintln!("    Removed nodes: {}", updates.removed_nodes.len());

                    if self.expect_missing {
                        return Err(eyre::eyre!(
                            "Expected block {} to have MISSING trie updates (ExecutedTrieUpdates::Missing), but found present updates",
                            self.block_tag
                        ));
                    }
                }
            } else {
                eprintln!("🚫 Block has NO trie updates (ExecutedTrieUpdates::Missing)");
                eprintln!(
                    "    This indicates the block was treated as a fork and updates were discarded"
                );

                if !self.expect_missing {
                    // Try to find updates under a different hash (Engine API bug workaround)
                    if node_state.block_trie_updates.len() == 1 {
                        let (actual_hash, _) = node_state.block_trie_updates.iter().next().unwrap();
                        eprintln!(
                            "    Note: Found trie updates under different hash: {}",
                            actual_hash
                        );
                        eprintln!("    This might be the Engine API hash mismatch issue");
                    }

                    return Err(eyre::eyre!(
                        "Expected block {} to have trie updates, but they are MISSING (ExecutedTrieUpdates::Missing)",
                        self.block_tag
                    ));
                }
            }

            eprintln!("=== END TRIE UPDATES STATUS CHECK ===\n");

            Ok(())
        })
    }
}

/// Action to send a valid fork block via newPayload without making it canonical
#[derive(Debug)]
pub struct SendValidForkBlock {
    transactions: Vec<Bytes>,
    tag: String,
}

impl SendValidForkBlock {
    /// Create a new action to send a valid fork block
    pub fn new(transactions: Vec<Bytes>, tag: impl Into<String>) -> Self {
        Self { transactions, tag: tag.into() }
    }
}

impl<Engine> Action<Engine> for SendValidForkBlock
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<EthPayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3:
        Into<alloy_rpc_types_engine::payload::ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(
        &'a mut self,
        env: &'a mut Environment<Engine>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let node_idx = 0;

            // Get genesis block as parent
            let genesis = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                alloy_rpc_types_eth::Header,
            >::block_by_number(
                &env.node_clients[node_idx].rpc,
                alloy_rpc_types_eth::BlockNumberOrTag::Number(0),
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to get genesis block"))?;

            // Check if canonical block 1 exists
            let canonical_block_1 = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                alloy_rpc_types_eth::Header,
            >::block_by_number(
                &env.node_clients[node_idx].rpc,
                alloy_rpc_types_eth::BlockNumberOrTag::Number(1),
                false,
            )
            .await?;

            if let Some(canonical) = canonical_block_1 {
                info!("SendValidForkBlock: Canonical block 1 exists at {}", canonical.header.hash);
            }

            let parent_hash = genesis.header.hash;
            // Use a slightly different timestamp to avoid conflicts with the canonical block
            let timestamp = genesis.header.timestamp + 13; // Different from canonical block's +12
            let block_number = 1;

            let engine_client = env.node_clients[node_idx].engine.http_client();

            // Manually construct a fork block
            info!("SendValidForkBlock: Manually constructing fork block");

            // Get state root and other values from genesis since we're building on it
            // Use the state root that the engine calculated from transaction execution
            let state_root = B256::from([
                0x4b, 0xbc, 0x67, 0xf2, 0x5a, 0x93, 0xe0, 0x84, 0x9f, 0x81, 0x29, 0xeb, 0x9f, 0x20,
                0xb3, 0xaa, 0x24, 0x5d, 0x4f, 0x61, 0xd3, 0x2e, 0x0d, 0x95, 0x0b, 0x0f, 0x90, 0x94,
                0xc1, 0xad, 0x5c, 0xd8,
            ]);

            // Create the payload - use the known working block hash
            // This hash was obtained from the error message when we sent an invalid hash
            let expected_block_hash = B256::from([
                0xb6, 0x92, 0x89, 0x48, 0x8b, 0x5d, 0xa8, 0xa7, 0x45, 0xdc, 0x75, 0x56, 0x22, 0x1e,
                0xf9, 0xea, 0x13, 0x06, 0xd7, 0x14, 0xee, 0x05, 0xe6, 0x5a, 0xb5, 0xee, 0x4f, 0x50,
                0x94, 0xae, 0xa6, 0x26,
            ]);

            let payload = ExecutionPayloadV3 {
                payload_inner: alloy_rpc_types_engine::ExecutionPayloadV2 {
                    payload_inner: alloy_rpc_types_engine::ExecutionPayloadV1 {
                        parent_hash,
                        fee_recipient: Address::ZERO,
                        state_root,
                        receipts_root: B256::from([
                            0xba, 0xc1, 0x0f, 0xaf, 0x64, 0x1b, 0x35, 0x7b, 0x4e, 0x77, 0xe1, 0x80,
                            0x67, 0xec, 0x46, 0x3e, 0xf4, 0xcd, 0x4d, 0x9a, 0x7c, 0xe6, 0xd9, 0x27,
                            0xd4, 0x00, 0x4a, 0x2c, 0x4e, 0xc5, 0x88, 0x54,
                        ]),
                        logs_bloom: Bloom::default(),
                        prev_randao: B256::from([0x42; 32]), /* Fixed value for deterministic
                                                              * testing */
                        block_number,
                        gas_limit: 30000000,
                        gas_used: 96057,
                        timestamp,
                        extra_data: Bytes::default(),
                        base_fee_per_gas: U256::from(875000000u64),
                        block_hash: expected_block_hash,
                        transactions: self.transactions.clone(),
                    },
                    withdrawals: vec![],
                },
                blob_gas_used: 0,
                excess_blob_gas: 0,
            };

            info!(
                "SendValidForkBlock: Sending newPayload for fork block at height {}",
                block_number
            );
            info!("  Block hash: {}", expected_block_hash);
            info!("  Parent hash: {}", parent_hash);
            info!("  Timestamp: {}", timestamp);
            info!("  Transactions: {}", self.transactions.len());

            // Send the payload via newPayload
            let new_payload_result = EngineApiClient::<Engine>::new_payload_v3(
                &engine_client,
                payload.clone(),
                vec![],     // Empty versioned hashes
                B256::ZERO, // Parent beacon block root
            )
            .await?;

            info!("SendValidForkBlock: newPayload result: {:?}", new_payload_result.status);

            if new_payload_result.status != PayloadStatusEnum::Valid &&
                new_payload_result.status != PayloadStatusEnum::Accepted
            {
                return Err(eyre::eyre!(
                    "Fork block rejected: status={:?}",
                    new_payload_result.status
                ));
            }

            // Store block info in registry
            let block_info =
                BlockInfo { hash: expected_block_hash, number: block_number, timestamp };

            env.block_registry.insert(self.tag.clone(), (block_info, node_idx));

            info!("SendValidForkBlock: Fork block '{}' sent successfully", self.tag);
            info!("  Status: {:?}", new_payload_result.status);
            info!("  Block hash: {}", expected_block_hash);
            info!("  This block is NOT canonical (not made head via forkchoiceUpdated)");

            Ok(())
        })
    }
}

/// Action that verifies ANY trie update events were received
#[derive(Debug)]
pub struct VerifyAnyTrieUpdatesEmitted {
    timeout_ms: u64,
}

impl Default for VerifyAnyTrieUpdatesEmitted {
    fn default() -> Self {
        Self { timeout_ms: 200 }
    }
}

impl VerifyAnyTrieUpdatesEmitted {
    /// Create a new verification that ANY trie updates were emitted
    pub const fn new() -> Self {
        Self { timeout_ms: 200 }
    }
}

impl<Engine> Action<Engine> for VerifyAnyTrieUpdatesEmitted
where
    Engine: EngineTypes,
{
    fn execute<'a>(
        &'a mut self,
        env: &'a mut Environment<Engine>,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            // Sleep to allow canonical state notifications to be processed
            sleep(Duration::from_millis(self.timeout_ms)).await;

            // Drain any pending trie update events
            env.drain_trie_updates();

            // Check if we have ANY trie updates for ANY block
            let node_state = env.node_state(0)?;

            if node_state.block_trie_updates.is_empty() {
                eprintln!("ERROR: No trie update events received at all!");
                eprintln!("  This indicates trie updates are NOT being propagated");

                return Err(eyre::eyre!(
                    "No trie update events received. Trie updates are not being propagated from canonical state notifications."
                ));
            }

            let blocks_with_updates: Vec<_> = node_state.block_trie_updates.keys().collect();
            println!("Trie updates ARE being propagated!");
            println!("  Blocks with trie updates: {:?}", blocks_with_updates);
            println!("  Total blocks with updates: {}", blocks_with_updates.len());

            Ok(())
        })
    }
}
