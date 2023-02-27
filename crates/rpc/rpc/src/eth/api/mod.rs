//! Provides everything related to `eth_` namespace
//!
//! The entire implementation of the namespace is quite large, hence it is divided across several
//! files.

use crate::eth::signer::EthSigner;
use async_trait::async_trait;
use reth_interfaces::Result;
use reth_network_api::NetworkInfo;
use reth_primitives::{
    Address, BlockId, BlockNumberOrTag, ChainInfo, TransactionSigned, H256, U64,
};
use reth_provider::{BlockProvider, EvmEnvProvider, StateProviderFactory};
use std::num::NonZeroUsize;

use crate::eth::error::EthResult;
use reth_interfaces::events::NewBlockNotifications;
use reth_provider::providers::ChainState;
use reth_rpc_types::{FeeHistoryCache, FeeHistoryCacheItem};
use reth_tasks::{TaskSpawner, TokioTaskExecutor};
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;
use tokio::task::JoinHandle;

mod block;
mod call;
mod server;
mod state;
mod transactions;

/// Cache limit of block-level fee history for `eth_feeHistory` RPC method.
const FEE_HISTORY_CACHE_LIMIT: usize = 2048;

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[async_trait]
pub trait EthApiSpec: Send + Sync {
    /// Returns the current ethereum protocol version.
    async fn protocol_version(&self) -> Result<U64>;

    /// Returns the chain id
    fn chain_id(&self) -> U64;

    /// Returns client chain info
    fn chain_info(&self) -> Result<ChainInfo>;

    /// Returns a list of addresses owned by client.
    fn accounts(&self) -> Vec<Address>;

    /// Returns the transaction by hash
    async fn transaction_by_hash(&self, hash: H256) -> Result<Option<TransactionSigned>>;
}

/// `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
/// These are implemented two-fold: Core functionality is implemented as [EthApiSpec]
/// trait. Additionally, the required server implementations (e.g. [`reth_rpc_api::EthApiServer`])
/// are implemented separately in submodules. The rpc handler implementation can then delegate to
/// the main impls. This way [`EthApi`] is not limited to [`jsonrpsee`] and can be used standalone
/// or in other network handlers (for example ipc).
#[derive(Clone)]
#[allow(missing_debug_implementations)]
pub struct EthApi<Client, Pool, Network> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Client, Pool, Network>>,
    /// LRU cache for the `eth_feeHistory` RPC response generation. It's populated in two ways:
    /// 1. Through Tokio task started by [`EthApi::start_fee_history_cache`] and chain event
    /// subscriptions passed into it.
    /// 2. Through [`EthApi::fee_history`] RPC calls which contain cache misses.
    /// Blocks not found in cache are populated by querying the database.
    fee_history_cache: FeeHistoryCache,
    /// The type that's used to spawn subscription tasks.
    task_spawner: Box<dyn TaskSpawner>,
}

impl<Client, Pool, Network> EthApi<Client, Pool, Network> {
    /// Creates a new, shareable instance.
    pub fn new(client: Client, pool: Pool, network: Network) -> Self {
        Self::new_with_task_spawner(client, pool, network, Box::<TokioTaskExecutor>::default())
    }

    /// Creates a new, shareable instance with task spawner.
    pub fn new_with_task_spawner(
        client: Client,
        pool: Pool,
        network: Network,
        task_spawner: Box<dyn TaskSpawner>,
    ) -> Self {
        let inner = EthApiInner { client, pool, network, signers: Default::default() };
        Self {
            inner: Arc::new(inner),
            fee_history_cache: FeeHistoryCache::new(
                NonZeroUsize::new(FEE_HISTORY_CACHE_LIMIT).unwrap(),
            ),
            task_spawner,
        }
    }

    /// Starts listening to the [`NewBlockNotifications`] and populates the LRU cache
    /// required for `eth_feeHistory` RPC with new blocks.
    pub fn start_fee_history_cache(
        &self,
        new_block_notifications: NewBlockNotifications,
    ) -> JoinHandle<()> {
        let mut new_blocks_rx = new_block_notifications;
        let fee_history_cache = self.fee_history_cache.clone();

        self.task_spawner.spawn(Box::pin(async move {
            while let Some(new_block) = new_blocks_rx.recv().await {
                let mut cache = fee_history_cache.0.lock().await;
                let fee_history_cache_item =
                    cache.get_or_insert_mut(new_block.header.number, Default::default);
                *fee_history_cache_item =
                    FeeHistoryCacheItem::new_from_header(new_block.hash, new_block.header.as_ref());
            }
        }))
    }

    /// Returns the inner `Client`
    pub(crate) fn client(&self) -> &Client {
        &self.inner.client
    }

    /// Returns the inner `Network`
    pub(crate) fn network(&self) -> &Network {
        &self.inner.network
    }

    /// Returns the inner `Pool`
    pub(crate) fn pool(&self) -> &Pool {
        &self.inner.pool
    }
}

// === State access helpers ===

impl<Client, Pool, Network> EthApi<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
{
    fn convert_block_number(&self, num: BlockNumberOrTag) -> Result<Option<u64>> {
        self.client().convert_block_number(num)
    }

    /// Helper function to execute a closure with the database at a specific block.
    pub(crate) fn with_state_at<F, T>(&self, _at: BlockId, _f: F) -> EthResult<T>
    where
        F: FnOnce(ChainState<'_>) -> T,
    {
        unimplemented!()
    }

    /// Returns the state at the given [BlockId] enum or the latest.
    pub(crate) fn state_at_block_id_or_latest(
        &self,
        block_id: Option<BlockId>,
    ) -> Result<Option<<Client as StateProviderFactory>::HistorySP<'_>>> {
        if let Some(block_id) = block_id {
            self.state_at_block_id(block_id)
        } else {
            self.latest_state()
        }
    }

    /// Returns the state at the given [BlockId] enum.
    pub(crate) fn state_at_block_id(
        &self,
        block_id: BlockId,
    ) -> Result<Option<<Client as StateProviderFactory>::HistorySP<'_>>> {
        match block_id {
            BlockId::Hash(hash) => self.state_at_hash(hash.into()).map(Some),
            BlockId::Number(num) => self.state_at_block_number(num),
        }
    }

    /// Returns the state at the given [BlockNumberOrTag] enum
    ///
    /// Returns `None` if no state available.
    pub(crate) fn state_at_block_number(
        &self,
        num: BlockNumberOrTag,
    ) -> Result<Option<<Client as StateProviderFactory>::HistorySP<'_>>> {
        if let Some(number) = self.convert_block_number(num)? {
            self.state_at_number(number).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Returns the state at the given block number
    pub(crate) fn state_at_hash(
        &self,
        block_hash: H256,
    ) -> Result<<Client as StateProviderFactory>::HistorySP<'_>> {
        self.client().history_by_block_hash(block_hash)
    }

    /// Returns the state at the given block number
    pub(crate) fn state_at_number(
        &self,
        block_number: u64,
    ) -> Result<<Client as StateProviderFactory>::HistorySP<'_>> {
        self.client().history_by_block_number(block_number)
    }

    /// Returns the _latest_ state
    pub(crate) fn latest_state(
        &self,
    ) -> Result<Option<<Client as StateProviderFactory>::HistorySP<'_>>> {
        self.state_at_block_number(BlockNumberOrTag::Latest)
    }
}

#[async_trait]
impl<Client, Pool, Network> EthApiSpec for EthApi<Client, Pool, Network>
where
    Pool: TransactionPool + Clone + 'static,
    Client: BlockProvider + StateProviderFactory + EvmEnvProvider + 'static,
    Network: NetworkInfo + 'static,
{
    /// Returns the current ethereum protocol version.
    ///
    /// Note: This returns an `U64`, since this should return as hex string.
    async fn protocol_version(&self) -> Result<U64> {
        let status = self.network().network_status().await?;
        Ok(U64::from(status.protocol_version))
    }

    /// Returns the chain id
    fn chain_id(&self) -> U64 {
        U64::from(self.network().chain_id())
    }

    /// Returns the current info for the chain
    fn chain_info(&self) -> Result<ChainInfo> {
        self.client().chain_info()
    }

    fn accounts(&self) -> Vec<Address> {
        self.inner.signers.iter().flat_map(|s| s.accounts()).collect()
    }

    async fn transaction_by_hash(&self, hash: H256) -> Result<Option<TransactionSigned>> {
        self.client().transaction_by_hash(hash)
    }
}

/// Container type `EthApi`
struct EthApiInner<Client, Pool, Network> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Client,
    /// An interface to interact with the network
    network: Network,
    /// All configured Signers
    signers: Vec<Box<dyn EthSigner>>,
}
