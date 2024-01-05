//! A [Consensus] implementation for local testing purposes
//! that automatically seals blocks.
//!
//! The Mining task polls a [MiningMode], and will return a list of transactions that are ready to
//! be mined.
//!
//! These downloaders poll the miner, assemble the block, and return transactions that are ready to
//! be mined.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use reth_beacon_consensus::BeaconEngineMessage;
use reth_interfaces::{
    consensus::{Consensus, ConsensusError},
    executor::{BlockExecutionError, BlockValidationError},
};
use reth_payload_builder::EthEngineTypes;
use reth_primitives::{
    constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
    proofs, Block, BlockBody, BlockHash, BlockHashOrNumber, BlockNumber, BlockWithSenders, Bloom,
    ChainSpec, Header, ReceiptWithBloom, SealedBlock, SealedHeader, TransactionSigned, B256,
    EMPTY_OMMER_ROOT_HASH, U256,
};
use reth_provider::{
    BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, CanonStateNotificationSender,
    StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
    processor::EVMProcessor, State,
};
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

    fn validate_block(&self, _block: &SealedBlock) -> Result<(), ConsensusError> {
        Ok(())
    }
}

/// Builder type for configuring the setup
#[derive(Debug)]
pub struct AutoSealBuilder<Client, Pool> {
    client: Client,
    consensus: AutoSealConsensus,
    pool: Pool,
    mode: MiningMode,
    storage: Storage,
    to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
    canon_state_notification: CanonStateNotificationSender,
}

// === impl AutoSealBuilder ===

impl<Client, Pool: TransactionPool> AutoSealBuilder<Client, Pool>
where
    Client: BlockReaderIdExt,
{
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        chain_spec: Arc<ChainSpec>,
        client: Client,
        pool: Pool,
        to_engine: UnboundedSender<BeaconEngineMessage<EthEngineTypes>>,
        canon_state_notification: CanonStateNotificationSender,
        mode: MiningMode,
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
            canon_state_notification,
        }
    }

    /// Sets the [MiningMode] it operates in, default is [MiningMode::Auto]
    pub fn mode(mut self, mode: MiningMode) -> Self {
        self.mode = mode;
        self
    }

    /// Consumes the type and returns all components
    #[track_caller]
    pub fn build(self) -> (AutoSealConsensus, AutoSealClient, MiningTask<Client, Pool>) {
        let Self { client, consensus, pool, mode, storage, to_engine, canon_state_notification } =
            self;
        let auto_client = AutoSealClient::new(storage.clone());
        let task = MiningTask::new(
            Arc::clone(&consensus.chain_spec),
            mode,
            to_engine,
            canon_state_notification,
            storage,
            client,
            pool,
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
    fn new(header: SealedHeader) -> Self {
        let (header, best_hash) = header.split();
        let mut storage = StorageInner {
            best_hash,
            total_difficulty: header.difficulty,
            best_block: header.number,
            ..Default::default()
        };
        storage.headers.insert(0, header);
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
        transactions: &[TransactionSigned],
        chain_spec: Arc<ChainSpec>,
    ) -> Header {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

        // check previous block for base fee
        let base_fee_per_gas = self
            .headers
            .get(&self.best_block)
            .and_then(|parent| parent.next_block_base_fee(chain_spec.base_fee_params(timestamp)));

        let mut header = Header {
            parent_hash: self.best_hash,
            ommers_hash: EMPTY_OMMER_ROOT_HASH,
            beneficiary: Default::default(),
            state_root: Default::default(),
            transactions_root: Default::default(),
            receipts_root: Default::default(),
            withdrawals_root: None,
            logs_bloom: Default::default(),
            difficulty: U256::from(2),
            number: self.best_block + 1,
            gas_limit: ETHEREUM_BLOCK_GAS_LIMIT,
            gas_used: 0,
            timestamp,
            mix_hash: Default::default(),
            nonce: 0,
            base_fee_per_gas,
            blob_gas_used: None,
            excess_blob_gas: None,
            extra_data: Default::default(),
            parent_beacon_block_root: None,
        };

        header.transactions_root = if transactions.is_empty() {
            EMPTY_TRANSACTIONS
        } else {
            proofs::calculate_transaction_root(transactions)
        };

        header
    }

    /// Executes the block with the given block and senders, on the provided [EVMProcessor].
    ///
    /// This returns the poststate from execution and post-block changes, as well as the gas used.
    pub(crate) fn execute(
        &mut self,
        block: &BlockWithSenders,
        executor: &mut EVMProcessor<'_>,
    ) -> Result<(BundleStateWithReceipts, u64), BlockExecutionError> {
        trace!(target: "consensus::auto", transactions=?&block.body, "executing transactions");
        // TODO: there isn't really a parent beacon block root here, so not sure whether or not to
        // call the 4788 beacon contract

        // set the first block to find the correct index in bundle state
        executor.set_first_block(block.number);

        let (receipts, gas_used) = executor.execute_transactions(block, U256::ZERO)?;

        // Save receipts.
        executor.save_receipts(receipts)?;

        // add post execution state change
        // Withdrawals, rewards etc.
        executor.apply_post_execution_state_change(block, U256::ZERO)?;

        // merge transitions
        executor.db_mut().merge_transitions(BundleRetention::Reverts);

        // apply post block changes
        Ok((executor.take_output_state(), gas_used))
    }

    /// Fills in the post-execution header fields based on the given BundleState and gas used.
    /// In doing this, the state root is calculated and the final header is returned.
    pub(crate) fn complete_header<S: StateProviderFactory>(
        &self,
        mut header: Header,
        bundle_state: &BundleStateWithReceipts,
        client: &S,
        gas_used: u64,
        #[cfg(feature = "optimism")] chain_spec: &ChainSpec,
    ) -> Result<Header, BlockExecutionError> {
        let receipts = bundle_state.receipts_by_block(header.number);
        header.receipts_root = if receipts.is_empty() {
            EMPTY_RECEIPTS
        } else {
            let receipts_with_bloom = receipts
                .iter()
                .map(|r| (*r).clone().expect("receipts have not been pruned").into())
                .collect::<Vec<ReceiptWithBloom>>();
            header.logs_bloom =
                receipts_with_bloom.iter().fold(Bloom::ZERO, |bloom, r| bloom | r.bloom);
            proofs::calculate_receipt_root(
                &receipts_with_bloom,
                #[cfg(feature = "optimism")]
                chain_spec,
                #[cfg(feature = "optimism")]
                header.timestamp,
            )
        };

        header.gas_used = gas_used;

        // calculate the state root
        let state_root = client
            .latest()
            .map_err(|_| BlockExecutionError::ProviderError)?
            .state_root(bundle_state)
            .unwrap();
        header.state_root = state_root;
        Ok(header)
    }

    /// Builds and executes a new block with the given transactions, on the provided [EVMProcessor].
    ///
    /// This returns the header of the executed block, as well as the poststate from execution.
    pub(crate) fn build_and_execute(
        &mut self,
        transactions: Vec<TransactionSigned>,
        client: &impl StateProviderFactory,
        chain_spec: Arc<ChainSpec>,
    ) -> Result<(SealedHeader, BundleStateWithReceipts), BlockExecutionError> {
        let header = self.build_header_template(&transactions, chain_spec.clone());

        let block = Block { header, body: transactions, ommers: vec![], withdrawals: None }
            .with_recovered_senders()
            .ok_or(BlockExecutionError::Validation(BlockValidationError::SenderRecoveryError))?;

        trace!(target: "consensus::auto", transactions=?&block.body, "executing transactions");

        // now execute the block
        let db = State::builder()
            .with_database_boxed(Box::new(StateProviderDatabase::new(client.latest().unwrap())))
            .with_bundle_update()
            .build();
        let mut executor = EVMProcessor::new_with_state(chain_spec.clone(), db);

        let (bundle_state, gas_used) = self.execute(&block, &mut executor)?;

        let Block { header, body, .. } = block.block;
        let body = BlockBody { transactions: body, ommers: vec![], withdrawals: None };

        trace!(target: "consensus::auto", ?bundle_state, ?header, ?body, "executed block, calculating state root and completing header");

        // fill in the rest of the fields
        let header = self.complete_header(
            header,
            &bundle_state,
            client,
            gas_used,
            #[cfg(feature = "optimism")]
            chain_spec.as_ref(),
        )?;

        trace!(target: "consensus::auto", root=?header.state_root, ?body, "calculated root");

        // finally insert into storage
        self.insert_new_block(header.clone(), body);

        // set new header with hash that should have been updated by insert_new_block
        let new_header = header.seal(self.best_hash);

        Ok((new_header, bundle_state))
    }
}
