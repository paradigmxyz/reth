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

use alloy_primitives::{BlockHash, BlockNumber, Bloom, B256, U256};
use reth_beacon_consensus::BeaconEngineMessage;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_consensus::{Consensus, ConsensusError, PostExecutionInput};
use reth_engine_primitives::EngineTypes;
use reth_execution_errors::{
    BlockExecutionError, BlockValidationError, InternalBlockExecutionError,
};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    proofs, Block, BlockBody, BlockHashOrNumber, BlockWithSenders, Header, Requests, SealedBlock,
    SealedHeader, TransactionSigned, Withdrawals,
};
use reth_provider::{BlockReaderIdExt, StateProviderFactory, StateRootProvider};
use reth_revm::database::StateProviderDatabase;
use reth_transaction_pool::TransactionPool;
use reth_trie::HashedPostState;
use revm_primitives::calc_excess_blob_gas;
use std::{
    collections::HashMap,
    fmt::Debug,
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
use reth_evm::execute::{BlockExecutorProvider, Executor};
pub use task::MiningTask;

/// A consensus implementation intended for local development and testing purposes.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AutoSealConsensus<ChainSpec> {
    /// Configuration
    chain_spec: Arc<ChainSpec>,
}

impl<ChainSpec> AutoSealConsensus<ChainSpec> {
    /// Create a new instance of [`AutoSealConsensus`]
    pub const fn new(chain_spec: Arc<ChainSpec>) -> Self {
        Self { chain_spec }
    }
}

impl<ChainSpec: Send + Sync + Debug> Consensus for AutoSealConsensus<ChainSpec> {
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
pub struct AutoSealBuilder<Client, Pool, Engine: EngineTypes, EvmConfig, ChainSpec> {
    client: Client,
    consensus: AutoSealConsensus<ChainSpec>,
    pool: Pool,
    mode: MiningMode,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage<Engine>>,
    evm_config: EvmConfig,
}

// === impl AutoSealBuilder ===

impl<Client, Pool, Engine, EvmConfig, ChainSpec>
    AutoSealBuilder<Client, Pool, Engine, EvmConfig, ChainSpec>
where
    Client: BlockReaderIdExt,
    Pool: TransactionPool,
    Engine: EngineTypes,
    ChainSpec: EthChainSpec,
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
        let latest_header = client.latest_header().ok().flatten().unwrap_or_else(|| {
            SealedHeader::new(chain_spec.genesis_header().clone(), chain_spec.genesis_hash())
        });

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
    ) -> (
        AutoSealConsensus<ChainSpec>,
        AutoSealClient,
        MiningTask<Client, Pool, EvmConfig, Engine, ChainSpec>,
    ) {
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
    pub(crate) fn build_header_template<ChainSpec>(
        &self,
        timestamp: u64,
        transactions: &[TransactionSigned],
        ommers: &[Header],
        withdrawals: Option<&Withdrawals>,
        requests: Option<&Requests>,
        chain_spec: &ChainSpec,
    ) -> Header
    where
        ChainSpec: EthChainSpec + EthereumHardforks,
    {
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
            transactions_root: proofs::calculate_transaction_root(transactions),
            withdrawals_root: withdrawals.map(|w| proofs::calculate_withdrawals_root(w)),
            difficulty: U256::from(2),
            number: self.best_block + 1,
            gas_limit: chain_spec.max_gas_limit(),
            timestamp,
            base_fee_per_gas,
            blob_gas_used: blob_gas_used.map(Into::into),
            requests_root: requests.map(|r| proofs::calculate_requests_root(&r.0)),
            ..Default::default()
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
                Some(calc_excess_blob_gas(parent_excess_blob_gas, parent_blob_gas_used))
        }

        header
    }

    /// Builds and executes a new block with the given transactions, on the provided executor.
    ///
    /// This returns the header of the executed block, as well as the poststate from execution.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_and_execute<Provider, Executor, ChainSpec>(
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
        ChainSpec: EthChainSpec + EthereumHardforks,
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
            body: BlockBody {
                transactions,
                ommers: ommers.clone(),
                withdrawals: withdrawals.clone(),
                requests: requests.clone(),
            },
        }
        .with_recovered_senders()
        .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        trace!(target: "consensus::auto", transactions=?&block.body, "executing transactions");

        let mut db = StateProviderDatabase::new(
            provider.latest().map_err(InternalBlockExecutionError::LatestBlock)?,
        );

        // execute the block
        let block_execution_output =
            executor.executor(&mut db).execute((&block, U256::ZERO).into())?;
        let gas_used = block_execution_output.gas_used;
        let execution_outcome = ExecutionOutcome::from((block_execution_output, block.number));
        let hashed_state = HashedPostState::from_bundle_state(&execution_outcome.state().state);

        // todo(onbjerg): we should not pass requests around as this is building a block, which
        // means we need to extract the requests from the execution output and compute the requests
        // root here

        let Block { mut header, body, .. } = block.block;
        let body = BlockBody { transactions: body.transactions, ommers, withdrawals, requests };

        trace!(target: "consensus::auto", ?execution_outcome, ?header, ?body, "executed block, calculating state root and completing header");

        // now we need to update certain header fields with the results of the execution
        header.state_root = db.state_root(hashed_state)?;
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
                .generic_receipts_root_slow(header.number, |receipts| {
                    reth_optimism_consensus::calculate_receipt_root_no_memo_optimism(
                        receipts,
                        &chain_spec,
                        header.timestamp,
                    )
                })
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
        let new_header = SealedHeader::new(header, self.best_hash);

        Ok((new_header, execution_outcome))
    }
}

#[cfg(test)]
mod tests {
    use reth_chainspec::{ChainHardforks, ChainSpec, EthereumHardfork, ForkCondition};
    use reth_primitives::Transaction;

    use super::*;

    #[test]
    fn test_block_hash() {
        let mut storage = StorageInner::default();

        // Define two block hashes and their corresponding block numbers.
        let block_hash_1: BlockHash = B256::random();
        let block_number_1: BlockNumber = 1;
        let block_hash_2: BlockHash = B256::random();
        let block_number_2: BlockNumber = 2;

        // Insert the block number and hash pairs into the `hash_to_number` map.
        storage.hash_to_number.insert(block_hash_1, block_number_1);
        storage.hash_to_number.insert(block_hash_2, block_number_2);

        // Verify that `block_hash` returns the correct block hash for the given block number.
        assert_eq!(storage.block_hash(block_number_1), Some(block_hash_1));
        assert_eq!(storage.block_hash(block_number_2), Some(block_hash_2));

        // Test that `block_hash` returns `None` for a non-existent block number.
        let block_number_3: BlockNumber = 3;
        assert_eq!(storage.block_hash(block_number_3), None);
    }

    #[test]
    fn test_header_by_hash_or_number() {
        let mut storage = StorageInner::default();

        // Define block numbers, headers, and hashes.
        let block_number_1: u64 = 1;
        let block_number_2: u64 = 2;
        let header_1 = Header { number: block_number_1, ..Default::default() };
        let header_2 = Header { number: block_number_2, ..Default::default() };
        let block_hash_1: BlockHash = B256::random();
        let block_hash_2: BlockHash = B256::random();

        // Insert headers and hash-to-number mappings.
        storage.headers.insert(block_number_1, header_1.clone());
        storage.headers.insert(block_number_2, header_2.clone());
        storage.hash_to_number.insert(block_hash_1, block_number_1);
        storage.hash_to_number.insert(block_hash_2, block_number_2);

        // Test header retrieval by block number.
        assert_eq!(
            storage.header_by_hash_or_number(BlockHashOrNumber::Number(block_number_1)),
            Some(header_1.clone())
        );
        assert_eq!(
            storage.header_by_hash_or_number(BlockHashOrNumber::Number(block_number_2)),
            Some(header_2.clone())
        );

        // Test header retrieval by block hash.
        assert_eq!(
            storage.header_by_hash_or_number(BlockHashOrNumber::Hash(block_hash_1)),
            Some(header_1)
        );
        assert_eq!(
            storage.header_by_hash_or_number(BlockHashOrNumber::Hash(block_hash_2)),
            Some(header_2)
        );

        // Test non-existent block number and hash.
        assert_eq!(storage.header_by_hash_or_number(BlockHashOrNumber::Number(999)), None);
        let non_existent_hash: BlockHash = B256::random();
        assert_eq!(
            storage.header_by_hash_or_number(BlockHashOrNumber::Hash(non_existent_hash)),
            None
        );
    }

    #[test]
    fn test_insert_new_block() {
        let mut storage = StorageInner::default();

        // Define headers and block bodies.
        let header_1 = Header { difficulty: U256::from(100), ..Default::default() };
        let body_1 = BlockBody::default();
        let header_2 = Header { difficulty: U256::from(200), ..Default::default() };
        let body_2 = BlockBody::default();

        // Insert the first block.
        storage.insert_new_block(header_1.clone(), body_1.clone());
        let best_block_1 = storage.best_block;
        let best_hash_1 = storage.best_hash;

        // Verify the block was inserted correctly.
        assert_eq!(
            storage.headers.get(&best_block_1),
            Some(&Header { number: 1, ..header_1.clone() })
        );
        assert_eq!(storage.bodies.get(&best_hash_1), Some(&body_1));
        assert_eq!(storage.hash_to_number.get(&best_hash_1), Some(&best_block_1));

        // Insert the second block.
        storage.insert_new_block(header_2.clone(), body_2.clone());
        let best_block_2 = storage.best_block;
        let best_hash_2 = storage.best_hash;

        // Verify the second block was inserted correctly.
        assert_eq!(
            storage.headers.get(&best_block_2),
            Some(&Header {
                number: 2,
                parent_hash: Header { number: 1, ..header_1 }.hash_slow(),
                ..header_2
            })
        );
        assert_eq!(storage.bodies.get(&best_hash_2), Some(&body_2));
        assert_eq!(storage.hash_to_number.get(&best_hash_2), Some(&best_block_2));

        // Check that the total difficulty was updated.
        assert_eq!(storage.total_difficulty, header_1.difficulty + header_2.difficulty);
    }

    #[test]
    fn test_build_basic_header_template() {
        let mut storage = StorageInner::default();
        let chain_spec = ChainSpec::default();

        let best_block_number = 1;
        let best_block_hash = B256::random();
        let timestamp = 1_600_000_000;

        // Set up best block information
        storage.best_block = best_block_number;
        storage.best_hash = best_block_hash;

        // Build header template
        let header = storage.build_header_template(
            timestamp,
            &[],  // no transactions
            &[],  // no ommers
            None, // no withdrawals
            None, // no requests
            &chain_spec,
        );

        // Verify basic fields
        assert_eq!(header.parent_hash, best_block_hash);
        assert_eq!(header.number, best_block_number + 1);
        assert_eq!(header.timestamp, timestamp);
        assert_eq!(header.gas_limit, chain_spec.max_gas_limit);
    }

    #[test]
    fn test_ommers_and_transactions_roots() {
        let storage = StorageInner::default();
        let chain_spec = ChainSpec::default();
        let timestamp = 1_600_000_000;

        // Setup ommers and transactions
        let ommers = vec![Header::default()];
        let transactions = vec![TransactionSigned::default()];

        // Build header template
        let header = storage.build_header_template(
            timestamp,
            &transactions,
            &ommers,
            None, // no withdrawals
            None, // no requests
            &chain_spec,
        );

        // Verify ommers and transactions roots
        assert_eq!(header.ommers_hash, proofs::calculate_ommers_root(&ommers));
        assert_eq!(header.transactions_root, proofs::calculate_transaction_root(&transactions));
    }

    // Test base fee calculation from the parent block
    #[test]
    fn test_base_fee_calculation() {
        let mut storage = StorageInner::default();
        let chain_spec = ChainSpec::default();
        let timestamp = 1_600_000_000;

        // Set up the parent header with base fee
        let base_fee = Some(100);
        let parent_header = Header { base_fee_per_gas: base_fee, ..Default::default() };
        storage.headers.insert(storage.best_block, parent_header);

        // Build header template
        let header = storage.build_header_template(
            timestamp,
            &[],  // no transactions
            &[],  // no ommers
            None, // no withdrawals
            None, // no requests
            &chain_spec,
        );

        // Verify base fee is correctly propagated
        assert_eq!(header.base_fee_per_gas, base_fee);
    }

    // Test blob gas and excess blob gas calculation when Cancun is active
    #[test]
    fn test_blob_gas_calculation_cancun() {
        let storage = StorageInner::default();
        let chain_spec = ChainSpec {
            hardforks: ChainHardforks::new(vec![(
                EthereumHardfork::Cancun.boxed(),
                ForkCondition::Timestamp(25),
            )]),
            ..Default::default()
        };
        let timestamp = 26;

        // Set up a transaction with blob gas
        let blob_tx = TransactionSigned {
            transaction: Transaction::Eip4844(Default::default()),
            ..Default::default()
        };
        let transactions = vec![blob_tx];

        // Build header template
        let header = storage.build_header_template(
            timestamp,
            &transactions,
            &[],  // no ommers
            None, // no withdrawals
            None, // no requests
            &chain_spec,
        );

        // Verify that the header has the correct fields including blob gas
        assert_eq!(
            header,
            Header {
                parent_hash: B256::ZERO,
                ommers_hash: proofs::calculate_ommers_root(&[]),
                transactions_root: proofs::calculate_transaction_root(&transactions),
                withdrawals_root: None,
                difficulty: U256::from(2),
                number: 1,
                gas_limit: chain_spec.max_gas_limit,
                timestamp,
                base_fee_per_gas: None,
                blob_gas_used: Some(0),
                requests_root: None,
                excess_blob_gas: Some(0),
                ..Default::default()
            }
        );
    }
}
