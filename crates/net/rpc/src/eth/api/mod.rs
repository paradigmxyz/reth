//! Provides everything related to `eth_` namespace

use reth_interfaces::Result;
use reth_primitives::U64;
use reth_provider::{BlockProvider, ChainInfo, StateProviderFactory};
use reth_rpc_types::Transaction;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

mod server;

/// `Eth` API trait.
///
/// Defines core functionality of the `eth` API implementation.
pub trait EthApiSpec: Send + Sync {
    /// Returns the current ethereum protocol version.
    fn protocol_version(&self) -> U64;

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
#[derive(Debug, Clone)]
pub struct EthApi<Pool, Client> {
    /// All nested fields bundled together.
    inner: Arc<EthApiInner<Pool, Client>>,
}

impl<Pool, Client> EthApi<Pool, Client>
where
    Pool: TransactionPool + 'static,
    Client: BlockProvider + StateProviderFactory + 'static,
{
    /// Creates a new, shareable instance.
    pub fn new(client: Arc<Client>, pool: Pool) -> Self {
        let inner = EthApiInner { client, pool };
        Self { inner: Arc::new(inner) }
    }

    /// Returns the inner `Client`
    fn client(&self) -> &Arc<Client> {
        &self.inner.client
    }
}

impl<Pool, Client> EthApiSpec for EthApi<Pool, Client>
where
    Pool: TransactionPool<Transaction = Transaction> + Clone + 'static,
    Client: BlockProvider + StateProviderFactory + 'static,
{
    /// Returns the current ethereum protocol version.
    ///
    /// Note: This returns an `U64`, since this should return as hex string.
    fn protocol_version(&self) -> U64 {
        1u64.into()
    }

    /// Returns the chain id
    fn chain_id(&self) -> U64 {
        todo!()
    }

    /// Returns the current info for the chain
    fn chain_info(&self) -> Result<ChainInfo> {
        self.client().chain_info()
    }
}

/// Container type `EthApi`
#[derive(Debug)]
struct EthApiInner<Pool, Client> {
    /// The transaction pool.
    pool: Pool,
    /// The client that can interact with the chain.
    client: Arc<Client>,
    // TODO needs network access to handle things like `eth_syncing`
}
