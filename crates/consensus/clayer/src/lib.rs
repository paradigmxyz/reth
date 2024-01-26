mod consensus;
mod engine_api;
mod engine_pbft;
mod error;
mod task;
mod timing;
use crate::engine_api::{
    auth::{strip_prefix, Auth, JwtKey},
    http::HttpJsonRpc,
};
pub use consensus::ClayerConsensusEngine;
use consensus::{clayer_block_from_genesis, PbftConfig, PbftState};
use engine_api::ApiService;
use futures_util::{future::BoxFuture, FutureExt};
use reth_interfaces::consensus::ForkchoiceState;
use reth_interfaces::{
    consensus::{Consensus, ConsensusError},
    executor::{BlockExecutionError, BlockValidationError},
};
use reth_network::{NetworkEvents, NetworkHandle};
use reth_primitives::{
    constants::{EMPTY_RECEIPTS, EMPTY_TRANSACTIONS, ETHEREUM_BLOCK_GAS_LIMIT},
    proofs, transaction, Address, Block, BlockBody, BlockHash, BlockHashOrNumber, BlockNumber,
    BlockWithSenders, Bloom, ChainSpec, Header, IntoRecoveredTransaction, ReceiptWithBloom,
    SealedBlock, SealedHeader, TransactionSigned, B256, EMPTY_OMMER_ROOT_HASH, U256,
};
use reth_provider::{
    BlockExecutor, BlockReaderIdExt, BundleStateWithReceipts, CanonChainTracker,
    CanonStateNotificationSender, Chain, StateProviderFactory,
};
use reth_revm::{
    database::StateProviderDatabase, db::states::bundle_state::BundleRetention,
    processor::EVMProcessor, State,
};
use reth_rpc_types::{BlockId, BlockNumberOrTag};
use reth_stages::PipelineEvent;
use reth_transaction_pool::{TransactionPool, ValidPoolTransaction};
use secp256k1::SecretKey;
use std::{
    collections::HashMap,
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};
use std::{path::PathBuf, str::FromStr};
use task::ClTask;
use tokio::sync::{mpsc::UnboundedSender, RwLock, RwLockReadGuard, RwLockWriteGuard};
use tokio::time::Interval;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;
use url::Url;
/// Indicates the default jwt authenticated execution endpoint.
pub const DEFAULT_EXECUTION_ENDPOINT: &str = "http://127.0.0.1:8551/";

/// Name for the default file used for the jwt secret.
pub const DEFAULT_JWT_FILE: &str = "jwt.hex";

pub fn create_auth_api(jwt_key: JwtKey) -> HttpJsonRpc {
    let execution_url = Url::from_str(DEFAULT_EXECUTION_ENDPOINT).unwrap();
    let execution_timeout_multiplier = Option::from(1);

    let auth = Auth::new(jwt_key, None, None);
    let api = match HttpJsonRpc::new_with_auth(execution_url, auth, execution_timeout_multiplier) {
        Ok(api) => api,
        Err(e) => {
            panic!("Failed to create execution api. Error: {:?}", e);
        }
    };
    api
}

pub fn create_api() -> HttpJsonRpc {
    let execution_url = Url::from_str(DEFAULT_EXECUTION_ENDPOINT).unwrap();
    let execution_timeout_multiplier = Option::from(1);

    let api = match HttpJsonRpc::new(execution_url, execution_timeout_multiplier) {
        Ok(api) => api,
        Err(e) => {
            panic!("Failed to create execution api. Error: {:?}", e);
        }
    };
    api
}

/// In memory storage
#[derive(Debug, Clone, Default)]
pub(crate) struct ClStorage {
    inner: Arc<RwLock<ClStorageInner>>,
}

// == impl Storage ===

impl ClStorage {
    fn new(header: SealedHeader) -> Self {
        let (header, best_hash) = header.split();
        let mut storage = ClStorageInner {
            best_hash,
            total_difficulty: header.difficulty,
            best_height: header.number,
            ..Default::default()
        };
        storage.headers.insert(0, header);
        storage.bodies.insert(best_hash, BlockBody::default());
        Self { inner: Arc::new(RwLock::new(storage)) }
    }

    /// Returns the write lock of the storage
    pub(crate) async fn write(&self) -> RwLockWriteGuard<'_, ClStorageInner> {
        self.inner.write().await
    }

    /// Returns the read lock of the storage
    pub(crate) async fn read(&self) -> RwLockReadGuard<'_, ClStorageInner> {
        self.inner.read().await
    }
}

/// In-memory storage for the chain the auto seal engine is building.
#[derive(Default, Debug)]
pub(crate) struct ClStorageInner {
    /// Headers buffered for download.
    pub(crate) headers: HashMap<BlockNumber, Header>,
    /// A mapping between block hash and number.
    pub(crate) hash_to_number: HashMap<BlockHash, BlockNumber>,
    /// Bodies buffered for download.
    pub(crate) bodies: HashMap<BlockHash, BlockBody>,
    /// Tracks height of best block
    pub(crate) best_height: u64,
    /// Tracks hash of best block
    pub(crate) best_hash: B256,
    /// The total difficulty of the chain until this block
    pub(crate) total_difficulty: U256,
}

// === impl StorageInner ===

impl ClStorageInner {
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
        header.number = self.best_height + 1;
        header.parent_hash = self.best_hash;

        self.best_hash = header.hash_slow();
        self.best_height = header.number;
        self.total_difficulty += header.difficulty;

        trace!(target: "consensus::auto", num=self.best_height, hash=?self.best_hash, "inserting new block");
        self.headers.insert(header.number, header);
        self.bodies.insert(self.best_hash, body);
        self.hash_to_number.insert(self.best_hash, self.best_height);
    }

    /// Fills in pre-execution header fields based on the current best block and given
    /// transactions.
    pub(crate) fn build_header_template(
        &self,
        transactions: &Vec<TransactionSigned>,
        chain_spec: Arc<ChainSpec>,
    ) -> Header {
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();

        // check previous block for base fee
        let base_fee_per_gas = self
            .headers
            .get(&self.best_height)
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
            number: self.best_height + 1,
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

pub struct ConsensusBuilder<Client, Pool, CDB> {
    chain_spec: Arc<ChainSpec>,
    client: Client,
    pool: Pool,
    storage: ClStorage,
    api: Arc<HttpJsonRpc>,
    network: NetworkHandle,
    consensus: ClayerConsensusEngine,
    storages: CDB,
    config: PbftConfig,
    state: PbftState,
}

impl<Client, Pool: TransactionPool, CDB> ConsensusBuilder<Client, Pool, CDB>
where
    Client: BlockReaderIdExt,
{
    /// Creates a new builder instance to configure all parts.
    pub fn new(
        jwt_key_bytes: &[u8],
        secret: SecretKey,
        chain_spec: Arc<ChainSpec>,
        client: Client,
        pool: Pool,
        network: NetworkHandle,
        clayer_consensus: ClayerConsensusEngine,
        storages: CDB,
    ) -> Self {
        let latest_header = client
            .latest_header()
            .ok()
            .flatten()
            .unwrap_or_else(|| chain_spec.sealed_genesis_header());
        let block = match client.block_by_id(BlockId::Number(BlockNumberOrTag::Latest)) {
            Ok(Some(block)) => block,
            _ => panic!("Failed to get latest block"),
        };
        if latest_header.number == 0 {}

        let jwt_key = JwtKey::from_slice(jwt_key_bytes).unwrap();
        let api = create_auth_api(jwt_key);
        let config = PbftConfig::new();
        let mut state = PbftState::new(secret, latest_header.number, &config);
        clayer_consensus.initialize(clayer_block_from_genesis(&latest_header), &config, &mut state);
        Self {
            chain_spec,
            storage: ClStorage::new(latest_header),
            client,
            pool,
            api: Arc::new(api),
            network,
            consensus: clayer_consensus,
            storages,
            config,
            state,
        }
    }
    /// Consumes the type and returns all components
    #[track_caller]
    pub fn build(self) -> ClTask<Client, Pool, CDB> {
        let Self {
            chain_spec,
            client,
            pool,
            storage,
            api,
            network,
            consensus,
            storages,
            config,
            state,
        } = self;
        let task = ClTask::new(
            Arc::clone(&chain_spec),
            storage,
            client,
            pool,
            Arc::clone(&api),
            network.clone(),
            consensus,
            storages,
            config,
            state,
        );
        task
    }
}
