//! Provides everything related to `eth_` namespace
//!
//! The entire implementation of the namespace is quite large, hence it is divided across several
//! files.

use crate::eth::{cache::EthStateCache, signer::EthSigner};
use async_trait::async_trait;
use reth_interfaces::Result;
use reth_network_api::NetworkInfo;
use reth_primitives::{Address, BlockId, BlockNumberOrTag, ChainInfo, H256, U256, U64};
use reth_provider::{providers::ChainState, BlockProvider, EvmEnvProvider, StateProviderFactory};
use reth_rpc_types::{FeeHistoryCache, SyncInfo, SyncStatus};
use reth_transaction_pool::TransactionPool;
use std::{num::NonZeroUsize, sync::Arc};

mod block;
mod call;
mod server;
mod sign;
mod state;
mod transactions;
use crate::eth::error::{EthApiError, EthResult};
pub use transactions::{EthTransactions, TransactionSource};

/// Cache limit of block-level fee history for `eth_feeHistory` RPC method.
const FEE_HISTORY_CACHE_LIMIT: usize = 2048;

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
#[async_trait]
pub trait EthApiSpec: EthTransactions + Send + Sync {
    /// Returns the current ethereum protocol version.
    async fn protocol_version(&self) -> Result<U64>;

    /// Returns the chain id
    fn chain_id(&self) -> U64;

    /// Returns client chain info
    fn chain_info(&self) -> Result<ChainInfo>;

    /// Returns a list of addresses owned by client.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns the [SyncStatus] of the network
    fn sync_status(&self) -> Result<SyncStatus>;
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
pub struct EthApi<Client, Pool, Network> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Client, Pool, Network>>,
    fee_history_cache: FeeHistoryCache,
}

impl<Client, Pool, Network> EthApi<Client, Pool, Network> {
    /// Creates a new, shareable instance.
    pub fn new(client: Client, pool: Pool, network: Network, eth_cache: EthStateCache) -> Self {
        let inner = EthApiInner { client, pool, network, signers: Default::default(), eth_cache };
        Self {
            inner: Arc::new(inner),
            fee_history_cache: FeeHistoryCache::new(
                NonZeroUsize::new(FEE_HISTORY_CACHE_LIMIT).unwrap(),
            ),
        }
    }

    /// Returns the state cache frontend
    pub(crate) fn cache(&self) -> &EthStateCache {
        &self.inner.eth_cache
    }

    /// Returns the inner `Client`
    pub fn client(&self) -> &Client {
        &self.inner.client
    }

    /// Returns the inner `Network`
    pub fn network(&self) -> &Network {
        &self.inner.network
    }

    /// Returns the inner `Pool`
    pub fn pool(&self) -> &Pool {
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

    /// Returns the state at the given [BlockId] enum.
    pub fn state_at_block_id(&self, at: BlockId) -> EthResult<ChainState<'_>> {
        match at {
            BlockId::Hash(hash) => Ok(self.state_at_hash(hash.into()).map(ChainState::boxed)?),
            BlockId::Number(num) => {
                self.state_at_block_number(num)?.ok_or(EthApiError::UnknownBlockNumber)
            }
        }
    }

    /// Returns the state at the given [BlockId] enum or the latest.
    pub fn state_at_block_id_or_latest(
        &self,
        block_id: Option<BlockId>,
    ) -> EthResult<ChainState<'_>> {
        if let Some(block_id) = block_id {
            self.state_at_block_id(block_id)
        } else {
            Ok(self.latest_state().map(ChainState::boxed)?)
        }
    }

    /// Returns the state at the given [BlockNumberOrTag] enum
    ///
    /// Returns `None` if no state available.
    pub fn state_at_block_number(&self, num: BlockNumberOrTag) -> Result<Option<ChainState<'_>>> {
        if let Some(number) = self.convert_block_number(num)? {
            self.state_at_number(number).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Returns the state at the given block number
    pub fn state_at_hash(
        &self,
        block_hash: H256,
    ) -> Result<<Client as StateProviderFactory>::HistorySP<'_>> {
        self.client().history_by_block_hash(block_hash)
    }

    /// Returns the state at the given block number
    pub fn state_at_number(&self, block_number: u64) -> Result<ChainState<'_>> {
        match self.convert_block_number(BlockNumberOrTag::Latest)? {
            Some(num) if num == block_number => self.latest_state().map(ChainState::boxed),
            _ => self.client().history_by_block_number(block_number).map(ChainState::boxed),
        }
    }

    /// Returns the _latest_ state
    pub fn latest_state(&self) -> Result<<Client as StateProviderFactory>::LatestSP<'_>> {
        self.client().latest()
    }
}

impl<Client, Pool, Events> std::fmt::Debug for EthApi<Client, Pool, Events> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthApi").finish_non_exhaustive()
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

    fn is_syncing(&self) -> bool {
        self.network().is_syncing()
    }

    /// Returns the [SyncStatus] of the network
    fn sync_status(&self) -> Result<SyncStatus> {
        let status = if self.is_syncing() {
            let current_block = U256::from(
                self.client().chain_info().map(|info| info.best_number).unwrap_or_default(),
            );
            SyncStatus::Info(SyncInfo {
                starting_block: U256::from(0),
                current_block,
                highest_block: current_block,
                warp_chunks_amount: None,
                warp_chunks_processed: None,
            })
        } else {
            SyncStatus::None
        };
        Ok(status)
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
    /// The async cache frontend for eth related data
    eth_cache: EthStateCache,
}
