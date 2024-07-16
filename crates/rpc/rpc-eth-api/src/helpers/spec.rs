//! Loads chain metadata.

use std::sync::Arc;

use futures::Future;
use reth_chainspec::{ChainInfo, ChainSpec};
use reth_errors::RethResult;
use reth_primitives::{Address, U64};
use reth_rpc_types::SyncStatus;

/// Core Ethereum API specification.
///
/// This trait defines the fundamental methods required for implementing
/// the Ethereum JSON-RPC API. It provides access to essential blockchain
/// information and node status.
///
/// Implementations of this trait form the basis for servicing Ethereum
/// API requests in a node or client application.
#[auto_impl::auto_impl(&, Arc)]
pub trait EthApiSpec: Send + Sync {
    /// Returns the current ethereum protocol version.
    fn protocol_version(&self) -> impl Future<Output = RethResult<U64>> + Send;

    /// Returns the chain id
    fn chain_id(&self) -> U64;

    /// Returns provider chain info
    fn chain_info(&self) -> RethResult<ChainInfo>;

    /// Returns a list of addresses owned by provider.
    fn accounts(&self) -> Vec<Address>;

    /// Returns `true` if the network is undergoing sync.
    fn is_syncing(&self) -> bool;

    /// Returns the [`SyncStatus`] of the network
    fn sync_status(&self) -> RethResult<SyncStatus>;

    /// Returns the configured [`ChainSpec`].
    fn chain_spec(&self) -> Arc<ChainSpec>;
}
