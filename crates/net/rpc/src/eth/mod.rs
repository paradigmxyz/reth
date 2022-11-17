//! Provides everything related to `eth_` namespace

use reth_interfaces::{
    provider::{BlockProvider, StateProviderFactory},
    Result,
};
use reth_primitives::{Transaction, U256, U64};
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

mod eth_server;

/// `Eth` API implementation.
///
/// This type provides the functionality for handling `eth_` related requests.
/// These are implemented two-fold: Core functionality is implemented as functions directly on this
/// type. Additionally, the required server implementations (e.g. [`reth_rpc_api::EthApiServer`])
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
    Pool: TransactionPool<Transaction = Transaction> + Clone,
    Client: BlockProvider + StateProviderFactory,
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

    /// Returns the current ethereum protocol version.
    ///
    /// Note: This returns an `U64`, since this should return as hex string.
    pub fn protocol_version(&self) -> U64 {
        1u64.into()
    }

    /// Returns the best block number
    pub fn block_number(&self) -> Result<U256> {
        Ok(self.client().chain_info()?.best_number.into())
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
