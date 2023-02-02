//! Provides everything related to `eth_` namespace
//!
//! The entire implementation of the namespace is quite large, hence it is divided across several
//! files.

use crate::eth::signer::EthSigner;
use async_trait::async_trait;
use reth_interfaces::Result;
use reth_network_api::NetworkInfo;
use reth_primitives::{ChainInfo, U64};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc_types::Transaction;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

mod server;
mod transactions;

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
pub struct EthApi<Pool, Client, Network> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Pool, Client, Network>>,
}

impl<Pool, Client, Network> EthApi<Pool, Client, Network> {
    /// Creates a new, shareable instance.
    pub fn new(client: Arc<Client>, pool: Pool, network: Network) -> Self {
        let inner = EthApiInner { client, pool, network, signers: Default::default() };
        Self { inner: Arc::new(inner) }
    }

    /// Returns the inner `Client`
    pub(crate) fn client(&self) -> &Arc<Client> {
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

#[async_trait]
impl<Pool, Client, Network> EthApiSpec for EthApi<Pool, Client, Network>
where
    Pool: TransactionPool<Transaction = Transaction> + Clone + 'static,
    Client: BlockProvider + StateProviderFactory + 'static,
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
}

/// Container type `EthApi`
struct EthApiInner<Pool, Client, Network> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Arc<Client>,
    /// An interface to interact with the network
    network: Network,
    /// All configured Signers
    signers: Vec<Box<dyn EthSigner>>,
}
