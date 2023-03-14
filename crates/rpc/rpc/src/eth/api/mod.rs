//! Provides everything related to `eth_` namespace
//!
//! The entire implementation of the namespace is quite large, hence it is divided across several
//! files.

use crate::eth::{cache::EthStateCache, error::EthResult, signer::EthSigner};
use async_trait::async_trait;
use reth_interfaces::Result;
use reth_network_api::NetworkInfo;
use reth_primitives::{Address, BlockId, BlockNumberOrTag, ChainInfo, H256, U64};
use reth_provider::{
    providers::ChainState, BlockProvider, EvmEnvProvider, StateProvider as StateProviderTrait,
    StateProviderFactory,
};
use reth_rpc_types::FeeHistoryCache;
use reth_transaction_pool::TransactionPool;
use std::{num::NonZeroUsize, ops::Deref, sync::Arc};

mod block;
mod call;
mod server;
mod state;
mod transactions;
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

// Transparent wrapper to enable state access helpers
// returning latest state provider when appropiate
pub(crate) enum StateProvider<'a, H, L> {
    History(H),
    Latest(L),
    _Unreachable(&'a ()), // like a PhantomData for 'a
}

type HistoryOrLatest<'a, Client> = StateProvider<
    'a,
    <Client as StateProviderFactory>::HistorySP<'a>,
    <Client as StateProviderFactory>::LatestSP<'a>,
>;

impl<'a, H, L> Deref for StateProvider<'a, H, L>
where
    Self: 'a,
    H: StateProviderTrait + 'a,
    L: StateProviderTrait + 'a,
{
    type Target = dyn StateProviderTrait + 'a;

    fn deref(&self) -> &Self::Target {
        match self {
            StateProvider::History(h) => h,
            StateProvider::Latest(l) => l,
            StateProvider::_Unreachable(()) => unreachable!(),
        }
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
    ) -> Result<Option<HistoryOrLatest<'_, Client>>> {
        if let Some(block_id) = block_id {
            self.state_at_block_id(block_id)
        } else {
            self.latest_state().map(|v| Some(StateProvider::Latest(v)))
        }
    }

    /// Returns the state at the given [BlockId] enum.
    pub(crate) fn state_at_block_id(
        &self,
        block_id: BlockId,
    ) -> Result<Option<HistoryOrLatest<'_, Client>>> {
        match block_id {
            BlockId::Hash(hash) => {
                self.state_at_hash(hash.into()).map(|s| Some(StateProvider::History(s)))
            }
            BlockId::Number(num) => self.state_at_block_number(num),
        }
    }

    /// Returns the state at the given [BlockNumberOrTag] enum
    ///
    /// Returns `None` if no state available.
    pub(crate) fn state_at_block_number(
        &self,
        num: BlockNumberOrTag,
    ) -> Result<Option<HistoryOrLatest<'_, Client>>> {
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
    pub(crate) fn state_at_number(&self, block_number: u64) -> Result<HistoryOrLatest<'_, Client>> {
        match self.convert_block_number(BlockNumberOrTag::Latest)? {
            Some(num) if num == block_number => self.latest_state().map(StateProvider::Latest),
            _ => self.client().history_by_block_number(block_number).map(StateProvider::History),
        }
    }

    /// Returns the _latest_ state
    pub(crate) fn latest_state(&self) -> Result<<Client as StateProviderFactory>::LatestSP<'_>> {
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
