//! Helper trait for interfacing with [`FullNodeComponents`].

use reth_node_api::FullNodeComponents;
use reth_rpc_eth_types::EthStateCache;

/// Helper trait to relax trait bounds on [`FullNodeComponents`].
///
/// Helpful when defining types that would otherwise have a generic `N: FullNodeComponents`. Using
/// `N: RpcNodeCore` instead, allows access to all the associated types on [`FullNodeComponents`]
/// that are used in RPC, but with more flexibility since they have no trait bounds (asides auto
/// traits).
pub trait RpcNodeCore: Clone + Send + Sync {
    /// The provider type used to interact with the node.
    type Provider: Send + Sync + Clone + Unpin;
    /// The transaction pool of the node.
    type Pool: Send + Sync + Clone + Unpin;
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: Send + Sync + Clone + Unpin;
    /// Network API.
    type Network: Send + Sync + Clone;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;
}

impl<T> RpcNodeCore for T
where
    T: FullNodeComponents,
{
    type Provider = T::Provider;
    type Pool = T::Pool;
    type Network = <T as FullNodeComponents>::Network;
    type Evm = <T as FullNodeComponents>::Evm;

    #[inline]
    fn pool(&self) -> &Self::Pool {
        FullNodeComponents::pool(self)
    }

    #[inline]
    fn evm_config(&self) -> &Self::Evm {
        FullNodeComponents::evm_config(self)
    }

    #[inline]
    fn network(&self) -> &Self::Network {
        FullNodeComponents::network(self)
    }

    #[inline]
    fn provider(&self) -> &Self::Provider {
        FullNodeComponents::provider(self)
    }
}

/// Additional components, asides the core node components, needed to run `eth_` namespace API
/// server.
pub trait RpcNodeCoreExt: RpcNodeCore {
    /// Returns handle to RPC cache service.
    fn cache(&self) -> &EthStateCache;
}
