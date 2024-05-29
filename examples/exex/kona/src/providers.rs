//! Provider adapters for kona.

use anyhow::Result;
use std::boxed::Box;
use std::sync::{Arc, Mutex};
use async_trait::async_trait;
use std::collections::HashMap;
use kona_derive::types::BlockInfo;
use kona_derive::traits::ChainProvider;
use kona_derive::traits::L2ChainProvider;
use reth::primitives::SealedBlockWithSenders;
use reth_primitives::alloy_primitives::B256;
use alloy_consensus::{Receipt, TxEnvelope, Header};
use kona_primitives::{SystemConfig, L2BlockInfo, RollupConfig, L2ExecutionPayloadEnvelope};

/// Stores and serves locally-derived L2 Chain data in memory.
#[derive(Debug, Clone)]
pub struct InMemoryL2ChainProvider {
    /// L2 Safe Blocks.
    blocks_by_number: HashMap<u64, L2BlockInfo>,
    /// Payload Attributes by number.
    payloads_by_number: HashMap<u64, L2ExecutionPayloadEnvelope>,
    /// Maps block numbers to a [SystemConfig] at that point in time.
    configs_by_number: HashMap<u64, SystemConfig>,
}

impl InMemoryL2ChainProvider {
    /// Instantiates a new [InMemoryL2ChainProvider].
    pub fn new() -> Self {
        Self {
            blocks_by_number: HashMap::new(),
            payloads_by_number: HashMap::new(),
            configs_by_number: HashMap::new(),
        }
    }

    /// Prunes all [L2BlockInfo] and [L2ExecutionPayloadEnvelope]s that
    /// have L1 Origins greater than the given number.
    /// Also prunes [SystemConfig]s that were set after the given number.
    pub fn prune(&mut self, number: u64) {
        // Get all l2 block numbers that have an l1 origin greater than the given number.
        let block_numbers_to_prune: Vec<u64> = self.blocks_by_number.iter()
            .filter(|(_, v)| v.l1_origin.number > number)
            .map(|(k, _)| *k)
            .collect();
        // Prune all l2 blocks with numbers in the block_numbers_to_prune list.
        for number in block_numbers_to_prune {
            self.blocks_by_number.remove(&number);
            self.payloads_by_number.remove(&number);
            self.configs_by_number.remove(&number);
        }
    }
}

/// [L2ChainProvider] for the execution extension.
#[derive(Debug, Clone)]
pub struct ExExL2ChainProvider(Arc<Mutex<InMemoryL2ChainProvider>>);

impl ExExL2ChainProvider {
    /// Creates a new [ExExL2ChainProvider].
    pub fn new(inner: Arc<Mutex<InMemoryL2ChainProvider>>) -> Self {
        Self(inner)
    }
}

#[async_trait]
impl L2ChainProvider for ExExL2ChainProvider {
    /// Returns the L2 block info given a block number.
    /// Errors if the block does not exist.
    async fn l2_block_info_by_number(&mut self, number: u64) -> Result<L2BlockInfo> {
        let err = |number: u64| anyhow::anyhow!("Failed to find L2 block info for block number {}", number);
        let locked = self.0.lock().map_err(|_| err(number))?;
        locked.blocks_by_number.get(&number).ok_or_else(|| err(number)).cloned()
    }

    /// Returns an execution payload for a given number.
    /// Errors if the execution payload does not exist.
    async fn payload_by_number(&mut self, number: u64) -> Result<L2ExecutionPayloadEnvelope> {
        let err = |number: u64| anyhow::anyhow!("Failed to find payload for block number {}", number);
        let locked = self.0.lock().map_err(|_| err(number))?;
        locked.payloads_by_number.get(&number).ok_or_else(|| err(number)).cloned()
    }

    /// Returns the [SystemConfig] by L2 number.
    async fn system_config_by_number(&mut self,number: u64, _: Arc<RollupConfig>) -> Result<SystemConfig> {
        let err = |number: u64| anyhow::anyhow!("Failed to find system config for block number {}", number);
        let locked = self.0.lock().map_err(|_| err(number))?;
        locked.configs_by_number.get(&number).ok_or_else(|| err(number)).cloned()
    }
}

/// InMemoryChainProvider
///
/// Implements the [ChainProvider] trait with in memory maps.
#[derive(Debug, Clone, PartialEq)]
pub struct InMemoryChainProvider {
    /// Maps block numbers to their respective [SealedBlockWithSenders].
    pub blocks: HashMap<u64, SealedBlockWithSenders>,
    /// Maps block hashes to their respective [Vec<Receipt>].
    pub receipts: HashMap<B256, Vec<Option<Receipt>>>,
}

impl InMemoryChainProvider {
    /// Creates a new [InMemoryChainProvider] with empty maps.
    pub fn new() -> Self {
        Self {
            blocks: HashMap::new(),
            receipts: HashMap::new(),
        }
    }

    /// Removes all blocks and receipts from the provider after the given block number.
    pub fn prune(&mut self, number: u64) {
        self.blocks.retain(|k, b| {
            if *k <= number { return true; }
            self.receipts.remove(&b.hash());
            false
        });
    }

    /// Inserts a list of blocks into the provider.
    pub fn insert_blocks(&mut self, blocks: Vec<SealedBlockWithSenders>) {
        for block in blocks {
            self.blocks.insert(block.number, block);
        }
    }

    /// Inserts a list of receipts into the provider.
    pub fn insert_receipts(&mut self, receipts: Vec<(B256, Vec<Option<Receipt>>)>) {
        for (hash, receipt) in receipts {
            if self.receipts.contains_key(&hash) {
                self.receipts.get_mut(&hash).expect(
                    "Receipts not found"
                ).extend(receipt.clone());
                continue;
            }
            self.receipts.insert(hash, receipt.clone());
        }
    }
}

fn simplify_block(block: &SealedBlockWithSenders) -> BlockInfo {
    BlockInfo {
        hash: block.hash(),
        number: block.number,
        parent_hash: block.parent_hash,
        timestamp: block.timestamp,
    }
}

/// [ChainProvider] for the execution extension.
#[derive(Debug, Clone)]
pub struct ExExChainProvider(Arc<Mutex<InMemoryChainProvider>>);

impl ExExChainProvider {
    /// Creates a new [ExExL2ChainProvider].
    pub fn new(inner: Arc<Mutex<InMemoryChainProvider>>) -> Self {
        Self(inner)
    }
}

#[async_trait]
impl ChainProvider for ExExChainProvider {
    async fn block_info_by_number(&mut self, number: u64) -> Result<BlockInfo> {
        let err = |number: u64| anyhow::anyhow!("Failed to find block info for block number {}", number);
        let locked = self.0.lock().map_err(|_| err(number))?;
        locked.blocks.get(&number).map(simplify_block).ok_or_else(|| err(number))
    }

    async fn receipts_by_hash(&mut self, hash: B256) -> Result<Vec<Receipt>> {
        let err = |hash: B256| anyhow::anyhow!("Failed to find receipts for block {}", hash);
        let locked = self.0.lock().map_err(|_| err(hash))?;
        let receipts = locked.receipts.get(&hash).ok_or_else(|| err(hash))?;
        Ok(receipts.iter().flatten().cloned().collect::<Vec<Receipt>>())
    }

    /// `header_by_hash` is not used by the [L1Traversal][1] stage of the
    /// derivation pipeline.
    ///
    /// [1]: kona_derive::stages::L1Traversal
    async fn header_by_hash(&mut self, _: B256) -> Result<Header> {
        unimplemented!()
    }

    /// `block_info_and_transactions_by_hash` is unused for the [L1Traversal][1]
    /// stage of the derivation pipeline.
    ///
    /// [1]: kona_derive::stages::L1Traversal
    async fn block_info_and_transactions_by_hash(
        &mut self,
        _: B256,
    ) -> Result<(BlockInfo, Vec<TxEnvelope>)> {
        unimplemented!()
    }
}
