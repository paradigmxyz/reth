//! A [Consensus] implementation for local testing purposes
//! that automatically seals blocks.
//!
//! The Mining task polls a [`MiningMode`], and will return a list of transactions that are ready to
//! be mined.
//!
//! These downloaders poll the miner, assemble the block, and return transactions that are ready to
//! be mined.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_beacon_consensus::BeaconEngineMessage;
use reth_chainspec::{ChainSpec, EthereumHardforks};
use reth_consensus::{Consensus, ConsensusError, PostExecutionInput};
use reth_engine_primitives::EngineTypes;
use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    eip4844::calculate_excess_blob_gas, proofs, Block, BlockBody, BlockHash, BlockHashOrNumber,
    BlockNumber, BlockWithSenders, Bloom, Header, Requests, SealedBlock, SealedHeader,
    TransactionSigned, Withdrawals, B256, U256,
};
use reth_provider::{BlockReaderIdExt, StateProviderFactory, StateRootProvider};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::TransactionPool;
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::{mpsc::UnboundedSender, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tracing::trace;

mod client;
mod mode;
mod task;

pub use crate::client::AutoSealClient;
pub use mode::{FixedBlockTimeMiner, MiningMode, ReadyTransactionMiner};
use reth_evm::execute::{BlockExecutionOutput, BlockExecutorProvider, Executor};
pub use task::MiningTask;

/// A consensus implementation intended for local development and testing purposes.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AutoSealConsensus {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl AutoSealConsensus {
    /// Create a new instance of [`AutoSealConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl Consensus for AutoSealConsensus {
    fn validate_header(&self, _header: &SealedHeader) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header_against_parent(
        &self,
        _header: &SealedHeader,
        _parent: &SealedHeader,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_header_with_total_difficulty(
        &self,
        _header: &Header,
        _total_difficulty: U256,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_block_pre_execution(&self, _block: &SealedBlock) -> Result<(), ConsensusError> {
        Ok(())
    }

    fn validate_block_post_execution(
        &self,
        _block: &BlockWithSenders,
        _input: PostExecutionInput<'_>,
    ) -> Result<(), ConsensusError> {
        Ok(())
    }
}

/// Builder type for configuring the setup
#[derive(Debug)]
pub struct AutoSealBuilder<Client, Pool, Engine: EngineTypes, EvmConfig> {
    client: Client,
    consensus: AutoSealConsensus,
    pool: Pool,
    mode: MiningMode,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    evm_config: EvmConfig,
}

// === impl AutoSealBuilder ===

impl<Client, Pool, Engine, EvmConfig> AutoSealBuilder<Client, Pool, Engine, EvmConfig>
where
    Client: BlockReaderIdExt,
    Pool: TransactionPool,
    Engine: EngineTypes,
{
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        pool: Pool,
        to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
        mode: MiningMode,
        evm_config: EvmConfig,
    ) -> Self {
        let latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());

        Self {
            storage: Storage::new(latest_header),
            client,
            consensus: AutoSealConsensus::new(chain_spec),
            pool,
            mode,
            to_engine,
            evm_config,
        }
    }

    /// Sets the [`MiningMode`] it operates in, default is [`MiningMode::Auto`]
    pub fn mode(mut self, mode: MiningMode) -> Self {
        self.mode = mode;
        self
    }

    /// Consumes the type and returns all components
    #[track_caller]
    pub fn build(
        self,
    ) -> (AutoSealConsensus, AutoSealClient, MiningTask<Client, Pool, EvmConfig, Engine>) {
        let Self { client, consensus, pool, mode, storage, to_engine, evm_config } = self;
        let auto_client = AutoSealClient::new(storage.clone());
        let task = MiningTask::new(
            Arc::clone(&consensus.chain_spec),
            mode,
            to_engine,
            storage,
            client,
            pool,
            evm_config,
        );
        (consensus, auto_client, task)
    }
}

/// In memory storage
#[derive(Debug, Clone, Default)]
pub(crate) struct Storage {
    inner: Arc<RwLock<StorageInner>>,
}

// == impl Storage ===

impl Storage {
    /// Initializes the [Storage] with the given best block. This should be initialized with the
    /// highest block in the chain, if there is a chain already stored on-disk.
    fn new(best_block: SealedHeader) -> Self {
        let (header, best_hash) = best_block.split();
        let mut storage = StorageInner {
            best_hash,
            total_difficulty: header.difficulty,
            best_block: header.number,
            ..Default::default()
        };
        storage.headers.insert(header.number, header);
        storage.bodies.insert(best_hash, BlockBody::default());
        Self { inner: Arc::new(RwLock::new(storage)) }
    }

    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, StorageInner> {
        self.inner.write().await
    }

    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, StorageInner> {
        self.inner.read().await
    }
}

/// In-memory storage for the chain the auto seal engine is building.
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
    pub(crate) best_hash: B256,
    /// The total difficulty of the chain until this block
    pub(crate) total_difficulty: U256,
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
    pub(crate) fn insert_new_block(&mut self, mut header: Header, body: BlockBody) {
        header.number = self.best_block + 1;
        header.parent_hash = self.best_hash;

        self.best_hash = header.hash_slow();
        self.best_block = header.number;
        self.total_difficulty += header.difficulty;

        trace!(target: "consensus::auto", num=self.best_block, hash=?self.best_hash, "inserting new block");
        self.headers.insert(header.number, header);
        self.bodies.insert(self.best_hash, body);
        self.hash_to_number.insert(self.best_hash, self.best_block);
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    pub(crate) fn build_header_template(
        &self,
        timestamp: u64,
        transactions: &[TransactionSigned],
        ommers: &[Header],
        withdrawals: Option<&Withdrawals>,
        requests: Option<&Requests>,
        chain_spec: &ChainSpec,
    ) -> Header {
        // check previous block for base fee
        let base_fee_per_gas = self.headers.get(&self.best_block).and_then(|parent| {
            parent.next_block_base_fee(chain_spec.base_fee_params_at_timestamp(timestamp))
        });

        let blob_gas_used = if chain_spec.is_cancun_active_at_timestamp(timestamp) {
            let mut sum_blob_gas_used = 0;
            for tx in transactions {
                if let Some(blob_tx) = tx.transaction.as_eip4844() {
                    sum_blob_gas_used += blob_tx.blob_gas();
                }
            }
            Some(sum_blob_gas_used)
        } else {
            None
        };

        let mut header = Header {
            parent_hash: self.best_hash,
            ommers_hash: proofs::calculate_ommers_root(ommers),
            beneficiary: Default::default(),
            state_root: Default::default(),
            transactions_root: proofs::calculate_transaction_root(transactions),
            receipts_root: Default::default(),
            withdrawals_root: withdrawals.map(|w| proofs::calculate_withdrawals_root(w)),
            logs_bloom: Default::default(),
            difficulty: U256::from(2),
            number: self.best_block + 1,
            gas_limit: chain_spec.max_gas_limit,
            gas_used: 0,
            timestamp,
            mix_hash: Default::default(),
            nonce: 0,
            base_fee_per_gas,
            blob_gas_used,
            excess_blob_gas: None,
            extra_data: Default::default(),
            parent_beacon_block_root: None,
            requests_root: requests.map(|r| proofs::calculate_requests_root(&r.0)),
        };

        if chain_spec.is_cancun_active_at_timestamp(timestamp) {
            let parent = self.headers.get(&self.best_block);
            header.parent_beacon_block_root =
                parent.and_then(|parent| parent.parent_beacon_block_root);
            header.blob_gas_used = Some(0);

            let (parent_excess_blob_gas, parent_blob_gas_used) = match parent {
                Some(parent_block)
                    if chain_spec.is_cancun_active_at_timestamp(parent_block.timestamp) =>
                {
                    (
                        parent_block.excess_blob_gas.unwrap_or_default(),
                        parent_block.blob_gas_used.unwrap_or_default(),
                    )
                }
                _ => (0, 0),
            };
            header.excess_blob_gas =
                Some(calculate_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
        }

        header
    }

    /// Builds and executes a new block with the given transactions, on the provided executor.
    ///
    /// This returns the header of the executed block, as well as the poststate from execution.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_execute<Provider, Executor>(
        &mut self,
        transactions: Vec<TransactionSigned>,
        ommers: Vec<Header>,
        provider: &Provider,
        chain_spec: Arc<ChainSpec>,
        executor: &Executor,
    ) -> Result<(SealedHeader, ExecutionOutcome), BlockExecutionError>
    where
        Executor: BlockExecutorProvider,
        Provider: StateProviderFactory,
    {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

        // if shanghai is active, include empty withdrawals
        let withdrawals =
            chain_spec.is_shanghai_active_at_timestamp(timestamp).then_some(Withdrawals::default());
        // if prague is active, include empty requests
        let requests =
            chain_spec.is_prague_active_at_timestamp(timestamp).then_some(Requests::default());

        let header = self.build_header_template(
            timestamp,
            &transactions,
            &ommers,
            withdrawals.as_ref(),
            requests.as_ref(),
            &chain_spec,
        );

        let block = Block {
            header,
            body: transactions,
            ommers: ommers.clone(),
            withdrawals: withdrawals.clone(),
            requests: requests.clone(),
        }
        .with_recovered_senders()
        .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        trace!(target: "consensus::auto", transactions=?&block.body, "executing transactions");

        let mut db = StateProviderDatabase::new(
            provider.latest().map_err(InternalBlockExecutionError::LatestBlock)?,
        );

        // execute the block
        let BlockExecutionOutput {
            state,
            receipts,
            requests: block_execution_requests,
            gas_used,
            ..
        } = executor.executor(&mut db).execute((&block, U256::ZERO).into())?;
        let execution_outcome = ExecutionOutcome::new(
            state,
            receipts.into(),
            block.number,
            vec![block_execution_requests.into()],
        );

        // todo(onbjerg): we should not pass requests around as this is building a block, which
        // means we need to extract the requests from the execution output and compute the requests
        // root here

        let Block { mut header, body, .. } = block.block;
        let body = BlockBody { transactions: body, ommers, withdrawals, requests };

        trace!(target: "consensus::auto", ?execution_outcome, ?header, ?body, "executed block, calculating state root and completing header");

        // now we need to update certain header fields with the results of the execution
        header.state_root = db.state_root(execution_outcome.state())?;
        header.gas_used = gas_used;

        let receipts = execution_outcome.receipts_by_block(header.number);

        // update logs bloom
        let receipts_with_bloom =
            receipts.iter().map(|r| r.as_ref().unwrap().bloom_slow()).collect::<Vec<Bloom>>();
        header.logs_bloom = receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | *r);

        // update receipts root
        header.receipts_root = {
            #[cfg(feature = "optimism")]
            let receipts_root = execution_outcome
                .optimism_receipts_root_slow(header.number, &chain_spec, header.timestamp)
                .expect("Receipts is present");

            #[cfg(not(feature = "optimism"))]
            let receipts_root =
                execution_outcome.receipts_root_slow(header.number).expect("Receipts is present");

            receipts_root
        };
        trace!(target: "consensus::auto", root=?header.state_root, ?body, "calculated root");

        // finally insert into storage
        self.insert_new_block(header.clone(), body);

        // set new header with hash that should have been updated by insert_new_block
        let new_header = header.seal(self.best_hash);

        Ok((new_header, execution_outcome))
    }
}
