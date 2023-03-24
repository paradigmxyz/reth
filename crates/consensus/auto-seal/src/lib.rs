#![warn(missing_docs, unreachable_pub, unused_crate_dependencies)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! A [Consensus](reth_interfaces::consensus::Consensus) implementation for local testing purposes
//! that automatically seals blocks.
//!
//! The Mining task polls a [], and will return a list of transactions that are ready to be
//! mined.
//!
//! These downloaders poll the miner, assemble the block, and return transactions that are ready to
//! be mined.

use reth_interfaces::consensus::{Consensus, ConsensusError};
use reth_primitives::{
    BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, ChainSpec, Header, SealedBlock,
    SealedHeader, H256, U256,
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{mpsc::UnboundedSender, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::trace;

mod client;
mod mode;
mod task;

pub use crate::client::AutoSealClient;
pub use mode::{FixedBlockTimeMiner, MiningMode, ReadyTransactionMiner};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_transaction_pool::TransactionPool;
pub use task::MiningTask;

/// A consensus implementation intended for local development and testing purposes.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AutoSealConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl AutoSealConsensus {
    /// Create a new instance of [AutoSealConsensus]
    pub fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl Consensus for AutoSealConsensus {
    fn pre_validate_header(
        &self,
        _header: &SealedHeader,
        _parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header(
        &self,
        _header: &SealedHeader,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn pre_validate_block(&self, _block: &SealedBlock) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn has_block_reward(&self, _total_difficulty: U256, _difficulty: U256) -> bool {
        false
    }
}

/// Builder type for configuring the setup
pub struct AutoSealBuilder<Pool> {
    consensus: AutoSealConsensus,
    pool: Pool,
    mode: MiningMode,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage>,
}

// === impl AutoSealBuilder ===

impl<Pool: TransactionPool> AutoSealBuilder<Pool> {
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        pool: Pool,
        to_engine: UnboundedSender<BeaconEngineMessage>,
    ) -> Self {
        let mode = MiningMode::instant(100, pool.pending_transactions_listener());
        Self {
            consensus: AutoSealConsensus::new(chain_spec),
            pool,
            mode,
            storage: Default::default(),
            to_engine,
        }
    }

    /// Sets the [MiningMode] it operates in, default is [MiningMode::Auto]
    pub fn mode(mut self, mode: MiningMode) -> Self {
        self.mode = mode;
        self
    }

    /// Consumes the type and returns all components
    pub fn build(self) -> (AutoSealConsensus, AutoSealClient, MiningTask<Pool>) {
        let Self { consensus, pool, mode, storage, to_engine } = self;
        let client = AutoSealClient::new(storage.clone());
        let task = MiningTask::new(mode, to_engine, storage, pool);
        (consensus, client, task)
    }
}

/// In memory storage
#[derive(Debug, Clone, Default)]
pub(crate) struct Storage {
    inner: Arc<RwLock<StorageInner>>,
}

// == impl Storage ===

impl Storage {
    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner> {
        self.inner.write().await
    }

    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner> {
        self.inner.read().await
    }
}

#[derive(Default, Debug)]
pub(crate) struct StorageInner {
    /// Headers buffered for download.
    pub(crate) headers: HashMap<BlockNumber, Header>,
    /// A mapping between block hash and number.
    pub(crate) hash_to_number: HashMap<BlockHash, BlockNumber>,
    /// Bodies buffered for download.
    pub(crate) bodies: HashMap<BlockHash, BlockBody>,
    /// Tracks best block
    pub(crate) best_block: u64,
    /// Tracks hash of best block
    pub(crate) best_hash: H256,
}

// === impl StorageInner ===

impl StorageInner {
    /// Returns the block hash for the given block number if it exists.
    pub(crate) fn block_hash(&self, num: u64) -> Option<BlockHash> {
        self.hash_to_number.iter().find_map(|(k, v)| num.eq(v).then_some(*k))
    }

    /// Returns the matching header if it exists.
    pub(crate) fn header_by_hash_or_number(
        &self,
        hash_or_num: BlockHashOrNumber,
    ) -> Option<Header> {
        let num = match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.hash_to_number.get(&hash).copied()?,
            BlockHashOrNumber::Number(num) => num,
        };
        self.headers.get(&num).cloned()
    }

    /// Inserts a new header+body pair
    pub(crate) fn insert_new_block(&mut self, header: Header, body: BlockBody) {
        self.best_hash = header.hash_slow();
        self.best_block = header.number;
        trace!(target: "consensus::auto", num=self.best_block, hash=?self.best_hash, "inserting new block");
        self.headers.insert(header.number, header);
        self.bodies.insert(self.best_hash, body);
        self.hash_to_number.insert(self.best_hash, self.best_block);
    }
}
